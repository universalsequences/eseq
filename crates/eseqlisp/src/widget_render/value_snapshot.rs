//! Exact, owned observations of Lisp paint data. Opaque and cyclic values
//! decline caching. The context reads each live slot once across all props.

use super::*;
use std::cell::RefCell;

#[derive(PartialEq)]
pub(super) enum ValueSnapshot {
    Number(u64),
    Reactive { namespace: String, field: String, index: Option<usize>, kind: crate::vm::BindingKind, bits: u64 },
    Bool(bool),
    Nil,
    String(String),
    Symbol(String),
    Keyword(String),
    List(Vec<ValueSnapshot>),
    Map(HashMap<String, ValueSnapshot>),
    // Derived interaction data must keep the current Lisp cells, even when
    // replacement cells happen to contain equal values. Paint only needs value
    // equality and leaves this optional observation disabled.
    Cell { identity: usize, value: Box<ValueSnapshot> },
}

#[derive(Default)]
pub(super) struct SnapshotContext {
    ancestors: Vec<*const RefCell<Value>>,
    pub(super) slots: HashMap<usize, u64>,
    pub(super) compare_cell_identity: bool,
}

impl ValueSnapshot {
    pub(super) fn capture(value: &Value, capture: &mut SnapshotContext) -> Option<Self> {
        Some(match value {
            Value::Number(value) => Self::Number(value.to_bits()),
            Value::Bool(value) => Self::Bool(*value),
            Value::Nil => Self::Nil,
            Value::String(value) => Self::String(value.clone()),
            Value::Symbol(value) => Self::Symbol(value.clone()),
            Value::Keyword(value) => Self::Keyword(value.clone()),
            Value::List(items) => Self::List(items.iter()
                .map(|cell| Self::capture_cell(cell, capture)).collect::<Option<_>>()?),
            Value::Map(items) => Self::Map(items.iter()
                .map(|(key, cell)| Some((key.clone(), Self::capture_cell(cell, capture)?)))
                .collect::<Option<_>>()?),
            Value::ReactiveRef { namespace, field, index, kind, slot } => {
                let bits = *capture.slots.entry(std::sync::Arc::as_ptr(slot) as usize)
                    .or_insert_with(|| slot.load(Ordering::Relaxed));
                Self::Reactive { namespace: namespace.clone(), field: field.clone(),
                    index: *index, kind: *kind, bits }
            }
            // Callables/host handles may reference mutable state we cannot
            // observe. Refuse input reuse instead of comparing an identity or
            // a hash that would hide mutations inside that state.
            _ => return None,
        })
    }

    fn capture_cell(cell: &Rc<RefCell<Value>>, capture: &mut SnapshotContext) -> Option<Self> {
        let identity = Rc::as_ptr(cell);
        if capture.ancestors.contains(&identity) { return None; }
        let value = cell.try_borrow().ok()?;
        capture.ancestors.push(identity);
        let snapshot = Self::capture(&value, capture);
        capture.ancestors.pop();
        snapshot.map(|value| if capture.compare_cell_identity {
            Self::Cell { identity: identity as usize, value: Box::new(value) }
        } else { value })
    }

    pub(super) fn to_value(&self) -> Value {
        match self {
            Self::Number(bits) => Value::Number(f64::from_bits(*bits)),
            Self::Reactive { namespace, field, index, kind, bits } => Value::ReactiveRef {
                namespace: namespace.clone(), field: field.clone(), index: *index, kind: *kind,
                slot: std::sync::Arc::new(AtomicU64::new(*bits)),
            },
            Self::Bool(value) => Value::Bool(*value),
            Self::Nil => Value::Nil,
            Self::String(value) => Value::String(value.clone()),
            Self::Symbol(value) => Value::Symbol(value.clone()),
            Self::Keyword(value) => Value::Keyword(value.clone()),
            Self::List(values) => Value::List(values.iter()
                .map(|value| Rc::new(RefCell::new(value.to_value()))).collect()),
            Self::Map(values) => Value::Map(values.iter()
                .map(|(key, value)| (key.clone(), Rc::new(RefCell::new(value.to_value())))).collect()),
            Self::Cell { value, .. } => value.to_value(),
        }
    }
    pub(super) fn matches(&self, value: &Value) -> bool {
        match (self, value) {
            (Self::Number(old), Value::Number(new)) => *old == new.to_bits(),
            (Self::Bool(old), Value::Bool(new)) => old == new,
            (Self::Nil, Value::Nil) => true,
            (Self::String(old), Value::String(new))
            | (Self::Symbol(old), Value::Symbol(new))
            | (Self::Keyword(old), Value::Keyword(new)) => old == new,
            (Self::List(old), Value::List(new)) => old.len() == new.len()
                && old.iter().zip(new).all(|(old, new)| old.matches_cell(new)),
            (Self::Map(old), Value::Map(new)) => old.len() == new.len()
                && old.iter().all(|(key, old)| new.get(key).is_some_and(|new|
                    old.matches_cell(new))),
            _ => false,
        }
    }

    fn matches_cell(&self, cell: &Rc<RefCell<Value>>) -> bool {
        let observed = match self {
            Self::Cell { identity, value } => {
                if *identity != Rc::as_ptr(cell) as usize { return false; }
                value.as_ref()
            }
            value => value,
        };
        cell.try_borrow().is_ok_and(|value| observed.matches(&value))
    }
}
