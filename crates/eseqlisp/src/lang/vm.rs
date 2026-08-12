use crate::compiler::{Chunk, Compiler, CompilerError, MacroDef, OpCode};
use crate::host::BufferId;
use crate::hot_reload::{SourceStackEntry, SourceManager, extract_defined_symbols_from_source};
use crate::parser::{Expr, ExprKind, Expression, Parser, SpannedASTParser};
use std::cell::RefCell;
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static RAND_STATE: AtomicU64 = AtomicU64::new(0x9e37_79b9_7f4a_7c15);
pub const SOURCE_BUFFER_ID_PROP: &str = "__source-buffer-id";
pub const SOURCE_MODULE_PATH_PROP: &str = "__source-module-path";
pub const SOURCE_SYMBOL_PROP: &str = "__source-symbol";
pub const SOURCE_START_BYTE_PROP: &str = "__source-start-byte";
pub const SOURCE_END_BYTE_PROP: &str = "__source-end-byte";
pub const SOURCE_REVISION_PROP: &str = "__source-revision";
pub const INLINE_ANCHOR_PROP: &str = "inline-anchor";
pub const INLINE_PLACEMENT_PROP: &str = "__inline-placement";
pub const INLINE_VALUE_START_BYTE_PROP: &str = "__inline-value-start-byte";
pub const INLINE_VALUE_END_BYTE_PROP: &str = "__inline-value-end-byte";
pub const INLINE_WRITEBACK_CALLBACK: &str = "__inline-code-widget-writeback";
pub const INLINE_PARENT_CALLEE_PROP: &str = "__inline-parent-callee";
pub const INLINE_PARENT_INLET_PROP: &str = "__inline-parent-inlet";
const SOURCE_ORIGIN_NATIVE: &str = "__source-origin";

#[derive(Debug, PartialEq)]
pub enum VMError {
    UnknownConstant,
    UnknownOpcode,
    StackUnderflow,
    IncorrectType,
    UnknownVariable(String),
    ReadonlyReactive(String),
    ExpectedFunction,
    ArityMismatch,
    ParseError,
    CompileError,
}

pub type NativeFn = Rc<dyn Fn(Vec<Value>, &mut VM) -> Value>;
pub type GlobalStoreHook = Rc<dyn Fn(&str, &Value)>;
pub type InlineWidgetMetadataResolver = Rc<dyn Fn(&str, &str) -> Option<InlineWidgetMetadata>>;
pub type NodeId = u32;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InlineWidgetMetadata {
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub step: Option<f64>,
}

fn debug_lisp_callback_errors_enabled() -> bool {
    std::env::var("ESEQLISP_DEBUG_LISP_ERRORS")
        .ok()
        .is_none_or(|value| !matches!(value.as_str(), "0" | "false" | "no" | "off"))
}

fn log_native_callback_error(vm: &VM, native_name: &str, index: usize, error: &VMError) {
    if debug_lisp_callback_errors_enabled() {
        let detail = vm.last_reactive_error_detail.as_deref().unwrap_or("-");
        eprintln!(
            "[lisp-error][{native_name}] callback index={index} error={error:?} detail={detail}"
        );
    }
}

fn log_native_misuse(native_name: &str, message: &str) {
    if debug_lisp_callback_errors_enabled() {
        eprintln!("[lisp-error][{native_name}] {message}");
    }
}

/// Run every listener registered on an extension hook, in registration order.
/// A listener error is logged and does not stop the remaining listeners.
fn run_extension_hook(vm: &mut VM, name: &str, args: Vec<Value>) -> Value {
    let callbacks: Vec<Value> = vm
        .extension_hooks
        .get(name)
        .map(|entries| entries.iter().map(|(_, callback)| callback.clone()).collect())
        .unwrap_or_default();
    for (idx, callback) in callbacks.into_iter().enumerate() {
        if let Err(error) = vm.invoke(callback, args.clone()) {
            log_native_callback_error(vm, name, idx, &error);
        }
    }
    Value::Nil
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ReactiveSource {
    NamespaceField { namespace: String, field: String },
    LocalState { name: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ReactiveFieldKey {
    pub namespace: String,
    pub field: String,
}

impl ReactiveFieldKey {
    pub fn new(namespace: impl Into<String>, field: impl Into<String>) -> Self {
        Self {
            namespace: namespace.into(),
            field: field.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ReactiveBindingKey {
    pub field: ReactiveFieldKey,
    pub index: Option<usize>,
}

impl ReactiveBindingKey {
    pub fn field(namespace: impl Into<String>, field: impl Into<String>) -> Self {
        Self {
            field: ReactiveFieldKey::new(namespace, field),
            index: None,
        }
    }

    pub fn indexed(namespace: impl Into<String>, field: impl Into<String>, index: usize) -> Self {
        Self {
            field: ReactiveFieldKey::new(namespace, field),
            index: Some(index),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BindingKind {
    Float,
}

pub enum Value {
    Number(f64),
    Bool(bool),
    Nil,
    String(String),
    Symbol(String),
    Keyword(String),
    List(Vec<Rc<RefCell<Value>>>),
    Map(HashMap<String, Rc<RefCell<Value>>>),
    Closure(usize, Vec<Rc<RefCell<Value>>>),
    Function(usize),
    NodeRef(NodeId),
    ReactiveRef {
        namespace: String,
        field: String,
        index: Option<usize>,
        kind: BindingKind,
        slot: Arc<AtomicU64>,
    },
    NativeFunction(NativeFn),
    HostHandle {
        kind: String,
        id: u64,
        callable: NativeFn,
    },
}

#[derive(Clone)]
pub enum ReactiveNode {
    Source {
        id: NodeId,
        source: ReactiveSource,
        value: Value,
        dependents: HashSet<NodeId>,
    },
    Derived {
        id: NodeId,
        chunk_idx: usize,
        value: Value,
        dependents: HashSet<NodeId>,
        dirty: bool,
    },
    Effect {
        id: NodeId,
        chunk_idx: usize,
        callable: Option<Value>,
        source_buffer_id: Option<BufferId>,
        source_file: Option<std::path::PathBuf>,
        source_revision: Option<u64>,
        target: EffectTarget,
        subtree_root_id: Option<u64>,
        parent_subtree_root_id: Option<u64>,
        stable_key: Option<String>,
        symbol_dependencies: HashSet<String>,
        dirty: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum EffectTarget {
    BufferId(Option<BufferId>),
    BufferName(String),
}

#[derive(Debug, Clone, Default)]
pub struct EvalProfile {
    pub parse: Duration,
    pub ast: Duration,
    pub compile: Duration,
    pub execute: Duration,
}

#[derive(Clone)]
pub struct PendingWidgetTree {
    pub source_buffer_id: Option<BufferId>,
    pub source_file: Option<std::path::PathBuf>,
    pub target: EffectTarget,
    pub tree: Value,
    pub reactive_dependencies: Vec<ReactiveFieldKey>,
}

#[derive(Clone)]
pub enum PendingUiUpdate {
    FullTree(PendingWidgetTree),
    ReplaceSubtree {
        source_buffer_id: Option<BufferId>,
        source_file: Option<std::path::PathBuf>,
        target: EffectTarget,
        subtree_root_id: u64,
        tree: Value,
        reactive_dependencies: Vec<ReactiveFieldKey>,
    },
}

impl PendingUiUpdate {
    pub fn source_buffer_id(&self) -> Option<BufferId> {
        match self {
            PendingUiUpdate::FullTree(pending) => pending.source_buffer_id,
            PendingUiUpdate::ReplaceSubtree {
                source_buffer_id, ..
            } => *source_buffer_id,
        }
    }

    pub fn source_file(&self) -> Option<&std::path::Path> {
        match self {
            PendingUiUpdate::FullTree(pending) => pending.source_file.as_deref(),
            PendingUiUpdate::ReplaceSubtree { source_file, .. } => source_file.as_deref(),
        }
    }

    pub fn target(&self) -> &EffectTarget {
        match self {
            PendingUiUpdate::FullTree(pending) => &pending.target,
            PendingUiUpdate::ReplaceSubtree { target, .. } => target,
        }
    }
}

#[derive(Clone)]
pub struct ReactiveExecTiming {
    pub label: String,
    pub elapsed: Duration,
    pub source_buffer_id: Option<BufferId>,
    pub target: EffectTarget,
    pub subtree_root_id: Option<u64>,
}

impl ReactiveExecTiming {
    pub fn profile_label(&self) -> String {
        let owner = self
            .source_buffer_id
            .map(|id| format!("owner:buf#{id}"))
            .unwrap_or_else(|| "owner:none".to_string());
        let target = match &self.target {
            EffectTarget::BufferId(Some(id)) => format!("target:buf#{id}"),
            EffectTarget::BufferId(None) => "target:active-buffer".to_string(),
            EffectTarget::BufferName(name) => format!("target:{name}"),
        };
        let subtree = self
            .subtree_root_id
            .map(|id| format!("root:{id}"))
            .unwrap_or_else(|| "root:-".to_string());
        format!("{}|{}|{}|{}", self.label, owner, target, subtree)
    }
}

const STABLE_WIDGET_ID_PROP: &str = "__stable-widget-id";
const SUBTREE_ROOT_ID_PROP: &str = "__subtree-root-id";
const PARENT_SUBTREE_ROOT_ID_PROP: &str = "__parent-subtree-root-id";
const STABLE_KEY_PROP: &str = "__stable-key";

#[derive(Debug, Clone)]
struct SubtreeCaptureContext {
    root_id: u64,
    parent_root_id: Option<u64>,
    stable_key: String,
}

#[derive(Debug, Clone)]
struct RegisteredSubtreeOwner {
    node_id: NodeId,
    root_id: u64,
    parent_root_id: Option<u64>,
    stable_key: String,
    callable: Value,
}

/// Sentinel "index" recorded when a dependent reads a list's length.
pub const LEN_READ_SENTINEL: usize = usize::MAX;

/// How a reactive source value changed, used to filter dependents by ReadScope.
enum ValueChange {
    Full,
    Indices(Vec<usize>),
}

/// None = unchanged. For list-to-list updates, reports the changed element
/// indices (plus LEN_READ_SENTINEL when the length changed); any other shape
/// change is Full.
fn value_change_scope(old: &Value, new: &Value) -> Option<ValueChange> {
    match (old, new) {
        (Value::List(old_items), Value::List(new_items)) => {
            let mut changed = Vec::new();
            let max_len = old_items.len().max(new_items.len());
            for index in 0..max_len {
                match (old_items.get(index), new_items.get(index)) {
                    (Some(old_item), Some(new_item)) => {
                        if *old_item.borrow() != *new_item.borrow() {
                            changed.push(index);
                        }
                    }
                    _ => changed.push(index),
                }
            }
            if old_items.len() != new_items.len() {
                changed.push(LEN_READ_SENTINEL);
            }
            if changed.is_empty() {
                None
            } else {
                Some(ValueChange::Indices(changed))
            }
        }
        _ => (old != new).then_some(ValueChange::Full),
    }
}

/// Release-mode probe for the reactive-clone-elimination effort
/// (docs/reactive-clone-elimination-spec.md §3.3 / P0). Set
/// `ESEQLISP_PROFILE_CLONES=1` to log, once per second, cumulative clone
/// time and allocation counts per site (each cloned Value node allocates
/// one `Rc<RefCell<..>>`, so the node count is the allocation proxy).
pub fn clone_probe_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("ESEQLISP_PROFILE_CLONES").is_some())
}

thread_local! {
    static CLONE_PROBE: RefCell<CloneProbeWindow> = RefCell::new(CloneProbeWindow {
        window_start: Instant::now(),
        sites: HashMap::new(),
    });
}

struct CloneProbeWindow {
    window_start: Instant,
    sites: HashMap<&'static str, (u64, u64, Duration)>,
}

/// Count the Value nodes a `deep_clone` of `value` allocates.
pub fn value_alloc_nodes(value: &Value) -> u64 {
    match value {
        Value::List(items) => {
            1 + items
                .iter()
                .map(|item| value_alloc_nodes(&item.borrow()))
                .sum::<u64>()
        }
        Value::Map(map) => {
            1 + map
                .values()
                .map(|item| value_alloc_nodes(&item.borrow()))
                .sum::<u64>()
        }
        _ => 1,
    }
}

pub fn clone_probe_record(site: &'static str, elapsed: Duration, nodes: u64) {
    if !clone_probe_enabled() {
        return;
    }
    CLONE_PROBE.with(|probe| {
        let mut probe = probe.borrow_mut();
        let entry = probe.sites.entry(site).or_insert((0, 0, Duration::ZERO));
        entry.0 += 1;
        entry.1 += nodes;
        entry.2 += elapsed;
        let secs = probe.window_start.elapsed().as_secs_f64();
        if secs < 1.0 {
            return;
        }
        let mut sites = probe.sites.drain().collect::<Vec<_>>();
        sites.sort_by(|a, b| b.1.2.cmp(&a.1.2));
        for (site, (calls, nodes, total)) in sites {
            eprintln!(
                "[clone-probe] site={site} calls/s={:.1} allocs/s={:.0} ms/s={:.3}",
                calls as f64 / secs,
                nodes as f64 / secs,
                total.as_secs_f64() * 1000.0 / secs,
            );
        }
        probe.window_start = Instant::now();
    });
}

/// `value.deep_clone()` wrapped in the clone probe; near-zero cost when
/// `ESEQLISP_PROFILE_CLONES` is unset.
pub fn probed_deep_clone(site: &'static str, value: &Value) -> Value {
    if !clone_probe_enabled() {
        return value.deep_clone();
    }
    let started = Instant::now();
    let cloned = value.deep_clone();
    clone_probe_record(site, started.elapsed(), value_alloc_nodes(&cloned));
    cloned
}

/// Shallow `value.clone()` (Rc bump on the top-level list/map) wrapped in the
/// clone probe, so P3 re-runs can confirm the converted W2 sites collapsed to
/// ~0; near-zero cost when `ESEQLISP_PROFILE_CLONES` is unset.
pub fn probed_shallow_clone(site: &'static str, value: &Value) -> Value {
    if !clone_probe_enabled() {
        return value.clone();
    }
    let started = Instant::now();
    let cloned = value.clone();
    clone_probe_record(site, started.elapsed(), 1);
    cloned
}

// ---------------------------------------------------------------------------
// Widget-tree freeze registry (docs/reactive-clone-elimination-spec.md §3.4)
//
// Rendered widget trees are immutable from the moment they are handed to the
// runtime (pending_widget_trees / current_widget_tree / Buffer::widget_tree);
// storage sites share them by shallow clone. This debug-only registry records
// every list/map cell of a frozen tree by Rc pointer identity; the (few) tree
// mutation helpers call `debug_assert_cell_not_frozen` so a post-storage
// mutation panics in dev/test runs instead of silently editing history for
// every holder of the Rc. Zero cost in release builds.
//
// Closure upvalue cells are deliberately NOT frozen: `Value::deep_clone`
// shares them too, and handler-captured state is legitimately mutated when
// handlers run. Freezing covers tree *structure* (list/map cells), which is
// exactly what the shallow-shared storage sites alias.
// ---------------------------------------------------------------------------

#[cfg(debug_assertions)]
thread_local! {
    static FROZEN_TREE_CELLS: RefCell<HashMap<usize, std::rc::Weak<RefCell<Value>>>> =
        RefCell::new(HashMap::new());
}

#[cfg(debug_assertions)]
thread_local! {
    static FROZEN_TREE_PRUNE_THRESHOLD: std::cell::Cell<usize> = const { std::cell::Cell::new(4096) };
}

/// Register every list/map cell reachable from `tree` as frozen. Idempotent
/// and cheap on re-freeze: an already-registered live cell short-circuits its
/// whole subtree (shared cells imply shared subtrees). No-op in release.
pub fn freeze_widget_tree(tree: &Value) {
    #[cfg(debug_assertions)]
    FROZEN_TREE_CELLS.with(|cells| {
        let mut cells = cells.borrow_mut();
        // A live Weak pins its allocation's memory, so dead entries only
        // release memory once pruned; prune when the registry doubles past
        // the last high-water mark.
        FROZEN_TREE_PRUNE_THRESHOLD.with(|threshold| {
            if cells.len() > threshold.get() {
                cells.retain(|_, weak| weak.strong_count() > 0);
                threshold.set((cells.len() * 2).max(4096));
            }
        });
        freeze_value_cells(tree, &mut cells);
    });
    #[cfg(not(debug_assertions))]
    let _ = tree;
}

#[cfg(debug_assertions)]
fn freeze_value_cells(value: &Value, cells: &mut HashMap<usize, std::rc::Weak<RefCell<Value>>>) {
    match value {
        Value::List(items) => {
            for cell in items {
                freeze_tree_cell(cell, cells);
            }
        }
        Value::Map(map) => {
            for cell in map.values() {
                freeze_tree_cell(cell, cells);
            }
        }
        _ => {}
    }
}

#[cfg(debug_assertions)]
fn freeze_tree_cell(
    cell: &Rc<RefCell<Value>>,
    cells: &mut HashMap<usize, std::rc::Weak<RefCell<Value>>>,
) {
    let ptr = Rc::as_ptr(cell) as usize;
    if let Some(existing) = cells.get(&ptr) {
        if existing.strong_count() > 0 {
            // A live Weak pins its allocation, so a matching pointer is the
            // same cell: this subtree is already frozen.
            return;
        }
    }
    cells.insert(ptr, Rc::downgrade(cell));
    freeze_value_cells(&cell.borrow(), cells);
}

/// Panic (debug builds only) if `cell` belongs to a frozen widget tree.
/// Mutation of a stored tree must instead deep-clone at the mutation site,
/// scoped to the subtree it modifies (spec §3.2).
#[inline]
pub fn debug_assert_cell_not_frozen(cell: &Rc<RefCell<Value>>, context: &'static str) {
    #[cfg(debug_assertions)]
    FROZEN_TREE_CELLS.with(|cells| {
        let mut cells = cells.borrow_mut();
        let ptr = Rc::as_ptr(cell) as usize;
        match cells.get(&ptr) {
            Some(frozen) if frozen.strong_count() > 0 => panic!(
                "widget-tree freeze violation: {context} mutating a cell of a stored \
                 (frozen) widget tree; deep-clone the subtree at the mutation site \
                 instead (docs/reactive-clone-elimination-spec.md §3.2/§3.4)"
            ),
            Some(_) => {
                // The frozen cell died and this is a new allocation at a
                // reused address (possible once the dead Weak is the only
                // reference left after pruning kept it — clean it up).
                cells.remove(&ptr);
            }
            None => {}
        }
    });
    #[cfg(not(debug_assertions))]
    let _ = (cell, context);
}

/// Which part of a reactive source a dependent actually read.
#[derive(Clone, Debug)]
pub enum ReadScope {
    /// Whole-value read: any change re-dirties the dependent.
    All,
    /// Indexed reads of a list source (may include LEN_READ_SENTINEL).
    Indices(HashSet<usize>),
}

#[derive(Clone)]
pub struct ReactiveDag {
    pub nodes: HashMap<NodeId, ReactiveNode>,
    pub edges: HashMap<NodeId, HashSet<NodeId>>,
    pub dirty_nodes: HashSet<NodeId>,
    pub next_id: NodeId,
    namespace_field_sources: HashMap<String, HashMap<String, NodeId>>,
    local_state_sources: HashMap<String, NodeId>,
    /// dependent -> set of dependencies (reverse of `edges`); lets
    /// clear_dependencies_of/remove_node avoid scanning every edge.
    reverse_edges: HashMap<NodeId, HashSet<NodeId>>,
    /// subtree_root_id -> effect node owning that subtree.
    subtree_effects: HashMap<u64, NodeId>,
    /// parent subtree_root_id -> effect nodes registered under it.
    subtree_children: HashMap<u64, HashSet<NodeId>>,
    /// dependent -> (dependency -> read scope). Missing entries mean All.
    dependency_scopes: HashMap<NodeId, HashMap<NodeId, ReadScope>>,
}

pub fn format_lisp_value(value: &Value) -> String {
    match value {
        Value::Number(n) => {
            if n.fract() == 0.0 {
                format!("{:.0}", n)
            } else {
                format!("{n}")
            }
        }
        Value::Bool(b) => b.to_string(),
        Value::Nil => "nil".to_string(),
        Value::String(s) => format!("{s:?}"),
        Value::Symbol(s) => format!("'{s}"),
        Value::Keyword(s) => format!(":{s}"),
        Value::List(items) => {
            let rendered = items
                .iter()
                .map(|item| format_lisp_value(&item.borrow()))
                .collect::<Vec<_>>()
                .join(" ");
            format!("({rendered})")
        }
        Value::Map(map) => {
            let mut entries = map
                .iter()
                .map(|(key, value)| (key.clone(), format_lisp_value(&value.borrow())))
                .collect::<Vec<_>>();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            let rendered = entries
                .into_iter()
                .map(|(key, value)| format!(":{key} {value}"))
                .collect::<Vec<_>>()
                .join(" ");
            format!("{{{rendered}}}")
        }
        Value::Closure(i, _) => format!("<closure:{i}>"),
        Value::Function(i) => format!("<fn:{i}>"),
        Value::NodeRef(id) => format!("<node:{id}>"),
        Value::ReactiveRef {
            namespace,
            field,
            index,
            ..
        } => format_binding_ref(namespace, field, *index),
        Value::NativeFunction(_) => "<native>".to_string(),
        Value::HostHandle { kind, id, .. } => format!("<{kind}:{id}>"),
    }
}

pub fn format_lisp_source(value: &Value) -> String {
    match value {
        Value::Number(n) => {
            if n.fract() == 0.0 {
                format!("{:.0}", n)
            } else {
                format!("{n}")
            }
        }
        Value::Bool(b) => b.to_string(),
        Value::Nil => "nil".to_string(),
        Value::String(s) => format!("{s:?}"),
        Value::Symbol(s) => s.clone(),
        Value::Keyword(s) => format!(":{s}"),
        Value::List(items) => {
            let rendered = items
                .iter()
                .map(|item| format_lisp_source(&item.borrow()))
                .collect::<Vec<_>>()
                .join(" ");
            format!("({rendered})")
        }
        Value::Map(map) => {
            let mut entries = map
                .iter()
                .map(|(key, value)| (key.clone(), format_lisp_source(&value.borrow())))
                .collect::<Vec<_>>();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            let rendered = entries
                .into_iter()
                .map(|(key, value)| format!(":{key} {value}"))
                .collect::<Vec<_>>()
                .join(" ");
            format!("(dict {rendered})")
        }
        Value::Closure(i, _) => format!("<closure:{i}>"),
        Value::Function(i) => format!("<fn:{i}>"),
        Value::NodeRef(id) => format!("<node:{id}>"),
        Value::ReactiveRef {
            namespace,
            field,
            index,
            ..
        } => format_binding_ref(namespace, field, *index),
        Value::NativeFunction(_) => "<native>".to_string(),
        Value::HostHandle { kind, id, .. } => format!("<{kind}:{id}>"),
    }
}

/// Parsed format spec from `{:>4.2}` style placeholders.
#[derive(Default)]
struct FmtSpec {
    precision: Option<usize>,
    width: Option<usize>,
    align: FmtAlign,
    fill: char,
}

#[derive(Default, Clone, Copy)]
enum FmtAlign {
    #[default]
    Right,
    Left,
}

fn format_fmt_value(value: &Value, spec: &FmtSpec) -> String {
    // Format the raw value first
    let raw = match (value, spec.precision) {
        (Value::Number(n), Some(precision)) => format!("{n:.precision$}"),
        (Value::String(s), _) => s.clone(),
        _ => format_lisp_value(value),
    };
    // Apply width + alignment padding
    let Some(width) = spec.width else {
        return raw;
    };
    let len = raw.chars().count();
    if len >= width {
        return raw;
    }
    let pad: String = std::iter::repeat(spec.fill).take(width - len).collect();
    match spec.align {
        FmtAlign::Right => format!("{pad}{raw}"),
        FmtAlign::Left => format!("{raw}{pad}"),
    }
}

fn parse_fmt_placeholder(template: &str, start: usize) -> Option<(usize, FmtSpec)> {
    let rest = template.get(start..)?;
    if rest.starts_with("{}") {
        return Some((2, FmtSpec::default()));
    }

    let spec_str = rest.strip_prefix("{:")?;
    let mut pos = 0;
    let bytes = spec_str.as_bytes();

    // Parse optional fill + alignment
    let mut fill = ' ';
    let mut align = FmtAlign::Right;

    if bytes.get(pos) == Some(&b'>') {
        align = FmtAlign::Right;
        pos += 1;
    } else if bytes.get(pos) == Some(&b'<') {
        align = FmtAlign::Left;
        pos += 1;
    } else if bytes.get(pos) == Some(&b'0')
        && bytes.get(pos + 1).is_some_and(|b| b.is_ascii_digit())
    {
        // Zero-pad: {:02}, {:04} etc.
        fill = '0';
        // don't advance pos — the digits are the width
    }

    // Parse optional width
    let width_start = pos;
    while pos < bytes.len() && bytes[pos].is_ascii_digit() {
        pos += 1;
    }
    let width = if pos > width_start {
        spec_str[width_start..pos].parse().ok()
    } else {
        None
    };

    // Parse optional .precision
    let precision = if bytes.get(pos) == Some(&b'.') {
        pos += 1;
        let prec_start = pos;
        while pos < bytes.len() && bytes[pos].is_ascii_digit() {
            pos += 1;
        }
        if pos > prec_start {
            spec_str[prec_start..pos].parse().ok()
        } else {
            None
        }
    } else {
        None
    };

    // Expect closing }
    if bytes.get(pos) != Some(&b'}') {
        return None;
    }

    let total_len = 2 + pos + 1; // "{:" + spec + "}"
    Some((
        total_len,
        FmtSpec {
            precision,
            width,
            align,
            fill,
        },
    ))
}

fn is_falsey(value: &Value) -> bool {
    match value {
        Value::Bool(false) | Value::Nil => true,
        Value::Number(n) => *n == 0.0,
        Value::String(s) => s.is_empty(),
        Value::List(items) => items.is_empty(),
        _ => false,
    }
}

fn list_from_values(values: impl IntoIterator<Item = Value>) -> Value {
    Value::List(
        values
            .into_iter()
            .map(|value| Rc::new(RefCell::new(value)))
            .collect(),
    )
}

impl std::fmt::Debug for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", format_lisp_value(self))
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Number(a), Self::Number(b)) => a == b,
            (Self::Bool(a), Self::Bool(b)) => a == b,
            (Self::Nil, Self::Nil) => true,
            (Self::String(a), Self::String(b)) => a == b,
            (Self::Symbol(a), Self::Symbol(b)) => a == b,
            (Self::Keyword(a), Self::Keyword(b)) => a == b,
            (Self::List(a), Self::List(b)) => {
                a.len() == b.len()
                    && a.iter()
                        .zip(b.iter())
                        .all(|(x, y)| *x.borrow() == *y.borrow())
            }
            (Self::Map(a), Self::Map(b)) => {
                a.len() == b.len()
                    && a.iter().all(|(key, left)| {
                        b.get(key)
                            .is_some_and(|right| *left.borrow() == *right.borrow())
                    })
            }
            (Self::Closure(a, _), Self::Closure(b, _)) => a == b,
            (Self::Function(a), Self::Function(b)) => a == b,
            (Self::NodeRef(a), Self::NodeRef(b)) => a == b,
            (
                Self::ReactiveRef {
                    namespace: a_ns,
                    field: a_field,
                    index: a_index,
                    kind: a_kind,
                    ..
                },
                Self::ReactiveRef {
                    namespace: b_ns,
                    field: b_field,
                    index: b_index,
                    kind: b_kind,
                    ..
                },
            ) => a_ns == b_ns && a_field == b_field && a_index == b_index && a_kind == b_kind,
            (
                Self::HostHandle {
                    kind: a_kind,
                    id: a_id,
                    ..
                },
                Self::HostHandle {
                    kind: b_kind,
                    id: b_id,
                    ..
                },
            ) => a_kind == b_kind && a_id == b_id,
            _ => false,
        }
    }
}

impl Clone for Value {
    fn clone(&self) -> Self {
        match self {
            Self::Number(n) => Self::Number(*n),
            Self::Bool(b) => Self::Bool(*b),
            Self::Nil => Self::Nil,
            Self::String(s) => Self::String(s.clone()),
            Self::Symbol(s) => Self::Symbol(s.clone()),
            Self::Keyword(s) => Self::Keyword(s.clone()),
            Self::List(l) => Self::List(l.clone()),
            Self::Map(m) => Self::Map(m.clone()),
            Self::Closure(i, u) => Self::Closure(*i, u.clone()),
            Self::Function(i) => Self::Function(*i),
            Self::NodeRef(id) => Self::NodeRef(*id),
            Self::ReactiveRef {
                namespace,
                field,
                index,
                kind,
                slot,
            } => Self::ReactiveRef {
                namespace: namespace.clone(),
                field: field.clone(),
                index: *index,
                kind: *kind,
                slot: slot.clone(),
            },
            Self::NativeFunction(f) => Self::NativeFunction(f.clone()),
            Self::HostHandle { kind, id, callable } => Self::HostHandle {
                kind: kind.clone(),
                id: *id,
                callable: callable.clone(),
            },
        }
    }
}

fn expr_head_symbol(expr: &Expr) -> Option<&str> {
    match &expr.kind {
        ExprKind::List(items) => items.first().and_then(|head| match &head.kind {
            ExprKind::Symbol(symbol) => Some(symbol.as_str()),
            _ => None,
        }),
        _ => None,
    }
}

fn collect_defwidget_names(expr: &Expr, out: &mut HashSet<String>) {
    let ExprKind::List(items) = &expr.kind else {
        return;
    };
    if matches!(items.first().map(|item| &item.kind), Some(ExprKind::Symbol(head)) if head == "defwidget")
        && let Some(ExprKind::Symbol(name)) = items.get(1).map(|item| &item.kind)
    {
        out.insert(name.clone());
    }
    for item in items {
        collect_defwidget_names(item, out);
    }
}

fn collect_defmacro_names(expr: &Expr, out: &mut HashSet<String>) {
    let ExprKind::List(items) = &expr.kind else {
        return;
    };
    if matches!(items.first().map(|item| &item.kind), Some(ExprKind::Symbol(head)) if head == "defmacro")
        && let Some(ExprKind::Symbol(name)) = items.get(1).map(|item| &item.kind)
    {
        out.insert(name.clone());
    }
    for item in items {
        collect_defmacro_names(item, out);
    }
}

fn is_source_prop_keyword(expr: &Expr) -> bool {
    matches!(
        &expr.kind,
        ExprKind::Keyword(key)
            if key == SOURCE_START_BYTE_PROP
                || key == SOURCE_END_BYTE_PROP
                || key == SOURCE_REVISION_PROP
    )
}

fn is_widget_constructor_name(name: &str, local_defwidgets: &HashSet<String>) -> bool {
    crate::widgets::is_builtin_widget_name(name)
        || is_inline_widget_constructor_name(name)
        || local_defwidgets.contains(name)
        || crate::widget_render::sdf_widget::sdf_widget_def(name).is_some()
}

fn is_inline_widget_constructor_name(name: &str) -> bool {
    matches!(name, "~slider" | "~knob" | "~toggle" | "~scope" | "~lane")
}

fn static_inline_args(items: &[Expr]) -> Option<Vec<Value>> {
    items.iter().skip(1).map(static_inline_value).collect()
}

fn static_inline_value(expr: &Expr) -> Option<Value> {
    match &expr.kind {
        ExprKind::Number(value) => Some(Value::Number(*value)),
        ExprKind::String(value) => Some(Value::String(value.clone())),
        ExprKind::Keyword(value) => Some(Value::Keyword(value.clone())),
        ExprKind::Symbol(value) if value == "nil" => Some(Value::Nil),
        ExprKind::List(items) if matches!(items.first().map(|item| &item.kind), Some(ExprKind::Symbol(name)) if name == "lane") =>
        {
            let mut values = vec![Rc::new(RefCell::new(Value::Keyword(
                "__process-lane".to_string(),
            )))];
            for item in items.iter().skip(1) {
                values.push(Rc::new(RefCell::new(static_inline_value(item)?)));
            }
            Some(Value::List(values))
        }
        _ => None,
    }
}

type InlineWidgetSourceIdentity = (String, usize, usize);

fn inline_widget_source_identity(widget: &Value) -> Option<InlineWidgetSourceIdentity> {
    let Value::Map(map) = widget else {
        return None;
    };
    let revision = map.get(SOURCE_REVISION_PROP).and_then(|value| {
        let value = value.borrow();
        match &*value {
            Value::String(revision) => Some(revision.clone()),
            _ => None,
        }
    })?;
    let source_byte = |key: &str| {
        map.get(key).and_then(|value| {
            let value = value.borrow();
            match &*value {
                Value::Number(byte) if byte.is_finite() && *byte >= 0.0 => Some(*byte as usize),
                _ => None,
            }
        })
    };
    Some((
        revision,
        source_byte(SOURCE_START_BYTE_PROP)?,
        source_byte(SOURCE_END_BYTE_PROP)?,
    ))
}

fn inline_widget_parent_identity(widget: &Value) -> Option<(String, String)> {
    let Value::Map(map) = widget else {
        return None;
    };
    let string_prop = |key: &str| {
        map.get(key).and_then(|value| match &*value.borrow() {
            Value::String(value) => Some(value.clone()),
            _ => None,
        })
    };
    Some((
        string_prop(INLINE_PARENT_CALLEE_PROP)?,
        string_prop(INLINE_PARENT_INLET_PROP)?,
    ))
}

fn is_source_quoted_prop_key(key: &str) -> bool {
    matches!(key, "shader" | "material" | "state" | "bindable")
}

fn convert_source_data_expr(
    expr: &Expr,
    source_revision: u64,
    local_defwidgets: &HashSet<String>,
    macro_names: &HashSet<String>,
) -> Expression {
    convert_source_expr(expr, source_revision, local_defwidgets, macro_names, false)
}

fn convert_let_bindings(
    expr: &Expr,
    source_revision: u64,
    local_defwidgets: &HashSet<String>,
    macro_names: &HashSet<String>,
) -> Expression {
    let ExprKind::List(bindings) = &expr.kind else {
        return convert_source_data_expr(expr, source_revision, local_defwidgets, macro_names);
    };
    Expression::List(
        bindings
            .iter()
            .map(|binding| {
                let ExprKind::List(parts) = &binding.kind else {
                    return convert_source_data_expr(
                        binding,
                        source_revision,
                        local_defwidgets,
                        macro_names,
                    );
                };
                Expression::List(
                    parts
                        .iter()
                        .enumerate()
                        .map(|(idx, part)| {
                            convert_source_expr(
                                part,
                                source_revision,
                                local_defwidgets,
                                macro_names,
                                idx > 0,
                            )
                        })
                        .collect(),
                )
            })
            .collect(),
    )
}

fn convert_list_with_code_from_idx(
    items: &[Expr],
    source_revision: u64,
    local_defwidgets: &HashSet<String>,
    macro_names: &HashSet<String>,
    code_start_idx: usize,
) -> Expression {
    Expression::List(
        items
            .iter()
            .enumerate()
            .map(|(idx, item)| {
                convert_source_expr(
                    item,
                    source_revision,
                    local_defwidgets,
                    macro_names,
                    idx >= code_start_idx,
                )
            })
            .collect(),
    )
}

fn convert_defwidget_list(
    items: &[Expr],
    source_revision: u64,
    local_defwidgets: &HashSet<String>,
    macro_names: &HashSet<String>,
) -> Expression {
    let mut converted = Vec::with_capacity(items.len());
    let mut idx = 0;
    while idx < items.len() {
        let item = &items[idx];
        converted.push(convert_source_expr(
            item,
            source_revision,
            local_defwidgets,
            macro_names,
            idx > 1,
        ));
        if let ExprKind::Keyword(key) = &item.kind
            && is_source_quoted_prop_key(key)
            && let Some(next) = items.get(idx + 1)
        {
            converted.push(convert_source_data_expr(
                next,
                source_revision,
                local_defwidgets,
                macro_names,
            ));
            idx += 2;
            continue;
        }
        idx += 1;
    }
    Expression::List(converted)
}

fn convert_match_list(
    items: &[Expr],
    source_revision: u64,
    local_defwidgets: &HashSet<String>,
    macro_names: &HashSet<String>,
) -> Expression {
    Expression::List(
        items
            .iter()
            .enumerate()
            .map(|(idx, item)| {
                let is_code = idx == 1 || (idx > 2 && idx % 2 == 1);
                convert_source_expr(
                    item,
                    source_revision,
                    local_defwidgets,
                    macro_names,
                    is_code,
                )
            })
            .collect(),
    )
}

fn convert_source_expr(
    expr: &Expr,
    source_revision: u64,
    local_defwidgets: &HashSet<String>,
    macro_names: &HashSet<String>,
    annotate_widgets: bool,
) -> Expression {
    match &expr.kind {
        ExprKind::Symbol(value) => Expression::Symbol(value.clone()),
        ExprKind::Keyword(value) => Expression::Keyword(value.clone()),
        ExprKind::String(value) => Expression::String(value.clone()),
        ExprKind::QuoteSymbol(value) => Expression::QuoteSymbol(value.clone()),
        ExprKind::QuoteList(items) => {
            Expression::QuoteList(items.iter().map(Expr::to_legacy).collect())
        }
        ExprKind::Number(value) => Expression::Number(*value),
        ExprKind::Quasiquote(inner) => Expression::Quasiquote(Box::new(inner.to_legacy())),
        ExprKind::Unquote(inner) => Expression::Unquote(Box::new(convert_source_expr(
            inner,
            source_revision,
            local_defwidgets,
            macro_names,
            annotate_widgets,
        ))),
        ExprKind::List(items) => {
            let mut converted = Vec::with_capacity(items.len() + 6);
            let head_name = expr_head_symbol(expr);
            if !annotate_widgets {
                return Expression::List(
                    items
                        .iter()
                        .map(|item| {
                            convert_source_data_expr(
                                item,
                                source_revision,
                                local_defwidgets,
                                macro_names,
                            )
                        })
                        .collect(),
                );
            }
            match head_name {
                Some("let" | "let*") if items.len() >= 2 => {
                    converted.push(convert_source_data_expr(
                        &items[0],
                        source_revision,
                        local_defwidgets,
                        macro_names,
                    ));
                    converted.push(convert_let_bindings(
                        &items[1],
                        source_revision,
                        local_defwidgets,
                        macro_names,
                    ));
                    converted.extend(items[2..].iter().map(|item| {
                        convert_source_expr(
                            item,
                            source_revision,
                            local_defwidgets,
                            macro_names,
                            true,
                        )
                    }));
                    return Expression::List(converted);
                }
                Some("lambda") if items.len() >= 2 => {
                    return convert_list_with_code_from_idx(
                        items,
                        source_revision,
                        local_defwidgets,
                        macro_names,
                        2,
                    );
                }
                Some("def") if items.len() >= 3 => {
                    let body_start_idx =
                        if matches!(items.get(2).map(|item| &item.kind), Some(ExprKind::List(_)))
                            && items.len() >= 4
                        {
                            3
                        } else {
                            2
                        };
                    return convert_list_with_code_from_idx(
                        items,
                        source_revision,
                        local_defwidgets,
                        macro_names,
                        body_start_idx,
                    );
                }
                Some("defmacro") => {
                    return Expression::List(
                        items
                            .iter()
                            .map(|item| {
                                convert_source_data_expr(
                                    item,
                                    source_revision,
                                    local_defwidgets,
                                    macro_names,
                                )
                            })
                            .collect(),
                    );
                }
                Some("defwidget") => {
                    return convert_defwidget_list(
                        items,
                        source_revision,
                        local_defwidgets,
                        macro_names,
                    );
                }
                Some("match") => {
                    return convert_match_list(
                        items,
                        source_revision,
                        local_defwidgets,
                        macro_names,
                    );
                }
                _ => {}
            }
            let is_macro_call =
                annotate_widgets && head_name.is_some_and(|name| macro_names.contains(name));
            let should_annotate = annotate_widgets
                && !is_macro_call
                && head_name.is_some_and(|name| is_widget_constructor_name(name, local_defwidgets));
            let mut idx = 0;
            while idx < items.len() {
                let item = &items[idx];
                if should_annotate && idx > 0 && is_source_prop_keyword(item) {
                    idx += 2;
                    continue;
                }
                let mut converted_item = convert_source_expr(
                    item,
                    source_revision,
                    local_defwidgets,
                    macro_names,
                    annotate_widgets,
                );
                if let (
                    Some(parent_callee),
                    Some(ExprKind::Keyword(parent_inlet)),
                    ExprKind::List(child_items),
                    Expression::List(child_converted),
                ) = (
                    head_name,
                    idx.checked_sub(1)
                        .and_then(|previous| items.get(previous))
                        .map(|expr| &expr.kind),
                    &item.kind,
                    &mut converted_item,
                ) && child_items.first().is_some_and(|head| {
                    matches!(&head.kind, ExprKind::Symbol(name) if is_inline_widget_constructor_name(name))
                }) {
                    child_converted.push(Expression::Keyword(
                        INLINE_PARENT_CALLEE_PROP.to_string(),
                    ));
                    child_converted.push(Expression::String(parent_callee.to_string()));
                    child_converted.push(Expression::Keyword(
                        INLINE_PARENT_INLET_PROP.to_string(),
                    ));
                    child_converted.push(Expression::String(parent_inlet.clone()));
                }
                converted.push(converted_item);
                if let ExprKind::Keyword(key) = &item.kind
                    && is_source_quoted_prop_key(key)
                    && let Some(next) = items.get(idx + 1)
                {
                    converted.push(convert_source_expr(
                        next,
                        source_revision,
                        local_defwidgets,
                        macro_names,
                        false,
                    ));
                    idx += 2;
                    continue;
                }
                idx += 1;
            }
            if should_annotate && !converted.is_empty() {
                if head_name
                    .is_some_and(|name| matches!(name, "~slider" | "~knob" | "~toggle" | "~lane"))
                    && let Some(value_expr) = items.get(1)
                {
                    converted.push(Expression::Keyword(
                        INLINE_VALUE_START_BYTE_PROP.to_string(),
                    ));
                    converted.push(Expression::Number(
                        value_expr.origin.primary_span.start_byte as f64,
                    ));
                    converted.push(Expression::Keyword(INLINE_VALUE_END_BYTE_PROP.to_string()));
                    converted.push(Expression::Number(
                        value_expr.origin.primary_span.end_byte as f64,
                    ));
                }
                converted.push(Expression::Keyword(SOURCE_START_BYTE_PROP.to_string()));
                converted.push(Expression::Number(
                    expr.origin.primary_span.start_byte as f64,
                ));
                converted.push(Expression::Keyword(SOURCE_END_BYTE_PROP.to_string()));
                converted.push(Expression::Number(expr.origin.primary_span.end_byte as f64));
                converted.push(Expression::Keyword(SOURCE_REVISION_PROP.to_string()));
                converted.push(Expression::String(source_revision.to_string()));
            }
            let converted = Expression::List(converted);
            if is_macro_call {
                Expression::List(vec![
                    Expression::Symbol(SOURCE_ORIGIN_NATIVE.to_string()),
                    Expression::Number(expr.origin.primary_span.start_byte as f64),
                    Expression::Number(expr.origin.primary_span.end_byte as f64),
                    Expression::String(source_revision.to_string()),
                    converted,
                ])
            } else {
                converted
            }
        }
    }
}

fn convert_source_exprs_with_origins(
    exprs: &[Expr],
    source_revision: u64,
    existing_macros: &HashSet<String>,
) -> Vec<Expression> {
    let mut local_defwidgets = HashSet::new();
    let mut macro_names = existing_macros.clone();
    for expr in exprs {
        collect_defwidget_names(expr, &mut local_defwidgets);
        collect_defmacro_names(expr, &mut macro_names);
    }
    exprs
        .iter()
        .map(|expr| {
            convert_source_expr(expr, source_revision, &local_defwidgets, &macro_names, true)
        })
        .collect()
}

fn source_origin_number_arg(value: &Value) -> Option<usize> {
    match value {
        Value::Number(number) if number.is_finite() && *number >= 0.0 => Some(*number as usize),
        _ => None,
    }
}

fn source_origin_revision_arg(value: &Value) -> Option<String> {
    match value {
        Value::String(value) if value.parse::<u64>().is_ok() => Some(value.clone()),
        Value::Number(number) if number.is_finite() && *number >= 0.0 => {
            Some((*number as u64).to_string())
        }
        _ => None,
    }
}

fn stamp_source_origin_value(value: &Value, start: usize, end: usize, revision: &str) -> Value {
    match value {
        Value::Map(map) => {
            let mut stamped = map
                .iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        Rc::new(RefCell::new(stamp_source_origin_value(
                            &value.borrow(),
                            start,
                            end,
                            revision,
                        ))),
                    )
                })
                .collect::<HashMap<_, _>>();
            if stamped.contains_key("type") && !stamped.contains_key(SOURCE_START_BYTE_PROP) {
                stamped.insert(
                    SOURCE_START_BYTE_PROP.to_string(),
                    Rc::new(RefCell::new(Value::Number(start as f64))),
                );
                stamped.insert(
                    SOURCE_END_BYTE_PROP.to_string(),
                    Rc::new(RefCell::new(Value::Number(end as f64))),
                );
                stamped.insert(
                    SOURCE_REVISION_PROP.to_string(),
                    Rc::new(RefCell::new(Value::String(revision.to_string()))),
                );
            }
            Value::Map(stamped)
        }
        Value::List(items) => Value::List(
            items
                .iter()
                .map(|item| {
                    Rc::new(RefCell::new(stamp_source_origin_value(
                        &item.borrow(),
                        start,
                        end,
                        revision,
                    )))
                })
                .collect(),
        ),
        other => other.deep_clone(),
    }
}

fn source_origin_native(args: Vec<Value>) -> Value {
    let [start, end, revision, value] = args.as_slice() else {
        return args.last().cloned().unwrap_or(Value::Nil);
    };
    let Some(start) = source_origin_number_arg(start) else {
        return value.deep_clone();
    };
    let Some(end) = source_origin_number_arg(end) else {
        return value.deep_clone();
    };
    let Some(revision) = source_origin_revision_arg(revision) else {
        return value.deep_clone();
    };
    stamp_source_origin_value(value, start, end, &revision)
}

impl Value {
    pub fn deep_clone(&self) -> Self {
        match self {
            Self::Number(n) => Self::Number(*n),
            Self::Bool(b) => Self::Bool(*b),
            Self::Nil => Self::Nil,
            Self::String(s) => Self::String(s.clone()),
            Self::Symbol(s) => Self::Symbol(s.clone()),
            Self::Keyword(s) => Self::Keyword(s.clone()),
            Self::List(items) => Self::List(
                items
                    .iter()
                    .map(|item| Rc::new(RefCell::new(item.borrow().deep_clone())))
                    .collect(),
            ),
            Self::Map(map) => Self::Map(
                map.iter()
                    .map(|(key, value)| {
                        (
                            key.clone(),
                            Rc::new(RefCell::new(value.borrow().deep_clone())),
                        )
                    })
                    .collect(),
            ),
            Self::Closure(i, upvalues) => Self::Closure(*i, upvalues.clone()),
            Self::Function(i) => Self::Function(*i),
            Self::NodeRef(id) => Self::NodeRef(*id),
            Self::ReactiveRef {
                namespace,
                field,
                index,
                kind,
                slot,
            } => Self::ReactiveRef {
                namespace: namespace.clone(),
                field: field.clone(),
                index: *index,
                kind: *kind,
                slot: slot.clone(),
            },
            Self::NativeFunction(f) => Self::NativeFunction(f.clone()),
            Self::HostHandle { kind, id, callable } => Self::HostHandle {
                kind: kind.clone(),
                id: *id,
                callable: callable.clone(),
            },
        }
    }
}

fn stable_key_value(map: &HashMap<String, Rc<RefCell<Value>>>) -> Option<String> {
    map.get("key").and_then(|value| match &*value.borrow() {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        Value::Keyword(s) => Some(format!(":{s}")),
        Value::Symbol(s) => Some(s.clone()),
        _ => None,
    })
}

fn stable_widget_hash(
    source_buffer_id: Option<BufferId>,
    target: &EffectTarget,
    parent_stable_id: Option<u64>,
    widget_type: &str,
    path: &[usize],
    key: Option<&str>,
) -> u64 {
    const MAX_SAFE_F64_INT: u64 = (1u64 << 53) - 1;
    let mut hasher = DefaultHasher::new();
    source_buffer_id.hash(&mut hasher);
    target.hash(&mut hasher);
    // Paths are local to an evaluated subtree. Namespace descendants by their
    // parent so identically shaped keyed subtrees cannot assign the same stable
    // identity to their focusable children.
    parent_stable_id.hash(&mut hasher);
    widget_type.hash(&mut hasher);
    path.hash(&mut hasher);
    key.hash(&mut hasher);
    hasher.finish() & MAX_SAFE_F64_INT
}

fn explicit_subtree_root_hash(
    source_buffer_id: Option<BufferId>,
    target: &EffectTarget,
    key: &str,
) -> u64 {
    const MAX_SAFE_F64_INT: u64 = (1u64 << 53) - 1;
    let mut hasher = DefaultHasher::new();
    source_buffer_id.hash(&mut hasher);
    target.hash(&mut hasher);
    "explicit-subtree".hash(&mut hasher);
    key.hash(&mut hasher);
    hasher.finish() & MAX_SAFE_F64_INT
}

fn subtree_key_string(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        Value::Keyword(s) => Some(format!(":{s}")),
        Value::Symbol(s) => Some(s.clone()),
        _ => None,
    }
}

fn prop_u64_rc(map: &HashMap<String, Rc<RefCell<Value>>>, key: &str) -> Option<u64> {
    match map.get(key).map(|value| value.borrow().clone()) {
        Some(Value::Number(n)) if n >= 0.0 && n.fract() == 0.0 => Some(n as u64),
        _ => None,
    }
}

fn prop_string_rc(map: &HashMap<String, Rc<RefCell<Value>>>, key: &str) -> Option<String> {
    match map.get(key).map(|value| value.borrow().clone()) {
        Some(Value::String(s)) => Some(s),
        _ => None,
    }
}

fn explicit_subtree_root_metadata(value: &Value) -> Option<(u64, String)> {
    let Value::Map(map) = value else {
        return None;
    };
    let root_id = prop_u64_rc(map, SUBTREE_ROOT_ID_PROP)?;
    let stable_key = prop_string_rc(map, STABLE_KEY_PROP)?;
    Some((root_id, stable_key))
}

fn annotate_explicit_subtree_root(
    value: &Value,
    subtree_root_id: u64,
    parent_root_id: Option<u64>,
    stable_key: &str,
    is_root: bool,
) -> Value {
    let Value::Map(map) = value else {
        return value.deep_clone();
    };
    let mut annotated = HashMap::new();
    let existing_root_id = prop_u64_rc(map, SUBTREE_ROOT_ID_PROP);
    let next_root_id = if is_root {
        subtree_root_id
    } else {
        existing_root_id.unwrap_or(subtree_root_id)
    };
    let next_parent_root_id = if is_root {
        parent_root_id
    } else if existing_root_id.is_some() {
        prop_u64_rc(map, PARENT_SUBTREE_ROOT_ID_PROP).or(Some(subtree_root_id))
    } else {
        Some(subtree_root_id)
    };

    for (name, child) in map {
        if name == "children" {
            let annotated_children = match &*child.borrow() {
                Value::List(children) => Value::List(
                    children
                        .iter()
                        .map(|child| {
                            Rc::new(RefCell::new(annotate_explicit_subtree_root(
                                &child.borrow(),
                                subtree_root_id,
                                Some(subtree_root_id),
                                stable_key,
                                false,
                            )))
                        })
                        .collect(),
                ),
                other => other.deep_clone(),
            };
            annotated.insert(name.clone(), Rc::new(RefCell::new(annotated_children)));
        } else {
            annotated.insert(
                name.clone(),
                Rc::new(RefCell::new(child.borrow().deep_clone())),
            );
        }
    }

    annotated.insert(
        SUBTREE_ROOT_ID_PROP.to_string(),
        Rc::new(RefCell::new(Value::Number(next_root_id as f64))),
    );
    if let Some(parent_root_id) = next_parent_root_id {
        annotated.insert(
            PARENT_SUBTREE_ROOT_ID_PROP.to_string(),
            Rc::new(RefCell::new(Value::Number(parent_root_id as f64))),
        );
    }
    if is_root {
        annotated.insert(
            STABLE_KEY_PROP.to_string(),
            Rc::new(RefCell::new(Value::String(stable_key.to_string()))),
        );
    }

    Value::Map(annotated)
}

fn annotate_widget_tree_stable_ids(
    value: &Value,
    source_buffer_id: Option<BufferId>,
    source_file: Option<&std::path::Path>,
    target: &EffectTarget,
    parent_stable_id: Option<u64>,
    path: &mut Vec<usize>,
) -> Value {
    let Value::Map(map) = value else {
        return value.deep_clone();
    };
    let Some(widget_type) = map.get("type").and_then(|value| match &*value.borrow() {
        Value::Keyword(widget_type) => Some(widget_type.clone()),
        Value::String(widget_type) => Some(widget_type.clone()),
        _ => None,
    }) else {
        return value.deep_clone();
    };

    let key = prop_string_rc(map, STABLE_KEY_PROP).or_else(|| stable_key_value(map));
    let stable_id = stable_widget_hash(
        source_buffer_id,
        target,
        parent_stable_id,
        &widget_type,
        path,
        key.as_deref(),
    );
    let mut annotated = HashMap::new();

    for (name, child) in map {
        if name == "children" {
            let annotated_children = match &*child.borrow() {
                Value::List(children) => Value::List(
                    children
                        .iter()
                        .enumerate()
                        .map(|(idx, child)| {
                            path.push(idx);
                            let annotated_child = annotate_widget_tree_stable_ids(
                                &child.borrow(),
                                source_buffer_id,
                                source_file,
                                target,
                                Some(stable_id),
                                path,
                            );
                            path.pop();
                            Rc::new(RefCell::new(annotated_child))
                        })
                        .collect(),
                ),
                other => other.deep_clone(),
            };
            annotated.insert(name.clone(), Rc::new(RefCell::new(annotated_children)));
        } else {
            annotated.insert(
                name.clone(),
                Rc::new(RefCell::new(child.borrow().deep_clone())),
            );
        }
    }

    annotated.insert(
        STABLE_WIDGET_ID_PROP.to_string(),
        Rc::new(RefCell::new(Value::Number(stable_id as f64))),
    );
    if let Some(source_buffer_id) = source_buffer_id {
        annotated.insert(
            SOURCE_BUFFER_ID_PROP.to_string(),
            Rc::new(RefCell::new(Value::Number(source_buffer_id as f64))),
        );
    }
    if let Some(source_file) = source_file
        && !annotated.contains_key(SOURCE_MODULE_PATH_PROP)
    {
        annotated.insert(
            SOURCE_MODULE_PATH_PROP.to_string(),
            Rc::new(RefCell::new(Value::String(
                source_file.display().to_string(),
            ))),
        );
    }
    let subtree_root_id = prop_u64_rc(map, SUBTREE_ROOT_ID_PROP).unwrap_or(stable_id);
    annotated.insert(
        SUBTREE_ROOT_ID_PROP.to_string(),
        Rc::new(RefCell::new(Value::Number(subtree_root_id as f64))),
    );
    if let Some(parent_id) = prop_u64_rc(map, PARENT_SUBTREE_ROOT_ID_PROP).or(parent_stable_id) {
        annotated.insert(
            PARENT_SUBTREE_ROOT_ID_PROP.to_string(),
            Rc::new(RefCell::new(Value::Number(parent_id as f64))),
        );
    }
    if let Some(key) = key {
        annotated.insert(
            STABLE_KEY_PROP.to_string(),
            Rc::new(RefCell::new(Value::String(key))),
        );
    }

    Value::Map(annotated)
}

struct Frame {
    locals: Vec<Option<Rc<RefCell<Value>>>>,
    upvalues: Vec<Rc<RefCell<Value>>>,
    pc: usize,
    chunk_idx: usize,
}

pub struct VM {
    pub chunks: Vec<Chunk>,
    current_chunk: usize,
    globals: Vec<Option<Rc<RefCell<Value>>>>,
    pub global_names: Vec<String>,
    pub pending_widget_trees: Vec<PendingUiUpdate>,
    pending_inline_widgets: Vec<Value>,
    registering_static_inline_widget: bool,
    recent_runtime_inline_widgets: HashMap<(String, String), InlineWidgetSourceIdentity>,
    pub dag: ReactiveDag,
    tracking_stack: Vec<NodeId>,
    pub reactive_namespaces: HashSet<String>,
    pub writable_reactive_namespaces: HashSet<String>,
    pub(crate) reactive_float_slots: crate::reactive::ReactiveBindingStore,
    pending_reactive_sets: Vec<(String, String, Value)>,
    pub derived_bindings: HashMap<String, NodeId>,
    pub state_bindings: HashMap<String, NodeId>,
    execution_depth: usize,
    processing_reactive: bool,
    reactive_exec_timings: Vec<ReactiveExecTiming>,
    last_reactive_error_context: Option<String>,
    last_reactive_error_detail: Option<String>,
    current_effect_source_buffer_id: Option<BufferId>,
    current_effect_target: EffectTarget,
    current_effect_reactive_reads: Option<HashSet<ReactiveFieldKey>>,
    current_effect_symbol_reads: Option<HashSet<String>>,
    current_subtree_capture_stack: Vec<SubtreeCaptureContext>,
    current_subtree_reactive_reads: HashMap<u64, HashSet<ReactiveFieldKey>>,
    pub macros: HashMap<String, MacroDef>,
    pub source_manager: SourceManager,
    pub(crate) source_load_errors: Vec<String>,
    preserve_state_on_redefinition: bool,
    global_store_hooks: Vec<GlobalStoreHook>,
    inline_widget_metadata_resolver: Option<InlineWidgetMetadataResolver>,
    /// Emacs-style extension points: hook name → ordered (entry key, callback)
    /// listeners. Re-adding with an existing key replaces that entry in place,
    /// so re-evaluating a module never duplicates its listeners.
    pub extension_hooks: HashMap<String, Vec<(String, Value)>>,
    /// Modules declared via `(module NAME)` → the file that declared them
    /// (None for include_str!-style sources with no path). `import`
    /// consults this for load-once semantics (spec §4).
    pub declared_modules: HashMap<String, Option<std::path::PathBuf>>,
    /// Migration compat aliases (spec §10 slice 3): old flat name → its
    /// new qualified home, declared via `(module-compat-alias old new)`.
    /// Consulted by both resolution ladders on a bare-name miss; the whole
    /// table (and the form) is deleted at the end of the migration.
    pub compat_aliases: HashMap<String, String>,
}

pub struct VmStateSnapshot {
    chunks: Vec<Chunk>,
    current_chunk: usize,
    globals: Vec<Option<Rc<RefCell<Value>>>>,
    global_names: Vec<String>,
    pending_widget_trees: Vec<PendingUiUpdate>,
    pending_inline_widgets: Vec<Value>,
    registering_static_inline_widget: bool,
    recent_runtime_inline_widgets: HashMap<(String, String), InlineWidgetSourceIdentity>,
    dag: ReactiveDag,
    tracking_stack: Vec<NodeId>,
    reactive_namespaces: HashSet<String>,
    writable_reactive_namespaces: HashSet<String>,
    pending_reactive_sets: Vec<(String, String, Value)>,
    derived_bindings: HashMap<String, NodeId>,
    state_bindings: HashMap<String, NodeId>,
    execution_depth: usize,
    processing_reactive: bool,
    reactive_exec_timings: Vec<ReactiveExecTiming>,
    last_reactive_error_context: Option<String>,
    last_reactive_error_detail: Option<String>,
    current_effect_source_buffer_id: Option<BufferId>,
    current_effect_target: EffectTarget,
    current_effect_reactive_reads: Option<HashSet<ReactiveFieldKey>>,
    current_effect_symbol_reads: Option<HashSet<String>>,
    current_subtree_capture_stack: Vec<SubtreeCaptureContext>,
    current_subtree_reactive_reads: HashMap<u64, HashSet<ReactiveFieldKey>>,
    macros: HashMap<String, MacroDef>,
    source_manager: SourceManager,
    source_load_errors: Vec<String>,
    preserve_state_on_redefinition: bool,
    extension_hooks: HashMap<String, Vec<(String, Value)>>,
    declared_modules: HashMap<String, Option<std::path::PathBuf>>,
    compat_aliases: HashMap<String, String>,
}

fn clone_globals_for_snapshot(
    globals: &[Option<Rc<RefCell<Value>>>],
) -> Vec<Option<Rc<RefCell<Value>>>> {
    globals
        .iter()
        .map(|value| {
            value
                .as_ref()
                .map(|value| Rc::new(RefCell::new(clone_value_for_snapshot(&value.borrow()))))
        })
        .collect()
}

fn clone_value_for_snapshot(value: &Value) -> Value {
    match value {
        Value::List(items) => Value::List(
            items
                .iter()
                .map(|item| Rc::new(RefCell::new(clone_value_for_snapshot(&item.borrow()))))
                .collect(),
        ),
        Value::Map(map) => Value::Map(
            map.iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        Rc::new(RefCell::new(clone_value_for_snapshot(&value.borrow()))),
                    )
                })
                .collect(),
        ),
        Value::Closure(chunk_idx, upvalues) => Value::Closure(
            *chunk_idx,
            upvalues
                .iter()
                .map(|value| Rc::new(RefCell::new(clone_value_for_snapshot(&value.borrow()))))
                .collect(),
        ),
        other => other.deep_clone(),
    }
}

/// Register built-in functions available in all contexts.
pub fn register_core_natives(vm: &mut VM) {
    // (dict :key val :key val ...) → Map
    vm.register_native("dict", |args| {
        let mut map = HashMap::new();
        let mut i = 0;
        while i + 1 < args.len() {
            if let Value::Keyword(k) = &args[i] {
                map.insert(k.clone(), Rc::new(RefCell::new(args[i + 1].clone())));
            }
            i += 2;
        }
        Value::Map(map)
    });

    // (ui/style :pressed (dict ...) :hover (dict ...)) → Map.
    // This is intentionally a thin constructor over dict so style values stay
    // ordinary Lisp data that can be shared, merged, or inspected.
    vm.register_native("ui/style", |args| {
        let mut map = HashMap::new();
        let mut i = 0;
        while i + 1 < args.len() {
            if let Value::Keyword(k) = &args[i] {
                map.insert(k.clone(), Rc::new(RefCell::new(args[i + 1].clone())));
            }
            i += 2;
        }
        Value::Map(map)
    });

    // (get collection :key) → value, or nil if missing.
    // Works on both Maps and keyword-value lists like (:label "foo" :children (...)).
    vm.register_native("get", |args| {
        let Some(Value::Keyword(k)) = args.get(1) else {
            return Value::Nil;
        };
        match args.first() {
            Some(Value::Map(m)) => m.get(k).map(|v| v.borrow().clone()).unwrap_or(Value::Nil),
            Some(Value::List(list)) => {
                let mut i = 0;
                while i + 1 < list.len() {
                    if matches!(&*list[i].borrow(), Value::Keyword(kk) if kk == k) {
                        return list[i + 1].borrow().clone();
                    }
                    i += 2;
                }
                Value::Nil
            }
            _ => Value::Nil,
        }
    });

    vm.register_native_with_vm("reactive-get", |args, vm| {
        let (Some(Value::String(namespace)), Some(Value::String(field))) =
            (args.first(), args.get(1))
        else {
            return Value::Nil;
        };
        vm.record_reactive_read(namespace, field);
        if let Some(ctx_id) = vm.tracking_stack.last().copied() {
            let source_id = vm.get_or_create_source_node(namespace, field);
            vm.dag.add_edge(source_id, ctx_id);
        }
        vm.current_reactive_value(namespace, field)
    });

    vm.register_native_with_vm("reactive-set", |args, vm| {
        let (Some(Value::String(namespace)), Some(Value::String(field)), Some(value)) =
            (args.first(), args.get(1), args.get(2))
        else {
            return Value::Bool(false);
        };
        if !vm.writable_reactive_namespaces.contains(namespace) {
            return Value::Bool(false);
        }
        let value = value.clone();
        match &value {
            Value::Number(number) => vm
                .reactive_float_slots
                .write_float(namespace, field, *number),
            Value::Bool(true) => vm.reactive_float_slots.write_float(namespace, field, 1.0),
            Value::Bool(false) => vm.reactive_float_slots.write_float(namespace, field, 0.0),
            _ => {}
        }
        vm.update_reactive_global(namespace, field, value.clone());
        vm.pending_reactive_sets
            .push((namespace.clone(), field.clone(), value.clone()));
        let source_id = vm.get_or_create_source_node(namespace, field);
        vm.mark_source_dependents_dirty(source_id, value);
        Value::Bool(true)
    });

    vm.register_native_with_vm("bind", |args, vm| {
        let (Some(Value::String(namespace)), Some(Value::String(field))) =
            (args.first(), args.get(1))
        else {
            return Value::Nil;
        };
        reactive_float_ref(&vm.reactive_float_slots, namespace, field)
    });

    vm.register_native_with_vm("bind-seq", |args, vm| {
        let Some(Value::String(field)) = args.first() else {
            return Value::Nil;
        };
        reactive_float_ref(&vm.reactive_float_slots, "SEQ", field)
    });

    vm.register_native_with_vm("reactive-value", |args, vm| {
        let Some(value) = args.first() else {
            return Value::Nil;
        };
        match value {
            Value::ReactiveRef {
                kind: BindingKind::Float,
                namespace,
                field,
                slot,
                ..
            } => {
                vm.record_reactive_read(namespace, field);
                if let Some(ctx_id) = vm.tracking_stack.last().copied() {
                    let source_id = vm.get_or_create_source_node(namespace, field);
                    vm.dag.add_edge(source_id, ctx_id);
                }
                Value::Number(crate::reactive::read_float_slot(slot))
            }
            other => other.clone(),
        }
    });

    vm.register_native_with_vm("bind-nth", |args, vm| {
        let (
            Some(Value::String(namespace)),
            Some(Value::String(field)),
            Some(Value::Number(index)),
        ) = (args.first(), args.get(1), args.get(2))
        else {
            return Value::Nil;
        };
        let Some(index) = binding_index(*index) else {
            return Value::Nil;
        };
        reactive_indexed_float_ref(&vm.reactive_float_slots, namespace, field, index)
    });

    vm.register_native_with_vm("bind-seq-nth", |args, vm| {
        let (Some(Value::String(field)), Some(Value::Number(index))) = (args.first(), args.get(1))
        else {
            return Value::Nil;
        };
        let Some(index) = binding_index(*index) else {
            return Value::Nil;
        };
        reactive_indexed_float_ref(&vm.reactive_float_slots, "SEQ", field, index)
    });

    vm.register_native_with_vm("subtree-owner", |args, vm| {
        let (Some(key_value), Some(callable)) = (args.first(), args.get(1)) else {
            return Value::Nil;
        };
        let Some(stable_key) = subtree_key_string(key_value) else {
            return Value::Nil;
        };
        vm.evaluate_subtree_owner(&stable_key, callable.clone())
            .unwrap_or(Value::Nil)
    });

    // (merge map :key val ...) → new map with overrides
    vm.register_native("merge", |args| {
        let mut map = if let Some(Value::Map(m)) = args.first() {
            m.iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect::<HashMap<_, _>>()
        } else {
            HashMap::new()
        };
        let mut i = 1;
        while i + 1 < args.len() {
            if let Value::Keyword(k) = &args[i] {
                map.insert(k.clone(), Rc::new(RefCell::new(args[i + 1].clone())));
            }
            i += 2;
        }
        Value::Map(map)
    });

    // (keys map) → List of keywords
    vm.register_native("keys", |args| {
        if let Some(Value::Map(m)) = args.first() {
            Value::List(
                m.keys()
                    .map(|k| Rc::new(RefCell::new(Value::Keyword(k.clone()))))
                    .collect(),
            )
        } else {
            Value::Nil
        }
    });

    // (first list) → first element or false
    vm.register_native("first", |args| {
        if let Some(Value::List(l)) = args.first() {
            l.first().map(|v| v.borrow().clone()).unwrap_or(Value::Nil)
        } else {
            Value::Nil
        }
    });

    // (rest list) → tail of list or empty list
    vm.register_native("rest", |args| {
        if let Some(Value::List(l)) = args.first() {
            Value::List(l[1..].to_vec())
        } else {
            Value::List(vec![])
        }
    });

    // (cons val list) → new list with val prepended
    vm.register_native("cons", |args| {
        if let (Some(head), Some(Value::List(tail))) = (args.first(), args.get(1)) {
            let mut new = vec![Rc::new(RefCell::new(head.clone()))];
            new.extend(tail.iter().cloned());
            Value::List(new)
        } else {
            Value::List(vec![])
        }
    });

    // (len list-or-string) → number
    vm.register_native("len", |args| match args.first() {
        Some(Value::List(l)) => Value::Number(l.len() as f64),
        Some(Value::String(s)) => Value::Number(s.chars().count() as f64),
        _ => Value::Number(0.0),
    });

    // (append list ...) → concatenated list
    vm.register_native("append", |args| {
        let mut result = vec![];
        for arg in &args {
            if let Value::List(l) = arg {
                result.extend(l.iter().cloned());
            }
        }
        Value::List(result)
    });

    // (list a b c) -> List
    vm.register_native("list", |args| list_from_values(args));

    vm.register_native("empty?", |args| match args.first() {
        Some(Value::List(items)) => Value::Bool(items.is_empty()),
        Some(Value::String(s)) => Value::Bool(s.is_empty()),
        Some(Value::Map(map)) => Value::Bool(map.is_empty()),
        Some(Value::Nil) | None => Value::Bool(true),
        _ => Value::Bool(false),
    });

    vm.register_native("set-nth", |args| {
        let (Some(Value::List(items)), Some(Value::Number(idx)), Some(value)) =
            (args.first(), args.get(1), args.get(2))
        else {
            return Value::Nil;
        };

        if *idx < 0.0 {
            return Value::List(items.clone());
        }

        let mut out = items.clone();
        let index = *idx as usize;
        if index < out.len() {
            out[index] = Rc::new(RefCell::new(value.clone()));
        }
        Value::List(out)
    });

    vm.register_native_with_vm("each", |args, vm| {
        let (Some(Value::List(items)), owner_path, Some(callback)) =
            (args.first(), args.get(1), args.get(2).cloned())
        else {
            return Value::List(vec![]);
        };

        let owner = match owner_path {
            Some(Value::String(path)) => Some(path.clone()),
            _ => None,
        };

        let mut out = Vec::with_capacity(items.len());
        for (idx, item) in items.iter().enumerate() {
            let mut value = item.borrow().clone();
            if let (Some(owner), Value::Map(map)) = (owner.clone(), &mut value) {
                map.insert(
                    "__eseq_owner".to_string(),
                    Rc::new(RefCell::new(Value::String(owner))),
                );
            }
            let result = vm
                .invoke(callback.clone(), vec![value, Value::Number(idx as f64)])
                .unwrap_or_else(|error| {
                    log_native_callback_error(vm, "each", idx, &error);
                    Some(Value::Nil)
                });
            out.push(Rc::new(RefCell::new(result.unwrap_or(Value::Nil))));
        }
        Value::List(out)
    });

    vm.register_native_with_vm("map", |args, vm| {
        let (Some(callback), Some(Value::List(items))) = (args.first().cloned(), args.get(1))
        else {
            return Value::List(vec![]);
        };

        let mut out = Vec::with_capacity(items.len());
        for (idx, item) in items.iter().enumerate() {
            let mapped = vm
                .invoke(callback.clone(), vec![item.borrow().clone()])
                .unwrap_or_else(|error| {
                    log_native_callback_error(vm, "map", idx, &error);
                    Some(Value::Nil)
                })
                .unwrap_or(Value::Nil);
            out.push(Rc::new(RefCell::new(mapped)));
        }
        Value::List(out)
    });

    vm.register_native_with_vm("filter", |args, vm| {
        let (Some(callback), Some(Value::List(items))) = (args.first().cloned(), args.get(1))
        else {
            return Value::List(vec![]);
        };

        let mut out = Vec::new();
        for (idx, item) in items.iter().enumerate() {
            let item_value = item.borrow().clone();
            let keep = vm
                .invoke(callback.clone(), vec![item_value.clone()])
                .unwrap_or_else(|error| {
                    log_native_callback_error(vm, "filter", idx, &error);
                    Some(Value::Nil)
                })
                .unwrap_or(Value::Nil);
            if !is_falsey(&keep) {
                out.push(Rc::new(RefCell::new(item_value)));
            }
        }
        Value::List(out)
    });

    vm.register_native_with_vm("reduce", |args, vm| {
        let (Some(callback), Some(mut acc), Some(Value::List(items))) =
            (args.first().cloned(), args.get(1).cloned(), args.get(2))
        else {
            return Value::Nil;
        };

        for (idx, item) in items.iter().enumerate() {
            acc = vm
                .invoke(callback.clone(), vec![acc, item.borrow().clone()])
                .unwrap_or_else(|error| {
                    log_native_callback_error(vm, "reduce", idx, &error);
                    Some(Value::Nil)
                })
                .unwrap_or(Value::Nil);
        }
        acc
    });

    vm.register_native_with_vm("for-each", |args, vm| {
        let (Some(callback), Some(Value::List(items))) = (args.first().cloned(), args.get(1))
        else {
            return Value::Nil;
        };

        for (idx, item) in items.iter().enumerate() {
            if let Err(error) = vm.invoke(callback.clone(), vec![item.borrow().clone()]) {
                log_native_callback_error(vm, "for-each", idx, &error);
            }
        }
        Value::Nil
    });

    // Module system (spec §2/§4). (module NAME) compiles to a
    // __module-declare call so the runtime knows the module is loaded and
    // which file declared it; (import NAME …) compiles to __import-module,
    // which resolves the name to a file and evaluates it exactly once.
    vm.register_native_with_vm("__module-declare", |args, vm| {
        let Some(Value::String(name)) = args.first() else {
            log_native_misuse("__module-declare", "expects a module name string");
            return Value::Nil;
        };
        let path = vm.current_source_file();
        vm.declared_modules.insert(name.clone(), path);
        Value::Nil
    });

    // (module-compat-alias old new) compiles to this call (spec §10 slice
    // 3): record old flat name → qualified home so later compile units and
    // runtime by-name lookups resolve legacy callers through the alias.
    vm.register_native_with_vm("__compat-alias", |args, vm| {
        let (Some(Value::String(old)), Some(Value::String(target))) =
            (args.first(), args.get(1))
        else {
            log_native_misuse("__compat-alias", "expects (old-name target-name) strings");
            return Value::Nil;
        };
        if crate::modules::is_qualified(old) || !crate::modules::is_qualified(target) {
            log_native_misuse(
                "__compat-alias",
                "old name must be flat and target must be qualified",
            );
            return Value::Nil;
        }
        vm.compat_aliases.insert(old.clone(), target.clone());
        Value::Nil
    });

    vm.register_native_with_vm("__import-module", |args, vm| {
        let Some(Value::String(name)) = args.first() else {
            log_native_misuse("__import-module", "expects a module name string");
            return Value::Nil;
        };
        if vm.declared_modules.contains_key(name)
            || crate::modules::CORE_NAMESPACES.contains(&name.as_str())
            || name == crate::modules::IMPLICIT_MODULE
        {
            return Value::Nil; // load-once: already evaluated (or built in)
        }
        let mut errors = Vec::new();
        for candidate in crate::modules::module_file_candidates(name) {
            match vm.source_manager.load_source(&candidate) {
                Ok(loaded) => {
                    let path_display = loaded.path.display().to_string();
                    return match vm.eval_module_source(
                        loaded.path,
                        &loaded.text,
                        loaded.revision,
                    ) {
                        Ok(_) => {
                            if !vm.declared_modules.contains_key(name) {
                                vm.source_load_errors.push(format!(
                                    "import {name}: {path_display} did not declare \
                                     (module {name})"
                                ));
                            }
                            Value::Nil
                        }
                        Err(e) => {
                            let message =
                                format!("import {name}: {path_display}: eval error: {e:?}");
                            vm.source_load_errors.push(message.clone());
                            Value::String(message)
                        }
                    };
                }
                Err(error) => errors.push(error),
            }
        }
        let message = format!(
            "import {name}: no module file found ({})",
            errors.join("; ")
        );
        vm.source_load_errors.push(message.clone());
        Value::String(message)
    });

    // Emacs-style extension hooks. (defhook "name") declares a hook and
    // defines a global function of that name that runs its listeners, so call
    // sites invoke hooks like ordinary functions. (add-hook name key fn)
    // registers/replaces a listener; the key makes re-evaluation idempotent.
    vm.register_native_with_vm("defhook", |args, vm| {
        let Some(Value::String(name)) = args.first().cloned() else {
            log_native_misuse("defhook", "expects a hook name string");
            return Value::Nil;
        };
        vm.extension_hooks.entry(name.clone()).or_default();
        let hook_name = name.clone();
        vm.register_native_with_vm(&name, move |args, vm| {
            run_extension_hook(vm, &hook_name, args)
        });
        Value::Nil
    });

    vm.register_native_with_vm("add-hook", |mut args, vm| {
        if args.len() != 3 {
            log_native_misuse("add-hook", "expects (add-hook hook-name entry-key callback)");
            return Value::Nil;
        }
        let callback = args.pop().expect("checked length");
        let (Value::String(name), Value::String(key)) = (args.remove(0), args.remove(0)) else {
            log_native_misuse("add-hook", "hook name and entry key must be strings");
            return Value::Nil;
        };
        let entries = vm.extension_hooks.entry(name).or_default();
        if let Some(existing) = entries.iter_mut().find(|(existing_key, _)| *existing_key == key)
        {
            existing.1 = callback;
        } else {
            entries.push((key, callback));
        }
        Value::Nil
    });

    vm.register_native_with_vm("remove-hook", |args, vm| {
        let (Some(Value::String(name)), Some(Value::String(key))) = (args.first(), args.get(1))
        else {
            log_native_misuse("remove-hook", "expects (remove-hook hook-name entry-key)");
            return Value::Nil;
        };
        if let Some(entries) = vm.extension_hooks.get_mut(name) {
            entries.retain(|(existing_key, _)| existing_key != key);
        }
        Value::Nil
    });

    vm.register_native_with_vm("run-hook", |mut args, vm| {
        if args.is_empty() {
            log_native_misuse("run-hook", "expects (run-hook hook-name args...)");
            return Value::Nil;
        }
        let Value::String(name) = args.remove(0) else {
            log_native_misuse("run-hook", "hook name must be a string");
            return Value::Nil;
        };
        run_extension_hook(vm, &name, args)
    });

    vm.register_native("zip", |args| {
        let lists = args
            .iter()
            .map(|arg| match arg {
                Value::List(items) => Some(items),
                _ => None,
            })
            .collect::<Option<Vec<_>>>();
        let Some(lists) = lists else {
            return Value::List(vec![]);
        };
        let Some(limit) = lists.iter().map(|items| items.len()).min() else {
            return Value::List(vec![]);
        };

        let mut zipped = Vec::with_capacity(limit);
        for idx in 0..limit {
            let row = lists
                .iter()
                .map(|items| items[idx].borrow().clone())
                .collect::<Vec<_>>();
            zipped.push(Rc::new(RefCell::new(list_from_values(row))));
        }
        Value::List(zipped)
    });

    // (nth list idx) -> value or nil; idx is 0-based
    vm.register_native("nth", |args| {
        let (Some(Value::List(list)), Some(Value::Number(idx))) = (args.first(), args.get(1))
        else {
            return Value::Nil;
        };
        if *idx < 0.0 {
            return Value::Nil;
        }
        list.get(*idx as usize)
            .map(|value| value.borrow().clone())
            .unwrap_or(Value::Nil)
    });

    // (reverse list) -> reversed list
    vm.register_native("reverse", |args| {
        let Some(Value::List(list)) = args.first() else {
            return Value::List(vec![]);
        };
        let mut result = list.clone();
        result.reverse();
        Value::List(result)
    });

    // (chunks list n) -> list of sub-lists, each up to n elements
    vm.register_native("chunks", |args| {
        let (Some(Value::List(list)), Some(Value::Number(n))) = (args.first(), args.get(1)) else {
            return Value::List(vec![]);
        };
        let size = *n as usize;
        if size == 0 {
            return Value::List(vec![]);
        }
        Value::List(
            list.chunks(size)
                .map(|chunk| Rc::new(RefCell::new(Value::List(chunk.to_vec()))))
                .collect(),
        )
    });

    // (range end) or (range start end) -> list of numbers
    vm.register_native("range", |args| {
        let (start, end) = match args.as_slice() {
            [Value::Number(end)] => (0_i64, *end as i64),
            [Value::Number(start), Value::Number(end)] => (*start as i64, *end as i64),
            _ => return Value::List(vec![]),
        };

        let mut values = Vec::new();
        if start <= end {
            for n in start..end {
                values.push(Rc::new(RefCell::new(Value::Number(n as f64))));
            }
        } else {
            for n in (end + 1..=start).rev() {
                values.push(Rc::new(RefCell::new(Value::Number(n as f64))));
            }
        }
        Value::List(values)
    });

    // (rand-int end) or (rand-int start end) -> integer in [0,end) or [start,end)
    vm.register_native("rand-int", |args| {
        let (start, end) = match args.as_slice() {
            [Value::Number(end)] => (0_i64, *end as i64),
            [Value::Number(start), Value::Number(end)] => (*start as i64, *end as i64),
            _ => return Value::Nil,
        };

        if end <= start {
            return Value::Nil;
        }

        let span = (end - start) as u64;
        let mut state = RAND_STATE.load(Ordering::Relaxed);
        loop {
            let next = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            match RAND_STATE.compare_exchange_weak(
                state,
                next,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    let value = start + (next % span) as i64;
                    return Value::Number(value as f64);
                }
                Err(current) => state = current,
            }
        }
    });

    // (not val) → bool
    vm.register_native("not", |args| {
        Value::Bool(matches!(
            args.first(),
            Some(Value::Bool(false)) | Some(Value::Nil) | None
        ))
    });

    // (str val ...) → concatenated Lisp string representation
    vm.register_native("str", |args| {
        let mut s = String::new();
        for v in &args {
            match v {
                Value::String(val) => s.push_str(val),
                other => s.push_str(&format_lisp_value(other)),
            }
        }
        Value::String(s)
    });

    // (substring s start [end]) → character-indexed substring
    vm.register_native("substring", |args| {
        let Some(Value::String(s)) = args.first() else {
            return Value::Nil;
        };
        let Some(Value::Number(start)) = args.get(1) else {
            return Value::Nil;
        };

        let chars: Vec<char> = s.chars().collect();
        let len = chars.len();
        let start = (*start).max(0.0) as usize;
        let start = start.min(len);
        let end = match args.get(2) {
            Some(Value::Number(end)) => ((*end).max(0.0) as usize).min(len),
            _ => len,
        };

        if end < start {
            return Value::String(String::new());
        }

        Value::String(chars[start..end].iter().collect())
    });

    // (str-contains? haystack needle) → case-insensitive substring test
    vm.register_native("str-contains?", |args| {
        let (Some(Value::String(haystack)), Some(Value::String(needle))) =
            (args.first(), args.get(1))
        else {
            return Value::Bool(false);
        };
        Value::Bool(
            haystack
                .to_lowercase()
                .contains(needle.to_lowercase().as_str()),
        )
    });

    // (source val ...) → concatenated evaluable Lisp source
    vm.register_native("source", |args| {
        let mut s = String::new();
        for v in &args {
            s.push_str(&format_lisp_source(v));
        }
        Value::String(s)
    });

    vm.register_native("fmt", |args| {
        let Some(Value::String(template)) = args.first() else {
            return Value::Nil;
        };

        let mut rendered = template.clone();
        for value in args.iter().skip(1) {
            let mut replaced = false;
            let mut search_from = 0;
            while let Some(relative_idx) = rendered[search_from..].find('{') {
                let idx = search_from + relative_idx;
                let Some((len, spec)) = parse_fmt_placeholder(&rendered, idx) else {
                    search_from = idx + 1;
                    continue;
                };
                let replacement = format_fmt_value(value, &spec);
                rendered.replace_range(idx..idx + len, &replacement);
                replaced = true;
                break;
            }
            if !replaced {
                break;
            }
        }
        Value::String(rendered)
    });
}

fn reactive_float_ref(
    slots: &crate::reactive::ReactiveBindingStore,
    namespace: &str,
    field: &str,
) -> Value {
    Value::ReactiveRef {
        namespace: namespace.to_string(),
        field: field.to_string(),
        index: None,
        kind: BindingKind::Float,
        slot: slots.slot(namespace, field),
    }
}

fn reactive_indexed_float_ref(
    slots: &crate::reactive::ReactiveBindingStore,
    namespace: &str,
    field: &str,
    index: usize,
) -> Value {
    Value::ReactiveRef {
        namespace: namespace.to_string(),
        field: field.to_string(),
        index: Some(index),
        kind: BindingKind::Float,
        slot: slots.indexed_slot(namespace, field, index),
    }
}

fn binding_index(value: f64) -> Option<usize> {
    if value.is_finite() && value >= 0.0 && value.fract() == 0.0 && value <= usize::MAX as f64 {
        Some(value as usize)
    } else {
        None
    }
}

fn format_binding_ref(namespace: &str, field: &str, index: Option<usize>) -> String {
    match index {
        Some(index) => format!("<bind:{namespace}.{field}[{index}]>"),
        None => format!("<bind:{namespace}.{field}>"),
    }
}

/// Register math intrinsics needed for SDF and general numeric work.
pub fn register_math_natives(vm: &mut VM) {
    // Single-arg f64 functions
    macro_rules! math1 {
        ($name:expr, $op:expr) => {
            vm.register_native($name, |args| {
                if let Some(Value::Number(x)) = args.first() {
                    Value::Number($op(*x))
                } else {
                    Value::Number(f64::NAN)
                }
            });
        };
    }
    math1!("abs", f64::abs);
    math1!("sqrt", f64::sqrt);
    math1!("sin", f64::sin);
    math1!("cos", f64::cos);
    math1!("floor", f64::floor);
    math1!("ceil", f64::ceil);
    math1!("round", f64::round);
    math1!("fract", f64::fract);

    macro_rules! math2 {
        ($name:expr, $op:expr) => {
            vm.register_native($name, |args| {
                if let (Some(Value::Number(a)), Some(Value::Number(b))) =
                    (args.first(), args.get(1))
                {
                    Value::Number($op(*a, *b))
                } else {
                    Value::Number(f64::NAN)
                }
            });
        };
    }
    math2!("pow", f64::powf);
    math2!("atan2", f64::atan2);
    math2!("mod", |a: f64, b: f64| a % b);

    macro_rules! math3 {
        ($name:expr, $op:expr) => {
            vm.register_native($name, |args| {
                if let (Some(Value::Number(a)), Some(Value::Number(b)), Some(Value::Number(c))) =
                    (args.first(), args.get(1), args.get(2))
                {
                    Value::Number($op(*a, *b, *c))
                } else {
                    Value::Number(f64::NAN)
                }
            });
        };
    }
    math3!("clamp", |v: f64, lo: f64, hi: f64| {
        if lo.is_nan() || hi.is_nan() || lo > hi {
            f64::NAN
        } else {
            v.clamp(lo, hi)
        }
    });
    math3!("mix", |a: f64, b: f64, t: f64| a + (b - a) * t);
    math3!("smoothstep", |e0: f64, e1: f64, x: f64| {
        let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    });

    // Vec2 operations (represented as 2-element List)
    vm.register_native("vec2", |args| {
        let x = match args.first() {
            Some(Value::Number(n)) => *n,
            _ => 0.0,
        };
        let y = match args.get(1) {
            Some(Value::Number(n)) => *n,
            _ => 0.0,
        };
        Value::List(vec![
            Rc::new(RefCell::new(Value::Number(x))),
            Rc::new(RefCell::new(Value::Number(y))),
        ])
    });

    fn extract_f64(val: &Rc<RefCell<Value>>) -> f64 {
        match &*val.borrow() {
            Value::Number(n) => *n,
            _ => f64::NAN,
        }
    }

    vm.register_native("length", |args| {
        if let Some(Value::List(items)) = args.first() {
            if items.len() == 2 {
                let x = extract_f64(&items[0]);
                let y = extract_f64(&items[1]);
                return Value::Number((x * x + y * y).sqrt());
            }
        }
        Value::Number(f64::NAN)
    });

    vm.register_native("dot", |args| {
        if let (Some(Value::List(a)), Some(Value::List(b))) = (args.first(), args.get(1)) {
            if a.len() == 2 && b.len() == 2 {
                let ax = extract_f64(&a[0]);
                let ay = extract_f64(&a[1]);
                let bx = extract_f64(&b[0]);
                let by = extract_f64(&b[1]);
                return Value::Number(ax * bx + ay * by);
            }
        }
        Value::Number(f64::NAN)
    });
}

impl ReactiveDag {
    pub fn new() -> Self {
        Self {
            nodes: HashMap::new(),
            edges: HashMap::new(),
            dirty_nodes: HashSet::new(),
            next_id: 0,
            namespace_field_sources: HashMap::new(),
            local_state_sources: HashMap::new(),
            reverse_edges: HashMap::new(),
            subtree_effects: HashMap::new(),
            subtree_children: HashMap::new(),
            dependency_scopes: HashMap::new(),
        }
    }

    pub fn alloc_id(&mut self) -> NodeId {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        id
    }

    fn index_subtree_effect_node(&mut self, node: &ReactiveNode) {
        let ReactiveNode::Effect {
            id,
            subtree_root_id,
            parent_subtree_root_id,
            ..
        } = node
        else {
            return;
        };
        if let Some(root_id) = subtree_root_id {
            self.subtree_effects.insert(*root_id, *id);
        }
        if let (Some(_), Some(parent_root_id)) = (subtree_root_id, parent_subtree_root_id) {
            self.subtree_children
                .entry(*parent_root_id)
                .or_default()
                .insert(*id);
        }
    }

    fn unindex_subtree_effect_node(&mut self, node: &ReactiveNode) {
        let ReactiveNode::Effect {
            id,
            subtree_root_id,
            parent_subtree_root_id,
            ..
        } = node
        else {
            return;
        };
        if let Some(root_id) = subtree_root_id
            && self.subtree_effects.get(root_id) == Some(id)
        {
            self.subtree_effects.remove(root_id);
        }
        if let Some(parent_root_id) = parent_subtree_root_id
            && let Some(children) = self.subtree_children.get_mut(parent_root_id)
        {
            children.remove(id);
            if children.is_empty() {
                self.subtree_children.remove(parent_root_id);
            }
        }
    }

    pub fn add_node(&mut self, node: ReactiveNode) {
        let id = match &node {
            ReactiveNode::Source { id, .. }
            | ReactiveNode::Derived { id, .. }
            | ReactiveNode::Effect { id, .. } => *id,
        };
        if let Some(existing) = self.nodes.remove(&id) {
            if let ReactiveNode::Source { source, .. } = &existing {
                self.unindex_source_node(source, id);
            }
            self.unindex_subtree_effect_node(&existing);
        }
        if let ReactiveNode::Source { source, .. } = &node {
            self.index_source_node(source, id);
        }
        self.index_subtree_effect_node(&node);
        self.nodes.insert(id, node);
    }

    pub fn remove_node(&mut self, id: NodeId) {
        if let Some(node) = self.nodes.remove(&id) {
            if let ReactiveNode::Source { source, .. } = &node {
                self.unindex_source_node(source, id);
            }
            self.unindex_subtree_effect_node(&node);
        }
        self.dirty_nodes.remove(&id);
        self.dependency_scopes.remove(&id);
        if let Some(dependents) = self.edges.remove(&id) {
            for dependent in dependents {
                if let Some(dependencies) = self.reverse_edges.get_mut(&dependent) {
                    dependencies.remove(&id);
                }
                if let Some(scopes) = self.dependency_scopes.get_mut(&dependent) {
                    scopes.remove(&id);
                }
            }
        }
        if let Some(dependencies) = self.reverse_edges.remove(&id) {
            for dependency in dependencies {
                if let Some(dependents) = self.edges.get_mut(&dependency) {
                    dependents.remove(&id);
                }
                if let Some(node) = self.nodes.get_mut(&dependency) {
                    match node {
                        ReactiveNode::Source { dependents, .. }
                        | ReactiveNode::Derived { dependents, .. } => {
                            dependents.remove(&id);
                        }
                        ReactiveNode::Effect { .. } => {}
                    }
                }
            }
        }
    }

    pub fn mark_dirty(&mut self, id: NodeId) {
        if let Some(node) = self.nodes.get_mut(&id) {
            match node {
                ReactiveNode::Derived { dirty, .. } | ReactiveNode::Effect { dirty, .. } => {
                    *dirty = true;
                    self.dirty_nodes.insert(id);
                }
                ReactiveNode::Source { .. } => {}
            }
        }
    }

    pub fn clear_dirty(&mut self, id: NodeId) {
        if let Some(node) = self.nodes.get_mut(&id) {
            match node {
                ReactiveNode::Derived { dirty, .. } | ReactiveNode::Effect { dirty, .. } => {
                    *dirty = false;
                    self.dirty_nodes.remove(&id);
                }
                ReactiveNode::Source { .. } => {}
            }
        }
    }

    pub fn topo_sort_dirty(&self) -> Vec<NodeId> {
        let dirty = self.dirty_nodes.clone();

        if dirty.is_empty() {
            return vec![];
        }

        let mut indegree = dirty
            .iter()
            .map(|id| (*id, 0_usize))
            .collect::<HashMap<_, _>>();
        for (dependency, dependents) in &self.edges {
            if !dirty.contains(dependency) {
                continue;
            }
            for dependent in dependents {
                if dirty.contains(dependent) {
                    *indegree.entry(*dependent).or_insert(0) += 1;
                }
            }
        }

        let mut queue = indegree
            .iter()
            .filter_map(|(id, degree)| (*degree == 0).then_some(*id))
            .collect::<Vec<_>>();
        queue.sort_unstable();

        let mut result = Vec::new();
        while let Some(id) = queue.pop() {
            result.push(id);
            if let Some(dependents) = self.edges.get(&id) {
                for dependent in dependents {
                    if !dirty.contains(dependent) {
                        continue;
                    }
                    if let Some(entry) = indegree.get_mut(dependent) {
                        *entry = entry.saturating_sub(1);
                        if *entry == 0 && !result.contains(dependent) && !queue.contains(dependent)
                        {
                            queue.push(*dependent);
                            queue.sort_unstable_by(|a, b| b.cmp(a));
                        }
                    }
                }
            }
        }

        if result.len() != dirty.len() {
            let mut remaining = dirty.into_iter().collect::<Vec<_>>();
            remaining.sort_unstable();
            for id in remaining {
                if !result.contains(&id) {
                    result.push(id);
                }
            }
        }

        result
    }

    pub fn clear_dependencies_of(&mut self, id: NodeId) {
        self.dependency_scopes.remove(&id);
        let Some(dependencies) = self.reverse_edges.remove(&id) else {
            return;
        };
        for dependency in dependencies {
            if let Some(dependents) = self.edges.get_mut(&dependency) {
                dependents.remove(&id);
            }
            if let Some(node) = self.nodes.get_mut(&dependency) {
                match node {
                    ReactiveNode::Source { dependents, .. }
                    | ReactiveNode::Derived { dependents, .. } => {
                        dependents.remove(&id);
                    }
                    ReactiveNode::Effect { .. } => {}
                }
            }
        }
    }

    pub fn add_edge(&mut self, dependency: NodeId, dependent: NodeId) {
        self.insert_edge(dependency, dependent);
        // Whole-value read: overrides any narrower indexed scope.
        self.dependency_scopes
            .entry(dependent)
            .or_default()
            .insert(dependency, ReadScope::All);
    }

    /// Record a dependency that only read `index` of a list source (or its
    /// length, via LEN_READ_SENTINEL). Whole-value reads of the same source
    /// keep their `All` scope.
    pub fn add_edge_indexed(&mut self, dependency: NodeId, dependent: NodeId, index: usize) {
        self.insert_edge(dependency, dependent);
        let scope = self
            .dependency_scopes
            .entry(dependent)
            .or_default()
            .entry(dependency)
            .or_insert_with(|| ReadScope::Indices(HashSet::new()));
        if let ReadScope::Indices(indices) = scope {
            indices.insert(index);
        }
    }

    fn insert_edge(&mut self, dependency: NodeId, dependent: NodeId) {
        self.edges.entry(dependency).or_default().insert(dependent);
        self.reverse_edges
            .entry(dependent)
            .or_default()
            .insert(dependency);
        if let Some(node) = self.nodes.get_mut(&dependency) {
            match node {
                ReactiveNode::Source { dependents, .. }
                | ReactiveNode::Derived { dependents, .. } => {
                    dependents.insert(dependent);
                }
                ReactiveNode::Effect { .. } => {}
            }
        }
    }

    pub fn dependency_scope(&self, dependent: NodeId, dependency: NodeId) -> Option<&ReadScope> {
        self.dependency_scopes
            .get(&dependent)
            .and_then(|scopes| scopes.get(&dependency))
    }

    /// Depth of a subtree root in the registered subtree tree (roots with no
    /// parent are depth 0). Used to order dirty subtree effects ancestors-first.
    pub fn subtree_depth(&self, subtree_root_id: u64) -> usize {
        let mut depth = 0usize;
        let mut current = subtree_root_id;
        let mut guard = 0usize;
        while guard < 256 {
            let parent = self
                .subtree_effects
                .get(&current)
                .and_then(|node_id| self.nodes.get(node_id))
                .and_then(|node| match node {
                    ReactiveNode::Effect {
                        parent_subtree_root_id,
                        ..
                    } => *parent_subtree_root_id,
                    _ => None,
                });
            match parent {
                Some(parent_root_id) if parent_root_id != current => {
                    depth += 1;
                    current = parent_root_id;
                    guard += 1;
                }
                _ => break,
            }
        }
        depth
    }

    fn index_source_node(&mut self, source: &ReactiveSource, id: NodeId) {
        match source {
            ReactiveSource::NamespaceField { namespace, field } => {
                self.namespace_field_sources
                    .entry(namespace.clone())
                    .or_default()
                    .insert(field.clone(), id);
            }
            ReactiveSource::LocalState { name } => {
                self.local_state_sources.insert(name.clone(), id);
            }
        }
    }

    fn unindex_source_node(&mut self, source: &ReactiveSource, id: NodeId) {
        match source {
            ReactiveSource::NamespaceField { namespace, field } => {
                let should_remove_namespace =
                    if let Some(fields) = self.namespace_field_sources.get_mut(namespace) {
                        if fields.get(field) == Some(&id) {
                            fields.remove(field);
                        }
                        fields.is_empty()
                    } else {
                        false
                    };
                if should_remove_namespace {
                    self.namespace_field_sources.remove(namespace);
                }
            }
            ReactiveSource::LocalState { name } => {
                if self.local_state_sources.get(name) == Some(&id) {
                    self.local_state_sources.remove(name);
                }
            }
        }
    }

    pub fn find_namespace_field_source_node(&self, namespace: &str, field: &str) -> Option<NodeId> {
        self.namespace_field_sources
            .get(namespace)
            .and_then(|fields| fields.get(field))
            .copied()
    }

    pub fn find_local_state_source_node(&self, name: &str) -> Option<NodeId> {
        self.local_state_sources.get(name).copied()
    }

    pub fn find_source_node(&self, source: &ReactiveSource) -> Option<NodeId> {
        match source {
            ReactiveSource::NamespaceField { namespace, field } => {
                self.find_namespace_field_source_node(namespace, field)
            }
            ReactiveSource::LocalState { name } => self.find_local_state_source_node(name),
        }
    }

    pub fn chunk_idx(&self, id: NodeId) -> Option<usize> {
        self.nodes.get(&id).and_then(|node| match node {
            ReactiveNode::Derived { chunk_idx, .. } | ReactiveNode::Effect { chunk_idx, .. } => {
                Some(*chunk_idx)
            }
            ReactiveNode::Source { .. } => None,
        })
    }

    pub fn derived_value(&self, id: NodeId) -> Option<Value> {
        self.nodes.get(&id).and_then(|node| match node {
            ReactiveNode::Derived { value, .. } => Some(value.clone()),
            _ => None,
        })
    }

    pub fn is_dirty(&self, id: NodeId) -> bool {
        self.nodes.get(&id).is_some_and(|node| match node {
            ReactiveNode::Derived { dirty, .. } | ReactiveNode::Effect { dirty, .. } => *dirty,
            ReactiveNode::Source { .. } => false,
        })
    }

    pub fn effect_ids(&self) -> Vec<NodeId> {
        self.nodes
            .iter()
            .filter_map(|(id, node)| matches!(node, ReactiveNode::Effect { .. }).then_some(*id))
            .collect()
    }

    pub fn effect_ids_for_owner(&self, owner_buffer_id: Option<BufferId>) -> Vec<NodeId> {
        self.nodes
            .iter()
            .filter_map(|(id, node)| match node {
                ReactiveNode::Effect {
                    source_buffer_id: current_owner,
                    ..
                } if *current_owner == owner_buffer_id => Some(*id),
                _ => None,
            })
            .collect()
    }

    pub fn subtree_effect_ids_for_context(
        &self,
        owner_buffer_id: Option<BufferId>,
        target: &EffectTarget,
    ) -> Vec<NodeId> {
        self.nodes
            .iter()
            .filter_map(|(id, node)| match node {
                ReactiveNode::Effect {
                    source_buffer_id: current_owner,
                    target: current_target,
                    subtree_root_id: Some(_),
                    ..
                } if *current_owner == owner_buffer_id && current_target == target => Some(*id),
                _ => None,
            })
            .collect()
    }

    pub fn descendant_subtree_effect_ids(&self, subtree_root_id: u64) -> Vec<NodeId> {
        let mut ids = Vec::new();
        let mut stack = vec![subtree_root_id];
        let mut visited_roots = HashSet::new();

        while let Some(parent_root_id) = stack.pop() {
            if !visited_roots.insert(parent_root_id) {
                continue;
            }

            let Some(children) = self.subtree_children.get(&parent_root_id) else {
                continue;
            };
            for id in children {
                let Some(ReactiveNode::Effect {
                    subtree_root_id: Some(current_root_id),
                    ..
                }) = self.nodes.get(id)
                else {
                    continue;
                };
                if *current_root_id != subtree_root_id {
                    ids.push(*id);
                    stack.push(*current_root_id);
                }
            }
        }

        ids.sort_unstable();
        ids.dedup();
        ids
    }

    pub fn remove_descendant_subtree_effects(&mut self, subtree_root_id: u64) -> HashSet<u64> {
        let descendants = self
            .descendant_subtree_effect_ids(subtree_root_id)
            .into_iter()
            .filter_map(|id| {
                let root = match self.nodes.get(&id) {
                    Some(ReactiveNode::Effect {
                        subtree_root_id: Some(root),
                        ..
                    }) => Some(*root),
                    _ => None,
                };
                self.remove_node(id);
                root
            })
            .collect::<HashSet<_>>();
        descendants
    }

    pub fn effect_id_for_subtree_root(&self, subtree_root_id: u64) -> Option<NodeId> {
        self.subtree_effects.get(&subtree_root_id).copied()
    }
}

impl VM {
    pub fn new(chunks: Vec<Chunk>) -> Self {
        let mut vm = VM {
            chunks,
            current_chunk: 0,
            globals: vec![None; 4096],
            global_names: vec![],
            pending_widget_trees: Vec::new(),
            pending_inline_widgets: Vec::new(),
            registering_static_inline_widget: false,
            recent_runtime_inline_widgets: HashMap::new(),
            dag: ReactiveDag::new(),
            tracking_stack: Vec::new(),
            reactive_namespaces: HashSet::new(),
            writable_reactive_namespaces: HashSet::new(),
            reactive_float_slots: crate::reactive::ReactiveBindingStore::default(),
            pending_reactive_sets: Vec::new(),
            derived_bindings: HashMap::new(),
            state_bindings: HashMap::new(),
            execution_depth: 0,
            processing_reactive: false,
            reactive_exec_timings: Vec::new(),
            last_reactive_error_context: None,
            last_reactive_error_detail: None,
            current_effect_source_buffer_id: None,
            current_effect_target: EffectTarget::BufferId(None),
            current_effect_reactive_reads: None,
            current_effect_symbol_reads: None,
            current_subtree_capture_stack: Vec::new(),
            current_subtree_reactive_reads: HashMap::new(),
            macros: HashMap::new(),
            source_manager: SourceManager::new(),
            source_load_errors: Vec::new(),
            preserve_state_on_redefinition: false,
            extension_hooks: HashMap::new(),
            declared_modules: HashMap::new(),
            compat_aliases: HashMap::new(),
            global_store_hooks: Vec::new(),
            inline_widget_metadata_resolver: None,
        };
        vm.register_native(SOURCE_ORIGIN_NATIVE, source_origin_native);
        vm
    }

    /// Register a Rust function as a named global callable from Lisp.
    pub fn register_native(&mut self, name: &str, f: impl Fn(Vec<Value>) -> Value + 'static) {
        self.register_native_with_vm(name, move |args, _vm| f(args));
    }

    pub fn register_native_with_vm(
        &mut self,
        name: &str,
        f: impl Fn(Vec<Value>, &mut VM) -> Value + 'static,
    ) {
        // Natives registered at startup (empty table) intern flat; natives
        // registered mid-run by name (defhook, defwidget) must land in the
        // qualified slot if the compiler already interned one for that name
        // — same resolve-existing rule as `set_global_value`.
        let idx = match self.resolve_global_read_index(name) {
            Some(idx) => idx,
            None => self.ensure_global(name),
        };
        if idx >= self.globals.len() {
            self.globals.resize(idx + 1, None);
        }
        self.globals[idx] = Some(Rc::new(RefCell::new(Value::NativeFunction(Rc::new(f)))));
    }

    /// Register a native under a namespace (spec §3 "Core namespaces"):
    /// `register_native_in_namespace("sdf", "layer", …)` interns
    /// `sdf/layer`. Blessed namespaces (`sdf`, `eseq.core`) need no import
    /// at the call site.
    pub fn register_native_in_namespace(
        &mut self,
        namespace: &str,
        name: &str,
        f: impl Fn(Vec<Value>, &mut VM) -> Value + 'static,
    ) {
        let qualified = crate::modules::qualify(namespace, name);
        self.register_native_with_vm(&qualified, f);
    }

    pub fn add_global_store_hook(&mut self, hook: GlobalStoreHook) {
        self.global_store_hooks.push(hook);
    }

    pub fn set_inline_widget_metadata_resolver(&mut self, resolver: InlineWidgetMetadataResolver) {
        self.inline_widget_metadata_resolver = Some(resolver);
    }

    pub fn resolve_inline_widget_metadata(
        &self,
        callee: &str,
        inlet: &str,
    ) -> Option<InlineWidgetMetadata> {
        self.inline_widget_metadata_resolver
            .as_ref()
            .and_then(|resolver| resolver(callee, inlet))
    }

    pub fn current_source_symbol(&self) -> Option<String> {
        self.chunks
            .get(self.current_chunk)
            .and_then(|chunk| chunk.source_symbol.clone())
    }

    /// The module of the currently executing chunk (spec §5): registration
    /// natives (widget constructors, `bind-key`, `define-mode`, …) call
    /// this to learn the module current at their call site. Outside a
    /// declared module this is the implicit `eseq.vanilla`.
    pub fn current_module_name(&self) -> &str {
        self.chunks
            .get(self.current_chunk)
            .and_then(|chunk| chunk.source_module.as_deref())
            .unwrap_or(crate::modules::IMPLICIT_MODULE)
    }

    /// Registry auto-qualification for widget `:key` strings (spec §5):
    /// inside a declared module the stable key is prefixed with the module
    /// BEFORE it feeds any identity hash — `:key (str "cell-glyph-" i)`
    /// in `eseq.mixer` becomes `"eseq.mixer/cell-glyph-3"` — by writing
    /// the qualified form into `__stable-key` (the raw `:key` prop stays
    /// as authored). Every downstream identity reader (stable widget
    /// hashing here, layout's FNV `stable_key_to_widget_id`) prefers
    /// `__stable-key`. Vanilla chunks change nothing, which keeps
    /// serialized layout/p-lock identity stable until a file converts.
    pub fn qualify_widget_stable_key(&self, widget: &mut Value) {
        let module = self.current_module_name();
        if module == crate::modules::IMPLICIT_MODULE {
            return;
        }
        let module = module.to_string();
        let Value::Map(map) = widget else {
            return;
        };
        if map.contains_key(STABLE_KEY_PROP) {
            return;
        }
        let Some(key) = stable_key_value(map) else {
            return;
        };
        if crate::modules::is_qualified(&key) {
            return;
        }
        map.insert(
            STABLE_KEY_PROP.to_string(),
            Rc::new(RefCell::new(Value::String(crate::modules::qualify(
                &module, &key,
            )))),
        );
    }

    pub fn current_source_file(&self) -> Option<std::path::PathBuf> {
        self.chunks
            .get(self.current_chunk)
            .and_then(|chunk| chunk.source_file.clone())
            .or_else(|| self.source_manager.current_source_file())
    }

    /// Compile and run `code` in this VM's existing context (globals persist).
    pub fn eval_str(&mut self, code: &str) -> Result<Option<Value>, VMError> {
        let tokens = Parser::new(code.to_string())
            .parse_spanned()
            .map_err(|_| VMError::ParseError)?;
        let spanned_exprs = SpannedASTParser::new(tokens)
            .parse()
            .map_err(|_| VMError::ParseError)?;
        let source_revision = self
            .source_manager
            .current_revision()
            .unwrap_or_else(|| crate::hot_reload::hash_source(code));
        let existing_macro_names = self.macros.keys().cloned().collect::<HashSet<_>>();
        let exprs = convert_source_exprs_with_origins(
            &spanned_exprs,
            source_revision,
            &existing_macro_names,
        );
        self.register_static_inline_widgets(&spanned_exprs, source_revision);

        let entry_idx = self.chunks.len();
        let names_len = self.global_names.len();
        // Move the program into the compiler instead of cloning it: this path
        // runs on every shortcut eval, and cloning (then dropping) thousands
        // of chunks made every keyboard action pay ~10 ms. The compiler only
        // appends chunks and global names, so an error restores the moved
        // state exactly by truncating to the pre-eval lengths.
        let existing = std::mem::take(&mut self.chunks);
        let names = std::mem::take(&mut self.global_names);
        let reactive_namespaces = self.reactive_namespaces.clone();
        let derived_bindings = self.derived_bindings.clone();
        let state_bindings = self.state_bindings.clone();
        let next_node_id = self.dag.next_id;

        let macros = self.macros.clone();
        let source_file = self.source_manager.current_source_file();
        let mut compiler = Compiler::new_repl(
            exprs,
            existing,
            names,
            reactive_namespaces,
            derived_bindings,
            state_bindings,
            next_node_id,
            macros,
            source_file,
        );
        compiler.set_compat_aliases(self.compat_aliases.clone());
        match compiler.compile() {
            Ok(chunks) => {
                self.chunks = chunks;
                self.global_names = compiler.take_global_names();
                self.derived_bindings = compiler.take_derived_bindings();
                self.state_bindings = compiler.take_state_bindings();
                self.dag.next_id = compiler.next_node_id();
                // The compiler's macro table started from this VM's, so
                // taking it wholesale keeps every existing macro and merges
                // any new definitions.
                self.macros = compiler.take_macros();
                for warning in compiler.take_warnings() {
                    self.source_manager.push_diagnostic(warning);
                }
                self.execute_from(entry_idx)
            }
            Err(error) => {
                if let CompilerError::Message(message) = &error {
                    self.source_load_errors.push(message.clone());
                }
                let mut chunks = compiler.take_chunks();
                chunks.truncate(entry_idx);
                self.chunks = chunks;
                let mut names = compiler.take_global_names();
                names.truncate(names_len);
                self.global_names = names;
                Err(VMError::CompileError)
            }
        }
    }

    pub fn eval_module_source(
        &mut self,
        path: std::path::PathBuf,
        source: &str,
        revision: u64,
    ) -> Result<Option<Value>, VMError> {
        let mut defined_symbols = extract_defined_symbols_from_source(source).map_err(|error| {
            self.source_load_errors
                .push(format!("{}: {error}", path.display()));
            VMError::ParseError
        })?;
        // Textual extraction yields bare names, but the compiler interns
        // most defs qualified (`eseq.vanilla/name`) and effect reads record
        // interned names. Track both forms so hot-reload invalidation
        // matches regardless of which slot a def resolved to (a def
        // shadowing a flat native stays flat). Superset only means
        // conservative over-invalidation.
        let qualified: Vec<String> = defined_symbols
            .iter()
            .filter(|name| !crate::modules::is_qualified(name))
            .map(|name| crate::modules::qualify(crate::modules::IMPLICIT_MODULE, name))
            .collect();
        defined_symbols.extend(qualified);
        self.clear_effects_for_module(&path);
        self.source_manager
            .remember_evaluated_source(path.clone(), revision, source);
        self.source_manager.enter_file(path.clone(), revision);
        let result = self.eval_str(source);
        self.source_manager.leave_file();
        if result.is_ok() {
            self.source_manager.record_module_success(
                path,
                source,
                revision,
                defined_symbols,
                Vec::new(),
            );
        } else {
            self.source_manager.discard_module_loads(&path);
        }
        result
    }

    pub fn take_source_load_errors(&mut self) -> Vec<String> {
        std::mem::take(&mut self.source_load_errors)
    }

    pub fn set_preserve_state_on_redefinition(&mut self, preserve: bool) {
        self.preserve_state_on_redefinition = preserve;
    }

    pub fn snapshot_state(&self) -> VmStateSnapshot {
        VmStateSnapshot {
            chunks: self.chunks.clone(),
            current_chunk: self.current_chunk,
            globals: clone_globals_for_snapshot(&self.globals),
            global_names: self.global_names.clone(),
            pending_widget_trees: self.pending_widget_trees.clone(),
            pending_inline_widgets: self.pending_inline_widgets.clone(),
            registering_static_inline_widget: self.registering_static_inline_widget,
            recent_runtime_inline_widgets: self.recent_runtime_inline_widgets.clone(),
            dag: self.dag.clone(),
            tracking_stack: self.tracking_stack.clone(),
            reactive_namespaces: self.reactive_namespaces.clone(),
            writable_reactive_namespaces: self.writable_reactive_namespaces.clone(),
            pending_reactive_sets: self.pending_reactive_sets.clone(),
            derived_bindings: self.derived_bindings.clone(),
            state_bindings: self.state_bindings.clone(),
            execution_depth: self.execution_depth,
            processing_reactive: self.processing_reactive,
            reactive_exec_timings: self.reactive_exec_timings.clone(),
            last_reactive_error_context: self.last_reactive_error_context.clone(),
            last_reactive_error_detail: self.last_reactive_error_detail.clone(),
            current_effect_source_buffer_id: self.current_effect_source_buffer_id,
            current_effect_target: self.current_effect_target.clone(),
            current_effect_reactive_reads: self.current_effect_reactive_reads.clone(),
            current_effect_symbol_reads: self.current_effect_symbol_reads.clone(),
            current_subtree_capture_stack: self.current_subtree_capture_stack.clone(),
            current_subtree_reactive_reads: self.current_subtree_reactive_reads.clone(),
            macros: self.macros.clone(),
            source_manager: self.source_manager.clone(),
            source_load_errors: self.source_load_errors.clone(),
            preserve_state_on_redefinition: self.preserve_state_on_redefinition,
            extension_hooks: self
                .extension_hooks
                .iter()
                .map(|(name, entries)| {
                    (
                        name.clone(),
                        entries
                            .iter()
                            .map(|(key, callback)| {
                                (key.clone(), clone_value_for_snapshot(callback))
                            })
                            .collect(),
                    )
                })
                .collect(),
            declared_modules: self.declared_modules.clone(),
            compat_aliases: self.compat_aliases.clone(),
        }
    }

    pub fn restore_state(&mut self, snapshot: VmStateSnapshot) {
        self.chunks = snapshot.chunks;
        self.current_chunk = snapshot.current_chunk;
        self.globals = snapshot.globals;
        self.global_names = snapshot.global_names;
        self.pending_widget_trees = snapshot.pending_widget_trees;
        self.pending_inline_widgets = snapshot.pending_inline_widgets;
        self.registering_static_inline_widget = snapshot.registering_static_inline_widget;
        self.recent_runtime_inline_widgets = snapshot.recent_runtime_inline_widgets;
        self.dag = snapshot.dag;
        self.tracking_stack = snapshot.tracking_stack;
        self.reactive_namespaces = snapshot.reactive_namespaces;
        self.writable_reactive_namespaces = snapshot.writable_reactive_namespaces;
        self.pending_reactive_sets = snapshot.pending_reactive_sets;
        self.derived_bindings = snapshot.derived_bindings;
        self.state_bindings = snapshot.state_bindings;
        self.execution_depth = snapshot.execution_depth;
        self.processing_reactive = snapshot.processing_reactive;
        self.reactive_exec_timings = snapshot.reactive_exec_timings;
        self.last_reactive_error_context = snapshot.last_reactive_error_context;
        self.last_reactive_error_detail = snapshot.last_reactive_error_detail;
        self.current_effect_source_buffer_id = snapshot.current_effect_source_buffer_id;
        self.current_effect_target = snapshot.current_effect_target;
        self.current_effect_reactive_reads = snapshot.current_effect_reactive_reads;
        self.current_effect_symbol_reads = snapshot.current_effect_symbol_reads;
        self.current_subtree_capture_stack = snapshot.current_subtree_capture_stack;
        self.current_subtree_reactive_reads = snapshot.current_subtree_reactive_reads;
        self.macros = snapshot.macros;
        self.source_manager = snapshot.source_manager;
        self.source_load_errors = snapshot.source_load_errors;
        self.preserve_state_on_redefinition = snapshot.preserve_state_on_redefinition;
        self.extension_hooks = snapshot.extension_hooks;
        self.declared_modules = snapshot.declared_modules;
        self.compat_aliases = snapshot.compat_aliases;
    }

    pub fn take_pending_reactive_sets(&mut self) -> Vec<(String, String, Value)> {
        std::mem::take(&mut self.pending_reactive_sets)
    }

    #[cfg(test)]
    pub(crate) fn profile_eval_str(
        &mut self,
        code: &str,
    ) -> Result<(Option<Value>, EvalProfile), VMError> {
        let mut profile = EvalProfile::default();

        let parse_started = std::time::Instant::now();
        let tokens = Parser::new(code.to_string())
            .parse_spanned()
            .map_err(|_| VMError::ParseError)?;
        profile.parse = parse_started.elapsed();

        let ast_started = std::time::Instant::now();
        let spanned_exprs = SpannedASTParser::new(tokens)
            .parse()
            .map_err(|_| VMError::ParseError)?;
        let source_revision = self
            .source_manager
            .current_revision()
            .unwrap_or_else(|| crate::hot_reload::hash_source(code));
        let existing_macro_names = self.macros.keys().cloned().collect::<HashSet<_>>();
        let exprs = convert_source_exprs_with_origins(
            &spanned_exprs,
            source_revision,
            &existing_macro_names,
        );
        profile.ast = ast_started.elapsed();

        let entry_idx = self.chunks.len();
        let names_len = self.global_names.len();
        // Same move-instead-of-clone contract as `eval_str`: the compiler
        // only appends chunks and global names, so truncating to the pre-eval
        // lengths restores the moved state exactly on a compile error.
        let existing = std::mem::take(&mut self.chunks);
        let names = std::mem::take(&mut self.global_names);
        let reactive_namespaces = self.reactive_namespaces.clone();
        let derived_bindings = self.derived_bindings.clone();
        let state_bindings = self.state_bindings.clone();
        let next_node_id = self.dag.next_id;

        let macros = self.macros.clone();
        let source_file = self.source_manager.current_source_file();
        let compile_started = std::time::Instant::now();
        let mut compiler = Compiler::new_repl(
            exprs,
            existing,
            names,
            reactive_namespaces,
            derived_bindings,
            state_bindings,
            next_node_id,
            macros,
            source_file,
        );
        compiler.set_compat_aliases(self.compat_aliases.clone());
        match compiler.compile() {
            Ok(chunks) => {
                self.chunks = chunks;
                self.global_names = compiler.take_global_names();
                self.derived_bindings = compiler.take_derived_bindings();
                self.state_bindings = compiler.take_state_bindings();
                self.dag.next_id = compiler.next_node_id();
                self.macros = compiler.take_macros();
                for warning in compiler.take_warnings() {
                    self.source_manager.push_diagnostic(warning);
                }
            }
            Err(error) => {
                if let CompilerError::Message(message) = &error {
                    self.source_load_errors.push(message.clone());
                }
                let mut chunks = compiler.take_chunks();
                chunks.truncate(entry_idx);
                self.chunks = chunks;
                let mut names = compiler.take_global_names();
                names.truncate(names_len);
                self.global_names = names;
                return Err(VMError::CompileError);
            }
        }
        profile.compile = compile_started.elapsed();

        let execute_started = std::time::Instant::now();
        let result = self.execute_from(entry_idx);
        profile.execute = execute_started.elapsed();

        result.map(|value| (value, profile))
    }

    fn ensure_global(&mut self, name: &str) -> usize {
        if let Some(idx) = self.global_names.iter().position(|n| n == name) {
            return idx;
        }
        let idx = self.global_names.len();
        self.global_names.push(name.to_string());
        idx
    }

    /// Runtime by-name resolution ladder (module-system spec §3, slice 0),
    /// mirroring `Compiler::resolve_global_name`: a qualified name resolves
    /// as-is; a bare name prefers the current module's qualified entry
    /// (`eseq.vanilla/name`) and falls back to the flat entry (natives,
    /// host-registered globals). Keep the two in sync.
    fn resolve_global_read_index(&self, name: &str) -> Option<usize> {
        if crate::modules::is_qualified(name) {
            let exact = self.global_names.iter().position(|n| n == name);
            if exact.is_some() {
                return exact;
            }
            // Core namespaces resolve bare or qualified without import
            // (spec §3): `eseq.core/label` falls back to the flat native.
            if let Some((ns, base)) = crate::modules::split_qualified(name) {
                if crate::modules::CORE_NAMESPACES.contains(&ns)
                    || ns == crate::modules::IMPLICIT_MODULE
                {
                    return self.global_names.iter().position(|n| n == base);
                }
            }
            return None;
        }
        // Migration compat alias (spec §10 slice 3): checked before the
        // implicit-module and flat entries, mirroring the compiler ladder —
        // a converted symbol's stale eseq.vanilla slot (left interned from
        // before the conversion) must not shadow the new qualified home.
        // Targets are validated qualified at record time, so this recurses
        // at most once (the qualified branch never consults aliases).
        if let Some(target) = self.compat_aliases.get(name)
            && let Some(idx) = self.resolve_global_read_index(target)
        {
            return Some(idx);
        }
        let qualified = crate::modules::qualify(crate::modules::IMPLICIT_MODULE, name);
        self.global_names
            .iter()
            .position(|n| *n == qualified)
            .or_else(|| self.global_names.iter().position(|n| n == name))
    }

    pub fn has_global(&self, name: &str) -> bool {
        self.resolve_global_read_index(name).is_some()
    }

    pub fn set_global_value(&mut self, name: &str, value: Value) {
        // Write into the slot a lisp reference would resolve to; only
        // create a new (flat, host-owned) slot when neither form exists.
        let idx = match self.resolve_global_read_index(name) {
            Some(idx) => idx,
            None => self.ensure_global(name),
        };
        if idx >= self.globals.len() {
            self.globals.resize(idx + 1, None);
        }
        self.globals[idx] = Some(Rc::new(RefCell::new(value)));
    }

    pub fn global_names(&self) -> &[String] {
        &self.global_names
    }

    pub fn global_value(&self, name: &str) -> Option<Value> {
        let idx = self.resolve_global_read_index(name)?;
        self.globals
            .get(idx)
            .and_then(|value| value.as_ref())
            .map(|value| value.borrow().clone())
    }

    /// Runtime state-binding lookup: exact key first (flat, or an already
    /// qualified name), then the executing chunk's module key (spec §5 —
    /// a declared module's `defstate` interns qualified, but its own code
    /// and defwidget shader uniforms read it by bare name).
    fn state_binding_node(&self, name: &str) -> Option<NodeId> {
        if let Some(node_id) = self.state_bindings.get(name) {
            return Some(*node_id);
        }
        let module = self.current_module_name();
        if module != crate::modules::IMPLICIT_MODULE && !crate::modules::is_qualified(name) {
            return self
                .state_bindings
                .get(&crate::modules::qualify(module, name))
                .copied();
        }
        None
    }

    pub fn read_tracked_state_value(&mut self, name: &str) -> Option<Value> {
        let node_id = self.state_binding_node(name)?;
        if let Some(ctx_id) = self.tracking_stack.last().copied() {
            self.dag.add_edge(node_id, ctx_id);
        }
        self.dag.nodes.get(&node_id).and_then(|node| match node {
            ReactiveNode::Source { value, .. } => Some(value.clone()),
            _ => None,
        })
    }

    pub fn clear_effects_for_owner(&mut self, owner_buffer_id: Option<BufferId>) {
        for id in self.dag.effect_ids_for_owner(owner_buffer_id) {
            self.dag.remove_node(id);
        }
        self.pending_widget_trees
            .retain(|pending| pending.source_buffer_id() != owner_buffer_id);
    }

    pub fn clear_subtree_effects_for_named_target(&mut self, target_name: &str) {
        let target = EffectTarget::BufferName(target_name.to_string());
        let ids = self
            .dag
            .nodes
            .iter()
            .filter_map(|(id, node)| match node {
                ReactiveNode::Effect {
                    target: current_target,
                    subtree_root_id: Some(_),
                    ..
                } if *current_target == target => Some(*id),
                _ => None,
            })
            .collect::<Vec<_>>();
        for id in ids {
            self.dag.remove_node(id);
        }
        self.pending_widget_trees.retain(|pending| match pending {
            PendingUiUpdate::ReplaceSubtree {
                target: pending_target,
                ..
            } => *pending_target != target,
            PendingUiUpdate::FullTree(_) => true,
        });
    }

    pub fn clear_effects_for_module(&mut self, module: &std::path::Path) {
        let ids = self
            .dag
            .nodes
            .iter()
            .filter_map(|(id, node)| match node {
                ReactiveNode::Effect {
                    source_file: Some(source_file),
                    ..
                } if source_file == module => Some(*id),
                _ => None,
            })
            .collect::<Vec<_>>();
        for id in ids {
            self.dag.remove_node(id);
        }
    }

    #[cfg(test)]
    pub fn effect_count_for_module(&self, module: &std::path::Path) -> usize {
        self.dag
            .nodes
            .values()
            .filter(|node| {
                matches!(
                    node,
                    ReactiveNode::Effect {
                        source_file: Some(source_file),
                        ..
                    } if source_file == module
                )
            })
            .count()
    }

    fn upsert_top_level_effect_node(
        &mut self,
        node_id: NodeId,
        chunk_idx: usize,
        target: EffectTarget,
    ) {
        let source_buffer_id = self.current_effect_source_buffer_id;
        let source_file = self.source_manager.current_source_file();
        let source_revision = self.source_manager.current_revision();
        match self.dag.nodes.get_mut(&node_id) {
            Some(ReactiveNode::Effect {
                chunk_idx: current_chunk_idx,
                callable,
                source_buffer_id: current_source_buffer_id,
                source_file: current_source_file,
                source_revision: current_source_revision,
                target: current_target,
                subtree_root_id: None,
                parent_subtree_root_id,
                stable_key,
                dirty,
                ..
            }) => {
                *current_chunk_idx = chunk_idx;
                *callable = None;
                *current_source_buffer_id = source_buffer_id;
                *current_source_file = source_file;
                *current_source_revision = source_revision;
                *current_target = target;
                *parent_subtree_root_id = None;
                *stable_key = None;
                *dirty = false;
                self.dag.dirty_nodes.remove(&node_id);
            }
            Some(_) => {
                self.dag.remove_node(node_id);
                self.dag.add_node(ReactiveNode::Effect {
                    id: node_id,
                    chunk_idx,
                    callable: None,
                    source_buffer_id,
                    source_file,
                    source_revision,
                    target,
                    subtree_root_id: None,
                    parent_subtree_root_id: None,
                    stable_key: None,
                    symbol_dependencies: HashSet::new(),
                    dirty: false,
                });
            }
            None => {
                self.dag.add_node(ReactiveNode::Effect {
                    id: node_id,
                    chunk_idx,
                    callable: None,
                    source_buffer_id,
                    source_file,
                    source_revision,
                    target,
                    subtree_root_id: None,
                    parent_subtree_root_id: None,
                    stable_key: None,
                    symbol_dependencies: HashSet::new(),
                    dirty: false,
                });
            }
        }
        self.source_manager.record_render_root(node_id);
    }

    pub fn mark_effects_depending_on_symbols(&mut self, symbols: &HashSet<String>) -> Vec<String> {
        if symbols.is_empty() {
            return Vec::new();
        }
        let mut rerendered = Vec::new();
        let ids = self
            .dag
            .nodes
            .iter()
            .filter_map(|(id, node)| match node {
                ReactiveNode::Effect {
                    subtree_root_id: None,
                    symbol_dependencies,
                    ..
                } if symbol_dependencies
                    .iter()
                    .any(|symbol| symbols.contains(symbol)) =>
                {
                    Some(*id)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        for id in ids {
            if let Some(label) = self.reactive_node_label(id) {
                rerendered.push(label);
            }
            self.dag.mark_dirty(id);
        }
        rerendered.sort();
        rerendered.dedup();
        rerendered
    }

    pub fn rerender_dirty_effects(&mut self) -> Result<(), VMError> {
        self.process_dirty_reactive()
    }

    fn clear_subtree_effects_for_current_context(&mut self) {
        let owner_buffer_id = self.current_effect_source_buffer_id;
        let target = self.current_effect_target.clone();
        for id in self
            .dag
            .subtree_effect_ids_for_context(owner_buffer_id, &target)
        {
            self.dag.remove_node(id);
        }
    }

    pub fn set_current_effect_context(&mut self, source_buffer_id: Option<BufferId>) {
        self.current_effect_source_buffer_id = source_buffer_id;
        self.current_effect_target = EffectTarget::BufferId(source_buffer_id);
    }

    pub fn inline_widget_registration_enabled(&self) -> bool {
        self.current_effect_source_buffer_id.is_some()
    }

    pub fn begin_inline_widget_capture(&mut self) {
        self.pending_inline_widgets.clear();
        self.recent_runtime_inline_widgets.clear();
    }

    pub fn register_inline_widget(&mut self, widget: Value) {
        if !self.inline_widget_registration_enabled() {
            return;
        }
        let identity = inline_widget_source_identity(&widget);
        let runtime_parent = (!self.registering_static_inline_widget)
            .then(|| inline_widget_parent_identity(&widget))
            .flatten();
        if let Some(identity) = identity.as_ref()
            && let Some(existing) = self
                .pending_inline_widgets
                .iter_mut()
                .find(|existing| inline_widget_source_identity(existing).as_ref() == Some(identity))
        {
            *existing = widget;
        } else {
            self.pending_inline_widgets.push(widget);
        }
        if let (Some(parent), Some(identity)) = (runtime_parent, identity) {
            self.recent_runtime_inline_widgets.insert(parent, identity);
        }
    }

    /// Registers literal-backed inline forms while their source is compiled.
    ///
    /// Inline forms normally register when their native function executes. A
    /// form inside a function or process body may not execute during authoring,
    /// however, and must still have an editor widget. This source pass invokes
    /// the same native constructors for statically representable forms. Later
    /// execution of the form replaces the registration with the same source
    /// identity, allowing process call sites to add runtime metadata/targets.
    fn register_static_inline_widgets(&mut self, exprs: &[Expr], source_revision: u64) {
        if !self.inline_widget_registration_enabled() {
            return;
        }
        for expr in exprs {
            self.register_static_inline_widget_expr(expr, source_revision, None);
        }
    }

    fn register_static_inline_widget_expr(
        &mut self,
        expr: &Expr,
        source_revision: u64,
        parent: Option<(&str, &str)>,
    ) {
        let ExprKind::List(items) = &expr.kind else {
            return;
        };
        let head = items.first().and_then(|item| match &item.kind {
            ExprKind::Symbol(name) => Some(name.as_str()),
            _ => None,
        });

        if let Some(form_name) = head.filter(|name| is_inline_widget_constructor_name(name))
            && let Some(mut args) = static_inline_args(items)
        {
            if let Some((parent_callee, parent_inlet)) = parent {
                args.extend([
                    Value::Keyword(INLINE_PARENT_CALLEE_PROP.to_string()),
                    Value::String(parent_callee.to_string()),
                    Value::Keyword(INLINE_PARENT_INLET_PROP.to_string()),
                    Value::String(parent_inlet.to_string()),
                ]);
            }
            if matches!(form_name, "~slider" | "~knob" | "~toggle" | "~lane")
                && let Some(value_expr) = items.get(1)
            {
                args.extend([
                    Value::Keyword(INLINE_VALUE_START_BYTE_PROP.to_string()),
                    Value::Number(value_expr.origin.primary_span.start_byte as f64),
                    Value::Keyword(INLINE_VALUE_END_BYTE_PROP.to_string()),
                    Value::Number(value_expr.origin.primary_span.end_byte as f64),
                ]);
            }
            args.extend([
                Value::Keyword(SOURCE_START_BYTE_PROP.to_string()),
                Value::Number(expr.origin.primary_span.start_byte as f64),
                Value::Keyword(SOURCE_END_BYTE_PROP.to_string()),
                Value::Number(expr.origin.primary_span.end_byte as f64),
                Value::Keyword(SOURCE_REVISION_PROP.to_string()),
                Value::String(source_revision.to_string()),
            ]);
            if let Some(Value::NativeFunction(function)) = self.global_value(form_name) {
                let previous_static_registration = self.registering_static_inline_widget;
                self.registering_static_inline_widget = true;
                function(args, self);
                self.registering_static_inline_widget = previous_static_registration;
            }
        }

        for (index, item) in items.iter().enumerate().skip(1) {
            let child_parent = head.and_then(|callee| {
                index
                    .checked_sub(1)
                    .and_then(|previous| items.get(previous))
                    .and_then(|previous| match &previous.kind {
                        ExprKind::Keyword(inlet) => Some((callee, inlet.as_str())),
                        _ => None,
                    })
            });
            self.register_static_inline_widget_expr(item, source_revision, child_parent);
        }
    }

    pub fn attach_inline_widget_runtime_target(
        &mut self,
        callee: &str,
        inlet: &str,
        target: Value,
    ) -> bool {
        let parent = (callee.to_string(), inlet.to_string());
        let Some(source_identity) = self.recent_runtime_inline_widgets.remove(&parent) else {
            return false;
        };
        let Some(Value::Map(map)) = self.pending_inline_widgets.iter_mut().find(|widget| {
            inline_widget_source_identity(widget).as_ref() == Some(&source_identity)
        }) else {
            return false;
        };
        map.insert(
            "__inline-runtime-target".to_string(),
            Rc::new(RefCell::new(target)),
        );
        true
    }

    pub fn take_inline_widgets(&mut self) -> Vec<Value> {
        self.recent_runtime_inline_widgets.clear();
        std::mem::take(&mut self.pending_inline_widgets)
    }

    pub fn take_reactive_exec_timings(&mut self) -> Vec<ReactiveExecTiming> {
        std::mem::take(&mut self.reactive_exec_timings)
    }

    pub fn take_last_reactive_error_context(&mut self) -> Option<String> {
        self.last_reactive_error_context.take()
    }

    pub fn take_last_reactive_error_detail(&mut self) -> Option<String> {
        self.last_reactive_error_detail.take()
    }

    fn record_reactive_read(&mut self, namespace: &str, field: &str) {
        let key = ReactiveFieldKey::new(namespace, field);
        if let Some(context) = self.current_subtree_capture_stack.last() {
            self.current_subtree_reactive_reads
                .entry(context.root_id)
                .or_default()
                .insert(key.clone());
        }
        if let Some(reads) = self.current_effect_reactive_reads.as_mut() {
            reads.insert(key);
        }
    }

    fn record_symbol_read(&mut self, name: &str) {
        if let Some(reads) = self.current_effect_symbol_reads.as_mut() {
            reads.insert(name.to_string());
        }
    }

    fn registered_subtree_owner(&self, root_id: u64) -> Option<RegisteredSubtreeOwner> {
        let node_id = self.dag.effect_id_for_subtree_root(root_id)?;
        let ReactiveNode::Effect {
            callable: Some(callable),
            parent_subtree_root_id,
            stable_key: Some(stable_key),
            subtree_root_id: Some(current_root_id),
            ..
        } = self.dag.nodes.get(&node_id)?
        else {
            return None;
        };
        Some(RegisteredSubtreeOwner {
            node_id,
            root_id: *current_root_id,
            parent_root_id: *parent_subtree_root_id,
            stable_key: stable_key.clone(),
            callable: callable.clone(),
        })
    }

    fn sync_subtree_owner_node(
        &mut self,
        root_id: u64,
        parent_root_id: Option<u64>,
        stable_key: String,
        callable: Value,
    ) -> RegisteredSubtreeOwner {
        let chunk_idx = match &callable {
            Value::Closure(chunk_idx, _) => *chunk_idx,
            _ => 0,
        };
        let node_id = self
            .dag
            .effect_id_for_subtree_root(root_id)
            .unwrap_or_else(|| self.dag.alloc_id());
        self.dag.add_node(ReactiveNode::Effect {
            id: node_id,
            chunk_idx,
            callable: Some(callable.clone()),
            source_buffer_id: self.current_effect_source_buffer_id,
            source_file: self.source_manager.current_source_file(),
            source_revision: self.source_manager.current_revision(),
            target: self.current_effect_target.clone(),
            subtree_root_id: Some(root_id),
            parent_subtree_root_id: parent_root_id,
            stable_key: Some(stable_key.clone()),
            symbol_dependencies: HashSet::new(),
            dirty: false,
        });
        RegisteredSubtreeOwner {
            node_id,
            root_id,
            parent_root_id,
            stable_key,
            callable,
        }
    }

    fn render_registered_subtree_owner(
        &mut self,
        owner: &RegisteredSubtreeOwner,
    ) -> Result<Value, VMError> {
        let removed_descendants = self.dag.remove_descendant_subtree_effects(owner.root_id);
        if !removed_descendants.is_empty() {
            self.pending_widget_trees.retain(|pending| match pending {
                PendingUiUpdate::ReplaceSubtree {
                    subtree_root_id, ..
                } => !removed_descendants.contains(subtree_root_id),
                PendingUiUpdate::FullTree(_) => true,
            });
        }
        self.dag.clear_dependencies_of(owner.node_id);
        self.current_subtree_reactive_reads
            .insert(owner.root_id, HashSet::new());
        self.current_subtree_capture_stack
            .push(SubtreeCaptureContext {
                root_id: owner.root_id,
                parent_root_id: owner.parent_root_id,
                stable_key: owner.stable_key.clone(),
            });
        self.tracking_stack.push(owner.node_id);
        let result = self.invoke(owner.callable.clone(), vec![]);
        let _ = self.tracking_stack.pop();
        let _ = self.current_subtree_capture_stack.pop();
        result.map(|value| {
            annotate_explicit_subtree_root(
                &value.unwrap_or(Value::Nil),
                owner.root_id,
                owner.parent_root_id,
                &owner.stable_key,
                true,
            )
        })
    }

    fn evaluate_subtree_owner(
        &mut self,
        stable_key: &str,
        callable: Value,
    ) -> Result<Value, VMError> {
        let parent_root_id = self
            .current_subtree_capture_stack
            .last()
            .map(|ctx| ctx.root_id);
        let root_id = explicit_subtree_root_hash(
            self.current_effect_source_buffer_id,
            &self.current_effect_target,
            stable_key,
        );
        let owner =
            self.sync_subtree_owner_node(root_id, parent_root_id, stable_key.to_string(), callable);
        self.render_registered_subtree_owner(&owner)
    }

    fn sorted_current_reactive_reads(&self) -> Vec<ReactiveFieldKey> {
        let mut reads = self
            .current_effect_reactive_reads
            .as_ref()
            .map(|reads| reads.iter().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        reads.sort();
        reads
    }

    fn current_symbol_reads(&self) -> HashSet<String> {
        self.current_effect_symbol_reads.clone().unwrap_or_default()
    }

    fn set_effect_symbol_dependencies(&mut self, node_id: NodeId, dependencies: HashSet<String>) {
        if let Some(ReactiveNode::Effect {
            symbol_dependencies,
            ..
        }) = self.dag.nodes.get_mut(&node_id)
        {
            *symbol_dependencies = dependencies;
        }
    }

    fn attach_reactive_dependencies_to_pending_trees(
        &mut self,
        pending_trees_start: usize,
        reactive_dependencies: Vec<ReactiveFieldKey>,
    ) {
        for pending in self
            .pending_widget_trees
            .iter_mut()
            .skip(pending_trees_start)
        {
            match pending {
                PendingUiUpdate::FullTree(pending) => {
                    pending.reactive_dependencies = reactive_dependencies.clone();
                }
                PendingUiUpdate::ReplaceSubtree {
                    reactive_dependencies: pending_dependencies,
                    ..
                } => {
                    *pending_dependencies = reactive_dependencies.clone();
                }
            }
        }
    }

    fn reactive_node_label(&self, node_id: NodeId) -> Option<String> {
        match self.dag.nodes.get(&node_id) {
            Some(ReactiveNode::Effect { target, .. }) => Some(match target {
                EffectTarget::BufferId(Some(id)) => format!("buf#{id}"),
                EffectTarget::BufferId(None) => "active-buffer".to_string(),
                EffectTarget::BufferName(name) => name.clone(),
            }),
            Some(ReactiveNode::Derived { id, .. }) => Some(format!("derived:{id}")),
            _ => None,
        }
    }

    fn execute_from(&mut self, entry_chunk: usize) -> Result<Option<Value>, VMError> {
        self.execution_depth = self.execution_depth.saturating_add(1);
        let previous_chunk = self.current_chunk;
        self.current_chunk = entry_chunk;
        let result = self.execute();
        self.current_chunk = previous_chunk;
        self.execution_depth = self.execution_depth.saturating_sub(1);
        if result.is_ok() && self.execution_depth == 0 && !self.processing_reactive {
            self.process_dirty_reactive()?;
        }
        result
    }

    pub fn invoke(&mut self, callable: Value, args: Vec<Value>) -> Result<Option<Value>, VMError> {
        match callable {
            Value::Closure(chunk_idx, upvalues) => {
                self.execution_depth = self.execution_depth.saturating_add(1);
                let current_chunk = self.current_chunk;
                let result = self.execute_callable_chunk(chunk_idx, upvalues, args);
                self.current_chunk = current_chunk;
                self.execution_depth = self.execution_depth.saturating_sub(1);
                if result.is_ok() && self.execution_depth == 0 && !self.processing_reactive {
                    self.process_dirty_reactive()?;
                }
                result
            }
            Value::NativeFunction(f) => {
                let result = f(args, self);
                if self.execution_depth == 0 && !self.processing_reactive {
                    self.process_dirty_reactive()?;
                }
                Ok(Some(result))
            }
            Value::HostHandle { callable, .. } => {
                let result = callable(args, self);
                if self.execution_depth == 0 && !self.processing_reactive {
                    self.process_dirty_reactive()?;
                }
                Ok(Some(result))
            }
            _ => Err(VMError::ExpectedFunction),
        }
    }

    fn execute_callable_chunk(
        &mut self,
        chunk_idx: usize,
        upvalues: Vec<Rc<RefCell<Value>>>,
        args: Vec<Value>,
    ) -> Result<Option<Value>, VMError> {
        let previous_chunk = self.current_chunk;
        self.current_chunk = chunk_idx;
        let mut frame = self.new_frame();
        frame.upvalues = upvalues;
        if args.len() > frame.locals.len() {
            self.current_chunk = previous_chunk;
            return Err(VMError::ArityMismatch);
        }
        for (idx, arg) in args.into_iter().enumerate() {
            frame.locals[idx] = Some(Rc::new(RefCell::new(arg)));
        }

        let result = self.execute_with_frames(vec![frame]);
        self.current_chunk = previous_chunk;
        result
    }

    pub fn update_reactive_global(&mut self, namespace: &str, field: &str, value: Value) {
        let idx = self.ensure_global(namespace);
        if idx >= self.globals.len() {
            self.globals.resize(idx + 1, None);
        }

        match self.globals.get_mut(idx) {
            Some(Some(existing)) => {
                debug_assert_cell_not_frozen(existing, "update_reactive_global");
                let mut borrowed = existing.borrow_mut();
                if let Value::Map(map) = &mut *borrowed {
                    if let Some(slot) = map.get(field) {
                        debug_assert_cell_not_frozen(slot, "update_reactive_global");
                        *slot.borrow_mut() = value;
                    } else {
                        map.insert(field.to_string(), Rc::new(RefCell::new(value)));
                    }
                } else {
                    let mut map = HashMap::new();
                    map.insert(field.to_string(), Rc::new(RefCell::new(value)));
                    *borrowed = Value::Map(map);
                }
            }
            Some(slot @ None) => {
                let mut map = HashMap::new();
                map.insert(field.to_string(), Rc::new(RefCell::new(value)));
                *slot = Some(Rc::new(RefCell::new(Value::Map(map))));
            }
            None => {}
        }
    }

    pub fn update_reactive_global_list_index(
        &mut self,
        namespace: &str,
        field: &str,
        index: usize,
        value: Value,
    ) {
        let idx = self.ensure_global(namespace);
        if idx >= self.globals.len() {
            self.globals.resize(idx + 1, None);
        }

        let set_index = |map: &mut HashMap<String, Rc<RefCell<Value>>>| {
            let slot = map
                .entry(field.to_string())
                .or_insert_with(|| Rc::new(RefCell::new(Value::List(Vec::new()))));
            debug_assert_cell_not_frozen(slot, "update_reactive_global_list_index");
            let mut borrowed = slot.borrow_mut();
            match &mut *borrowed {
                Value::List(items) => {
                    while items.len() <= index {
                        items.push(Rc::new(RefCell::new(Value::Nil)));
                    }
                    debug_assert_cell_not_frozen(
                        &items[index],
                        "update_reactive_global_list_index",
                    );
                    *items[index].borrow_mut() = value.clone();
                }
                other => {
                    let mut items = Vec::with_capacity(index + 1);
                    for _ in 0..index {
                        items.push(Rc::new(RefCell::new(Value::Nil)));
                    }
                    items.push(Rc::new(RefCell::new(value.clone())));
                    *other = Value::List(items);
                }
            }
        };

        match self.globals.get_mut(idx) {
            Some(Some(existing)) => {
                let mut borrowed = existing.borrow_mut();
                if let Value::Map(map) = &mut *borrowed {
                    set_index(map);
                } else {
                    let mut map = HashMap::new();
                    set_index(&mut map);
                    *borrowed = Value::Map(map);
                }
            }
            Some(slot @ None) => {
                let mut map = HashMap::new();
                set_index(&mut map);
                *slot = Some(Rc::new(RefCell::new(Value::Map(map))));
            }
            None => {}
        }
    }

    fn current_reactive_value(&self, namespace: &str, field: &str) -> Value {
        self.global_value(namespace)
            .and_then(|value| match value {
                Value::Map(map) => map.get(field).map(|value| value.borrow().clone()),
                _ => None,
            })
            .unwrap_or(Value::Nil)
    }

    fn get_or_create_source_node(&mut self, namespace: &str, field: &str) -> NodeId {
        if let Some(id) = self.dag.find_namespace_field_source_node(namespace, field) {
            return id;
        }

        let id = self.dag.alloc_id();
        let value = self.current_reactive_value(namespace, field).deep_clone();
        let source = ReactiveSource::NamespaceField {
            namespace: namespace.to_string(),
            field: field.to_string(),
        };
        self.dag.add_node(ReactiveNode::Source {
            id,
            source,
            value,
            dependents: HashSet::new(),
        });
        id
    }

    pub fn has_reactive_subscribers(&self, namespace: &str, field: &str) -> bool {
        self.dag
            .find_namespace_field_source_node(namespace, field)
            .and_then(|id| self.dag.nodes.get(&id))
            .is_some_and(|node| match node {
                ReactiveNode::Source { dependents, .. } => !dependents.is_empty(),
                _ => false,
            })
    }

    fn get_or_create_local_state_node(
        &mut self,
        node_id: NodeId,
        name: &str,
        initial: Value,
    ) -> NodeId {
        if let Some(id) = self.dag.find_local_state_source_node(name) {
            if !self.preserve_state_on_redefinition {
                self.mark_source_dependents_dirty(id, initial);
            }
            return id;
        }

        let source = ReactiveSource::LocalState {
            name: name.to_string(),
        };
        self.dag.add_node(ReactiveNode::Source {
            id: node_id,
            source,
            value: initial,
            dependents: HashSet::new(),
        });
        node_id
    }

    fn mark_source_dependents_dirty(&mut self, source_id: NodeId, value: Value) {
        if let Some(ReactiveNode::Source {
            value: current_value,
            dependents,
            ..
        }) = self.dag.nodes.get_mut(&source_id)
        {
            let Some(change) = value_change_scope(current_value, &value) else {
                return;
            };
            let probe_started = clone_probe_enabled().then(Instant::now);
            let mut probe_allocs = 0u64;
            // Update the stored value. The store must stay a private deep
            // copy — never share Rcs with the caller's value. For list→list
            // changes patch only the changed indices; everything else keeps
            // the whole-value deep clone. The patch-vs-full decision is made
            // by this single match (never a partial patch then a Full
            // fallback), and `changed` may contain LEN_READ_SENTINEL, which
            // is not an index: length changes are handled by the explicit
            // truncate/extend below.
            let probe_label = match (&change, &mut *current_value, &value) {
                (ValueChange::Indices(changed), Value::List(old_items), Value::List(new_items)) => {
                    for &index in changed {
                        if index == LEN_READ_SENTINEL || index >= old_items.len() {
                            continue;
                        }
                        let Some(new_item) = new_items.get(index) else {
                            continue;
                        };
                        let cloned = new_item.borrow().deep_clone();
                        if probe_started.is_some() {
                            probe_allocs += value_alloc_nodes(&cloned);
                        }
                        old_items[index] = Rc::new(RefCell::new(cloned));
                    }
                    if old_items.len() > new_items.len() {
                        old_items.truncate(new_items.len());
                    } else {
                        for new_item in &new_items[old_items.len()..] {
                            let cloned = new_item.borrow().deep_clone();
                            if probe_started.is_some() {
                                probe_allocs += value_alloc_nodes(&cloned);
                            }
                            old_items.push(Rc::new(RefCell::new(cloned)));
                        }
                    }
                    "w1:patch"
                }
                _ => {
                    *current_value = value.deep_clone();
                    if probe_started.is_some() {
                        probe_allocs = value_alloc_nodes(current_value);
                    }
                    match &value {
                        Value::Map(_) => "w1:full-map",
                        Value::List(_) => "w1:full-list",
                        _ => "w1:full-other",
                    }
                }
            };
            if let Some(started) = probe_started {
                clone_probe_record(probe_label, started.elapsed(), probe_allocs);
            }
            let dependents = dependents.clone().into_iter().collect::<Vec<_>>();
            for dependent in dependents {
                let affected = match (&change, self.dag.dependency_scope(dependent, source_id)) {
                    (_, None) | (_, Some(ReadScope::All)) => true,
                    (ValueChange::Full, _) => true,
                    (ValueChange::Indices(changed), Some(ReadScope::Indices(read))) => {
                        changed.iter().any(|index| read.contains(index))
                    }
                };
                if affected {
                    static SCENE_TRACE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
                    if *SCENE_TRACE
                        .get_or_init(|| std::env::var("ESEQ_SCENE_TRACE").is_ok_and(|v| v == "1"))
                    {
                        let source_label = match self.dag.nodes.get(&source_id) {
                            Some(ReactiveNode::Source { source, .. }) => format!("{source:?}"),
                            _ => format!("node:{source_id}"),
                        };
                        let dependent_label = match self.dag.nodes.get(&dependent) {
                            Some(ReactiveNode::Effect {
                                subtree_root_id,
                                stable_key,
                                ..
                            }) => {
                                format!("effect root={subtree_root_id:?} key={stable_key:?}")
                            }
                            Some(ReactiveNode::Derived { .. }) => format!("derived:{dependent}"),
                            _ => format!("node:{dependent}"),
                        };
                        eprintln!(
                            "[mark-dirty] source={source_label} -> {dependent_label} scope={:?}",
                            self.dag.dependency_scope(dependent, source_id)
                        );
                    }
                    self.dag.mark_dirty(dependent);
                }
            }
        }
    }

    fn mark_owner_path_dirty(&mut self, owner_path: &str) {
        // Module-qualified names (`test.mod/panel`) are state keys, never
        // reactive namespace.field paths — the first `/` wins over `.`
        // (mirrors compile_set_statement).
        let parts = if crate::modules::is_qualified(owner_path) {
            vec![owner_path]
        } else {
            owner_path.splitn(2, '.').collect::<Vec<_>>()
        };
        if parts.len() == 1 {
            if let Some(node_id) = self.state_binding_node(parts[0]) {
                let current = self
                    .dag
                    .nodes
                    .get(&node_id)
                    .and_then(|node| match node {
                        ReactiveNode::Source { value, .. } => Some(value.clone()),
                        _ => None,
                    })
                    .unwrap_or(Value::Nil);
                self.mark_source_dependents_dirty(node_id, current);
            }
            return;
        }

        let namespace = parts[0];
        let field = parts[1];
        let current = self.current_reactive_value(namespace, field);
        let source_id = self.get_or_create_source_node(namespace, field);
        self.mark_source_dependents_dirty(source_id, current);
    }

    fn process_dirty_reactive(&mut self) -> Result<(), VMError> {
        if self.processing_reactive {
            return Ok(());
        }

        self.processing_reactive = true;
        self.last_reactive_error_context = None;
        self.last_reactive_error_detail = None;
        self.reactive_exec_timings.clear();
        let result = (|| -> Result<(), VMError> {
            loop {
                let mut sorted = self.dag.topo_sort_dirty();
                if sorted.is_empty() {
                    break;
                }
                // Run ancestor subtrees before descendants: rerunning a parent
                // re-renders (and re-registers) its dirty descendants, so any
                // descendant that already ran would be wasted work. Stable sort
                // keeps the topological order among unrelated nodes.
                sorted.sort_by_key(|node_id| match self.dag.nodes.get(node_id) {
                    Some(ReactiveNode::Derived { .. }) => 0usize,
                    Some(ReactiveNode::Effect {
                        subtree_root_id: None,
                        ..
                    }) => 1,
                    Some(ReactiveNode::Effect {
                        subtree_root_id: Some(root_id),
                        ..
                    }) => 2 + self.dag.subtree_depth(*root_id),
                    _ => 0,
                });

                let mut progressed = false;
                for node_id in sorted {
                    if !self.dag.is_dirty(node_id) {
                        continue;
                    }
                    let Some(chunk_idx) = self.dag.chunk_idx(node_id) else {
                        continue;
                    };
                    progressed = true;
                    let previous_owner = (
                        self.current_effect_source_buffer_id,
                        self.source_manager.source_stack_snapshot(),
                        self.current_effect_target.clone(),
                    );
                    if let Some((
                        source_buffer_id,
                        source_file,
                        source_revision,
                        target,
                        subtree_root_id,
                    )) = self.dag.nodes.get(&node_id).and_then(|node| match node {
                        ReactiveNode::Effect {
                            source_buffer_id,
                            source_file,
                            source_revision,
                            target,
                            subtree_root_id,
                            ..
                        } => Some((
                            *source_buffer_id,
                            source_file.clone(),
                            *source_revision,
                            target.clone(),
                            *subtree_root_id,
                        )),
                        _ => None,
                    }) {
                        self.current_effect_source_buffer_id = source_buffer_id;
                        let source_file_stack = match (source_file.clone(), source_revision) {
                            (Some(path), Some(revision)) => {
                                vec![SourceStackEntry { path, revision }]
                            }
                            _ => Vec::new(),
                        };
                        self.source_manager
                            .restore_source_stack(source_file_stack);
                        self.current_effect_target = target;
                        if let Some(root_id) = subtree_root_id {
                            let Some(owner) = self.registered_subtree_owner(root_id) else {
                                continue;
                            };
                            let previous_reactive_reads = self.current_effect_reactive_reads.take();
                            let previous_symbol_reads = self.current_effect_symbol_reads.take();
                            let previous_subtree_capture_stack =
                                std::mem::take(&mut self.current_subtree_capture_stack);
                            let previous_subtree_reactive_reads =
                                std::mem::take(&mut self.current_subtree_reactive_reads);
                            self.dag.clear_dependencies_of(node_id);
                            self.current_effect_reactive_reads = Some(HashSet::new());
                            self.current_effect_symbol_reads = Some(HashSet::new());
                            let label = self.reactive_node_label(node_id);
                            let started = Instant::now();
                            self.tracking_stack.push(node_id);
                            let render_result = self.render_registered_subtree_owner(&owner);
                            let _ = self.tracking_stack.pop();
                            let rendered_tree = render_result.map_err(|error| {
                                self.last_reactive_error_context =
                                    label.clone().or_else(|| Some(format!("node:{node_id}")));
                                error
                            })?;
                            let render_elapsed = started.elapsed();
                            let mut path = Vec::new();
                            let annotated_tree = annotate_widget_tree_stable_ids(
                                &rendered_tree,
                                self.current_effect_source_buffer_id,
                                source_file.as_deref(),
                                &self.current_effect_target,
                                None,
                                &mut path,
                            );
                            freeze_widget_tree(&annotated_tree);
                            {
                                static SCENE_TRACE: std::sync::OnceLock<bool> =
                                    std::sync::OnceLock::new();
                                if *SCENE_TRACE.get_or_init(|| {
                                    std::env::var("ESEQ_SCENE_TRACE").is_ok_and(|v| v == "1")
                                }) {
                                    eprintln!(
                                        "[subtree-render-split] root={} render_ms={:.3} annotate_ms={:.3}",
                                        root_id,
                                        render_elapsed.as_secs_f64() * 1000.0,
                                        (started.elapsed() - render_elapsed).as_secs_f64() * 1000.0,
                                    );
                                }
                            }
                            let mut reactive_dependencies = self
                                .current_subtree_reactive_reads
                                .get(&root_id)
                                .map(|reads| reads.iter().cloned().collect::<Vec<_>>())
                                .unwrap_or_default();
                            reactive_dependencies.sort();
                            self.pending_widget_trees
                                .push(PendingUiUpdate::ReplaceSubtree {
                                    source_buffer_id: self.current_effect_source_buffer_id,
                                    source_file: source_file.clone(),
                                    target: self.current_effect_target.clone(),
                                    subtree_root_id: root_id,
                                    tree: annotated_tree,
                                    reactive_dependencies,
                                });
                            self.dag.clear_dirty(node_id);
                            if let Some(label) = label {
                                let elapsed = started.elapsed();
                                self.reactive_exec_timings.push(ReactiveExecTiming {
                                    label,
                                    elapsed,
                                    source_buffer_id: self.current_effect_source_buffer_id,
                                    target: self.current_effect_target.clone(),
                                    subtree_root_id: Some(root_id),
                                });
                            }
                            self.current_effect_reactive_reads = previous_reactive_reads;
                            self.current_effect_symbol_reads = previous_symbol_reads;
                            self.current_subtree_capture_stack = previous_subtree_capture_stack;
                            self.current_subtree_reactive_reads = previous_subtree_reactive_reads;
                            self.current_effect_source_buffer_id = previous_owner.0;
                            self.source_manager.restore_source_stack(previous_owner.1);
                            self.current_effect_target = previous_owner.2;
                            continue;
                        }
                    }
                    let pending_trees_start = self.pending_widget_trees.len();
                    let previous_reactive_reads = self.current_effect_reactive_reads.take();
                    let previous_symbol_reads = self.current_effect_symbol_reads.take();
                    let previous_subtree_capture_stack =
                        std::mem::take(&mut self.current_subtree_capture_stack);
                    let previous_subtree_reactive_reads =
                        std::mem::take(&mut self.current_subtree_reactive_reads);
                    let capturing_effect_reads = matches!(
                        self.dag.nodes.get(&node_id),
                        Some(ReactiveNode::Effect { .. })
                    );
                    let is_top_level_effect = matches!(
                        self.dag.nodes.get(&node_id),
                        Some(ReactiveNode::Effect {
                            subtree_root_id: None,
                            ..
                        })
                    );
                    if capturing_effect_reads {
                        self.dag.clear_dependencies_of(node_id);
                        self.current_effect_reactive_reads = Some(HashSet::new());
                        self.current_effect_symbol_reads = Some(HashSet::new());
                        self.tracking_stack.push(node_id);
                    }
                    if is_top_level_effect {
                        self.clear_subtree_effects_for_current_context();
                    }
                    let label = self.reactive_node_label(node_id);
                    let started = Instant::now();
                    let execute_result = self.execute_from(chunk_idx);
                    if capturing_effect_reads {
                        let _ = self.tracking_stack.pop();
                    }
                    execute_result.map_err(|error| {
                        self.last_reactive_error_context =
                            label.clone().or_else(|| Some(format!("node:{node_id}")));
                        error
                    })?;
                    let captured_reactive_reads = if capturing_effect_reads {
                        self.sorted_current_reactive_reads()
                    } else {
                        Vec::new()
                    };
                    let captured_symbol_reads = if capturing_effect_reads {
                        self.current_symbol_reads()
                    } else {
                        HashSet::new()
                    };
                    if capturing_effect_reads {
                        self.set_effect_symbol_dependencies(node_id, captured_symbol_reads);
                        self.attach_reactive_dependencies_to_pending_trees(
                            pending_trees_start,
                            captured_reactive_reads,
                        );
                    }
                    if let Some(label) = label {
                        let elapsed = started.elapsed();
                        self.reactive_exec_timings.push(ReactiveExecTiming {
                            label,
                            elapsed,
                            source_buffer_id: self.current_effect_source_buffer_id,
                            target: self.current_effect_target.clone(),
                            subtree_root_id: self.dag.nodes.get(&node_id).and_then(
                                |node| match node {
                                    ReactiveNode::Effect {
                                        subtree_root_id, ..
                                    } => *subtree_root_id,
                                    _ => None,
                                },
                            ),
                        });
                    }
                    self.current_effect_reactive_reads = previous_reactive_reads;
                    self.current_effect_symbol_reads = previous_symbol_reads;
                    self.current_subtree_capture_stack = previous_subtree_capture_stack;
                    self.current_subtree_reactive_reads = previous_subtree_reactive_reads;
                    self.current_effect_source_buffer_id = previous_owner.0;
                    self.source_manager.restore_source_stack(previous_owner.1);
                    self.current_effect_target = previous_owner.2;
                }

                if !progressed {
                    break;
                }
            }
            Ok(())
        })();
        self.processing_reactive = false;
        result
    }

    pub fn apply_reactive_changes(
        &mut self,
        changes: Vec<(String, String, Value)>,
    ) -> Result<(), VMError> {
        if changes.is_empty() {
            return Ok(());
        }

        for (namespace, field, value) in changes {
            self.update_reactive_global(&namespace, &field, value.clone());
            let source_id = self.get_or_create_source_node(&namespace, &field);
            self.mark_source_dependents_dirty(source_id, value);
        }
        self.process_dirty_reactive()
    }

    fn chunk(&self) -> &Chunk {
        self.chunks.get(self.current_chunk).unwrap()
    }

    fn new_frame(&self) -> Frame {
        Frame {
            locals: vec![None; self.chunk().symbols.len()],
            upvalues: vec![],
            pc: 0,
            chunk_idx: self.current_chunk,
        }
    }

    fn unknown_local(&self, frame: &Frame, idx: usize) -> VMError {
        let name = self
            .chunks
            .get(frame.chunk_idx)
            .and_then(|chunk| chunk.symbols.get(idx))
            .cloned()
            .unwrap_or_else(|| format!("<local:{idx}>"));
        VMError::UnknownVariable(name)
    }

    fn unknown_upvalue(&self, frame: &Frame, idx: usize) -> VMError {
        let name = self
            .chunks
            .get(frame.chunk_idx)
            .and_then(|chunk| chunk.upvalues.get(idx))
            .cloned()
            .unwrap_or_else(|| format!("<upvalue:{idx}>"));
        VMError::UnknownVariable(name)
    }

    fn unknown_global(&self, idx: usize) -> VMError {
        // Report the bare name: the implicit-module prefix is an interning
        // detail users never typed (module-system spec slice 0).
        let name = self
            .global_names
            .get(idx)
            .map(|name| crate::modules::strip_implicit(name).to_string())
            .unwrap_or_else(|| format!("<global:{idx}>"));
        VMError::UnknownVariable(name)
    }

    pub fn execute(&mut self) -> Result<Option<Value>, VMError> {
        self.execute_with_frames(vec![self.new_frame()])
    }

    fn execute_with_frames(&mut self, mut frames: Vec<Frame>) -> Result<Option<Value>, VMError> {
        let mut stack: Vec<Rc<RefCell<Value>>> = vec![];

        while frames.last().unwrap().pc < self.chunks[self.current_chunk].ops.len() {
            let op = self.chunks[self.current_chunk].ops[frames.last().unwrap().pc].clone();
            match op {
                OpCode::PushConst(x) => {
                    if let Some(constant) = self.chunks[self.current_chunk].constants.get(x) {
                        stack.push(Rc::new(RefCell::new(Value::Number(*constant))));
                        frames.last_mut().unwrap().pc += 1;
                    } else {
                        return Err(VMError::UnknownConstant);
                    }
                }
                OpCode::PushStr(x) => {
                    if let Some(s) = self.chunks[self.current_chunk].strings.get(x) {
                        stack.push(Rc::new(RefCell::new(Value::String(s.clone()))));
                        frames.last_mut().unwrap().pc += 1;
                    } else {
                        return Err(VMError::UnknownConstant);
                    }
                }
                OpCode::PushBool(value) => {
                    stack.push(Rc::new(RefCell::new(Value::Bool(value))));
                    frames.last_mut().unwrap().pc += 1;
                }
                OpCode::PushNil => {
                    stack.push(Rc::new(RefCell::new(Value::Nil)));
                    frames.last_mut().unwrap().pc += 1;
                }
                OpCode::Add(arity) => {
                    if stack.len() < arity {
                        return Err(VMError::StackUnderflow);
                    }
                    let mut sum: f64 = 0.0;
                    for _ in 0..arity {
                        if let Some(val) = stack.pop() {
                            match &*val.borrow() {
                                Value::Number(val) => sum += val,
                                other => {
                                    self.last_reactive_error_detail =
                                        Some(format!("Add operand={other:?}"));
                                    return Err(VMError::IncorrectType);
                                }
                            }
                        }
                    }
                    stack.push(Rc::new(RefCell::new(Value::Number(sum))));
                    frames.last_mut().unwrap().pc += 1;
                }
                OpCode::Sub(arity) => {
                    if stack.len() < arity {
                        return Err(VMError::StackUnderflow);
                    }
                    let mut nums: Vec<f64> = vec![];
                    for _ in 0..arity {
                        if let Some(val) = stack.pop() {
                            match &*val.borrow() {
                                Value::Number(val) => nums.push(*val),
                                other => {
                                    self.last_reactive_error_detail =
                                        Some(format!("Sub operand={other:?}"));
                                    return Err(VMError::IncorrectType);
                                }
                            }
                        }
                    }
                    nums.reverse();
                    let diff = nums[1..].iter().fold(nums[0], |acc, x| acc - x);
                    stack.push(Rc::new(RefCell::new(Value::Number(diff))));
                    frames.last_mut().unwrap().pc += 1;
                }
                OpCode::Mul(arity) => {
                    if stack.len() < arity {
                        return Err(VMError::StackUnderflow);
                    }
                    let mut product: f64 = 1.0;
                    for _ in 0..arity {
                        if let Some(val) = stack.pop() {
                            match &*val.borrow() {
                                Value::Number(val) => product *= val,
                                other => {
                                    self.last_reactive_error_detail =
                                        Some(format!("Mul operand={other:?}"));
                                    return Err(VMError::IncorrectType);
                                }
                            }
                        }
                    }
                    stack.push(Rc::new(RefCell::new(Value::Number(product))));
                    frames.last_mut().unwrap().pc += 1;
                }
                OpCode::Div(arity) => {
                    if stack.len() < arity || arity == 0 {
                        return Err(VMError::StackUnderflow);
                    }
                    let mut nums: Vec<f64> = vec![];
                    for _ in 0..arity {
                        if let Some(val) = stack.pop() {
                            match &*val.borrow() {
                                Value::Number(val) => nums.push(*val),
                                other => {
                                    self.last_reactive_error_detail =
                                        Some(format!("Div operand={other:?}"));
                                    return Err(VMError::IncorrectType);
                                }
                            }
                        }
                    }
                    nums.reverse();
                    let quotient = if nums.len() == 1 {
                        1.0 / nums[0]
                    } else {
                        nums[1..].iter().fold(nums[0], |acc, x| acc / x)
                    };
                    stack.push(Rc::new(RefCell::new(Value::Number(quotient))));
                    frames.last_mut().unwrap().pc += 1;
                }
                OpCode::Min(arity) => {
                    if stack.len() < arity || arity == 0 {
                        return Err(VMError::StackUnderflow);
                    }
                    let mut current = f64::INFINITY;
                    for _ in 0..arity {
                        if let Some(val) = stack.pop() {
                            match &*val.borrow() {
                                Value::Number(val) => current = current.min(*val),
                                other => {
                                    self.last_reactive_error_detail =
                                        Some(format!("Min operand={other:?}"));
                                    return Err(VMError::IncorrectType);
                                }
                            }
                        }
                    }
                    stack.push(Rc::new(RefCell::new(Value::Number(current))));
                    frames.last_mut().unwrap().pc += 1;
                }
                OpCode::Max(arity) => {
                    if stack.len() < arity || arity == 0 {
                        return Err(VMError::StackUnderflow);
                    }
                    let mut current = f64::NEG_INFINITY;
                    for _ in 0..arity {
                        if let Some(val) = stack.pop() {
                            match &*val.borrow() {
                                Value::Number(val) => current = current.max(*val),
                                other => {
                                    self.last_reactive_error_detail =
                                        Some(format!("Max operand={other:?}"));
                                    return Err(VMError::IncorrectType);
                                }
                            }
                        }
                    }
                    stack.push(Rc::new(RefCell::new(Value::Number(current))));
                    frames.last_mut().unwrap().pc += 1;
                }
                OpCode::Pop => {
                    if stack.pop().is_none() {
                        return Err(VMError::StackUnderflow);
                    }
                    frames.last_mut().unwrap().pc += 1;
                }
                OpCode::Eq => {
                    if stack.len() < 2 {
                        return Err(VMError::StackUnderflow);
                    }
                    let mut result = false;
                    if let (Some(a), Some(b)) = (stack.pop(), stack.pop()) {
                        result = *a.borrow() == *b.borrow();
                    }
                    stack.push(Rc::new(RefCell::new(Value::Bool(result))));
                    frames.last_mut().unwrap().pc += 1;
                }
                op @ (OpCode::Lt | OpCode::Gt | OpCode::Lte | OpCode::Gte) => {
                    if stack.len() < 2 {
                        return Err(VMError::StackUnderflow);
                    }
                    if let (Some(a), Some(b)) = (stack.pop(), stack.pop()) {
                        match (&*a.borrow(), &*b.borrow()) {
                            (Value::Number(a), Value::Number(b)) => {
                                let result = match op {
                                    OpCode::Lt => b < a,
                                    OpCode::Gt => b > a,
                                    OpCode::Lte => b <= a,
                                    _ => b >= a,
                                };
                                stack.push(Rc::new(RefCell::new(Value::Bool(result))));
                            }
                            (right, left) => {
                                self.last_reactive_error_detail =
                                    Some(format!("{op:?} left={left:?} right={right:?}"));
                                return Err(VMError::IncorrectType);
                            }
                        }
                    }
                    frames.last_mut().unwrap().pc += 1;
                }
                OpCode::MakeList(arity) => {
                    if stack.len() < arity {
                        return Err(VMError::StackUnderflow);
                    }
                    let mut list: Vec<_> = (0..arity).filter_map(|_| stack.pop()).collect();
                    list.reverse();
                    stack.push(Rc::new(RefCell::new(Value::List(list))));
                    frames.last_mut().unwrap().pc += 1;
                }
                OpCode::Jump(pc) => {
                    if let Some(frame) = frames.last_mut() {
                        frame.pc += pc;
                    }
                }
                OpCode::JumpIfFalse(pc) => {
                    if stack.is_empty() {
                        return Err(VMError::StackUnderflow);
                    }
                    if let Some(result) = stack.pop()
                        && let Some(frame) = frames.last_mut()
                    {
                        let is_false = is_falsey(&result.borrow());
                        if is_false {
                            frame.pc += pc;
                        } else {
                            frame.pc += 1;
                        }
                    }
                }
                OpCode::StoreLocal(idx) => {
                    if let Some(frame) = frames.last_mut() {
                        frame.locals[idx] = stack.pop();
                        frame.pc += 1;
                    }
                }
                OpCode::LoadLocal(idx) => {
                    if let Some(frame) = frames.last_mut() {
                        if let Some(Some(val)) = frame.locals.get(idx) {
                            stack.push(Rc::clone(val));
                            frame.pc += 1;
                        } else {
                            return Err(self.unknown_local(frame, idx));
                        }
                    }
                }
                OpCode::StoreUpvalue(idx) => {
                    if let Some(frame) = frames.last_mut() {
                        frame.upvalues[idx] = stack.pop().unwrap();
                        frame.pc += 1;
                    }
                }
                OpCode::StoreState(node_id) => {
                    let Some(value) = stack.pop() else {
                        return Err(VMError::StackUnderflow);
                    };
                    let new_value = value.borrow().clone();
                    self.mark_source_dependents_dirty(node_id, new_value);
                    if let Some(frame) = frames.last_mut() {
                        frame.pc += 1;
                    }
                }
                OpCode::StoreField(field_idx) => {
                    let Some(value) = stack.pop() else {
                        return Err(VMError::StackUnderflow);
                    };
                    let Some(target) = stack.pop() else {
                        return Err(VMError::StackUnderflow);
                    };
                    let field = self.chunks[self.current_chunk].strings[field_idx].clone();
                    let new_value = value.borrow().clone();
                    let owner_path = match &*target.borrow() {
                        Value::Map(map) => map.get("__eseq_owner").and_then(|entry| match &*entry
                            .borrow()
                        {
                            Value::String(path) => Some(path.clone()),
                            _ => None,
                        }),
                        _ => None,
                    };
                    debug_assert_cell_not_frozen(&target, "OpCode::StoreField");
                    match &mut *target.borrow_mut() {
                        Value::Map(map) => {
                            if let Some(slot) = map.get(&field) {
                                debug_assert_cell_not_frozen(slot, "OpCode::StoreField");
                                *slot.borrow_mut() = new_value.clone();
                            } else {
                                map.insert(field.clone(), Rc::new(RefCell::new(new_value.clone())));
                            }
                        }
                        _ => return Err(VMError::IncorrectType),
                    }
                    if let Some(owner) = owner_path {
                        self.mark_owner_path_dirty(&owner);
                    }
                    stack.push(Rc::new(RefCell::new(new_value)));
                    if let Some(frame) = frames.last_mut() {
                        frame.pc += 1;
                    }
                }
                OpCode::LoadUpvalue(idx) => {
                    if let Some(frame) = frames.last_mut() {
                        if let Some(val) = frame.upvalues.get(idx) {
                            stack.push(Rc::clone(val));
                            frame.pc += 1;
                        } else {
                            return Err(self.unknown_upvalue(frame, idx));
                        }
                    }
                }
                OpCode::StoreGlobal(idx) => {
                    if let Some(frame) = frames.last_mut() {
                        if idx >= self.globals.len() {
                            self.globals.resize(idx + 1, None);
                        }
                        let stored = stack.pop();
                        self.globals[idx] = stored.clone();
                        if let (Some(name), Some(value)) =
                            (self.global_names.get(idx), stored.as_ref())
                        {
                            // Hooks see the bare name: consumers (process/
                            // channel naming) treat it as a user-visible
                            // identifier. Revisit when registries qualify
                            // per-module (spec §5, slice 3).
                            let value = value.borrow().clone();
                            let hook_name = crate::modules::strip_implicit(name);
                            for hook in &self.global_store_hooks {
                                hook(hook_name, &value);
                            }
                        }
                        frame.pc += 1;
                    }
                }
                OpCode::LoadGlobal(idx) => {
                    if let Some(frame) = frames.last_mut() {
                        if let Some(Some(val)) = self.globals.get(idx).cloned() {
                            if let Some(name) = self.global_names.get(idx).cloned() {
                                self.record_symbol_read(&name);
                            }
                            stack.push(val);
                            frame.pc += 1;
                        } else {
                            let _ = frame;
                            return Err(self.unknown_global(idx));
                        }
                    }
                }
                OpCode::InitDerived(node_id, chunk_idx) => {
                    if !self.dag.nodes.contains_key(&node_id) {
                        self.dag.add_node(ReactiveNode::Derived {
                            id: node_id,
                            chunk_idx,
                            value: Value::Nil,
                            dependents: HashSet::new(),
                            dirty: false,
                        });
                    }

                    let current_chunk = self.current_chunk;
                    let result = self.execute_from(chunk_idx)?;
                    self.current_chunk = current_chunk;
                    let _ = result;
                    stack.push(Rc::new(RefCell::new(Value::NodeRef(node_id))));
                    frames.last_mut().unwrap().pc += 1;
                }
                OpCode::InitState(node_id) => {
                    let Some(initial) = stack.pop() else {
                        return Err(VMError::StackUnderflow);
                    };
                    let initial_value = initial.borrow().clone();
                    let name = self
                        .state_bindings
                        .iter()
                        .find_map(|(name, id)| (*id == node_id).then_some(name.clone()))
                        .unwrap_or_else(|| format!("state:{node_id}"));
                    self.get_or_create_local_state_node(node_id, &name, initial_value);
                    stack.push(Rc::new(RefCell::new(Value::NodeRef(node_id))));
                    frames.last_mut().unwrap().pc += 1;
                }
                OpCode::InitEffect(node_id, chunk_idx) => {
                    self.upsert_top_level_effect_node(
                        node_id,
                        chunk_idx,
                        self.current_effect_target.clone(),
                    );

                    let current_chunk = self.current_chunk;
                    let pending_trees_start = self.pending_widget_trees.len();
                    let previous_reactive_reads = self.current_effect_reactive_reads.take();
                    let previous_symbol_reads = self.current_effect_symbol_reads.take();
                    self.dag.clear_dependencies_of(node_id);
                    self.current_effect_reactive_reads = Some(HashSet::new());
                    self.current_effect_symbol_reads = Some(HashSet::new());
                    self.tracking_stack.push(node_id);
                    let result = self.execute_from(chunk_idx);
                    let _ = self.tracking_stack.pop();
                    let captured_reactive_reads = self.sorted_current_reactive_reads();
                    let captured_symbol_reads = self.current_symbol_reads();
                    self.set_effect_symbol_dependencies(node_id, captured_symbol_reads);
                    self.attach_reactive_dependencies_to_pending_trees(
                        pending_trees_start,
                        captured_reactive_reads,
                    );
                    self.current_effect_reactive_reads = previous_reactive_reads;
                    self.current_effect_symbol_reads = previous_symbol_reads;
                    self.current_chunk = current_chunk;
                    let _ = result?;
                    stack.push(Rc::new(RefCell::new(Value::Nil)));
                    frames.last_mut().unwrap().pc += 1;
                }
                OpCode::InitNamedEffect(node_id, chunk_idx, name_idx) => {
                    let target_name = self.chunks[self.current_chunk].strings[name_idx].clone();
                    self.upsert_top_level_effect_node(
                        node_id,
                        chunk_idx,
                        EffectTarget::BufferName(target_name.clone()),
                    );

                    let current_chunk = self.current_chunk;
                    let previous_target = self.current_effect_target.clone();
                    let pending_trees_start = self.pending_widget_trees.len();
                    let previous_reactive_reads = self.current_effect_reactive_reads.take();
                    let previous_symbol_reads = self.current_effect_symbol_reads.take();
                    self.current_effect_target = EffectTarget::BufferName(target_name);
                    self.dag.clear_dependencies_of(node_id);
                    self.current_effect_reactive_reads = Some(HashSet::new());
                    self.current_effect_symbol_reads = Some(HashSet::new());
                    self.tracking_stack.push(node_id);
                    let result = self.execute_from(chunk_idx);
                    let _ = self.tracking_stack.pop();
                    let captured_reactive_reads = self.sorted_current_reactive_reads();
                    let captured_symbol_reads = self.current_symbol_reads();
                    self.set_effect_symbol_dependencies(node_id, captured_symbol_reads);
                    self.attach_reactive_dependencies_to_pending_trees(
                        pending_trees_start,
                        captured_reactive_reads,
                    );
                    self.current_effect_reactive_reads = previous_reactive_reads;
                    self.current_effect_symbol_reads = previous_symbol_reads;
                    self.current_chunk = current_chunk;
                    self.current_effect_target = previous_target;
                    let _ = result?;
                    stack.push(Rc::new(RefCell::new(Value::Nil)));
                    frames.last_mut().unwrap().pc += 1;
                }
                OpCode::LoadDerived(node_id) => {
                    let value = self.dag.derived_value(node_id).unwrap_or(Value::Nil);
                    if let Some(ctx_id) = self.tracking_stack.last().copied() {
                        self.dag.add_edge(node_id, ctx_id);
                    }
                    stack.push(Rc::new(RefCell::new(value)));
                    frames.last_mut().unwrap().pc += 1;
                }
                OpCode::LoadState(node_id) => {
                    let value = self
                        .dag
                        .nodes
                        .get(&node_id)
                        .and_then(|node| match node {
                            ReactiveNode::Source { value, .. } => Some(value.clone()),
                            _ => None,
                        })
                        .unwrap_or(Value::Nil);
                    if let Some(ctx_id) = self.tracking_stack.last().copied() {
                        self.dag.add_edge(node_id, ctx_id);
                    }
                    stack.push(Rc::new(RefCell::new(value)));
                    frames.last_mut().unwrap().pc += 1;
                }
                OpCode::DerivedBegin(node_id) => {
                    self.dag.clear_dependencies_of(node_id);
                    self.tracking_stack.push(node_id);
                    frames.last_mut().unwrap().pc += 1;
                }
                OpCode::DerivedEnd(node_id) => {
                    let Some(computed) = stack.pop() else {
                        return Err(VMError::StackUnderflow);
                    };
                    let _ = self.tracking_stack.pop();
                    let new_value = computed.borrow().clone();
                    let mut changed_dependents = Vec::new();
                    let mut should_clear_dirty = false;

                    if let Some(ReactiveNode::Derived {
                        value, dependents, ..
                    }) = self.dag.nodes.get_mut(&node_id)
                    {
                        let changed = *value != new_value;
                        *value = new_value.clone();
                        should_clear_dirty = true;
                        if changed {
                            changed_dependents = dependents.iter().copied().collect();
                        }
                    }

                    if should_clear_dirty {
                        self.dag.clear_dirty(node_id);
                    }

                    for dependent in changed_dependents {
                        self.dag.mark_dirty(dependent);
                    }

                    stack.push(Rc::new(RefCell::new(new_value)));
                    frames.last_mut().unwrap().pc += 1;
                }
                OpCode::EffectBegin(node_id) => {
                    self.dag.clear_dependencies_of(node_id);
                    self.tracking_stack.push(node_id);
                    frames.last_mut().unwrap().pc += 1;
                }
                OpCode::EffectEnd(node_id) => {
                    let _ = self.tracking_stack.pop();
                    self.dag.clear_dirty(node_id);
                    frames.last_mut().unwrap().pc += 1;
                }
                OpCode::SubtreeBegin => {
                    let Some(key_value) = stack.pop() else {
                        return Err(VMError::StackUnderflow);
                    };
                    let Some(stable_key) = subtree_key_string(&key_value.borrow()) else {
                        return Err(VMError::IncorrectType);
                    };
                    let parent_root_id = self
                        .current_subtree_capture_stack
                        .last()
                        .map(|ctx| ctx.root_id);
                    let root_id = explicit_subtree_root_hash(
                        self.current_effect_source_buffer_id,
                        &self.current_effect_target,
                        &stable_key,
                    );
                    self.current_subtree_capture_stack
                        .push(SubtreeCaptureContext {
                            root_id,
                            parent_root_id,
                            stable_key,
                        });
                    self.current_subtree_reactive_reads
                        .entry(root_id)
                        .or_default();
                    frames.last_mut().unwrap().pc += 1;
                }
                OpCode::SubtreeEnd => {
                    let Some(tree) = stack.pop() else {
                        return Err(VMError::StackUnderflow);
                    };
                    let Some(context) = self.current_subtree_capture_stack.pop() else {
                        return Err(VMError::StackUnderflow);
                    };
                    let annotated_tree = annotate_explicit_subtree_root(
                        &tree.borrow(),
                        context.root_id,
                        context.parent_root_id,
                        &context.stable_key,
                        true,
                    );
                    stack.push(Rc::new(RefCell::new(annotated_tree)));
                    frames.last_mut().unwrap().pc += 1;
                }
                OpCode::LoadReactive(ns_idx, field_idx) => {
                    let namespace = self.chunks[self.current_chunk].strings[ns_idx].clone();
                    let field = self.chunks[self.current_chunk].strings[field_idx].clone();
                    self.record_reactive_read(&namespace, &field);
                    let Some(global_idx) =
                        self.global_names.iter().position(|name| name == &namespace)
                    else {
                        return Err(VMError::UnknownVariable(namespace));
                    };
                    let Some(Some(val)) = self.globals.get(global_idx) else {
                        return Err(self.unknown_global(global_idx));
                    };
                    let result = match &*val.borrow() {
                        Value::Map(map) => map
                            .get(&field)
                            .cloned()
                            .unwrap_or_else(|| Rc::new(RefCell::new(Value::Nil))),
                        _ => return Err(VMError::IncorrectType),
                    };
                    if let Some(ctx_id) = self.tracking_stack.last().copied() {
                        let source_id = self.get_or_create_source_node(&namespace, &field);
                        self.dag.add_edge(source_id, ctx_id);
                    }
                    stack.push(result);
                    frames.last_mut().unwrap().pc += 1;
                }
                OpCode::LoadReactiveNth(ns_idx, field_idx) => {
                    let namespace = self.chunks[self.current_chunk].strings[ns_idx].clone();
                    let field = self.chunks[self.current_chunk].strings[field_idx].clone();
                    let Some(index_value) = stack.pop() else {
                        return Err(VMError::StackUnderflow);
                    };
                    self.record_reactive_read(&namespace, &field);
                    let Some(global_idx) =
                        self.global_names.iter().position(|name| name == &namespace)
                    else {
                        return Err(VMError::UnknownVariable(namespace));
                    };
                    let Some(Some(val)) = self.globals.get(global_idx) else {
                        return Err(self.unknown_global(global_idx));
                    };
                    let cell = match &*val.borrow() {
                        Value::Map(map) => map.get(&field).cloned(),
                        _ => return Err(VMError::IncorrectType),
                    };
                    let index = match &*index_value.borrow() {
                        Value::Number(idx) if *idx >= 0.0 => Some(*idx as usize),
                        _ => None,
                    };
                    // Same semantics as the `nth` native: (List, Number >= 0)
                    // returns the element (or Nil out of range); anything else
                    // is Nil.
                    let (result, read_index) = match (cell.as_ref(), index) {
                        (Some(cell), Some(idx)) => match &*cell.borrow() {
                            Value::List(items) => (
                                items
                                    .get(idx)
                                    .map(|item| item.borrow().clone())
                                    .unwrap_or(Value::Nil),
                                Some(idx),
                            ),
                            _ => (Value::Nil, None),
                        },
                        _ => (Value::Nil, None),
                    };
                    if let Some(ctx_id) = self.tracking_stack.last().copied() {
                        let source_id = self.get_or_create_source_node(&namespace, &field);
                        match read_index {
                            Some(idx) => self.dag.add_edge_indexed(source_id, ctx_id, idx),
                            None => self.dag.add_edge(source_id, ctx_id),
                        }
                    }
                    stack.push(Rc::new(RefCell::new(result)));
                    frames.last_mut().unwrap().pc += 1;
                }
                OpCode::LoadReactiveLen(ns_idx, field_idx) => {
                    let namespace = self.chunks[self.current_chunk].strings[ns_idx].clone();
                    let field = self.chunks[self.current_chunk].strings[field_idx].clone();
                    self.record_reactive_read(&namespace, &field);
                    let Some(global_idx) =
                        self.global_names.iter().position(|name| name == &namespace)
                    else {
                        return Err(VMError::UnknownVariable(namespace));
                    };
                    let Some(Some(val)) = self.globals.get(global_idx) else {
                        return Err(self.unknown_global(global_idx));
                    };
                    let cell = match &*val.borrow() {
                        Value::Map(map) => map.get(&field).cloned(),
                        _ => return Err(VMError::IncorrectType),
                    };
                    // Same semantics as the `len` native.
                    let (result, len_read) = match cell.as_ref() {
                        Some(cell) => match &*cell.borrow() {
                            Value::List(items) => (Value::Number(items.len() as f64), true),
                            Value::String(s) => (Value::Number(s.chars().count() as f64), false),
                            _ => (Value::Number(0.0), false),
                        },
                        None => (Value::Number(0.0), false),
                    };
                    if let Some(ctx_id) = self.tracking_stack.last().copied() {
                        let source_id = self.get_or_create_source_node(&namespace, &field);
                        if len_read {
                            self.dag
                                .add_edge_indexed(source_id, ctx_id, LEN_READ_SENTINEL);
                        } else {
                            self.dag.add_edge(source_id, ctx_id);
                        }
                    }
                    stack.push(Rc::new(RefCell::new(result)));
                    frames.last_mut().unwrap().pc += 1;
                }
                OpCode::StoreReactive(ns_idx, field_idx) => {
                    let Some(value) = stack.pop() else {
                        return Err(VMError::StackUnderflow);
                    };
                    let namespace = self.chunks[self.current_chunk].strings[ns_idx].clone();
                    let field = self.chunks[self.current_chunk].strings[field_idx].clone();
                    if !self.writable_reactive_namespaces.contains(&namespace) {
                        return Err(VMError::ReadonlyReactive(namespace));
                    }
                    let new_value = value.borrow().clone();
                    self.reactive_float_slots
                        .store_value(&namespace, &field, &new_value);
                    self.update_reactive_global(&namespace, &field, new_value.clone());
                    let source_id = self.get_or_create_source_node(&namespace, &field);
                    self.mark_source_dependents_dirty(source_id, new_value);
                    frames.last_mut().unwrap().pc += 1;
                }
                OpCode::MakeClosure(chunk_idx, num_upvalues) => {
                    if let Some(frame) = frames.last_mut() {
                        let mut upvalues = vec![];
                        for _ in 0..num_upvalues {
                            upvalues.push(stack.pop().unwrap());
                        }
                        upvalues.reverse();
                        stack.push(Rc::new(Value::Closure(chunk_idx, upvalues).into()));
                        frame.pc += 1;
                    }
                }
                OpCode::Call(arity) => {
                    if let Some(v) = stack.pop() {
                        let borrowed = v.borrow();
                        match &*borrowed {
                            Value::Closure(chunk_idx, upvalues) => {
                                let chunk_idx = *chunk_idx;
                                let upvalues = upvalues.clone();
                                drop(borrowed);
                                self.current_chunk = chunk_idx;
                                let mut frame = self.new_frame();
                                frame.upvalues = upvalues;
                                if arity > frame.locals.len() {
                                    return Err(VMError::ArityMismatch);
                                }
                                if stack.len() < arity {
                                    return Err(VMError::StackUnderflow);
                                }
                                for i in 0..arity {
                                    let local_idx = arity - i - 1;
                                    let Some(slot) = frame.locals.get_mut(local_idx) else {
                                        return Err(VMError::ArityMismatch);
                                    };
                                    *slot = stack.pop();
                                }
                                frames.last_mut().unwrap().pc += 1;
                                frames.push(frame);
                            }
                            Value::NativeFunction(f) => {
                                // Clone the Rc so we can drop the borrow before touching the stack
                                let f = f.clone();
                                drop(borrowed);
                                let mut args: Vec<Value> = (0..arity)
                                    .filter_map(|_| stack.pop())
                                    .map(|v| v.borrow().clone())
                                    .collect();
                                args.reverse();
                                let result = f(args, self);
                                stack.push(Rc::new(RefCell::new(result)));
                                frames.last_mut().unwrap().pc += 1;
                            }
                            Value::HostHandle { callable, .. } => {
                                let f = callable.clone();
                                drop(borrowed);
                                let mut args: Vec<Value> = (0..arity)
                                    .filter_map(|_| stack.pop())
                                    .map(|v| v.borrow().clone())
                                    .collect();
                                args.reverse();
                                let result = f(args, self);
                                stack.push(Rc::new(RefCell::new(result)));
                                frames.last_mut().unwrap().pc += 1;
                            }
                            _ => {
                                return Err(VMError::ExpectedFunction);
                            }
                        }
                    }
                }
                OpCode::Eval => match stack.pop() {
                    Some(val) => {
                        if let Value::String(code) = &*(val.borrow()) {
                            let current_chunk = self.current_chunk;
                            match (self.eval_str(code)?, frames.last_mut()) {
                                (result, Some(frame)) => {
                                    self.current_chunk = current_chunk;
                                    stack.push(Rc::new(RefCell::new(result.unwrap_or(Value::Nil))));
                                    frame.pc += 1;
                                }
                                _ => {
                                    return Err(VMError::IncorrectType);
                                }
                            }
                        }
                    }
                    None => {
                        return Err(VMError::StackUnderflow);
                    }
                },
                OpCode::PushKeyword(idx) => {
                    let kw = self.chunks[self.current_chunk].strings[idx].clone();
                    stack.push(Rc::new(RefCell::new(Value::Keyword(kw))));
                    frames.last_mut().unwrap().pc += 1;
                }
                OpCode::PushSymbol(idx) => {
                    let sym = self.chunks[self.current_chunk].strings[idx].clone();
                    stack.push(Rc::new(RefCell::new(Value::Symbol(sym))));
                    frames.last_mut().unwrap().pc += 1;
                }
                OpCode::GetField(idx) => {
                    let key = self.chunks[self.current_chunk].strings[idx].clone();
                    match stack.pop() {
                        Some(val) => {
                            let result = match &*val.borrow() {
                                Value::Map(m) => m
                                    .get(&key)
                                    .cloned()
                                    .unwrap_or_else(|| Rc::new(RefCell::new(Value::Nil))),
                                _ => return Err(VMError::IncorrectType),
                            };
                            stack.push(result);
                            frames.last_mut().unwrap().pc += 1;
                        }
                        None => return Err(VMError::StackUnderflow),
                    }
                }
                OpCode::EmitTree => match stack.pop() {
                    Some(tree) => {
                        let mut path = Vec::new();
                        let annotated_tree = annotate_widget_tree_stable_ids(
                            &tree.borrow(),
                            self.current_effect_source_buffer_id,
                            self.source_manager.current_source_file().as_deref(),
                            &self.current_effect_target,
                            None,
                            &mut path,
                        );
                        freeze_widget_tree(&annotated_tree);
                        if let Some((subtree_root_id, _stable_key)) =
                            explicit_subtree_root_metadata(&annotated_tree)
                        {
                            let mut reactive_dependencies = self
                                .current_subtree_reactive_reads
                                .get(&subtree_root_id)
                                .map(|reads| reads.iter().cloned().collect::<Vec<_>>())
                                .unwrap_or_default();
                            reactive_dependencies.sort();
                            self.pending_widget_trees
                                .push(PendingUiUpdate::ReplaceSubtree {
                                    source_buffer_id: self.current_effect_source_buffer_id,
                                    source_file: self.source_manager.current_source_file(),
                                    target: self.current_effect_target.clone(),
                                    subtree_root_id,
                                    tree: annotated_tree,
                                    reactive_dependencies,
                                });
                        } else {
                            self.pending_widget_trees.push(PendingUiUpdate::FullTree(
                                PendingWidgetTree {
                                    source_buffer_id: self.current_effect_source_buffer_id,
                                    source_file: self.source_manager.current_source_file(),
                                    target: self.current_effect_target.clone(),
                                    tree: annotated_tree,
                                    reactive_dependencies: Vec::new(),
                                },
                            ));
                        }
                        frames.last_mut().unwrap().pc += 1;
                    }
                    None => return Err(VMError::StackUnderflow),
                },
                OpCode::Return => match stack.pop() {
                    Some(return_value) => {
                        frames.pop();
                        if let Some(caller_frame) = frames.last() {
                            self.current_chunk = caller_frame.chunk_idx;
                            stack.push(return_value);
                        } else {
                            // Last frame returned — this is the final result
                            return Ok(Some(return_value.borrow().clone()));
                        }
                    }
                    None => return Err(VMError::StackUnderflow),
                },
                _ => return Err(VMError::UnknownOpcode),
            }
        }

        if let Some(result) = stack.last() {
            return Ok(Some(result.borrow().clone()));
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use std::{
        cell::RefCell,
        collections::{HashMap, HashSet},
        rc::Rc,
    };

    use super::{
        EffectTarget, LEN_READ_SENTINEL, NodeId, PendingUiUpdate, ReactiveDag, ReactiveNode,
        ReactiveSource, SOURCE_BUFFER_ID_PROP, SOURCE_END_BYTE_PROP, SOURCE_MODULE_PATH_PROP,
        SOURCE_REVISION_PROP, SOURCE_START_BYTE_PROP, SOURCE_SYMBOL_PROP, STABLE_KEY_PROP, VM,
        Value, debug_assert_cell_not_frozen, freeze_widget_tree,
    };

    fn hook_test_vm() -> (VM, Rc<RefCell<Vec<f64>>>) {
        let mut vm = VM::new(Vec::new());
        super::register_core_natives(&mut vm);
        let calls = Rc::new(RefCell::new(Vec::<f64>::new()));
        let sink = calls.clone();
        vm.register_native("record-hook-call", move |args| {
            if let Some(Value::Number(n)) = args.first() {
                sink.borrow_mut().push(*n);
            }
            Value::Nil
        });
        (vm, calls)
    }

    #[test]
    fn extension_hooks_run_in_order_replace_by_key_and_remove() {
        let (mut vm, calls) = hook_test_vm();
        let path = std::env::temp_dir().join(format!(
            "eseqlisp-hooks-order-{}.lisp",
            std::process::id()
        ));
        let source = r#"
(defhook "test-hook")
(add-hook "test-hook" "first" (lambda () (record-hook-call 1)))
(add-hook "test-hook" "second" (lambda () (record-hook-call 2)))
(test-hook)
(add-hook "test-hook" "first" (lambda () (record-hook-call 10)))
(test-hook)
(remove-hook "test-hook" "second")
(test-hook)
"#;
        vm.eval_module_source(path, source, 1).expect("module eval");
        // Run 1: both listeners in registration order. Run 2: re-adding
        // "first" replaced it IN PLACE (order kept). Run 3: "second" removed.
        assert_eq!(*calls.borrow(), vec![1.0, 2.0, 10.0, 2.0, 10.0]);
    }

    #[test]
    fn extension_hooks_forward_args_and_tolerate_no_listeners() {
        let (mut vm, calls) = hook_test_vm();
        let path = std::env::temp_dir().join(format!(
            "eseqlisp-hooks-args-{}.lisp",
            std::process::id()
        ));
        let source = r#"
(defhook "arg-hook")
(arg-hook)
(add-hook "arg-hook" "k" (lambda (x) (record-hook-call x)))
(arg-hook 7)
(run-hook "arg-hook" 9)
"#;
        vm.eval_module_source(path, source, 1).expect("module eval");
        assert_eq!(*calls.borrow(), vec![7.0, 9.0]);
    }

    fn module_test_vm() -> VM {
        let mut vm = VM::new(Vec::new());
        super::register_core_natives(&mut vm);
        vm
    }

    fn temp_lisp_path(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "eseqlisp-modules-{tag}-{}.lisp",
            std::process::id()
        ))
    }

    #[test]
    fn module_form_switches_interning_namespace() {
        let mut vm = module_test_vm();
        let path = temp_lisp_path("switch");
        let source = r#"
(module test.mod)
(def foo () 42)
(foo)
"#;
        let result = vm.eval_module_source(path, source, 1).expect("module eval");
        assert_eq!(result, Some(Value::Number(42.0)));
        assert!(
            vm.global_names().iter().any(|n| n == "test.mod/foo"),
            "def inside (module test.mod) should intern qualified"
        );
        assert!(vm.declared_modules.contains_key("test.mod"));
        // A later headerless unit reaches it only qualified.
        let qualified = vm.eval_str("(test.mod/foo)").expect("qualified call");
        assert_eq!(qualified, Some(Value::Number(42.0)));
    }

    #[test]
    fn module_form_resets_per_compile_unit() {
        let mut vm = module_test_vm();
        vm.eval_module_source(temp_lisp_path("reset-a"), "(module test.reset)\n(def a () 1)", 1)
            .expect("module eval");
        // Headerless unit: defs go back to the implicit module.
        vm.eval_str("(def b () 2)").expect("headerless eval");
        assert!(vm.global_names().iter().any(|n| n == "test.reset/a"));
        assert!(vm.global_names().iter().any(|n| n == "eseq.vanilla/b"));
    }

    #[test]
    fn duplicate_module_form_is_a_compile_error() {
        let mut vm = module_test_vm();
        let result =
            vm.eval_str("(module test.dup)\n(module test.other)\n(def x () 1)");
        assert!(matches!(result, Err(super::VMError::CompileError)));
        assert!(
            vm.take_source_load_errors()
                .iter()
                .any(|e| e.contains("duplicate (module")),
            "expected duplicate-module error message"
        );
    }

    #[test]
    fn import_registers_alias_and_loads_once() {
        let mut vm = module_test_vm();
        let helper_path = temp_lisp_path("alias-helper");
        std::fs::write(
            &helper_path,
            "(module test.alias-helper)\n(def helper-val () 5)\n(def %secret () 6)",
        )
        .expect("write helper");
        let main_path = helper_path.with_file_name(format!(
            "eseqlisp-modules-alias-main-{}.lisp",
            std::process::id()
        ));
        let source = format!(
            "(import {} :as th)\n(th/helper-val)",
            "test.alias-helper"
        );
        // Candidate resolution is relative to the importing file, so give
        // the helper the name the module convention expects.
        let conventional = helper_path.with_file_name("test.alias-helper.lisp");
        std::fs::rename(&helper_path, &conventional).expect("rename helper");
        let result = vm
            .eval_module_source(main_path.clone(), &source, 1)
            .expect("import eval");
        assert_eq!(result, Some(Value::Number(5.0)));
        assert!(vm.declared_modules.contains_key("test.alias-helper"));
        // Load-once: importing again evaluates nothing new (same result).
        let again = vm
            .eval_module_source(main_path, &source, 2)
            .expect("second import eval");
        assert_eq!(again, Some(Value::Number(5.0)));
        let _ = std::fs::remove_file(conventional);
    }

    #[test]
    fn import_refer_binds_bare_symbols() {
        let mut vm = module_test_vm();
        let helper = std::env::temp_dir().join(format!(
            "test.refer-helper-{}.lisp",
            std::process::id()
        ));
        std::fs::write(&helper, format!("(module test.refer-helper-{})\n(def refer-val () 11)", std::process::id()))
            .expect("write helper");
        let main_path = helper.with_file_name(format!(
            "eseqlisp-modules-refer-main-{}.lisp",
            std::process::id()
        ));
        let source = format!(
            "(import test.refer-helper-{} :refer (refer-val))\n(refer-val)",
            std::process::id()
        );
        let result = vm
            .eval_module_source(main_path, &source, 1)
            .expect("refer eval");
        assert_eq!(result, Some(Value::Number(11.0)));
        let _ = std::fs::remove_file(helper);
    }

    #[test]
    fn unknown_alias_is_a_compile_error() {
        let mut vm = module_test_vm();
        let result = vm.eval_str("(zz9/nothing 1)");
        assert!(matches!(result, Err(super::VMError::CompileError)));
        assert!(
            vm.take_source_load_errors()
                .iter()
                .any(|e| e.contains("unknown alias or namespace 'zz9'")),
            "expected unknown-alias error message"
        );
    }

    #[test]
    fn private_reference_from_outside_warns_but_resolves() {
        let mut vm = module_test_vm();
        vm.eval_module_source(
            temp_lisp_path("privacy"),
            "(module test.privacy)\n(def %secret () 6)",
            1,
        )
        .expect("module eval");
        let result = vm.eval_str("(test.privacy/%secret)").expect("private call");
        assert_eq!(result, Some(Value::Number(6.0)));
        let diagnostics = vm.source_manager.diagnostics();
        assert!(
            diagnostics
                .iter()
                .any(|d| d.contains("%secret") && d.contains("internal")),
            "expected %-privacy warning, got {diagnostics:?}"
        );
    }

    #[test]
    fn macros_defined_in_module_are_namespace_aware() {
        let mut vm = module_test_vm();
        let result = vm
            .eval_module_source(
                temp_lisp_path("macros"),
                "(module test.mac)\n(defmacro twice (x) `(+ ,x ,x))\n(twice 3)",
                1,
            )
            .expect("module eval");
        // Bare lookup inside the defining module.
        assert_eq!(result, Some(Value::Number(6.0)));
        // Qualified lookup from a headerless unit.
        let qualified = vm.eval_str("(test.mac/twice 4)").expect("qualified macro");
        assert_eq!(qualified, Some(Value::Number(8.0)));
        assert!(vm.macros.contains_key("test.mac/twice"));
    }

    #[test]
    fn namespaced_natives_resolve_qualified_without_import() {
        let mut vm = module_test_vm();
        vm.register_native_in_namespace("sdf", "answer", |_args, _vm| Value::Number(41.0));
        let result = vm.eval_str("(sdf/answer)").expect("namespaced native");
        assert_eq!(result, Some(Value::Number(41.0)));
        // Blessed core namespace falls back to flat natives when qualified.
        vm.register_native("flat-answer", |_args| Value::Number(40.0));
        let core = vm
            .eval_str("(eseq.core/flat-answer)")
            .expect("core-qualified native");
        assert_eq!(core, Some(Value::Number(40.0)));
    }

    #[test]
    fn module_defstate_interns_qualified_and_reads_bare() {
        let mut vm = module_test_vm();
        let source = r#"
(module test.statemod)
(defstate counter 1)
(def bump () (set! counter (+ counter 1)))
(bump)
counter
"#;
        let result = vm
            .eval_module_source(temp_lisp_path("state-qual"), source, 1)
            .expect("module eval");
        assert_eq!(result, Some(Value::Number(2.0)));
        assert!(
            vm.state_bindings.contains_key("test.statemod/counter"),
            "declared-module defstate should key state_bindings qualified, got {:?}",
            vm.state_bindings.keys().collect::<Vec<_>>()
        );
        assert!(
            !vm.state_bindings.contains_key("counter"),
            "no flat key should be created for a declared-module defstate"
        );
        // A later unit reaches the state through its qualified name.
        let qualified = vm
            .eval_str("test.statemod/counter")
            .expect("qualified state read");
        assert_eq!(qualified, Some(Value::Number(2.0)));
    }

    #[test]
    fn vanilla_defstate_stays_flat_keyed() {
        let mut vm = module_test_vm();
        vm.eval_str("(defstate plain-state 7)").expect("defstate");
        assert!(
            vm.state_bindings.contains_key("plain-state"),
            "headerless defstate must keep today's flat key"
        );
        assert!(
            !vm.state_bindings.keys().any(|k| k.contains('/')),
            "no qualified state keys for vanilla code, got {:?}",
            vm.state_bindings.keys().collect::<Vec<_>>()
        );
        assert_eq!(
            vm.eval_str("plain-state").expect("state read"),
            Some(Value::Number(7.0))
        );
    }

    #[test]
    fn compat_alias_resolves_flat_name_to_qualified_home() {
        let mut vm = module_test_vm();
        let source = r#"
(module test.aliasmod)
(def fresh-home () 42)
(module-compat-alias legacy-alias-name fresh-home)
(legacy-alias-name)
"#;
        // Same-unit: the alias is visible to later forms in the declaring
        // unit (compiler-side table).
        let result = vm
            .eval_module_source(temp_lisp_path("compat-alias"), source, 1)
            .expect("module eval");
        assert_eq!(result, Some(Value::Number(42.0)));
        assert_eq!(
            vm.compat_aliases.get("legacy-alias-name").map(String::as_str),
            Some("test.aliasmod/fresh-home")
        );
        // Later headerless unit: flat call resolves through the alias.
        let later = vm.eval_str("(legacy-alias-name)").expect("aliased call");
        assert_eq!(later, Some(Value::Number(42.0)));
        // Runtime by-name ladder (host reads, string handlers) too.
        assert!(vm.has_global("legacy-alias-name"));
        assert!(matches!(
            vm.global_value("legacy-alias-name"),
            Some(Value::Closure { .. })
        ));
    }

    #[test]
    fn compat_alias_beats_stale_flat_slot() {
        let mut vm = module_test_vm();
        // A legacy flat def exists (as it would mid-session before a
        // conversion re-eval); the alias must still win.
        vm.eval_str("(def stale-legacy () 1)").expect("legacy def");
        let source = r#"
(module test.aliasmod2)
(def replacement () 2)
(module-compat-alias stale-legacy replacement)
"#;
        vm.eval_module_source(temp_lisp_path("compat-stale"), source, 1)
            .expect("module eval");
        let result = vm.eval_str("(stale-legacy)").expect("aliased call");
        assert_eq!(result, Some(Value::Number(2.0)));
    }

    #[test]
    fn compat_alias_requires_flat_old_and_qualified_target() {
        let mut vm = module_test_vm();
        let result = vm.eval_str("(module-compat-alias flat-old flat-new)");
        assert!(matches!(result, Err(super::VMError::CompileError)));
        assert!(
            vm.take_source_load_errors()
                .iter()
                .any(|e| e.contains("must be qualified")),
            "expected qualified-target error"
        );
    }

    #[test]
    fn compat_alias_survives_and_rolls_back_with_snapshots() {
        let mut vm = module_test_vm();
        vm.eval_module_source(
            temp_lisp_path("compat-snap"),
            "(module test.snapmod)\n(def kept () 1)\n(module-compat-alias kept-alias kept)",
            1,
        )
        .expect("module eval");
        let snapshot = vm.snapshot_state();
        vm.eval_module_source(
            temp_lisp_path("compat-snap2"),
            "(module test.snapmod2)\n(def dropped () 2)\n(module-compat-alias dropped-alias dropped)",
            2,
        )
        .expect("second module eval");
        assert!(vm.compat_aliases.contains_key("dropped-alias"));
        vm.restore_state(snapshot);
        assert!(
            vm.compat_aliases.contains_key("kept-alias"),
            "pre-snapshot alias must survive restore"
        );
        assert!(
            !vm.compat_aliases.contains_key("dropped-alias"),
            "post-snapshot alias must roll back"
        );
    }

    #[test]
    fn module_widget_key_qualifies_stable_key_prop() {
        let mut vm = module_test_vm();
        crate::widgets::register_widget_natives(&mut vm);
        let source = r#"
(module test.widgetmod)
(def make-panel () (v-stack :key "panel"))
"#;
        vm.eval_module_source(temp_lisp_path("widget-key"), source, 1)
            .expect("module eval");
        // Constructed from the declared module's chunk (even when called
        // from a headerless unit): the identity key is module-prefixed.
        let widget = vm
            .eval_str("(test.widgetmod/make-panel)")
            .expect("widget call")
            .expect("widget value");
        let Value::Map(map) = &widget else {
            panic!("expected widget map, got {widget:?}");
        };
        assert_eq!(
            map.get(STABLE_KEY_PROP).map(|v| v.borrow().clone()),
            Some(Value::String("test.widgetmod/panel".to_string())),
            "declared-module widget :key should qualify into __stable-key"
        );
        // The authored :key prop is untouched.
        assert_eq!(
            map.get("key").map(|v| v.borrow().clone()),
            Some(Value::String("panel".to_string()))
        );
        // Vanilla construction stays exactly as today: no __stable-key.
        let vanilla = vm
            .eval_str(r#"(v-stack :key "panel")"#)
            .expect("vanilla widget")
            .expect("widget value");
        let Value::Map(map) = &vanilla else {
            panic!("expected widget map");
        };
        assert!(
            !map.contains_key(STABLE_KEY_PROP),
            "vanilla widget keys must not gain __stable-key at construction"
        );
    }

    #[test]
    fn module_def_process_name_qualifies_and_constructor_resolves() {
        let mut vm = module_test_vm();
        let captured = Rc::new(RefCell::new(Vec::<String>::new()));
        let sink = captured.clone();
        // Stub of the sequencer's def-process native: record the class
        // name and register the constructor under it (the real one does
        // the same via register_process_constructor_native).
        vm.register_native_with_vm("def-process", move |args, vm| {
            let Some(Value::Symbol(name)) = args.first() else {
                panic!("def-process expects a symbol, got {:?}", args.first());
            };
            sink.borrow_mut().push(name.clone());
            vm.register_native_with_vm(name, |_args, _vm| Value::Number(7.0));
            Value::String(name.clone())
        });
        let source = r#"
(module test.procmod)
(def-process my-proc)
(my-proc)
"#;
        let result = vm
            .eval_module_source(temp_lisp_path("def-process"), source, 1)
            .expect("module eval");
        // The bare constructor call inside the module resolves to the
        // qualified registration.
        assert_eq!(result, Some(Value::Number(7.0)));
        vm.eval_str("(def-process plain-proc)").expect("vanilla def");
        assert_eq!(
            *captured.borrow(),
            vec!["test.procmod/my-proc".to_string(), "plain-proc".to_string()],
            "declared-module class names qualify; vanilla stays flat"
        );
    }

    #[test]
    fn chunk_module_provenance_reaches_natives() {
        let mut vm = module_test_vm();
        let seen = Rc::new(RefCell::new(Vec::<String>::new()));
        let sink = seen.clone();
        vm.register_native_with_vm("capture-module", move |_args, vm| {
            sink.borrow_mut().push(vm.current_module_name().to_string());
            Value::Nil
        });
        let source = r#"
(module test.chunkmod)
(capture-module)
(def late-capture () (capture-module))
"#;
        vm.eval_module_source(temp_lisp_path("chunk-module"), source, 1)
            .expect("module eval");
        // Late-bound: calling the module's function from a headerless unit
        // still reports the defining module (chunk provenance, not caller).
        vm.eval_str("(test.chunkmod/late-capture)")
            .expect("late call");
        vm.eval_str("(capture-module)").expect("vanilla call");
        assert_eq!(
            *seen.borrow(),
            vec![
                "test.chunkmod".to_string(),
                "test.chunkmod".to_string(),
                "eseq.vanilla".to_string()
            ]
        );
    }

    fn map_prop<'a>(value: &'a Value, key: &str) -> Option<std::cell::Ref<'a, Value>> {
        let Value::Map(map) = value else {
            return None;
        };
        Some(map.get(key)?.borrow())
    }

    fn first_child(value: &Value) -> Option<Value> {
        let children = map_prop(value, "children")?;
        let Value::List(children) = &*children else {
            return None;
        };
        children.first().map(|child| child.borrow().clone())
    }

    fn child_values(value: &Value) -> Vec<Value> {
        let Some(children) = map_prop(value, "children") else {
            return Vec::new();
        };
        let Value::List(children) = &*children else {
            return Vec::new();
        };
        children
            .iter()
            .map(|child| child.borrow().clone())
            .collect()
    }

    fn source_byte_prop(value: &Value, key: &str) -> usize {
        let Some(prop) = map_prop(value, key) else {
            panic!("missing {key}");
        };
        let Value::Number(number) = *prop else {
            panic!("{key} is not a number: {prop:?}");
        };
        number as usize
    }

    #[test]
    fn global_store_hooks_observe_def_bindings() {
        let mut vm = VM::new(Vec::new());
        let observed = Rc::new(RefCell::new(Vec::<(String, Value)>::new()));
        let observed_for_hook = Rc::clone(&observed);
        vm.add_global_store_hook(Rc::new(move |name, value| {
            observed_for_hook
                .borrow_mut()
                .push((name.to_string(), value.clone()));
        }));

        vm.eval_str("(def process-instance 42)")
            .expect("global def eval");

        assert_eq!(
            observed.borrow().as_slice(),
            &[("process-instance".to_string(), Value::Number(42.0))]
        );
    }

    #[test]
    fn host_handles_are_callable_lisp_values() {
        let mut vm = VM::new(Vec::new());
        let call_count = Rc::new(RefCell::new(0usize));
        let call_count_for_maker = Rc::clone(&call_count);
        vm.register_native("make-host-handle", move |_| {
            let call_count_for_handle = Rc::clone(&call_count_for_maker);
            Value::HostHandle {
                kind: "test".to_string(),
                id: 7,
                callable: Rc::new(move |args, _| {
                    *call_count_for_handle.borrow_mut() += 1;
                    match args.as_slice() {
                        [Value::Number(left), Value::Number(right)] => Value::Number(left + right),
                        _ => Value::Nil,
                    }
                }),
            }
        });

        let result = vm
            .eval_str("(def h (make-host-handle)) (h 2 5)")
            .expect("host handle eval");

        assert_eq!(result, Some(Value::Number(7.0)));
        assert_eq!(*call_count.borrow(), 1);
    }

    #[test]
    fn keyword_map_arguments_keep_normal_expression_evaluation() {
        let mut vm = VM::new(Vec::new());
        vm.register_native("map-argument-probe", |args| {
            args.get(1).cloned().unwrap_or(Value::Nil)
        });

        let result = vm
            .eval_str("(map-argument-probe :map (list 1 2))")
            .expect("evaluate :map expression argument");

        let Some(Value::List(items)) = result else {
            panic!("expected evaluated list argument, got {result:?}");
        };
        assert_eq!(items.len(), 2);
        assert_eq!(*items[0].borrow(), Value::Number(1.0));
        assert_eq!(*items[1].borrow(), Value::Number(2.0));
    }

    #[test]
    fn eval_str_grows_global_storage_for_large_programs() {
        let mut vm = VM::new(Vec::new());
        let mut source = String::new();
        for idx in 0..4105 {
            source.push_str(&format!("(def generated-global-{idx} {idx})\n"));
        }

        vm.eval_str(&source).expect("large global table eval");

        assert_eq!(
            vm.global_value("generated-global-4104"),
            Some(Value::Number(4104.0))
        );
    }

    #[test]
    fn wrong_arity_macro_call_returns_error_instead_of_recursing() {
        let mut vm = VM::new(Vec::new());
        vm.eval_str("(defmacro demo (x) `(+ ,x 1))")
            .expect("macro definition");

        assert!(vm.eval_str("(demo)").is_err());
        assert!(vm.eval_str("(demo 1 2)").is_err());
    }

    #[test]
    fn self_recursive_macro_returns_error_instead_of_recursing() {
        let mut vm = VM::new(Vec::new());
        vm.eval_str("(defmacro again (x) `(again ,x))")
            .expect("macro definition");

        assert!(vm.eval_str("(again 1)").is_err());
    }

    #[test]
    fn source_metadata_marks_emitted_widget_tree_with_source_buffer() {
        let mut vm = VM::new(Vec::new());
        crate::widgets::register_widget_natives(&mut vm);
        vm.set_current_effect_context(Some(42));

        vm.eval_str(r#"(effect (box :debug-name "root" (label "hello")))"#)
            .expect("effect eval");

        let Some(PendingUiUpdate::FullTree(pending)) = vm.pending_widget_trees.pop() else {
            panic!("expected emitted widget tree");
        };
        assert_eq!(pending.source_buffer_id, Some(42));
        assert_eq!(
            map_prop(&pending.tree, SOURCE_BUFFER_ID_PROP).as_deref(),
            Some(&Value::Number(42.0))
        );
        let child = first_child(&pending.tree).expect("child widget");
        assert_eq!(
            map_prop(&child, SOURCE_BUFFER_ID_PROP).as_deref(),
            Some(&Value::Number(42.0))
        );
    }

    #[test]
    fn runtime_targets_follow_executed_inline_call_sites_not_static_registration_order() {
        fn inline_widget(start_byte: usize) -> Value {
            let mut widget = crate::widgets::build_widget(
                "hslider",
                vec![
                    Value::Keyword("value".to_string()),
                    Value::Number(start_byte as f64),
                ],
            );
            let Value::Map(map) = &mut widget else {
                panic!("inline widget map");
            };
            for (key, value) in [
                (
                    SOURCE_REVISION_PROP,
                    Value::String("runtime-target-order".to_string()),
                ),
                (SOURCE_START_BYTE_PROP, Value::Number(start_byte as f64)),
                (SOURCE_END_BYTE_PROP, Value::Number((start_byte + 1) as f64)),
                (
                    super::INLINE_PARENT_CALLEE_PROP,
                    Value::String("process-class".to_string()),
                ),
                (
                    super::INLINE_PARENT_INLET_PROP,
                    Value::String("amount".to_string()),
                ),
            ] {
                map.insert(key.to_string(), Rc::new(RefCell::new(value)));
            }
            widget
        }

        let mut vm = VM::new(Vec::new());
        vm.set_current_effect_context(Some(42));
        vm.begin_inline_widget_capture();

        let first = inline_widget(10);
        let second = inline_widget(20);
        vm.registering_static_inline_widget = true;
        vm.register_inline_widget(first.deep_clone());
        vm.register_inline_widget(second.deep_clone());
        vm.registering_static_inline_widget = false;

        vm.register_inline_widget(first);
        assert!(vm.attach_inline_widget_runtime_target(
            "process-class",
            "amount",
            Value::String("first-handle".to_string()),
        ));
        vm.register_inline_widget(second);
        assert!(vm.attach_inline_widget_runtime_target(
            "process-class",
            "amount",
            Value::String("second-handle".to_string()),
        ));

        let widgets = vm.take_inline_widgets();
        let target_at = |start_byte| {
            widgets
                .iter()
                .find(|widget| source_byte_prop(widget, SOURCE_START_BYTE_PROP) == start_byte)
                .and_then(|widget| map_prop(widget, "__inline-runtime-target"))
                .map(|value| value.clone())
        };
        assert_eq!(
            target_at(10),
            Some(Value::String("first-handle".to_string()))
        );
        assert_eq!(
            target_at(20),
            Some(Value::String("second-handle".to_string()))
        );
    }

    #[test]
    fn source_metadata_marks_module_emitted_widget_tree_with_module_path() {
        let mut vm = VM::new(Vec::new());
        crate::widgets::register_widget_natives(&mut vm);
        let path = std::env::temp_dir().join(format!(
            "eseqlisp-source-metadata-{}.lisp",
            std::process::id()
        ));

        vm.eval_module_source(path.clone(), r#"(effect (label "hello"))"#, 1)
            .expect("module eval");

        let Some(PendingUiUpdate::FullTree(pending)) = vm.pending_widget_trees.pop() else {
            panic!("expected emitted widget tree");
        };
        assert_eq!(
            map_prop(&pending.tree, SOURCE_MODULE_PATH_PROP).as_deref(),
            Some(&Value::String(path.display().to_string()))
        );
    }

    #[test]
    fn source_metadata_marks_exact_widget_constructor_span() {
        let mut vm = VM::new(Vec::new());
        crate::widgets::register_widget_natives(&mut vm);
        let path = std::env::temp_dir().join(format!(
            "eseqlisp-source-span-direct-{}.lisp",
            std::process::id()
        ));
        let source = r#"(effect
  (box
    (knob-number :label "base")))"#;

        vm.eval_module_source(path, source, 11)
            .expect("module eval");

        let Some(PendingUiUpdate::FullTree(pending)) = vm.pending_widget_trees.pop() else {
            panic!("expected emitted widget tree");
        };
        let child = first_child(&pending.tree).expect("child widget");
        let expected_start = source.find("(knob-number").expect("widget form");
        let expected_end = source[expected_start..]
            .find("))")
            .map(|offset| expected_start + offset + 1)
            .expect("widget form end");
        assert_eq!(
            source_byte_prop(&child, SOURCE_START_BYTE_PROP),
            expected_start
        );
        assert_eq!(source_byte_prop(&child, SOURCE_END_BYTE_PROP), expected_end);
        assert_eq!(
            map_prop(&child, SOURCE_REVISION_PROP).as_deref(),
            Some(&Value::String("11".to_string()))
        );
    }

    #[test]
    fn source_metadata_does_not_treat_widget_named_let_bindings_as_widget_calls() {
        let mut vm = VM::new(Vec::new());

        vm.eval_str(
            r#"
            (def local-widget-names ()
              (let ((tabs (list 1 2))
                    (label (list 3 4)))
                (list tabs label)))
            "#,
        )
        .expect("widget-named bindings should compile as let bindings");
    }

    #[test]
    fn source_metadata_marks_helper_returned_widget_constructor_span() {
        let mut vm = VM::new(Vec::new());
        crate::widgets::register_widget_natives(&mut vm);
        let path = std::env::temp_dir().join(format!(
            "eseqlisp-source-span-helper-{}.lisp",
            std::process::id()
        ));
        let source = r#"(def sampler-param-knob ()
  (knob-number :label "base"))
(effect (sampler-param-knob))"#;

        vm.eval_module_source(path, source, 12)
            .expect("module eval");

        let Some(PendingUiUpdate::FullTree(pending)) = vm.pending_widget_trees.pop() else {
            panic!("expected emitted widget tree");
        };
        let expected_start = source.find("(knob-number").expect("widget form");
        assert_eq!(
            source_byte_prop(&pending.tree, SOURCE_START_BYTE_PROP),
            expected_start
        );
        assert_eq!(
            map_prop(&pending.tree, SOURCE_SYMBOL_PROP).as_deref(),
            Some(&Value::String("sampler-param-knob".to_string()))
        );
    }

    #[test]
    fn source_metadata_marks_each_items_with_template_widget_span() {
        let mut vm = VM::new(Vec::new());
        super::register_core_natives(&mut vm);
        crate::widgets::register_widget_natives(&mut vm);
        let path = std::env::temp_dir().join(format!(
            "eseqlisp-source-span-each-{}.lisp",
            std::process::id()
        ));
        let source = r#"(effect
  (h-stack
    (each (list 1 2) |x|
      (knob-number :label "base"))))"#;

        vm.eval_module_source(path, source, 13)
            .expect("module eval");

        let Some(PendingUiUpdate::FullTree(pending)) = vm.pending_widget_trees.pop() else {
            panic!("expected emitted widget tree");
        };
        let expected_start = source.find("(knob-number").expect("widget form");
        let children = child_values(&pending.tree);
        assert_eq!(children.len(), 2);
        for child in children {
            assert_eq!(
                source_byte_prop(&child, SOURCE_START_BYTE_PROP),
                expected_start
            );
            assert_eq!(
                map_prop(&child, SOURCE_REVISION_PROP).as_deref(),
                Some(&Value::String("13".to_string()))
            );
        }
    }

    #[test]
    fn source_metadata_marks_destructuring_zip_each_items_with_template_widget_span() {
        let mut vm = VM::new(Vec::new());
        super::register_core_natives(&mut vm);
        crate::widgets::register_widget_natives(&mut vm);
        let path = std::env::temp_dir().join(format!(
            "eseqlisp-source-span-zip-each-{}.lisp",
            std::process::id()
        ));
        let source = r#"(effect
  (h-stack
    (each (zip '(0 1) '(2 3)) |(enabled level)|
      (knob-number :label "zip" :value level))))"#;

        vm.eval_module_source(path, source, 14)
            .expect("module eval");

        let Some(PendingUiUpdate::FullTree(pending)) = vm.pending_widget_trees.pop() else {
            panic!("expected emitted widget tree");
        };
        let expected_start = source.find("(knob-number").expect("widget form");
        let children = child_values(&pending.tree);
        assert_eq!(children.len(), 2);
        for child in children {
            assert_eq!(
                source_byte_prop(&child, SOURCE_START_BYTE_PROP),
                expected_start
            );
            assert_eq!(
                map_prop(&child, SOURCE_REVISION_PROP).as_deref(),
                Some(&Value::String("14".to_string()))
            );
        }
    }

    #[test]
    fn source_metadata_marks_dynamic_sdf_widget_constructor_span() {
        let mut runtime = crate::runtime::Runtime::new();
        let source = r#"
            (defwidget sdf-source-test
              :width 3 :height 3
              :shader (sdf/layer
                        (sdf/fill (sdf/circle 0.7) :accent)))
            (sdf-source-test)
        "#;

        let value = runtime
            .eval_str(source)
            .expect("dynamic SDF widget eval")
            .expect("widget value");

        let expected_start = source.find("(sdf-source-test)").expect("widget call");
        assert_eq!(
            source_byte_prop(&value, SOURCE_START_BYTE_PROP),
            expected_start
        );
        assert_eq!(
            map_prop(&value, SOURCE_REVISION_PROP).as_deref(),
            Some(&Value::String(
                crate::hot_reload::hash_source(source).to_string()
            ))
        );
    }

    #[test]
    fn source_metadata_marks_macro_generated_widget_with_macro_callsite_span() {
        let mut vm = VM::new(Vec::new());
        crate::widgets::register_widget_natives(&mut vm);
        let path = std::env::temp_dir().join(format!(
            "eseqlisp-source-span-macro-generated-{}.lisp",
            std::process::id()
        ));
        let source = r#"(defmacro make-base ()
  `(knob-number :label "base"))
(effect (make-base))"#;

        vm.eval_module_source(path, source, 15)
            .expect("module eval");

        let Some(PendingUiUpdate::FullTree(pending)) = vm.pending_widget_trees.pop() else {
            panic!("expected emitted widget tree");
        };
        let expected_start = source.find("(make-base)").expect("macro call");
        assert_eq!(
            source_byte_prop(&pending.tree, SOURCE_START_BYTE_PROP),
            expected_start
        );
        assert_eq!(
            map_prop(&pending.tree, SOURCE_REVISION_PROP).as_deref(),
            Some(&Value::String("15".to_string()))
        );
    }

    #[test]
    fn source_metadata_preserves_unquoted_macro_arg_widget_span() {
        let mut vm = VM::new(Vec::new());
        crate::widgets::register_widget_natives(&mut vm);
        let path = std::env::temp_dir().join(format!(
            "eseqlisp-source-span-macro-arg-{}.lisp",
            std::process::id()
        ));
        let source = r#"(defmacro wrap-control (child)
  `(box ,child))
(effect
  (wrap-control
    (knob-number :label "base")))"#;

        vm.eval_module_source(path, source, 16)
            .expect("module eval");

        let Some(PendingUiUpdate::FullTree(pending)) = vm.pending_widget_trees.pop() else {
            panic!("expected emitted widget tree");
        };
        let call_start = source.find("(wrap-control").expect("macro call");
        let child_start = source.rfind("(knob-number").expect("widget arg");
        assert_eq!(
            source_byte_prop(&pending.tree, SOURCE_START_BYTE_PROP),
            call_start
        );
        let child = first_child(&pending.tree).expect("child widget");
        assert_eq!(
            source_byte_prop(&child, SOURCE_START_BYTE_PROP),
            child_start
        );
        assert_eq!(
            map_prop(&child, SOURCE_REVISION_PROP).as_deref(),
            Some(&Value::String("16".to_string()))
        );
    }

    #[test]
    fn source_metadata_marks_named_function_that_created_widget() {
        let mut vm = VM::new(Vec::new());
        crate::widgets::register_widget_natives(&mut vm);
        vm.set_current_effect_context(Some(42));

        vm.eval_str(
            r#"(def make-inspected-ui () (box (label "hello"))) (effect (make-inspected-ui))"#,
        )
        .expect("effect eval");

        let Some(PendingUiUpdate::FullTree(pending)) = vm.pending_widget_trees.pop() else {
            panic!("expected emitted widget tree");
        };
        assert_eq!(
            map_prop(&pending.tree, SOURCE_SYMBOL_PROP).as_deref(),
            Some(&Value::String("make-inspected-ui".to_string()))
        );
        let child = first_child(&pending.tree).expect("child widget");
        assert_eq!(
            map_prop(&child, SOURCE_SYMBOL_PROP).as_deref(),
            Some(&Value::String("make-inspected-ui".to_string()))
        );
    }

    #[test]
    fn source_metadata_preserves_cross_module_widget_function_origin() {
        let mut vm = VM::new(Vec::new());
        crate::widgets::register_widget_natives(&mut vm);
        let function_path = std::env::temp_dir().join(format!(
            "eseqlisp-source-function-{}.lisp",
            std::process::id()
        ));
        let effect_path = std::env::temp_dir().join(format!(
            "eseqlisp-source-effect-{}.lisp",
            std::process::id()
        ));

        vm.eval_module_source(
            function_path.clone(),
            r#"(def instrument-panel () (waveform :height 4.85))"#,
            1,
        )
        .expect("function module eval");
        vm.eval_module_source(
            effect_path,
            r#"(effect-buffer "*fx*" (instrument-panel))"#,
            1,
        )
        .expect("effect module eval");

        let Some(PendingUiUpdate::FullTree(pending)) = vm.pending_widget_trees.pop() else {
            panic!("expected emitted widget tree");
        };
        assert_eq!(
            map_prop(&pending.tree, SOURCE_MODULE_PATH_PROP).as_deref(),
            Some(&Value::String(function_path.display().to_string()))
        );
        assert_eq!(
            map_prop(&pending.tree, SOURCE_SYMBOL_PROP).as_deref(),
            Some(&Value::String("instrument-panel".to_string()))
        );
    }

    #[test]
    fn source_metadata_file_load_before_effect_buffer_keeps_later_defs_available() {
        let mut runtime = crate::runtime::Runtime::new();
        let root = std::env::temp_dir().join(format!(
            "eseqlisp-nested-load-effect-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).expect("create temp lisp dir");
        let dep_path = root.join("dep.lisp");
        let main_path = root.join("main.lisp");
        std::fs::write(&dep_path, r#"(def dep-ready () true)"#).expect("write dep source");
        let source = r#"(load "dep.lisp")
(def render-row (x)
  (label (str "row-" x)))
(effect-buffer "*nested-load-effect*"
  (v-stack
    (each (list 1 2) |x|
      (render-row x))))"#;

        let report = runtime.eval_source_transactional(Some(main_path), source, Vec::new());
        assert!(
            report.success,
            "nested load before effect-buffer failed: {}",
            report.failure_message()
        );
    }

    #[test]
    fn reactive_dag_indexes_source_nodes() {
        let mut dag = ReactiveDag::new();
        let source = ReactiveSource::NamespaceField {
            namespace: "SEQ".to_string(),
            field: "send-a".to_string(),
        };
        dag.add_node(ReactiveNode::Source {
            id: 7,
            source: source.clone(),
            value: Value::Number(0.0),
            dependents: HashSet::new(),
        });

        assert_eq!(dag.find_source_node(&source), Some(7));

        dag.add_node(ReactiveNode::Effect {
            id: 7,
            chunk_idx: 0,
            callable: None,
            source_buffer_id: None,
            source_file: None,
            source_revision: None,
            target: EffectTarget::BufferId(None),
            subtree_root_id: None,
            parent_subtree_root_id: None,
            stable_key: None,
            symbol_dependencies: HashSet::new(),
            dirty: false,
        });
        assert_eq!(dag.find_source_node(&source), None);

        dag.add_node(ReactiveNode::Source {
            id: 8,
            source: source.clone(),
            value: Value::Number(1.0),
            dependents: HashSet::new(),
        });
        assert_eq!(dag.find_source_node(&source), Some(8));
        dag.remove_node(8);
        assert_eq!(dag.find_source_node(&source), None);
    }

    fn number_list(items: &[f64]) -> Value {
        Value::List(
            items
                .iter()
                .map(|n| Rc::new(RefCell::new(Value::Number(*n))))
                .collect(),
        )
    }

    fn add_source(vm: &mut VM, value: Value) -> NodeId {
        let id = vm.dag.alloc_id();
        vm.dag.add_node(ReactiveNode::Source {
            id,
            source: ReactiveSource::LocalState {
                name: format!("test-source-{id}"),
            },
            value,
            dependents: HashSet::new(),
        });
        id
    }

    fn add_effect_dependent(vm: &mut VM) -> NodeId {
        let id = vm.dag.alloc_id();
        vm.dag.add_node(ReactiveNode::Effect {
            id,
            chunk_idx: 0,
            callable: None,
            source_buffer_id: None,
            source_file: None,
            source_revision: None,
            target: EffectTarget::BufferId(None),
            subtree_root_id: None,
            parent_subtree_root_id: None,
            stable_key: None,
            symbol_dependencies: HashSet::new(),
            dirty: false,
        });
        id
    }

    fn stored_source_list(vm: &VM, id: NodeId) -> Vec<Rc<RefCell<Value>>> {
        match vm.dag.nodes.get(&id) {
            Some(ReactiveNode::Source {
                value: Value::List(items),
                ..
            }) => items.clone(),
            _ => panic!("source {id} does not hold a list"),
        }
    }

    fn stored_source_value(vm: &VM, id: NodeId) -> Value {
        match vm.dag.nodes.get(&id) {
            Some(ReactiveNode::Source { value, .. }) => value.clone(),
            _ => panic!("missing source {id}"),
        }
    }

    #[test]
    fn list_source_single_index_write_patches_only_that_element() {
        let mut vm = VM::new(Vec::new());
        let source = add_source(&mut vm, number_list(&[1.0, 2.0, 3.0]));
        let reads_index_1 = add_effect_dependent(&mut vm);
        let reads_index_2 = add_effect_dependent(&mut vm);
        vm.dag.add_edge_indexed(source, reads_index_1, 1);
        vm.dag.add_edge_indexed(source, reads_index_2, 2);

        let before = stored_source_list(&vm, source);
        let new_value = number_list(&[1.0, 9.0, 3.0]);
        let Value::List(caller_items) = new_value.clone() else {
            unreachable!();
        };
        vm.mark_source_dependents_dirty(source, new_value);

        let after = stored_source_list(&vm, source);
        assert_eq!(
            stored_source_value(&vm, source),
            number_list(&[1.0, 9.0, 3.0])
        );
        // Unchanged elements keep their existing Rcs; only index 1 is replaced.
        assert!(Rc::ptr_eq(&before[0], &after[0]));
        assert!(!Rc::ptr_eq(&before[1], &after[1]));
        assert!(Rc::ptr_eq(&before[2], &after[2]));
        // The store never aliases the caller's value.
        assert!(!Rc::ptr_eq(&caller_items[1], &after[1]));
        // Dirty scope unchanged: only the index-1 reader is affected.
        assert!(vm.dag.is_dirty(reads_index_1));
        assert!(!vm.dag.is_dirty(reads_index_2));
    }

    #[test]
    fn list_source_length_grow_and_shrink_patch_the_stored_list() {
        let mut vm = VM::new(Vec::new());
        let source = add_source(&mut vm, number_list(&[1.0, 2.0]));
        let reads_len = add_effect_dependent(&mut vm);
        vm.dag
            .add_edge_indexed(source, reads_len, LEN_READ_SENTINEL);

        let before = stored_source_list(&vm, source);
        let grown = number_list(&[1.0, 2.0, 3.0, 4.0]);
        let Value::List(caller_items) = grown.clone() else {
            unreachable!();
        };
        vm.mark_source_dependents_dirty(source, grown);

        let after = stored_source_list(&vm, source);
        assert_eq!(
            stored_source_value(&vm, source),
            number_list(&[1.0, 2.0, 3.0, 4.0])
        );
        assert!(Rc::ptr_eq(&before[0], &after[0]));
        assert!(Rc::ptr_eq(&before[1], &after[1]));
        assert!(!Rc::ptr_eq(&caller_items[2], &after[2]));
        assert!(!Rc::ptr_eq(&caller_items[3], &after[3]));
        assert!(vm.dag.is_dirty(reads_len));

        vm.dag.clear_dirty(reads_len);
        vm.mark_source_dependents_dirty(source, number_list(&[1.0]));
        let shrunk = stored_source_list(&vm, source);
        assert_eq!(stored_source_value(&vm, source), number_list(&[1.0]));
        assert_eq!(shrunk.len(), 1);
        assert!(Rc::ptr_eq(&after[0], &shrunk[0]));
        assert!(vm.dag.is_dirty(reads_len));
    }

    #[test]
    fn shape_change_between_list_and_scalar_falls_back_to_full_clone() {
        let mut vm = VM::new(Vec::new());
        let source = add_source(&mut vm, number_list(&[1.0, 2.0]));
        let reads_index_0 = add_effect_dependent(&mut vm);
        vm.dag.add_edge_indexed(source, reads_index_0, 0);

        vm.mark_source_dependents_dirty(source, Value::Number(5.0));
        assert_eq!(stored_source_value(&vm, source), Value::Number(5.0));
        // Full change dirties even index-scoped readers.
        assert!(vm.dag.is_dirty(reads_index_0));

        vm.dag.clear_dirty(reads_index_0);
        let back_to_list = number_list(&[7.0, 8.0]);
        let Value::List(caller_items) = back_to_list.clone() else {
            unreachable!();
        };
        vm.mark_source_dependents_dirty(source, back_to_list);
        let after = stored_source_list(&vm, source);
        assert_eq!(stored_source_value(&vm, source), number_list(&[7.0, 8.0]));
        assert!(!Rc::ptr_eq(&caller_items[0], &after[0]));
        assert!(!Rc::ptr_eq(&caller_items[1], &after[1]));
        assert!(vm.dag.is_dirty(reads_index_0));
    }

    #[test]
    fn unchanged_write_marks_nothing_dirty_and_leaves_the_store_untouched() {
        let mut vm = VM::new(Vec::new());
        let source = add_source(&mut vm, number_list(&[1.0, 2.0]));
        let reads_index_0 = add_effect_dependent(&mut vm);
        let reads_all = add_effect_dependent(&mut vm);
        vm.dag.add_edge_indexed(source, reads_index_0, 0);
        vm.dag.add_edge(source, reads_all);

        let before = stored_source_list(&vm, source);
        vm.mark_source_dependents_dirty(source, number_list(&[1.0, 2.0]));

        let after = stored_source_list(&vm, source);
        assert!(Rc::ptr_eq(&before[0], &after[0]));
        assert!(Rc::ptr_eq(&before[1], &after[1]));
        assert!(!vm.dag.is_dirty(reads_index_0));
        assert!(!vm.dag.is_dirty(reads_all));
    }

    fn tree_with_label(label: &str) -> (Value, Rc<RefCell<Value>>) {
        let label_cell = Rc::new(RefCell::new(Value::String(label.to_string())));
        let child = {
            let mut map = HashMap::new();
            map.insert("label".to_string(), label_cell.clone());
            Rc::new(RefCell::new(Value::Map(map)))
        };
        let mut root = HashMap::new();
        root.insert(
            "children".to_string(),
            Rc::new(RefCell::new(Value::List(vec![child]))),
        );
        (Value::Map(root), label_cell)
    }

    #[test]
    #[should_panic(expected = "widget-tree freeze violation")]
    fn mutating_a_frozen_tree_cell_panics_in_debug() {
        let (tree, label_cell) = tree_with_label("frozen");
        freeze_widget_tree(&tree);
        debug_assert_cell_not_frozen(&label_cell, "test mutation");
    }

    #[test]
    fn deep_cloned_variant_of_a_frozen_tree_stays_mutable() {
        let (tree, _) = tree_with_label("frozen");
        freeze_widget_tree(&tree);
        let variant = tree.deep_clone();
        let Value::Map(map) = &variant else {
            unreachable!();
        };
        // Every cell of the deep clone is fresh, so mutating it is allowed.
        debug_assert_cell_not_frozen(&map["children"], "test mutation");
        let Value::List(children) = &*map["children"].borrow() else {
            unreachable!();
        };
        debug_assert_cell_not_frozen(&children[0], "test mutation");
    }

    #[test]
    fn shallow_clone_of_a_frozen_tree_shares_frozen_cells() {
        let (tree, label_cell) = tree_with_label("frozen");
        freeze_widget_tree(&tree);
        let shared = tree.clone();
        let Value::Map(map) = &shared else {
            unreachable!();
        };
        let Value::List(children) = &*map["children"].borrow() else {
            unreachable!();
        };
        let Value::Map(child) = &*children[0].borrow() else {
            unreachable!();
        };
        assert!(Rc::ptr_eq(&child["label"], &label_cell));
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            debug_assert_cell_not_frozen(&child["label"], "test mutation");
        }));
        assert!(result.is_err(), "shared frozen cell must still assert");
    }
}
