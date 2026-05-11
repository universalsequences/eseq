use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use crate::layout::LayoutNode;
use crate::vm::ReactiveBindingKey;
use crate::vm::Value;

static REACTIVE_FLOATS: OnceLock<Mutex<HashMap<ReactiveBindingKey, Arc<AtomicU64>>>> =
    OnceLock::new();

fn reactive_floats() -> &'static Mutex<HashMap<ReactiveBindingKey, Arc<AtomicU64>>> {
    REACTIVE_FLOATS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn numeric_value(value: &Value) -> Option<f64> {
    match value {
        Value::Number(n) => Some(*n),
        Value::Bool(true) => Some(1.0),
        Value::Bool(false) => Some(0.0),
        _ => None,
    }
}

pub fn reactive_float_slot(namespace: &str, field: &str) -> Arc<AtomicU64> {
    reactive_float_slot_for_key(ReactiveBindingKey::field(namespace, field))
}

pub fn reactive_indexed_float_slot(namespace: &str, field: &str, index: usize) -> Arc<AtomicU64> {
    reactive_float_slot_for_key(ReactiveBindingKey::indexed(namespace, field, index))
}

fn reactive_float_slot_for_key(key: ReactiveBindingKey) -> Arc<AtomicU64> {
    let mut slots = reactive_floats()
        .lock()
        .expect("reactive float store lock poisoned");
    slots
        .entry(key)
        .or_insert_with(|| Arc::new(AtomicU64::new(0.0f64.to_bits())))
        .clone()
}

pub fn read_float_slot(slot: &AtomicU64) -> f64 {
    f64::from_bits(slot.load(Ordering::Relaxed))
}

fn store_float_slot(slot: &AtomicU64, value: f64) {
    slot.store(value.to_bits(), Ordering::Relaxed);
}

fn store_float_slots(namespace: &str, field: &str, value: &Value) {
    let Some(number) = numeric_value(value) else {
        if let Value::List(items) = value {
            for (index, item) in items.iter().enumerate() {
                if let Some(number) = numeric_value(&item.borrow()) {
                    store_float_slot(
                        &reactive_indexed_float_slot(namespace, field, index),
                        number,
                    );
                }
            }
        }
        return;
    };
    store_float_slot(&reactive_float_slot(namespace, field), number);
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

pub struct ReactiveRegistry {
    namespaces: HashMap<String, Namespace>,
    dirty: Vec<(String, String, Value)>,
    batched: Vec<(String, String, Value)>,
    field_to_widgets: HashMap<ReactiveBindingKey, HashSet<u64>>,
    batching: bool,
}

struct Namespace {
    fields: HashMap<String, Value>,
    map: HashMap<String, Rc<RefCell<Value>>>,
    writable: bool,
}

#[derive(Debug, Default)]
pub struct ReactiveSetOutcome {
    pub effect_dirty: bool,
    pub widget_ids: Vec<u64>,
}

impl ReactiveRegistry {
    pub fn new() -> Self {
        Self {
            namespaces: HashMap::new(),
            dirty: Vec::new(),
            batched: Vec::new(),
            field_to_widgets: HashMap::new(),
            batching: false,
        }
    }

    pub fn register(&mut self, name: &str, fields: Vec<(&str, Value)>, writable: bool) -> Value {
        let mut stored_fields = HashMap::new();
        let mut map = HashMap::new();

        for (field, value) in fields {
            store_float_slots(name, field, &value);
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
            return ReactiveSetOutcome::default();
        }

        let key = ReactiveBindingKey::field(namespace, field);
        store_float_slots(namespace, field, &value);
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
            let dirty = (namespace.to_string(), field.to_string(), value);
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
    }

    pub fn replace_widget_bindings_from_layouts<'a>(
        &mut self,
        layouts: impl IntoIterator<Item = &'a LayoutNode>,
    ) {
        self.field_to_widgets.clear();
        for layout in layouts {
            self.collect_widget_bindings(layout);
        }
    }

    pub fn widget_bindings_snapshot(&self) -> HashMap<ReactiveBindingKey, HashSet<u64>> {
        self.field_to_widgets.clone()
    }

    pub fn restore_widget_bindings(&mut self, bindings: HashMap<ReactiveBindingKey, HashSet<u64>>) {
        self.field_to_widgets = bindings;
    }

    fn collect_widget_bindings(&mut self, node: &LayoutNode) {
        for value in node.props.values() {
            if let Value::ReactiveRef {
                namespace,
                field,
                index,
                ..
            } = value
            {
                let key = match index {
                    Some(index) => {
                        ReactiveBindingKey::indexed(namespace.clone(), field.clone(), *index)
                    }
                    None => ReactiveBindingKey::field(namespace.clone(), field.clone()),
                };
                self.field_to_widgets
                    .entry(key)
                    .or_default()
                    .insert(node.widget_id);
            }
        }
        for child in &node.children {
            self.collect_widget_bindings(child);
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

    pub fn is_writable(&self, namespace: &str) -> bool {
        self.namespaces
            .get(namespace)
            .map(|ns| ns.writable)
            .unwrap_or(false)
    }
}
