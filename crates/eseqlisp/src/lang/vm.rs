use crate::compiler::{Chunk, Compiler, MacroDef, OpCode};
use crate::host::BufferId;
use crate::hot_reload::{SourceManager, extract_defined_symbols_from_source};
use crate::parser::{ASTParser, Parser};
use std::cell::RefCell;
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static RAND_STATE: AtomicU64 = AtomicU64::new(0x9e37_79b9_7f4a_7c15);

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
pub type NodeId = u32;

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
        source_module: Option<std::path::PathBuf>,
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
    pub target: EffectTarget,
    pub tree: Value,
    pub reactive_dependencies: Vec<ReactiveFieldKey>,
}

#[derive(Clone)]
pub enum PendingUiUpdate {
    FullTree(PendingWidgetTree),
    ReplaceSubtree {
        source_buffer_id: Option<BufferId>,
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
        }
    }
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
    widget_type: &str,
    path: &[usize],
    key: Option<&str>,
) -> u64 {
    const MAX_SAFE_F64_INT: u64 = (1u64 << 53) - 1;
    let mut hasher = DefaultHasher::new();
    source_buffer_id.hash(&mut hasher);
    target.hash(&mut hasher);
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
    let stable_id =
        stable_widget_hash(source_buffer_id, target, &widget_type, path, key.as_deref());
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
    pub dag: ReactiveDag,
    tracking_stack: Vec<NodeId>,
    pub reactive_namespaces: HashSet<String>,
    pub writable_reactive_namespaces: HashSet<String>,
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
}

pub struct VmStateSnapshot {
    chunks: Vec<Chunk>,
    current_chunk: usize,
    globals: Vec<Option<Rc<RefCell<Value>>>>,
    global_names: Vec<String>,
    pending_widget_trees: Vec<PendingUiUpdate>,
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
            Value::Number(number) => crate::reactive::write_float_slot(namespace, field, *number),
            Value::Bool(true) => crate::reactive::write_float_slot(namespace, field, 1.0),
            Value::Bool(false) => crate::reactive::write_float_slot(namespace, field, 0.0),
            _ => {}
        }
        vm.update_reactive_global(namespace, field, value.clone());
        vm.pending_reactive_sets
            .push((namespace.clone(), field.clone(), value.clone()));
        let source_id = vm.get_or_create_source_node(namespace, field);
        vm.mark_source_dependents_dirty(source_id, value);
        Value::Bool(true)
    });

    vm.register_native("bind", |args| {
        let (Some(Value::String(namespace)), Some(Value::String(field))) =
            (args.first(), args.get(1))
        else {
            return Value::Nil;
        };
        reactive_float_ref(namespace, field)
    });

    vm.register_native("bind-seq", |args| {
        let Some(Value::String(field)) = args.first() else {
            return Value::Nil;
        };
        reactive_float_ref("SEQ", field)
    });

    vm.register_native("reactive-value", |args| {
        let Some(value) = args.first() else {
            return Value::Nil;
        };
        match value {
            Value::ReactiveRef {
                kind: BindingKind::Float,
                slot,
                ..
            } => Value::Number(crate::reactive::read_float_slot(slot)),
            other => other.clone(),
        }
    });

    vm.register_native("bind-nth", |args| {
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
        reactive_indexed_float_ref(namespace, field, index)
    });

    vm.register_native("bind-seq-nth", |args| {
        let (Some(Value::String(field)), Some(Value::Number(index))) = (args.first(), args.get(1))
        else {
            return Value::Nil;
        };
        let Some(index) = binding_index(*index) else {
            return Value::Nil;
        };
        reactive_indexed_float_ref("SEQ", field, index)
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

fn reactive_float_ref(namespace: &str, field: &str) -> Value {
    Value::ReactiveRef {
        namespace: namespace.to_string(),
        field: field.to_string(),
        index: None,
        kind: BindingKind::Float,
        slot: crate::reactive::reactive_float_slot(namespace, field),
    }
}

fn reactive_indexed_float_ref(namespace: &str, field: &str, index: usize) -> Value {
    Value::ReactiveRef {
        namespace: namespace.to_string(),
        field: field.to_string(),
        index: Some(index),
        kind: BindingKind::Float,
        slot: crate::reactive::reactive_indexed_float_slot(namespace, field, index),
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
        VM {
            chunks,
            current_chunk: 0,
            globals: vec![None; 4096],
            global_names: vec![],
            pending_widget_trees: Vec::new(),
            dag: ReactiveDag::new(),
            tracking_stack: Vec::new(),
            reactive_namespaces: HashSet::new(),
            writable_reactive_namespaces: HashSet::new(),
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
        }
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
        let idx = self.ensure_global(name);
        self.globals[idx] = Some(Rc::new(RefCell::new(Value::NativeFunction(Rc::new(f)))));
    }

    /// Compile and run `code` in this VM's existing context (globals persist).
    pub fn eval_str(&mut self, code: &str) -> Result<Option<Value>, VMError> {
        let tokens = Parser::new(code.to_string())
            .parse()
            .map_err(|_| VMError::ParseError)?;
        let exprs = ASTParser::new(tokens)
            .parse()
            .map_err(|_| VMError::ParseError)?;

        let entry_idx = self.chunks.len();
        let existing = self.chunks.clone();
        let names = self.global_names.clone();
        let reactive_namespaces = self.reactive_namespaces.clone();
        let derived_bindings = self.derived_bindings.clone();
        let state_bindings = self.state_bindings.clone();
        let next_node_id = self.dag.next_id;

        let macros = self.macros.clone();
        let mut compiler = Compiler::new_repl(
            exprs,
            existing,
            names,
            reactive_namespaces,
            derived_bindings,
            state_bindings,
            next_node_id,
            macros,
        );
        match compiler.compile() {
            Ok(chunks) => {
                self.chunks = chunks;
                self.global_names = compiler.global_names();
                self.derived_bindings = compiler.derived_bindings();
                self.state_bindings = compiler.state_bindings();
                self.dag.next_id = compiler.next_node_id();
                // Merge any new macro definitions back into the VM
                for (name, def) in compiler.macros() {
                    self.macros.insert(name.clone(), def.clone());
                }
                self.execute_from(entry_idx)
            }
            Err(_) => Err(VMError::CompileError),
        }
    }

    pub fn eval_module_source(
        &mut self,
        path: std::path::PathBuf,
        source: &str,
        revision: u64,
    ) -> Result<Option<Value>, VMError> {
        let defined_symbols = extract_defined_symbols_from_source(source).map_err(|error| {
            self.source_load_errors
                .push(format!("{}: {error}", path.display()));
            VMError::ParseError
        })?;
        self.clear_effects_for_module(&path);
        self.source_manager.enter_module(path.clone());
        let result = self.eval_str(source);
        self.source_manager.leave_module();
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
        }
    }

    pub fn restore_state(&mut self, snapshot: VmStateSnapshot) {
        self.chunks = snapshot.chunks;
        self.current_chunk = snapshot.current_chunk;
        self.globals = snapshot.globals;
        self.global_names = snapshot.global_names;
        self.pending_widget_trees = snapshot.pending_widget_trees;
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
            .parse()
            .map_err(|_| VMError::ParseError)?;
        profile.parse = parse_started.elapsed();

        let ast_started = std::time::Instant::now();
        let exprs = ASTParser::new(tokens)
            .parse()
            .map_err(|_| VMError::ParseError)?;
        profile.ast = ast_started.elapsed();

        let entry_idx = self.chunks.len();
        let existing = self.chunks.clone();
        let names = self.global_names.clone();
        let reactive_namespaces = self.reactive_namespaces.clone();
        let derived_bindings = self.derived_bindings.clone();
        let state_bindings = self.state_bindings.clone();
        let next_node_id = self.dag.next_id;

        let macros = self.macros.clone();
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
        );
        let chunks = compiler.compile().map_err(|_| VMError::CompileError)?;
        self.chunks = chunks;
        self.global_names = compiler.global_names();
        self.derived_bindings = compiler.derived_bindings();
        self.state_bindings = compiler.state_bindings();
        self.dag.next_id = compiler.next_node_id();
        for (name, def) in compiler.macros() {
            self.macros.insert(name.clone(), def.clone());
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

    pub fn has_global(&self, name: &str) -> bool {
        self.global_names.iter().any(|n| n == name)
    }

    pub fn set_global_value(&mut self, name: &str, value: Value) {
        let idx = self.ensure_global(name);
        if idx >= self.globals.len() {
            self.globals.resize(idx + 1, None);
        }
        self.globals[idx] = Some(Rc::new(RefCell::new(value)));
    }

    pub fn global_names(&self) -> &[String] {
        &self.global_names
    }

    pub fn global_value(&self, name: &str) -> Option<Value> {
        let idx = self.global_names.iter().position(|global| global == name)?;
        self.globals
            .get(idx)
            .and_then(|value| value.as_ref())
            .map(|value| value.borrow().clone())
    }

    pub fn read_tracked_state_value(&mut self, name: &str) -> Option<Value> {
        let node_id = self.state_bindings.get(name).copied()?;
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
                    source_module: Some(source_module),
                    ..
                } if source_module == module => Some(*id),
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
                        source_module: Some(source_module),
                        ..
                    } if source_module == module
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
        let source_module = self.source_manager.current_module();
        match self.dag.nodes.get_mut(&node_id) {
            Some(ReactiveNode::Effect {
                chunk_idx: current_chunk_idx,
                callable,
                source_buffer_id: current_source_buffer_id,
                source_module: current_source_module,
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
                *current_source_module = source_module;
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
                    source_module,
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
                    source_module,
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
            source_module: self.source_manager.current_module(),
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
                if !self.processing_reactive {
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
                let mut borrowed = existing.borrow_mut();
                if let Value::Map(map) = &mut *borrowed {
                    if let Some(slot) = map.get(field) {
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
            let mut borrowed = slot.borrow_mut();
            match &mut *borrowed {
                Value::List(items) => {
                    while items.len() <= index {
                        items.push(Rc::new(RefCell::new(Value::Nil)));
                    }
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
            *current_value = value.deep_clone();
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
        let parts = owner_path.splitn(2, '.').collect::<Vec<_>>();
        if parts.len() == 1 {
            if let Some(node_id) = self.state_bindings.get(parts[0]).copied() {
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
                        self.current_effect_target.clone(),
                    );
                    if let Some((source_buffer_id, target, subtree_root_id)) =
                        self.dag.nodes.get(&node_id).and_then(|node| match node {
                            ReactiveNode::Effect {
                                source_buffer_id,
                                target,
                                subtree_root_id,
                                ..
                            } => Some((*source_buffer_id, target.clone(), *subtree_root_id)),
                            _ => None,
                        })
                    {
                        self.current_effect_source_buffer_id = source_buffer_id;
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
                                &self.current_effect_target,
                                None,
                                &mut path,
                            );
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
                            self.current_effect_target = previous_owner.1;
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
                    self.current_effect_target = previous_owner.1;
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
        let name = self
            .global_names
            .get(idx)
            .cloned()
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
                    match &mut *target.borrow_mut() {
                        Value::Map(map) => {
                            if let Some(slot) = map.get(&field) {
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
                        self.globals[idx] = stack.pop();
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
                            &self.current_effect_target,
                            None,
                            &mut path,
                        );
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
                                    target: self.current_effect_target.clone(),
                                    subtree_root_id,
                                    tree: annotated_tree,
                                    reactive_dependencies,
                                });
                        } else {
                            self.pending_widget_trees.push(PendingUiUpdate::FullTree(
                                PendingWidgetTree {
                                    source_buffer_id: self.current_effect_source_buffer_id,
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
    use std::collections::HashSet;

    use super::{EffectTarget, ReactiveDag, ReactiveNode, ReactiveSource, VM, Value};

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
            source_module: None,
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
}
