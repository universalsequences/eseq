use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::layout::LayoutNode;
use crate::vm::ReactiveBindingKey;
use crate::vm::Value;

#[derive(Clone, Default)]
pub struct ReactiveBindingStore {
    slots: Arc<Mutex<HashMap<ReactiveBindingKey, Arc<AtomicU64>>>>,
}

fn numeric_value(value: &Value) -> Option<f64> {
    match value {
        Value::Number(n) => Some(*n),
        Value::Bool(true) => Some(1.0),
        Value::Bool(false) => Some(0.0),
        _ => None,
    }
}

pub fn read_float_slot(slot: &AtomicU64) -> f64 {
    f64::from_bits(slot.load(Ordering::Relaxed))
}

fn store_float_slot(slot: &AtomicU64, value: f64) {
    slot.store(value.to_bits(), Ordering::Relaxed);
}

impl ReactiveBindingStore {
    pub(crate) fn slot(&self, namespace: &str, field: &str) -> Arc<AtomicU64> {
        self.slot_for_key(ReactiveBindingKey::field(namespace, field))
    }

    pub(crate) fn indexed_slot(
        &self,
        namespace: &str,
        field: &str,
        index: usize,
    ) -> Arc<AtomicU64> {
        self.slot_for_key(ReactiveBindingKey::indexed(namespace, field, index))
    }

    fn slot_for_key(&self, key: ReactiveBindingKey) -> Arc<AtomicU64> {
        let mut slots = self
            .slots
            .lock()
            .expect("reactive float store lock poisoned");
        slots
            .entry(key)
            .or_insert_with(|| Arc::new(AtomicU64::new(0.0f64.to_bits())))
            .clone()
    }

    pub fn write_float(&self, namespace: &str, field: &str, value: f64) {
        store_float_slot(&self.slot(namespace, field), value);
    }

    pub fn seeded_float_ref(&self, namespace: &str, field: impl Into<String>, value: f64) -> Value {
        let field = field.into();
        self.write_float(namespace, &field, value);
        Value::ReactiveRef {
            namespace: namespace.to_string(),
            field: field.clone(),
            index: None,
            kind: crate::vm::BindingKind::Float,
            slot: self.slot(namespace, &field),
        }
    }

    pub(crate) fn store_value(&self, namespace: &str, field: &str, value: &Value) {
        let Some(number) = numeric_value(value) else {
            if let Value::List(items) = value {
                for (index, item) in items.iter().enumerate() {
                    if let Some(number) = numeric_value(&item.borrow()) {
                        store_float_slot(&self.indexed_slot(namespace, field, index), number);
                    }
                }
            }
            return;
        };
        store_float_slot(&self.slot(namespace, field), number);
    }
}

fn changed_numeric_indices(previous: Option<&Value>, next: &Value) -> Vec<usize> {
    let Value::List(next_items) = next else {
        return match previous {
            Some(Value::List(items)) => (0..items.len()).collect(),
            _ => Vec::new(),
        };
    };
    let previous_items = match previous {
        Some(Value::List(items)) => Some(items.as_slice()),
        _ => None,
    };
    let max_len = previous_items
        .map(|items| items.len())
        .unwrap_or(0)
        .max(next_items.len());
    let mut changed = Vec::new();
    for index in 0..max_len {
        let previous_number = previous_items
            .and_then(|items| items.get(index))
            .and_then(|value| numeric_value(&value.borrow()));
        let next_number = next_items
            .get(index)
            .and_then(|value| numeric_value(&value.borrow()));
        if previous_number != next_number {
            changed.push(index);
        }
    }
    changed
}

#[derive(Clone)]
pub struct ReactiveRegistry {
    float_slots: ReactiveBindingStore,
    namespaces: HashMap<String, Namespace>,
    dirty: Vec<(String, String, Value)>,
    batched: Vec<(String, String, Value)>,
    field_to_widgets: HashMap<ReactiveBindingKey, HashSet<u64>>,
    widget_bindings_revision: u64,
    batching: bool,
}

#[derive(Clone)]
struct Namespace {
    fields: HashMap<String, Value>,
    map: HashMap<String, Rc<RefCell<Value>>>,
    writable: bool,
}

#[derive(Debug, Default)]
pub struct ReactiveSetOutcome {
    pub registered: bool,
    pub changed: bool,
    pub effect_dirty: bool,
    pub widget_ids: Vec<u64>,
}

impl ReactiveRegistry {
    pub fn new() -> Self {
        Self::with_float_slots(ReactiveBindingStore::default())
    }

    pub(crate) fn with_float_slots(float_slots: ReactiveBindingStore) -> Self {
        Self {
            float_slots,
            namespaces: HashMap::new(),
            dirty: Vec::new(),
            batched: Vec::new(),
            field_to_widgets: HashMap::new(),
            widget_bindings_revision: 0,
            batching: false,
        }
    }

    pub fn register(&mut self, name: &str, fields: Vec<(&str, Value)>, writable: bool) -> Value {
        let mut stored_fields = HashMap::new();
        let mut map = HashMap::new();

        for (field, value) in fields {
            self.float_slots.store_value(name, field, &value);
            stored_fields.insert(field.to_string(), value.clone());
            map.insert(field.to_string(), Rc::new(RefCell::new(value)));
        }

        self.namespaces.insert(
            name.to_string(),
            Namespace {
                fields: stored_fields,
                map: map.clone(),
                writable,
            },
        );

        Value::Map(map)
    }

    /// Cheap unchanged check used by Runtime::set_reactive to skip the full
    /// set pipeline (subscriber lookup, value clones) for no-op writes.
    pub fn is_unchanged(&self, namespace: &str, field: &str, value: &Value) -> bool {
        self.namespaces
            .get(namespace)
            .and_then(|namespace_entry| namespace_entry.fields.get(field))
            .is_some_and(|current| current == value)
    }

    pub fn set(
        &mut self,
        namespace: &str,
        field: &str,
        value: Value,
        enqueue_effect_dirty: bool,
    ) -> ReactiveSetOutcome {
        let Some(namespace_entry) = self.namespaces.get_mut(namespace) else {
            return ReactiveSetOutcome::default();
        };

        let previous = namespace_entry.fields.get(field);
        let changed_indices = changed_numeric_indices(previous, &value);
        let unchanged = previous.is_some_and(|current| *current == value);
        if unchanged {
            return ReactiveSetOutcome {
                registered: true,
                ..ReactiveSetOutcome::default()
            };
        }
        {
            static SCENE_TRACE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
            if *SCENE_TRACE
                .get_or_init(|| std::env::var("ESEQ_SCENE_TRACE").is_ok_and(|v| v == "1"))
            {
                eprintln!(
                    "[reactive-set-changed] {namespace}.{field} changed_indices={:?} had_previous={}",
                    changed_indices,
                    previous.is_some()
                );
            }
        }

        let key = ReactiveBindingKey::field(namespace, field);
        self.float_slots.store_value(namespace, field, &value);
        namespace_entry
            .fields
            .insert(field.to_string(), value.clone());
        if let Some(slot) = namespace_entry.map.get(field) {
            *slot.borrow_mut() = value.clone();
        } else {
            namespace_entry
                .map
                .insert(field.to_string(), Rc::new(RefCell::new(value.clone())));
        }

        if enqueue_effect_dirty {
            let dirty_value = namespace_entry
                .fields
                .get(field)
                .cloned()
                .unwrap_or(Value::Nil);
            let dirty = (namespace.to_string(), field.to_string(), dirty_value);
            if self.batching {
                self.batched.push(dirty);
            } else {
                self.dirty.push(dirty);
            }
        }
        let mut widgets: Vec<u64> = self
            .field_to_widgets
            .get(&key)
            .map(|widgets| widgets.iter().copied().collect())
            .unwrap_or_default();
        for index in changed_indices {
            if let Some(index_widgets) = self
                .field_to_widgets
                .get(&ReactiveBindingKey::indexed(namespace, field, index))
            {
                widgets.extend(index_widgets.iter().copied());
            }
        }
        widgets.sort_unstable();
        widgets.dedup();
        ReactiveSetOutcome {
            registered: true,
            changed: true,
            effect_dirty: enqueue_effect_dirty,
            widget_ids: widgets,
        }
    }

    pub fn set_list_index(
        &mut self,
        namespace: &str,
        field: &str,
        index: usize,
        value: Value,
        enqueue_effect_dirty: bool,
    ) -> ReactiveSetOutcome {
        let Some(namespace_entry) = self.namespaces.get_mut(namespace) else {
            return ReactiveSetOutcome::default();
        };

        let previous_item = namespace_entry
            .fields
            .get(field)
            .and_then(|stored| match stored {
                Value::List(items) => items.get(index).map(|item| item.borrow().clone()),
                _ => None,
            });
        if previous_item
            .as_ref()
            .is_some_and(|previous| *previous == value)
        {
            return ReactiveSetOutcome {
                registered: true,
                ..ReactiveSetOutcome::default()
            };
        }

        if let Some(number) = numeric_value(&value) {
            store_float_slot(
                &self.float_slots.indexed_slot(namespace, field, index),
                number,
            );
        }

        let stored = namespace_entry
            .fields
            .entry(field.to_string())
            .or_insert_with(|| Value::List(Vec::new()));
        match stored {
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

        let map_slot = namespace_entry
            .map
            .entry(field.to_string())
            .or_insert_with(|| Rc::new(RefCell::new(Value::List(Vec::new()))));
        {
            let mut borrowed = map_slot.borrow_mut();
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
        }

        if enqueue_effect_dirty {
            let dirty_value = namespace_entry
                .fields
                .get(field)
                .cloned()
                .unwrap_or(Value::Nil);
            let dirty = (namespace.to_string(), field.to_string(), dirty_value);
            if self.batching {
                self.batched.push(dirty);
            } else {
                self.dirty.push(dirty);
            }
        }

        let mut widgets: Vec<u64> = self
            .field_to_widgets
            .get(&ReactiveBindingKey::field(namespace, field))
            .map(|widgets| widgets.iter().copied().collect())
            .unwrap_or_default();
        if let Some(index_widgets) = self
            .field_to_widgets
            .get(&ReactiveBindingKey::indexed(namespace, field, index))
        {
            widgets.extend(index_widgets.iter().copied());
        }
        widgets.sort_unstable();
        widgets.dedup();
        ReactiveSetOutcome {
            registered: true,
            changed: true,
            effect_dirty: enqueue_effect_dirty,
            widget_ids: widgets,
        }
    }

    pub fn batch_begin(&mut self) {
        self.batching = true;
    }

    pub fn batch_end(&mut self) {
        self.batching = false;
        self.dirty.append(&mut self.batched);
    }

    pub fn drain_dirty(&mut self) -> Vec<(String, String, Value)> {
        std::mem::take(&mut self.dirty)
    }

    pub fn replace_widget_bindings_from_layout(&mut self, layout: Option<&LayoutNode>) {
        self.field_to_widgets.clear();
        if let Some(layout) = layout {
            self.collect_widget_bindings(layout);
        }
        self.bump_widget_bindings_revision();
    }

    pub fn replace_widget_bindings_for_layout_subtree(
        &mut self,
        old_subtree: &LayoutNode,
        new_subtree: &LayoutNode,
    ) {
        let mut removed_widget_ids = HashSet::new();
        collect_layout_widget_ids(old_subtree, &mut removed_widget_ids);
        for widgets in self.field_to_widgets.values_mut() {
            widgets.retain(|widget_id| !removed_widget_ids.contains(widget_id));
        }
        self.field_to_widgets
            .retain(|_, widgets| !widgets.is_empty());
        self.collect_widget_bindings(new_subtree);
        self.bump_widget_bindings_revision();
    }

    pub fn replace_widget_bindings_from_layouts<'a>(
        &mut self,
        layouts: impl IntoIterator<Item = &'a LayoutNode>,
    ) {
        self.field_to_widgets.clear();
        for layout in layouts {
            self.collect_widget_bindings(layout);
        }
        self.bump_widget_bindings_revision();
    }

    /// Rebuilds the binding table from pre-extracted per-layout entry lists.
    /// Lets callers cache `collect_widget_binding_entries` output per visible
    /// tile so only layouts that actually changed get rescanned.
    pub fn replace_widget_bindings_from_entry_lists<'a>(
        &mut self,
        entry_lists: impl IntoIterator<Item = &'a [(ReactiveBindingKey, u64)]>,
    ) {
        self.field_to_widgets.clear();
        for entries in entry_lists {
            for (key, widget_id) in entries {
                self.field_to_widgets
                    .entry(key.clone())
                    .or_default()
                    .insert(*widget_id);
            }
        }
        self.bump_widget_bindings_revision();
    }

    /// Applies a per-layout delta to the binding table instead of rebuilding
    /// it. Widget ids are globally unique, so a `(field, widget_id)` pair can
    /// only be contributed by one layout: removing the pairs of the layouts
    /// that changed and inserting the pairs of their replacements leaves the
    /// same table `replace_widget_bindings_from_entry_lists` would produce,
    /// at a cost proportional to what changed rather than to total UI size.
    pub fn update_widget_bindings_with_tile_delta<'a>(
        &mut self,
        removed: impl IntoIterator<Item = &'a [(ReactiveBindingKey, u64)]>,
        added: impl IntoIterator<Item = &'a [(ReactiveBindingKey, u64)]>,
    ) {
        for entries in removed {
            for (key, widget_id) in entries {
                if let Some(widgets) = self.field_to_widgets.get_mut(key) {
                    widgets.remove(widget_id);
                    if widgets.is_empty() {
                        self.field_to_widgets.remove(key);
                    }
                }
            }
        }
        for entries in added {
            for (key, widget_id) in entries {
                self.field_to_widgets
                    .entry(key.clone())
                    .or_default()
                    .insert(*widget_id);
            }
        }
        self.bump_widget_bindings_revision();
    }

    pub fn widget_bindings_revision(&self) -> u64 {
        self.widget_bindings_revision
    }

    pub fn widget_bindings_snapshot(&self) -> HashMap<ReactiveBindingKey, HashSet<u64>> {
        self.field_to_widgets.clone()
    }

    pub fn restore_widget_bindings(&mut self, bindings: HashMap<ReactiveBindingKey, HashSet<u64>>) {
        self.field_to_widgets = bindings;
        self.bump_widget_bindings_revision();
    }

    fn bump_widget_bindings_revision(&mut self) {
        self.widget_bindings_revision = self.widget_bindings_revision.wrapping_add(1);
    }

    fn collect_widget_bindings(&mut self, node: &LayoutNode) {
        for value in node.props.values() {
            self.collect_widget_bindings_from_value(node.widget_id, value);
        }
        for child in &node.children {
            self.collect_widget_bindings(child);
        }
    }

    /// Extracts a layout's widget bindings as a flat entry list, matching the
    /// traversal `collect_widget_bindings` performs, so the result can be
    /// cached per layout and merged via
    /// `replace_widget_bindings_from_entry_lists`.
    pub fn collect_widget_binding_entries(
        node: &LayoutNode,
        out: &mut Vec<(ReactiveBindingKey, u64)>,
    ) {
        fn collect_value(widget_id: u64, value: &Value, out: &mut Vec<(ReactiveBindingKey, u64)>) {
            match value {
                Value::ReactiveRef {
                    namespace,
                    field,
                    index,
                    ..
                } => {
                    let key = match index {
                        Some(index) => {
                            ReactiveBindingKey::indexed(namespace.clone(), field.clone(), *index)
                        }
                        None => ReactiveBindingKey::field(namespace.clone(), field.clone()),
                    };
                    out.push((key, widget_id));
                }
                Value::List(items) => {
                    for item in items {
                        collect_value(widget_id, &item.borrow(), out);
                    }
                }
                Value::Map(map) => {
                    for item in map.values() {
                        collect_value(widget_id, &item.borrow(), out);
                    }
                }
                _ => {}
            }
        }
        for value in node.props.values() {
            collect_value(node.widget_id, value, out);
        }
        for child in &node.children {
            Self::collect_widget_binding_entries(child, out);
        }
    }

    fn collect_widget_bindings_from_value(&mut self, widget_id: u64, value: &Value) {
        match value {
            Value::ReactiveRef {
                namespace,
                field,
                index,
                ..
            } => {
                let key = match index {
                    Some(index) => {
                        ReactiveBindingKey::indexed(namespace.clone(), field.clone(), *index)
                    }
                    None => ReactiveBindingKey::field(namespace.clone(), field.clone()),
                };
                self.field_to_widgets
                    .entry(key)
                    .or_default()
                    .insert(widget_id);
            }
            Value::List(items) => {
                for item in items {
                    self.collect_widget_bindings_from_value(widget_id, &item.borrow());
                }
            }
            Value::Map(map) => {
                for item in map.values() {
                    self.collect_widget_bindings_from_value(widget_id, &item.borrow());
                }
            }
            _ => {}
        }
    }

    pub fn namespace_names(&self) -> Vec<String> {
        let mut names = self.namespaces.keys().cloned().collect::<Vec<_>>();
        names.sort();
        names
    }

    pub fn namespace_value(&self, name: &str) -> Option<Value> {
        let namespace = self.namespaces.get(name)?;
        Some(Value::Map(namespace.map.clone()))
    }

    /// Borrows a single field of a reactive namespace.
    ///
    /// Reading one field through `Vm::global_value` clones the whole namespace
    /// map (every key `String` plus an `Rc` bump per field), which is
    /// proportional to total UI state rather than to the field being read.
    /// Callers that only need one field must use this instead.
    pub fn field_value(&self, namespace: &str, field: &str) -> Option<&Value> {
        self.namespaces.get(namespace)?.fields.get(field)
    }

    pub fn is_writable(&self, namespace: &str) -> bool {
        self.namespaces
            .get(namespace)
            .map(|ns| ns.writable)
            .unwrap_or(false)
    }
}

fn collect_layout_widget_ids(node: &LayoutNode, ids: &mut HashSet<u64>) {
    ids.insert(node.widget_id);
    for child in &node.children {
        collect_layout_widget_ids(child, ids);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(field: &str, widget_id: u64) -> (ReactiveBindingKey, u64) {
        (ReactiveBindingKey::field("SEQ", field), widget_id)
    }

    /// The delta path must produce exactly the table a full rebuild would.
    #[test]
    fn widget_binding_tile_delta_matches_a_full_rebuild() {
        let unchanged_tile = vec![entry("volume", 1), entry("pan", 2)];
        let before_tile = vec![entry("steps", 3), entry("selected", 4)];
        let after_tile = vec![entry("steps", 5), entry("mute", 6)];

        let mut incremental = ReactiveRegistry::new();
        incremental.replace_widget_bindings_from_entry_lists([
            unchanged_tile.as_slice(),
            before_tile.as_slice(),
        ]);
        incremental.update_widget_bindings_with_tile_delta(
            [before_tile.as_slice()],
            [after_tile.as_slice()],
        );

        let mut rebuilt = ReactiveRegistry::new();
        rebuilt.replace_widget_bindings_from_entry_lists([
            unchanged_tile.as_slice(),
            after_tile.as_slice(),
        ]);

        assert_eq!(
            incremental.widget_bindings_snapshot(),
            rebuilt.widget_bindings_snapshot()
        );
    }

    /// Removing a tile whose fields are shared with a surviving tile must keep
    /// the surviving tile's widgets bound.
    #[test]
    fn widget_binding_tile_delta_keeps_shared_fields_bound_when_a_tile_goes_away() {
        let kept_tile = vec![entry("steps", 1)];
        let closed_tile = vec![entry("steps", 2), entry("only-here", 3)];

        let mut registry = ReactiveRegistry::new();
        registry.replace_widget_bindings_from_entry_lists([
            kept_tile.as_slice(),
            closed_tile.as_slice(),
        ]);
        registry.update_widget_bindings_with_tile_delta([closed_tile.as_slice()], []);

        let snapshot = registry.widget_bindings_snapshot();
        assert_eq!(
            snapshot.get(&ReactiveBindingKey::field("SEQ", "steps")),
            Some(&HashSet::from([1]))
        );
        assert!(
            !snapshot.contains_key(&ReactiveBindingKey::field("SEQ", "only-here")),
            "a field with no remaining widgets must be dropped, not left empty"
        );
    }

    #[test]
    fn widget_binding_tile_delta_bumps_the_revision() {
        let mut registry = ReactiveRegistry::new();
        let before = registry.widget_bindings_revision();
        registry.update_widget_bindings_with_tile_delta([], [[entry("steps", 1)].as_slice()]);
        assert_ne!(registry.widget_bindings_revision(), before);
    }
}
