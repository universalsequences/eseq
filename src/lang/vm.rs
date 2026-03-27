use crate::compiler::{Chunk, Compiler, MacroDef, OpCode};
use crate::host::BufferId;
use crate::parser::{ASTParser, Parser};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReactiveSource {
    NamespaceField { namespace: String, field: String },
    LocalState { name: String },
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
    NativeFunction(NativeFn),
}

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
        source_buffer_id: Option<BufferId>,
        target: EffectTarget,
        dirty: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
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
}

pub struct ReactiveDag {
    pub nodes: HashMap<NodeId, ReactiveNode>,
    pub edges: HashMap<NodeId, HashSet<NodeId>>,
    pub next_id: NodeId,
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
    } else if bytes.get(pos) == Some(&b'0') && bytes.get(pos + 1).is_some_and(|b| b.is_ascii_digit()) {
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
    Some((total_len, FmtSpec { precision, width, align, fill }))
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
            (Self::Closure(a, _), Self::Closure(b, _)) => a == b,
            (Self::Function(a), Self::Function(b)) => a == b,
            (Self::NodeRef(a), Self::NodeRef(b)) => a == b,
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
            Self::NativeFunction(f) => Self::NativeFunction(f.clone()),
        }
    }
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
    pub pending_widget_trees: Vec<PendingWidgetTree>,
    pub dag: ReactiveDag,
    tracking_stack: Vec<NodeId>,
    pub reactive_namespaces: HashSet<String>,
    pub writable_reactive_namespaces: HashSet<String>,
    pub derived_bindings: HashMap<String, NodeId>,
    pub state_bindings: HashMap<String, NodeId>,
    execution_depth: usize,
    processing_reactive: bool,
    current_effect_source_buffer_id: Option<BufferId>,
    current_effect_target: EffectTarget,
    pub macros: HashMap<String, MacroDef>,
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

    // (get collection :key) → value, or nil if missing.
    // Works on both Maps and keyword-value lists like (:label "foo" :children (...)).
    vm.register_native("get", |args| {
        let Some(Value::Keyword(k)) = args.get(1) else {
            return Value::Nil;
        };
        match args.first() {
            Some(Value::Map(m)) => {
                m.get(k).map(|v| v.borrow().clone()).unwrap_or(Value::Nil)
            }
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
        Some(Value::String(s)) => Value::Number(s.len() as f64),
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
                .unwrap_or(Some(Value::Nil));
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
        for item in items {
            let mapped = vm
                .invoke(callback.clone(), vec![item.borrow().clone()])
                .unwrap_or(Some(Value::Nil))
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
        for item in items {
            let item_value = item.borrow().clone();
            let keep = vm
                .invoke(callback.clone(), vec![item_value.clone()])
                .unwrap_or(Some(Value::Nil))
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

        for item in items {
            acc = vm
                .invoke(callback.clone(), vec![acc, item.borrow().clone()])
                .unwrap_or(Some(Value::Nil))
                .unwrap_or(Value::Nil);
        }
        acc
    });

    vm.register_native_with_vm("for-each", |args, vm| {
        let (Some(callback), Some(Value::List(items))) = (args.first().cloned(), args.get(1))
        else {
            return Value::Nil;
        };

        for item in items {
            let _ = vm.invoke(callback.clone(), vec![item.borrow().clone()]);
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
            next_id: 0,
        }
    }

    pub fn alloc_id(&mut self) -> NodeId {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        id
    }

    pub fn add_node(&mut self, node: ReactiveNode) {
        let id = match &node {
            ReactiveNode::Source { id, .. }
            | ReactiveNode::Derived { id, .. }
            | ReactiveNode::Effect { id, .. } => *id,
        };
        self.nodes.insert(id, node);
    }

    pub fn remove_node(&mut self, id: NodeId) {
        self.nodes.remove(&id);
        self.edges.remove(&id);
        for dependents in self.edges.values_mut() {
            dependents.remove(&id);
        }
        for node in self.nodes.values_mut() {
            match node {
                ReactiveNode::Source { dependents, .. }
                | ReactiveNode::Derived { dependents, .. } => {
                    dependents.remove(&id);
                }
                ReactiveNode::Effect { .. } => {}
            }
        }
    }

    pub fn mark_dirty(&mut self, id: NodeId) {
        if let Some(node) = self.nodes.get_mut(&id) {
            match node {
                ReactiveNode::Derived { dirty, .. } | ReactiveNode::Effect { dirty, .. } => {
                    *dirty = true;
                }
                ReactiveNode::Source { .. } => {}
            }
        }
    }

    pub fn topo_sort_dirty(&self) -> Vec<NodeId> {
        let dirty = self
            .nodes
            .iter()
            .filter_map(|(id, node)| match node {
                ReactiveNode::Derived { dirty, .. } | ReactiveNode::Effect { dirty, .. }
                    if *dirty =>
                {
                    Some(*id)
                }
                _ => None,
            })
            .collect::<HashSet<_>>();

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
        for (dependency, dependents) in self.edges.iter_mut() {
            if dependents.remove(&id)
                && let Some(node) = self.nodes.get_mut(dependency)
            {
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
        self.edges.entry(dependency).or_default().insert(dependent);
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

    pub fn find_source_node(&self, source: &ReactiveSource) -> Option<NodeId> {
        self.nodes.iter().find_map(|(id, node)| match node {
            ReactiveNode::Source {
                source: current, ..
            } if current == source => Some(*id),
            _ => None,
        })
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
}

impl VM {
    pub fn new(chunks: Vec<Chunk>) -> Self {
        VM {
            chunks,
            current_chunk: 0,
            globals: vec![None; 512],
            global_names: vec![],
            pending_widget_trees: Vec::new(),
            dag: ReactiveDag::new(),
            tracking_stack: Vec::new(),
            reactive_namespaces: HashSet::new(),
            writable_reactive_namespaces: HashSet::new(),
            derived_bindings: HashMap::new(),
            state_bindings: HashMap::new(),
            execution_depth: 0,
            processing_reactive: false,
            current_effect_source_buffer_id: None,
            current_effect_target: EffectTarget::BufferId(None),
            macros: HashMap::new(),
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
            .retain(|pending| pending.source_buffer_id != owner_buffer_id);
    }

    pub fn set_current_effect_context(&mut self, source_buffer_id: Option<BufferId>) {
        self.current_effect_source_buffer_id = source_buffer_id;
        self.current_effect_target = EffectTarget::BufferId(source_buffer_id);
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

    fn current_reactive_value(&self, namespace: &str, field: &str) -> Value {
        self.global_value(namespace)
            .and_then(|value| match value {
                Value::Map(map) => map.get(field).map(|value| value.borrow().clone()),
                _ => None,
            })
            .unwrap_or(Value::Nil)
    }

    fn get_or_create_source_node(&mut self, namespace: &str, field: &str) -> NodeId {
        let source = ReactiveSource::NamespaceField {
            namespace: namespace.to_string(),
            field: field.to_string(),
        };
        if let Some(id) = self.dag.find_source_node(&source) {
            return id;
        }

        let id = self.dag.alloc_id();
        let value = self.current_reactive_value(namespace, field);
        self.dag.add_node(ReactiveNode::Source {
            id,
            source,
            value,
            dependents: HashSet::new(),
        });
        id
    }

    fn get_or_create_local_state_node(
        &mut self,
        node_id: NodeId,
        name: &str,
        initial: Value,
    ) -> NodeId {
        let source = ReactiveSource::LocalState {
            name: name.to_string(),
        };
        if let Some(id) = self.dag.find_source_node(&source) {
            self.mark_source_dependents_dirty(id, initial);
            return id;
        }

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
            *current_value = value;
            let dependents = dependents.clone().into_iter().collect::<Vec<_>>();
            for dependent in dependents {
                self.dag.mark_dirty(dependent);
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
        let result = (|| -> Result<(), VMError> {
            loop {
                let sorted = self.dag.topo_sort_dirty();
                if sorted.is_empty() {
                    break;
                }

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
                    if let Some(ReactiveNode::Effect {
                        source_buffer_id,
                        target,
                        ..
                    }) = self.dag.nodes.get(&node_id)
                    {
                        self.current_effect_source_buffer_id = *source_buffer_id;
                        self.current_effect_target = target.clone();
                    }
                    self.execute_from(chunk_idx)?;
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
                            match *val.borrow() {
                                Value::Number(val) => sum += val,
                                _ => return Err(VMError::IncorrectType),
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
                            match *val.borrow() {
                                Value::Number(val) => nums.push(val),
                                _ => return Err(VMError::IncorrectType),
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
                            match *val.borrow() {
                                Value::Number(val) => product *= val,
                                _ => return Err(VMError::IncorrectType),
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
                            match *val.borrow() {
                                Value::Number(val) => nums.push(val),
                                _ => return Err(VMError::IncorrectType),
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
                            match *val.borrow() {
                                Value::Number(val) => current = current.min(val),
                                _ => return Err(VMError::IncorrectType),
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
                            match *val.borrow() {
                                Value::Number(val) => current = current.max(val),
                                _ => return Err(VMError::IncorrectType),
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
                            _ => return Err(VMError::IncorrectType),
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
                        self.globals[idx] = stack.pop();
                        frame.pc += 1;
                    }
                }
                OpCode::LoadGlobal(idx) => {
                    if let Some(frame) = frames.last_mut() {
                        if let Some(Some(val)) = self.globals.get(idx) {
                            stack.push(Rc::clone(val));
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
                    if !self.dag.nodes.contains_key(&node_id) {
                        self.dag.add_node(ReactiveNode::Effect {
                            id: node_id,
                            chunk_idx,
                            source_buffer_id: self.current_effect_source_buffer_id,
                            target: self.current_effect_target.clone(),
                            dirty: false,
                        });
                    }

                    let current_chunk = self.current_chunk;
                    let result = self.execute_from(chunk_idx)?;
                    self.current_chunk = current_chunk;
                    let _ = result;
                    stack.push(Rc::new(RefCell::new(Value::Nil)));
                    frames.last_mut().unwrap().pc += 1;
                }
                OpCode::InitNamedEffect(node_id, chunk_idx, name_idx) => {
                    let target_name = self.chunks[self.current_chunk].strings[name_idx].clone();
                    if !self.dag.nodes.contains_key(&node_id) {
                        self.dag.add_node(ReactiveNode::Effect {
                            id: node_id,
                            chunk_idx,
                            source_buffer_id: self.current_effect_source_buffer_id,
                            target: EffectTarget::BufferName(target_name.clone()),
                            dirty: false,
                        });
                    }

                    let current_chunk = self.current_chunk;
                    let previous_target = self.current_effect_target.clone();
                    self.current_effect_target = EffectTarget::BufferName(target_name);
                    let result = self.execute_from(chunk_idx)?;
                    self.current_chunk = current_chunk;
                    self.current_effect_target = previous_target;
                    let _ = result;
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

                    if let Some(ReactiveNode::Derived {
                        value,
                        dependents,
                        dirty,
                        ..
                    }) = self.dag.nodes.get_mut(&node_id)
                    {
                        let changed = *value != new_value;
                        *value = new_value.clone();
                        *dirty = false;
                        if changed {
                            changed_dependents = dependents.iter().copied().collect();
                        }
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
                    if let Some(ReactiveNode::Effect { dirty, .. }) =
                        self.dag.nodes.get_mut(&node_id)
                    {
                        *dirty = false;
                    }
                    frames.last_mut().unwrap().pc += 1;
                }
                OpCode::LoadReactive(ns_idx, field_idx) => {
                    let namespace = self.chunks[self.current_chunk].strings[ns_idx].clone();
                    let field = self.chunks[self.current_chunk].strings[field_idx].clone();
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
                        self.pending_widget_trees.push(PendingWidgetTree {
                            source_buffer_id: self.current_effect_source_buffer_id,
                            target: self.current_effect_target.clone(),
                            tree: tree.borrow().clone(),
                        });
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
