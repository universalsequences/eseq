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

    /// Whether anything ever bound (or wrote) this field's float slot. Lets a
    /// host skip echoing writes nobody has a handle to.
    pub fn has_field(&self, namespace: &str, field: &str) -> bool {
        self.slots
            .lock()
            .expect("reactive float store lock poisoned")
            .contains_key(&ReactiveBindingKey::field(namespace, field))
    }

    /// Current value of a field's float slot, if it exists.
    pub fn read_field(&self, namespace: &str, field: &str) -> Option<f64> {
        self.slots
            .lock()
            .expect("reactive float store lock poisoned")
            .get(&ReactiveBindingKey::field(namespace, field))
            .map(|slot| read_float_slot(slot))
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

    /// Handle to element `index` of a numeric-list field (what `bind-nth`
    /// returns), for host natives that hand Lisp a batch of element bindings.
    pub fn indexed_float_ref(&self, namespace: &str, field: impl Into<String>, index: usize) -> Value {
        let field = field.into();
        Value::ReactiveRef {
            namespace: namespace.to_string(),
            slot: self.indexed_slot(namespace, &field, index),
            field,
            index: Some(index),
            kind: crate::vm::BindingKind::Float,
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

fn summarize_diff_leaf(value: &Value) -> String {
    match value {
        Value::Number(n) => format!("{n}"),
        Value::Bool(b) => format!("{b}"),
        Value::Nil => "nil".to_string(),
        Value::String(s) | Value::Symbol(s) | Value::Keyword(s) => {
            let mut s = s.clone();
            if s.len() > 32 {
                s.truncate(32);
                s.push('…');
            }
            format!("{s:?}")
        }
        Value::List(items) => format!("list(len={})", items.len()),
        Value::Map(map) => format!("map(len={})", map.len()),
        _ => "<opaque>".to_string(),
    }
}

/// Walk two values in parallel and record the paths where they differ.
/// Recurses only into lists and maps; everything else is a leaf compared
/// with PartialEq. Stops once `limit` diffs have been collected.
fn collect_value_diffs(
    previous: &Value,
    next: &Value,
    path: &str,
    out: &mut Vec<String>,
    limit: usize,
) {
    if out.len() >= limit {
        return;
    }
    match (previous, next) {
        (Value::List(prev_items), Value::List(next_items)) => {
            if prev_items.len() != next_items.len() {
                out.push(format!(
                    "{path}: list len {} -> {}",
                    prev_items.len(),
                    next_items.len()
                ));
            }
            for (index, (prev_item, next_item)) in
                prev_items.iter().zip(next_items.iter()).enumerate()
            {
                collect_value_diffs(
                    &prev_item.borrow(),
                    &next_item.borrow(),
                    &format!("{path}[{index}]"),
                    out,
                    limit,
                );
            }
        }
        (Value::Map(prev_map), Value::Map(next_map)) => {
            let mut keys: Vec<&String> = prev_map.keys().chain(next_map.keys()).collect();
            keys.sort();
            keys.dedup();
            for key in keys {
                match (prev_map.get(key), next_map.get(key)) {
                    (Some(prev_item), Some(next_item)) => collect_value_diffs(
                        &prev_item.borrow(),
                        &next_item.borrow(),
                        &format!("{path}.{key}"),
                        out,
                        limit,
                    ),
                    (Some(prev_item), None) => out.push(format!(
                        "{path}.{key}: {} -> <absent>",
                        summarize_diff_leaf(&prev_item.borrow())
                    )),
                    (None, Some(next_item)) => out.push(format!(
                        "{path}.{key}: <absent> -> {}",
                        summarize_diff_leaf(&next_item.borrow())
                    )),
                    (None, None) => {}
                }
                if out.len() >= limit {
                    return;
                }
            }
        }
        _ => {
            if previous != next {
                out.push(format!(
                    "{path}: {} -> {}",
                    summarize_diff_leaf(previous),
                    summarize_diff_leaf(next)
                ));
            }
        }
    }
}

/// Classification of a candidate value against the stored one for the
/// value-patch fast path (docs/fx-value-delta-spec.md).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ReactiveValueDelta {
    /// Deep-equal: nothing to do.
    Equal,
    /// Same shape everywhere; only Number/Bool leaves differ. Safe to write
    /// into the stored tree's shared cells without dirtying subscribers —
    /// widgets display these leaves through per-param field bindings.
    Patchable,
    /// Shape, string, or variant change (or unregistered field): must go
    /// through the full set_reactive pipeline so dependents re-evaluate.
    Structural,
}

fn classify_value_delta(stored: &Value, next: &Value) -> ReactiveValueDelta {
    match (stored, next) {
        (Value::List(stored_items), Value::List(next_items)) => {
            if stored_items.len() != next_items.len() {
                return ReactiveValueDelta::Structural;
            }
            let mut class = ReactiveValueDelta::Equal;
            for (stored_item, next_item) in stored_items.iter().zip(next_items.iter()) {
                match classify_value_delta(&stored_item.borrow(), &next_item.borrow()) {
                    ReactiveValueDelta::Structural => return ReactiveValueDelta::Structural,
                    ReactiveValueDelta::Patchable => class = ReactiveValueDelta::Patchable,
                    ReactiveValueDelta::Equal => {}
                }
            }
            class
        }
        (Value::Map(stored_map), Value::Map(next_map)) => {
            if stored_map.len() != next_map.len() {
                return ReactiveValueDelta::Structural;
            }
            let mut class = ReactiveValueDelta::Equal;
            for (key, stored_item) in stored_map {
                let Some(next_item) = next_map.get(key) else {
                    return ReactiveValueDelta::Structural;
                };
                match classify_value_delta(&stored_item.borrow(), &next_item.borrow()) {
                    ReactiveValueDelta::Structural => return ReactiveValueDelta::Structural,
                    ReactiveValueDelta::Patchable => class = ReactiveValueDelta::Patchable,
                    ReactiveValueDelta::Equal => {}
                }
            }
            class
        }
        (Value::Number(stored_number), Value::Number(next_number)) => {
            if stored_number == next_number {
                ReactiveValueDelta::Equal
            } else {
                ReactiveValueDelta::Patchable
            }
        }
        (Value::Bool(stored_bool), Value::Bool(next_bool)) => {
            if stored_bool == next_bool {
                ReactiveValueDelta::Equal
            } else {
                ReactiveValueDelta::Patchable
            }
        }
        _ => {
            if stored == next {
                ReactiveValueDelta::Equal
            } else {
                ReactiveValueDelta::Structural
            }
        }
    }
}

fn patch_value_cells(stored: &Rc<RefCell<Value>>, next: &Rc<RefCell<Value>>, patched: &mut usize) {
    enum Step {
        Recurse,
        Write(Value),
        Nothing,
    }
    let step = {
        match (&*stored.borrow(), &*next.borrow()) {
            (Value::List(_), Value::List(_)) | (Value::Map(_), Value::Map(_)) => Step::Recurse,
            (Value::Number(stored_number), Value::Number(next_number)) => {
                if stored_number != next_number {
                    Step::Write(Value::Number(*next_number))
                } else {
                    Step::Nothing
                }
            }
            (Value::Bool(stored_bool), Value::Bool(next_bool)) => {
                if stored_bool != next_bool {
                    Step::Write(Value::Bool(*next_bool))
                } else {
                    Step::Nothing
                }
            }
            _ => Step::Nothing,
        }
    };
    match step {
        Step::Recurse => {
            let stored_ref = stored.borrow();
            let next_ref = next.borrow();
            match (&*stored_ref, &*next_ref) {
                (Value::List(stored_items), Value::List(next_items)) => {
                    for (stored_item, next_item) in stored_items.iter().zip(next_items.iter()) {
                        patch_value_cells(stored_item, next_item, patched);
                    }
                }
                (Value::Map(stored_map), Value::Map(next_map)) => {
                    for (key, stored_item) in stored_map {
                        if let Some(next_item) = next_map.get(key) {
                            patch_value_cells(stored_item, next_item, patched);
                        }
                    }
                }
                _ => {}
            }
        }
        Step::Write(next_value) => {
            *stored.borrow_mut() = next_value;
            *patched += 1;
        }
        Step::Nothing => {}
    }
}

/// Group subscriptions by namespace and field so demand queries need no scan
/// or temporary owned key. Empty binding, field and namespace entries are
/// removed together; presence of a field means it has at least one reader.
#[derive(Clone, Default)]
struct WidgetSubscriptions {
    namespaces: HashMap<String, HashMap<String, FieldWidgetReaders>>,
}

#[derive(Clone, Default)]
struct FieldWidgetReaders {
    scalar: HashSet<u64>,
    // Most fields have only scalar readers. Keep their set inline instead of
    // allocating an index table for every individual control parameter.
    indexed: HashMap<usize, HashSet<u64>>,
}

impl FieldWidgetReaders {
    fn is_empty(&self) -> bool {
        self.scalar.is_empty() && self.indexed.is_empty()
    }
}

impl WidgetSubscriptions {
    fn clear(&mut self) {
        self.namespaces.clear();
    }

    fn has_readers(&self, namespace: &str, field: &str) -> bool {
        self.namespaces.get(namespace).is_some_and(|fields| fields.contains_key(field))
    }

    fn get(&self, namespace: &str, field: &str, index: Option<usize>) -> Option<&HashSet<u64>> {
        let readers = self.namespaces.get(namespace)?.get(field)?;
        match index {
            Some(index) => readers.indexed.get(&index),
            None => (!readers.scalar.is_empty()).then_some(&readers.scalar),
        }
    }

    fn insert(&mut self, key: ReactiveBindingKey, widget_id: u64) {
        let readers = self.namespaces.entry(key.field.namespace).or_default()
            .entry(key.field.field).or_default();
        match key.index {
            Some(index) => { readers.indexed.entry(index).or_default().insert(widget_id); }
            None => { readers.scalar.insert(widget_id); }
        }
    }

    fn remove(&mut self, key: &ReactiveBindingKey, widget_id: u64) {
        let Some(fields) = self.namespaces.get_mut(&key.field.namespace) else { return; };
        if let Some(readers) = fields.get_mut(&key.field.field) {
            match key.index {
                Some(index) => {
                    if let Some(widgets) = readers.indexed.get_mut(&index) {
                        widgets.remove(&widget_id);
                        if widgets.is_empty() { readers.indexed.remove(&index); }
                    }
                }
                None => { readers.scalar.remove(&widget_id); }
            }
            if readers.is_empty() { fields.remove(&key.field.field); }
        }
        if fields.is_empty() { self.namespaces.remove(&key.field.namespace); }
    }

    fn remove_widgets(&mut self, removed: &HashSet<u64>) {
        self.namespaces.retain(|_, fields| {
            fields.retain(|_, readers| {
                readers.scalar.retain(|widget_id| !removed.contains(widget_id));
                readers.indexed.retain(|_, widgets| {
                    widgets.retain(|widget_id| !removed.contains(widget_id));
                    !widgets.is_empty()
                });
                !readers.is_empty()
            });
            !fields.is_empty()
        });
    }

    fn snapshot(&self) -> HashMap<ReactiveBindingKey, HashSet<u64>> {
        let mut bindings = HashMap::new();
        for (namespace, fields) in &self.namespaces {
            for (field, readers) in fields {
                if !readers.scalar.is_empty() {
                    bindings.insert(ReactiveBindingKey::field(namespace, field), readers.scalar.clone());
                }
                for (index, widgets) in &readers.indexed {
                    bindings.insert(ReactiveBindingKey::indexed(namespace, field, *index), widgets.clone());
                }
            }
        }
        bindings
    }

    fn restore(&mut self, bindings: HashMap<ReactiveBindingKey, HashSet<u64>>) {
        self.clear();
        for (key, widgets) in bindings {
            if !widgets.is_empty() {
                let readers = self.namespaces.entry(key.field.namespace).or_default()
                    .entry(key.field.field).or_default();
                match key.index {
                    Some(index) => { readers.indexed.insert(index, widgets); }
                    None => { readers.scalar = widgets; }
                }
            }
        }
    }
}

#[derive(Clone)]
pub struct ReactiveRegistry {
    float_slots: ReactiveBindingStore,
    namespaces: HashMap<String, Namespace>,
    dirty: Vec<(String, String, Value)>,
    batched: Vec<(String, String, Value)>,
    field_to_widgets: WidgetSubscriptions,
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
            field_to_widgets: WidgetSubscriptions::default(),
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

    /// Classifies `next` against the stored value for the value-patch fast
    /// path. Returns None when the field is unregistered. Only container
    /// (list/map) roots are ever Patchable: a scalar root lives by value in
    /// this registry and in the VM global, so writing it here would not
    /// propagate — scalar roots must take the full set pipeline.
    pub fn classify_value_patch(
        &self,
        namespace: &str,
        field: &str,
        next: &Value,
    ) -> Option<ReactiveValueDelta> {
        let stored = self
            .namespaces
            .get(namespace)
            .and_then(|namespace_entry| namespace_entry.fields.get(field))?;
        if !matches!(
            (stored, next),
            (Value::List(_), Value::List(_)) | (Value::Map(_), Value::Map(_))
        ) {
            return Some(if stored == next {
                ReactiveValueDelta::Equal
            } else {
                ReactiveValueDelta::Structural
            });
        }
        // Root lists with numeric elements carry per-index float slots for
        // bind-seq subscribers; an in-place patch would leave those slots
        // stale, so such fields always take the full set pipeline.
        if let Value::List(items) = stored
            && items
                .iter()
                .any(|item| matches!(&*item.borrow(), Value::Number(_) | Value::Bool(_)))
        {
            return Some(if stored == next {
                ReactiveValueDelta::Equal
            } else {
                ReactiveValueDelta::Structural
            });
        }
        Some(classify_value_delta(stored, next))
    }

    /// Writes `next`'s differing Number/Bool leaves into the stored tree's
    /// cells. Those cells are shared (Value::clone is shallow at the cell
    /// level) with the VM global namespace and anything the Lisp side
    /// captured from earlier evals, so every reader sees the new values on
    /// its next evaluation without any subscriber dirtying. Callers must
    /// have classified the delta as Patchable first. Returns the number of
    /// leaves written.
    pub fn apply_value_patch(&self, namespace: &str, field: &str, next: &Value) -> usize {
        let Some(stored) = self
            .namespaces
            .get(namespace)
            .and_then(|namespace_entry| namespace_entry.fields.get(field))
        else {
            return 0;
        };
        let mut patched = 0;
        match (stored, next) {
            (Value::List(stored_items), Value::List(next_items)) => {
                for (stored_item, next_item) in stored_items.iter().zip(next_items.iter()) {
                    patch_value_cells(stored_item, next_item, &mut patched);
                }
            }
            (Value::Map(stored_map), Value::Map(next_map)) => {
                for (key, stored_item) in stored_map {
                    if let Some(next_item) = next_map.get(key) {
                        patch_value_cells(stored_item, next_item, &mut patched);
                    }
                }
            }
            _ => {}
        }
        patched
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
        {
            // ESEQ_SCENE_TRACE_DIFF=field1,field2 dumps the value paths that
            // actually differ on a changed set of those fields.
            static DIFF_FIELDS: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
            let diff_fields = DIFF_FIELDS.get_or_init(|| {
                std::env::var("ESEQ_SCENE_TRACE_DIFF")
                    .map(|v| {
                        v.split(',')
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty())
                            .collect()
                    })
                    .unwrap_or_default()
            });
            if diff_fields.iter().any(|f| f == field) {
                if let Some(previous) = previous {
                    const DIFF_LIMIT: usize = 60;
                    let mut diffs = Vec::new();
                    collect_value_diffs(previous, &value, "", &mut diffs, DIFF_LIMIT);
                    for line in &diffs {
                        eprintln!("[reactive-diff] {namespace}.{field}{line}");
                    }
                    if diffs.len() >= DIFF_LIMIT {
                        eprintln!(
                            "[reactive-diff] {namespace}.{field}: ... (truncated at {DIFF_LIMIT})"
                        );
                    }
                }
            }
        }

        self.float_slots.store_value(namespace, field, &value);
        namespace_entry
            .fields
            .insert(field.to_string(), value.clone());
        if let Some(slot) = namespace_entry.map.get(field) {
            crate::vm::debug_assert_cell_not_frozen(slot, "ReactiveStore::set");
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
            .get(namespace, field, None)
            .map(|widgets| widgets.iter().copied().collect())
            .unwrap_or_default();
        for index in changed_indices {
            if let Some(index_widgets) = self
                .field_to_widgets
                .get(namespace, field, Some(index))
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
                crate::vm::debug_assert_cell_not_frozen(
                    &items[index],
                    "ReactiveStore::set_index (fields)",
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

        let map_slot = namespace_entry
            .map
            .entry(field.to_string())
            .or_insert_with(|| Rc::new(RefCell::new(Value::List(Vec::new()))));
        {
            crate::vm::debug_assert_cell_not_frozen(map_slot, "ReactiveStore::set_index");
            let mut borrowed = map_slot.borrow_mut();
            match &mut *borrowed {
                Value::List(items) => {
                    while items.len() <= index {
                        items.push(Rc::new(RefCell::new(Value::Nil)));
                    }
                    crate::vm::debug_assert_cell_not_frozen(
                        &items[index],
                        "ReactiveStore::set_index (map)",
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
            .get(namespace, field, None)
            .map(|widgets| widgets.iter().copied().collect())
            .unwrap_or_default();
        if let Some(index_widgets) = self
            .field_to_widgets
            .get(namespace, field, Some(index))
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

    pub(crate) fn has_widget_readers(&self, namespace: &str, field: &str) -> bool {
        self.field_to_widgets.has_readers(namespace, field)
    }

    pub fn replace_widget_bindings_for_layout_subtree(
        &mut self,
        old_subtree: &LayoutNode,
        new_subtree: &LayoutNode,
    ) {
        let mut removed_widget_ids = HashSet::new();
        collect_layout_widget_ids(old_subtree, &mut removed_widget_ids);
        self.field_to_widgets.remove_widgets(&removed_widget_ids);
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
                self.field_to_widgets.insert(key.clone(), *widget_id);
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
                self.field_to_widgets.remove(key, *widget_id);
            }
        }
        for entries in added {
            for (key, widget_id) in entries {
                self.field_to_widgets.insert(key.clone(), *widget_id);
            }
        }
        self.bump_widget_bindings_revision();
    }

    pub fn widget_bindings_revision(&self) -> u64 {
        self.widget_bindings_revision
    }

    pub fn widget_bindings_snapshot(&self) -> HashMap<ReactiveBindingKey, HashSet<u64>> {
        self.field_to_widgets.snapshot()
    }

    pub fn restore_widget_bindings(&mut self, bindings: HashMap<ReactiveBindingKey, HashSet<u64>>) {
        self.field_to_widgets.restore(bindings);
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
                self.field_to_widgets.insert(key, widget_id);
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

    #[test]
    fn widget_readers_survive_mixed_binding_tile_removals() {
        let scalar = entry("levels", 1);
        let indexed = (ReactiveBindingKey::indexed("SEQ", "levels", 3), 2);
        let other_namespace = (ReactiveBindingKey::indexed("OTHER", "levels", 3), 3);
        let mut registry = ReactiveRegistry::new();
        registry.replace_widget_bindings_from_entry_lists([
            [scalar.clone(), scalar.clone(), indexed.clone(), other_namespace.clone()].as_slice(),
        ]);
        assert!(registry.has_widget_readers("SEQ", "levels"));
        assert!(!registry.has_widget_readers("SEQ", "absent"));
        registry.update_widget_bindings_with_tile_delta([[scalar.clone(), scalar].as_slice()], []);
        assert!(registry.has_widget_readers("SEQ", "levels"), "indexed reader survives scalar removal");
        registry.update_widget_bindings_with_tile_delta([[indexed.clone(), indexed].as_slice()], []);
        assert!(!registry.has_widget_readers("SEQ", "levels"));
        assert!(registry.has_widget_readers("OTHER", "levels"));
        registry.update_widget_bindings_with_tile_delta([[other_namespace].as_slice()], []);
        assert!(!registry.has_widget_readers("OTHER", "levels"));
        assert!(registry.widget_bindings_snapshot().is_empty());
    }

    #[test]
    fn widget_readers_follow_snapshot_restore_and_clone() {
        let binding = entry("position", 1);
        let mut registry = ReactiveRegistry::new();
        registry.replace_widget_bindings_from_entry_lists([[binding.clone()].as_slice()]);
        let mut snapshot = registry.widget_bindings_snapshot();
        snapshot.insert(ReactiveBindingKey::field("EMPTY", "unused"), HashSet::new());
        let cloned = registry.clone();
        registry.replace_widget_bindings_from_layout(None);
        assert!(!registry.has_widget_readers("SEQ", "position"));
        assert!(cloned.has_widget_readers("SEQ", "position"));
        registry.restore_widget_bindings(snapshot);
        assert!(registry.has_widget_readers("SEQ", "position"));
        assert!(!registry.has_widget_readers("EMPTY", "unused"));
        registry.update_widget_bindings_with_tile_delta([[binding].as_slice()], []);
        assert!(!registry.has_widget_readers("SEQ", "position"));
        assert!(cloned.has_widget_readers("SEQ", "position"));
    }

    fn bound_node(widget_id: u64, key: ReactiveBindingKey, children: Vec<LayoutNode>) -> LayoutNode {
        let binding = Value::ReactiveRef {
            namespace: key.field.namespace, field: key.field.field, index: key.index,
            kind: crate::vm::BindingKind::Float, slot: Arc::new(AtomicU64::new(0)),
        };
        LayoutNode {
            widget_id, stable_widget_id: None, subtree_root_id: None, parent_subtree_root_id: None,
            stable_key: None, widget_type: "text".into(),
            rect: crate::layout::Rect { row: 0.0, col: 0.0, width: 1.0, height: 1.0 },
            props: HashMap::from([("value".into(), binding)]), children,
            focusable: false, animation: Default::default(),
        }
    }

    #[test]
    fn widget_readers_follow_subtree_and_full_layout_replacement() {
        let old = bound_node(2, ReactiveBindingKey::indexed("SEQ", "levels", 0), vec![
            bound_node(3, ReactiveBindingKey::field("SEQ", "removed"), vec![]),
        ]);
        let kept = bound_node(4, ReactiveBindingKey::indexed("SEQ", "levels", 1), vec![]);
        let root = bound_node(1, ReactiveBindingKey::field("SEQ", "parent"), vec![old.clone(), kept]);
        let new = bound_node(2, ReactiveBindingKey::field("SEQ", "replacement"), vec![]);
        let mut registry = ReactiveRegistry::new();
        registry.replace_widget_bindings_from_layout(Some(&root));
        registry.replace_widget_bindings_for_layout_subtree(&old, &new);
        assert!(registry.has_widget_readers("SEQ", "levels"));
        assert!(registry.has_widget_readers("SEQ", "parent"));
        assert!(registry.has_widget_readers("SEQ", "replacement"));
        assert!(!registry.has_widget_readers("SEQ", "removed"));
        let snapshot = registry.widget_bindings_snapshot();
        assert_eq!(snapshot.get(&ReactiveBindingKey::indexed("SEQ", "levels", 0)), None);
        assert_eq!(snapshot.get(&ReactiveBindingKey::indexed("SEQ", "levels", 1)), Some(&HashSet::from([4])));
        registry.replace_widget_bindings_from_layouts([&old, &new]);
        assert!(!registry.has_widget_readers("SEQ", "parent"));
        assert!(registry.has_widget_readers("SEQ", "removed"));
        registry.replace_widget_bindings_from_layouts([]);
        assert!(!registry.has_widget_readers("SEQ", "levels"));
        assert!(!registry.has_widget_readers("SEQ", "replacement"));
    }

    #[test]
    fn widget_publication_preserves_scalar_and_indexed_subscribers() {
        let list = |values: &[f64]| Value::List(values.iter()
            .map(|value| Rc::new(RefCell::new(Value::Number(*value)))).collect());
        let mut registry = ReactiveRegistry::new();
        registry.register("SEQ", vec![("levels", list(&[0.0, 0.0]))], true);
        let bindings = [
            entry("levels", 1),
            (ReactiveBindingKey::indexed("SEQ", "levels", 0), 2),
            (ReactiveBindingKey::indexed("SEQ", "levels", 1), 3),
            (ReactiveBindingKey::indexed("SEQ", "levels", 0), 1),
            (ReactiveBindingKey::indexed("OTHER", "levels", 0), 4),
        ];
        registry.replace_widget_bindings_from_entry_lists([bindings.as_slice()]);
        assert_eq!(registry.set_list_index("SEQ", "levels", 0, Value::Number(1.0), false).widget_ids, vec![1, 2]);
        assert_eq!(registry.set("SEQ", "levels", list(&[1.0, 2.0]), false).widget_ids, vec![1, 3]);
        assert_eq!(registry.set("SEQ", "levels", list(&[2.0, 3.0]), false).widget_ids, vec![1, 2, 3]);
        assert!(registry.set("SEQ", "levels", list(&[2.0, 3.0]), false).widget_ids.is_empty());
    }
}
