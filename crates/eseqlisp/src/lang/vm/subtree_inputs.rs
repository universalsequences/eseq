//! Exact captured-input observations for subtree reuse. Store only comparison
//! data in one byte buffer, rather than a second mutable Lisp object graph.
//! This is private, in-memory data, never a persisted/wire format. Map entries
//! keep their capture order; comparison looks up each saved key in the live map
//! so hash iteration order does not affect equality. No hashes certify reuse.

use super::{BindingKind, Value};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

pub(super) struct SubtreeInputs {
    bytes: Box<[u8]>,
    // Repeated descriptor keys are owned once. Besides saving space, retaining
    // validated strings avoids decoding UTF-8 during every warm comparison.
    keys: Box<[String]>,
}

enum Tag {
    Number, Bool, Nil, String, Symbol, Keyword, List, Map, NodeRef, Instance, ReactiveRef,
}

#[derive(Default)]
struct Capture {
    bytes: Vec<u8>,
    keys: HashMap<String, usize>,
    ancestors: Vec<*const RefCell<Value>>,
}

impl SubtreeInputs {
    /// Callables, opaque handles, cycles and exclusively borrowed cells cannot establish
    /// equality. Decline reuse without retaining any partial observation.
    pub(super) fn capture(cells: &[Rc<RefCell<Value>>]) -> Option<Self> {
        if cells.is_empty() {
            return Some(Self { bytes: Box::default(), keys: Box::default() });
        }
        let mut capture = Capture {
            bytes: Vec::with_capacity(cells.len() * size_of::<Value>()),
            ..Capture::default()
        };
        for cell in cells {
            capture.cell(cell)?;
        }
        let mut keys = vec![String::new(); capture.keys.len()];
        for (key, index) in capture.keys { keys[index] = key; }
        Some(Self { bytes: capture.bytes.into_boxed_slice(), keys: keys.into_boxed_slice() })
    }

    pub(super) fn matches(&self, cells: &[Rc<RefCell<Value>>]) -> bool {
        let mut reader = Reader { bytes: &self.bytes, keys: &self.keys };
        cells.iter().all(|cell| reader.cell(cell).is_some())
            && reader.bytes.is_empty()
    }

    #[cfg(test)]
    pub(super) fn retained_bytes(&self) -> usize {
        self.bytes.len() + self.keys.len() * size_of::<String>()
            + self.keys.iter().map(String::capacity).sum::<usize>()
    }
}

fn tag(value: &Value) -> Option<Tag> {
    Some(match value {
        Value::Number(_) => Tag::Number,
        Value::Bool(_) => Tag::Bool,
        Value::Nil => Tag::Nil,
        Value::String(_) => Tag::String,
        Value::Symbol(_) => Tag::Symbol,
        Value::Keyword(_) => Tag::Keyword,
        Value::List(_) => Tag::List,
        Value::Map(_) => Tag::Map,
        Value::NodeRef(_) => Tag::NodeRef,
        Value::Instance(_) => Tag::Instance,
        Value::ReactiveRef { .. } => Tag::ReactiveRef,
        _ => return None,
    })
}

fn binding_kind(kind: BindingKind) -> (u8, u64) {
    match kind {
        BindingKind::Float => (0, 0),
        BindingKind::InstanceFloat(id) => (1, id),
        BindingKind::InstanceRgb(id) => (2, id),
    }
}

impl Capture {
    fn usize(&mut self, value: usize) {
        self.bytes.extend_from_slice(&value.to_ne_bytes());
    }

    fn string(&mut self, value: &str) {
        self.usize(value.len());
        self.bytes.extend_from_slice(value.as_bytes());
    }

    fn cell(&mut self, cell: &Rc<RefCell<Value>>) -> Option<()> {
        let value = cell.try_borrow().ok()?;
        // Only containers can form cycles. Scalar parameter/instance captures
        // need neither ancestor scans nor a traversal-stack allocation.
        if !matches!(&*value, Value::List(_) | Value::Map(_)) {
            return self.value(&value);
        }
        let identity = Rc::as_ptr(cell);
        if self.ancestors.contains(&identity) { return None; }
        self.ancestors.push(identity);
        let result = self.value(&value);
        self.ancestors.pop();
        result
    }

    fn value(&mut self, value: &Value) -> Option<()> {
        self.bytes.push(tag(value)? as u8);
        match value {
            Value::Number(value) => self.bytes.extend_from_slice(&value.to_ne_bytes()),
            Value::Bool(value) => self.bytes.push(u8::from(*value)),
            Value::Nil => {},
            Value::String(value) | Value::Symbol(value) | Value::Keyword(value) => self.string(value),
            Value::NodeRef(value) => self.bytes.extend_from_slice(&value.to_ne_bytes()),
            Value::Instance(value) => self.bytes.extend_from_slice(&value.to_ne_bytes()),
            Value::List(items) => {
                self.usize(items.len());
                for cell in items { self.cell(cell)?; }
            }
            Value::Map(items) => {
                self.usize(items.len());
                for (key, cell) in items {
                    let index = if let Some(index) = self.keys.get(key) {
                        *index
                    } else {
                        let index = self.keys.len();
                        self.keys.insert(key.clone(), index);
                        index
                    };
                    self.usize(index);
                    self.cell(cell)?;
                }
            }
            Value::ReactiveRef { namespace, field, index, kind, .. } => {
                self.string(namespace);
                self.string(field);
                self.bytes.push(u8::from(index.is_some()));
                self.usize(index.unwrap_or(0));
                let (kind, id) = binding_kind(*kind);
                self.bytes.push(kind);
                self.bytes.extend_from_slice(&id.to_ne_bytes());
            }
            _ => return None,
        }
        Some(())
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    keys: &'a [String],
}

impl<'a> Reader<'a> {
    fn take(&mut self, len: usize) -> Option<&'a [u8]> {
        let (value, rest) = self.bytes.split_at_checked(len)?;
        self.bytes = rest;
        Some(value)
    }

    fn byte(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }

    fn usize(&mut self) -> Option<usize> {
        Some(usize::from_ne_bytes(self.take(size_of::<usize>())?.try_into().ok()?))
    }

    fn string_bytes(&mut self) -> Option<&'a [u8]> {
        let len = self.usize()?;
        self.take(len)
    }

    fn cell(&mut self, cell: &Rc<RefCell<Value>>) -> Option<()> {
        self.value(&*cell.try_borrow().ok()?)
    }

    fn value(&mut self, value: &Value) -> Option<()> {
        if self.byte()? != tag(value)? as u8 { return None; }
        let equal = match value {
            // Preserve Lisp numeric equality: signed zero matches, NaN does not.
            Value::Number(value) => f64::from_ne_bytes(self.take(8)?.try_into().ok()?) == *value,
            Value::Bool(value) => self.byte()? == u8::from(*value),
            Value::Nil => true,
            Value::String(value) | Value::Symbol(value) | Value::Keyword(value) => self.string_bytes()? == value.as_bytes(),
            Value::NodeRef(value) => self.take(4)? == value.to_ne_bytes(),
            Value::Instance(value) => self.take(8)? == value.to_ne_bytes(),
            Value::List(items) => {
                if self.usize()? != items.len() { return None; }
                for cell in items { self.cell(cell)?; }
                true
            }
            Value::Map(items) => {
                if self.usize()? != items.len() { return None; }
                for _ in 0..items.len() {
                    let index = self.usize()?;
                    let key = self.keys.get(index)?;
                    self.cell(items.get(key)?)?;
                }
                true
            }
            Value::ReactiveRef { namespace, field, index, kind, .. } => {
                let (kind, id) = binding_kind(*kind);
                self.string_bytes()? == namespace.as_bytes() && self.string_bytes()? == field.as_bytes()
                    && self.byte()? == u8::from(index.is_some())
                    && self.usize()? == index.unwrap_or(0)
                    && self.byte()? == kind && self.take(8)? == id.to_ne_bytes()
            }
            _ => false,
        };
        equal.then_some(())
    }
}
