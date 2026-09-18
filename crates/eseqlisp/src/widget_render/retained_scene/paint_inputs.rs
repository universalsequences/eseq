//! Immutable observations of paint props. Cloning `Value` is insufficient:
//! lists/maps share mutable cells. Live atomic slots cannot be snapshotted
//! independently of the painter's reads, so reactive props decline input reuse.

use super::*;
use std::cell::RefCell;

pub(super) struct PaintInputs {
    focusable: bool,
    props: HashMap<String, PaintValue>,
}

impl PaintInputs {
    pub(super) fn capture(layout: &LayoutNode) -> Option<Self> {
        // This is the same explicit painter contract as the primitive cache:
        // own props, rect, viewport, theme and tracked widget state. The box's
        // descendant geometry is checked by RetainedScene::same_topology.
        // Unknown painters still paint when dirty; output equality can retain
        // their geometry without making assumptions about their inputs.
        if !cacheable_widget_primitives(&layout.widget_type) { return None; }
        let mut ancestors = Vec::new();
        let props = layout.props.iter().filter(|(key, _)| paint_prop(key))
            .map(|(key, value)| Some((key.clone(), PaintValue::capture(value, &mut ancestors)?)))
            .collect::<Option<_>>()?;
        Some(Self { focusable: layout.focusable, props })
    }

    pub(super) fn matches(&self, layout: &LayoutNode) -> bool {
        self.focusable == layout.focusable
            && self.props.len() == layout.props.keys().filter(|key| paint_prop(key)).count()
            && self.props.iter().all(|(key, old)|
                layout.props.get(key).is_some_and(|value| old.matches(value)))
    }
}

fn paint_prop(key: &str) -> bool {
    // These callbacks are consumed by event dispatch, never by the painters
    // opted in above. Do not ignore arbitrary `on-*` props or opaque values:
    // a custom paint prop must remain observable even if it looks like one.
    !is_internal_source_prop(key) && !matches!(key,
        "on-click" | "on-right-click" | "on-double-click" | "on-change"
        | "on-press" | "on-release" | "on-drag" | "on-drop"
        | "on-mouse-down" | "on-mouse-up")
}

enum PaintValue {
    Number(u64),
    Bool(bool),
    Nil,
    String(String),
    Symbol(String),
    Keyword(String),
    List(Vec<PaintValue>),
    Map(HashMap<String, PaintValue>),
}

impl PaintValue {
    fn capture(value: &Value, ancestors: &mut Vec<*const RefCell<Value>>) -> Option<Self> {
        Some(match value {
            Value::Number(value) => Self::Number(value.to_bits()),
            Value::Bool(value) => Self::Bool(*value),
            Value::Nil => Self::Nil,
            Value::String(value) => Self::String(value.clone()),
            Value::Symbol(value) => Self::Symbol(value.clone()),
            Value::Keyword(value) => Self::Keyword(value.clone()),
            Value::List(items) => Self::List(items.iter()
                .map(|cell| Self::capture_cell(cell, ancestors)).collect::<Option<_>>()?),
            Value::Map(items) => Self::Map(items.iter()
                .map(|(key, cell)| Some((key.clone(), Self::capture_cell(cell, ancestors)?)))
                .collect::<Option<_>>()?),
            // The audio/scheduler thread can change a slot during painting.
            // Even equal before/after reads cannot rule out an intermediate
            // value that the painter observed. Only output equality is a safe
            // reuse proof until painters consume a coherent frozen snapshot.
            Value::ReactiveRef { .. } => return None,
            // Callables/host handles may reference mutable state we cannot
            // observe. Refuse input reuse instead of comparing an identity or
            // a hash that would hide mutations inside that state.
            _ => return None,
        })
    }

    fn capture_cell(cell: &Rc<RefCell<Value>>, ancestors: &mut Vec<*const RefCell<Value>>) -> Option<Self> {
        let identity = Rc::as_ptr(cell);
        if ancestors.contains(&identity) { return None; }
        let value = cell.try_borrow().ok()?;
        ancestors.push(identity);
        let snapshot = Self::capture(&value, ancestors);
        ancestors.pop();
        snapshot
    }

    fn matches(&self, value: &Value) -> bool {
        match (self, value) {
            (Self::Number(old), Value::Number(new)) => *old == new.to_bits(),
            (Self::Bool(old), Value::Bool(new)) => old == new,
            (Self::Nil, Value::Nil) => true,
            (Self::String(old), Value::String(new))
            | (Self::Symbol(old), Value::Symbol(new))
            | (Self::Keyword(old), Value::Keyword(new)) => old == new,
            (Self::List(old), Value::List(new)) => old.len() == new.len()
                && old.iter().zip(new).all(|(old, new)|
                    new.try_borrow().is_ok_and(|new| old.matches(&new))),
            (Self::Map(old), Value::Map(new)) => old.len() == new.len()
                && old.iter().all(|(key, old)| new.get(key).is_some_and(|new|
                    new.try_borrow().is_ok_and(|new| old.matches(&new)))),
            _ => false,
        }
    }
}
