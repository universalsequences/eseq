use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::vm::Value;

pub struct ReactiveRegistry {
    namespaces: HashMap<String, Namespace>,
    dirty: Vec<(String, String, Value)>,
    batched: Vec<(String, String, Value)>,
    batching: bool,
}

struct Namespace {
    fields: HashMap<String, Value>,
    map: HashMap<String, Rc<RefCell<Value>>>,
    writable: bool,
}

impl ReactiveRegistry {
    pub fn new() -> Self {
        Self {
            namespaces: HashMap::new(),
            dirty: Vec::new(),
            batched: Vec::new(),
            batching: false,
        }
    }

    pub fn register(&mut self, name: &str, fields: Vec<(&str, Value)>, writable: bool) -> Value {
        let mut stored_fields = HashMap::new();
        let mut map = HashMap::new();

        for (field, value) in fields {
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

    pub fn set(&mut self, namespace: &str, field: &str, value: Value) {
        let Some(namespace_entry) = self.namespaces.get_mut(namespace) else {
            return;
        };

        let unchanged = namespace_entry
            .fields
            .get(field)
            .is_some_and(|current| *current == value);
        if unchanged {
            return;
        }

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

        let dirty = (namespace.to_string(), field.to_string(), value);
        if self.batching {
            self.batched.push(dirty);
        } else {
            self.dirty.push(dirty);
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
