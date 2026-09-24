//! Reactive instance records (instance-kinds spec §4, §12.1).
//!
//! A package defines *kinds*; the host owns *instances* of them. Lisp sees an
//! instance as the ordinary value `Value::Instance(id)` and reads its fields
//! with the existing dotted `x.field` syntax (load + `GetField`). Writes use
//! the existing dotted `(set! x.field v)` path (load + `StoreField`); both
//! opcodes dispatch on `Value::Instance` here.
//!
//! Every (instance, field) pair is its own reactive DAG source, so a write
//! dirties only the effects that read that one field of that one instance.
//! The sources are ordinary `ReactiveSource::NamespaceField` nodes under a
//! reserved per-instance namespace (`%instance/<id>`); that keeps subtree
//! render caching, read capture and effect scheduling identical to host
//! reactive namespaces, with no new DAG machinery. The field *values* live in
//! this store (indexed by schema position), never in a VM global.
//!
//! Fields are either host fields (`id`, `kind`, `owner`, `label`) or the
//! kind's declared `:state` fields. `id` and `kind` come from the record,
//! `owner` and `label` are pushed by the host
//! (`VM::set_instance_host_field`). Only `label` is writable from Lisp: with a
//! label hook installed the write is forwarded to the host (a rename is an
//! undoable project edit, the host pushes the accepted label back); without
//! one the label cell is written locally.
//!
//! A dropped instance keeps a tombstone naming its kind: reads answer the
//! kind's defaults, writes are silent no-ops (spec §4 "stale self").

use std::collections::HashMap;
use std::rc::Rc;

use super::{ReactiveNode, ReactiveSource, VM, VMError, Value, clone_value_for_snapshot};

/// Host-assigned instance id, stable within a project.
pub type InstanceId = u64;

/// Receives a Lisp `(set! x.label v)` when the host wants renames routed
/// through its own (undoable) edit path. The host is expected to push the
/// accepted label back with `VM::set_instance_host_field`.
pub type InstanceLabelHook = Rc<dyn Fn(InstanceId, &Value)>;

/// Reserved DAG namespace prefix for instance field sources.
pub const INSTANCE_NAMESPACE_PREFIX: &str = "%instance/";

/// Host-owned field names every instance answers, in this order.
pub const INSTANCE_HOST_FIELDS: [&str; 4] = ["id", "kind", "owner", "label"];

/// The host fields whose value the host pushes (`id`/`kind` are fixed by the
/// record).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstanceHostField {
    Owner,
    Label,
}

impl InstanceHostField {
    pub fn name(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Label => "label",
        }
    }
}

/// Ordered `:state` fields of one kind with their default values.
#[derive(Clone, Debug)]
pub struct InstanceKindSchema {
    /// Kind id, `<package name>:<kind name>` (spec §5).
    pub kind: String,
    /// `(field default)` pairs, in declaration order.
    pub fields: Vec<(String, Value)>,
    /// The kind's `:view`, a function of one argument (the instance). The
    /// host renders `(view self)` per instance; `None` for a view-less kind.
    pub view: Option<Value>,
    /// The kind's optional `:keymap`: the mode name the host gives every
    /// instance's view buffer (what `set-buffer-mode-for` did by hand).
    pub keymap: Option<String>,
    /// The kind's optional `:on-create`, a function of one argument (the
    /// instance) the host calls once when the user creates a fresh instance
    /// (not on duplicate, kit load, migration or project open): the place
    /// for document defaults the `:sequencer` body cannot express, such as
    /// a weight pattern computed from `from`/`to` (spec §11).
    pub on_create: Option<Value>,
}

impl InstanceKindSchema {
    pub fn new(kind: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            fields: Vec::new(),
            view: None,
            keymap: None,
            on_create: None,
        }
    }

    pub fn with_on_create(mut self, on_create: Option<Value>) -> Self {
        self.on_create = on_create;
        self
    }

    pub fn with_view(mut self, view: Option<Value>) -> Self {
        self.view = view;
        self
    }

    pub fn with_keymap(mut self, keymap: Option<String>) -> Self {
        self.keymap = keymap;
        self
    }

    pub fn field(mut self, name: impl Into<String>, default: Value) -> Self {
        self.fields.push((name.into(), default));
        self
    }

    fn index_of(&self, field: &str) -> Option<usize> {
        self.fields.iter().position(|(name, _)| name == field)
    }

    fn default_of(&self, index: usize) -> Value {
        self.fields
            .get(index)
            .map(|(_, default)| default.deep_clone())
            .unwrap_or(Value::Nil)
    }

    /// The checks [`VM::register_instance_kind`] applies: a non-empty kind
    /// id, and field names that are non-empty, free of `.`, unique, and
    /// never a host field. Hosts run it before recording a kind anywhere
    /// else, so a schema the VM would reject is never half-registered.
    pub fn validate(&self) -> Result<(), InstanceError> {
        if self.kind.is_empty() {
            return Err(InstanceError::InvalidSchema("empty kind id".to_string()));
        }
        let mut seen = std::collections::HashSet::new();
        for (name, _) in &self.fields {
            if INSTANCE_HOST_FIELDS.contains(&name.as_str()) {
                return Err(InstanceError::InvalidSchema(format!(
                    "kind '{}' declares state field '{name}', which is a host field",
                    self.kind
                )));
            }
            if name.is_empty() || name.contains('.') {
                return Err(InstanceError::InvalidSchema(format!(
                    "kind '{}' declares invalid field name '{name}'",
                    self.kind
                )));
            }
            if !seen.insert(name.clone()) {
                return Err(InstanceError::InvalidSchema(format!(
                    "kind '{}' declares field '{name}' twice",
                    self.kind
                )));
            }
        }
        Ok(())
    }

    /// Every readable field name, host fields first.
    pub fn all_field_names(&self) -> Vec<String> {
        INSTANCE_HOST_FIELDS
            .iter()
            .map(|name| (*name).to_string())
            .chain(self.fields.iter().map(|(name, _)| name.clone()))
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum InstanceError {
    UnknownKind(String),
    UnknownInstance(InstanceId),
    DuplicateInstance(InstanceId),
    UnknownField {
        kind: String,
        field: String,
        fields: Vec<String>,
    },
    ReadOnlyField {
        kind: String,
        field: String,
    },
    InvalidSchema(String),
}

impl std::fmt::Display for InstanceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownKind(kind) => write!(f, "unknown instance kind '{kind}'"),
            Self::UnknownInstance(id) => write!(f, "unknown instance {id}"),
            Self::DuplicateInstance(id) => write!(f, "instance {id} already exists"),
            Self::UnknownField {
                kind,
                field,
                fields,
            } => write!(
                f,
                "kind '{kind}' has no field '{field}'; fields: {}",
                fields.join(", ")
            ),
            Self::ReadOnlyField { kind, field } => {
                write!(f, "field '{field}' of kind '{kind}' is read-only")
            }
            Self::InvalidSchema(message) => write!(f, "invalid instance kind schema: {message}"),
        }
    }
}

impl From<InstanceError> for VMError {
    fn from(error: InstanceError) -> Self {
        VMError::Instance(error.to_string())
    }
}

#[derive(Clone)]
struct InstanceRecord {
    kind: String,
    /// One value per schema field, in schema order.
    state: Vec<Value>,
    owner: Value,
    label: Value,
}

/// Which cell a field name resolves to.
#[derive(Clone, Copy)]
enum FieldSlot {
    Id,
    Kind,
    Owner,
    Label,
    State(usize),
}

#[derive(Default)]
pub(crate) struct InstanceStore {
    kinds: HashMap<String, InstanceKindSchema>,
    live: HashMap<InstanceId, InstanceRecord>,
    /// Dropped instance -> its kind, so a stale handle still reads defaults.
    dropped: HashMap<InstanceId, String>,
    label_hook: Option<InstanceLabelHook>,
}

impl InstanceStore {
    /// Snapshot for transactional eval rollback. Values are deep-copied so a
    /// failed eval cannot mutate the snapshot through shared cells; the label
    /// hook is host wiring, not eval state, and is carried over.
    pub(crate) fn snapshot(&self) -> Self {
        Self {
            kinds: self
                .kinds
                .iter()
                .map(|(name, schema)| {
                    let mut schema = schema.clone();
                    for (_, default) in &mut schema.fields {
                        *default = clone_value_for_snapshot(default);
                    }
                    if let Some(view) = &mut schema.view {
                        *view = clone_value_for_snapshot(view);
                    }
                    if let Some(on_create) = &mut schema.on_create {
                        *on_create = clone_value_for_snapshot(on_create);
                    }
                    (name.clone(), schema)
                })
                .collect(),
            live: self
                .live
                .iter()
                .map(|(id, record)| {
                    (
                        *id,
                        InstanceRecord {
                            kind: record.kind.clone(),
                            state: record.state.iter().map(clone_value_for_snapshot).collect(),
                            owner: clone_value_for_snapshot(&record.owner),
                            label: clone_value_for_snapshot(&record.label),
                        },
                    )
                })
                .collect(),
            dropped: self.dropped.clone(),
            label_hook: self.label_hook.clone(),
        }
    }

    /// Roll back to `snapshot`, keeping the host's current label hook.
    pub(crate) fn restore_from(&mut self, snapshot: Self) {
        let hook = self.label_hook.take();
        *self = snapshot;
        self.label_hook = hook;
    }

    fn kind_of(&self, id: InstanceId) -> Option<&str> {
        self.live
            .get(&id)
            .map(|record| record.kind.as_str())
            .or_else(|| self.dropped.get(&id).map(String::as_str))
    }

    fn resolve(&self, id: InstanceId, field: &str) -> Result<(String, FieldSlot), InstanceError> {
        let kind = self
            .kind_of(id)
            .ok_or(InstanceError::UnknownInstance(id))?
            .to_string();
        let slot = match field {
            "id" => FieldSlot::Id,
            "kind" => FieldSlot::Kind,
            "owner" => FieldSlot::Owner,
            "label" => FieldSlot::Label,
            _ => {
                let schema = self
                    .kinds
                    .get(&kind)
                    .ok_or_else(|| InstanceError::UnknownKind(kind.clone()))?;
                let index = schema
                    .index_of(field)
                    .ok_or_else(|| InstanceError::UnknownField {
                        kind: kind.clone(),
                        field: field.to_string(),
                        fields: schema.all_field_names(),
                    })?;
                FieldSlot::State(index)
            }
        };
        Ok((kind, slot))
    }

    /// Current value of a resolved slot: the live cell, or the default for a
    /// dropped instance.
    fn value(&self, id: InstanceId, kind: &str, slot: FieldSlot) -> Value {
        let record = self.live.get(&id);
        match slot {
            FieldSlot::Id => Value::Number(id as f64),
            FieldSlot::Kind => Value::String(kind.to_string()),
            FieldSlot::Owner => record.map(|r| r.owner.clone()).unwrap_or(Value::Nil),
            FieldSlot::Label => record.map(|r| r.label.clone()).unwrap_or(Value::Nil),
            FieldSlot::State(index) => match record.and_then(|r| r.state.get(index)) {
                Some(value) => value.clone(),
                None => self
                    .kinds
                    .get(kind)
                    .map(|schema| schema.default_of(index))
                    .unwrap_or(Value::Nil),
            },
        }
    }
}

pub(crate) fn instance_namespace(id: InstanceId) -> String {
    format!("{INSTANCE_NAMESPACE_PREFIX}{id}")
}

impl VM {
    // ---- host API -------------------------------------------------------

    /// Register (or re-register, on hot reload) a kind's `:state` schema.
    /// Live instances of the kind keep their values by field name; new fields
    /// start at their default and removed fields are dropped (spec §5).
    pub fn register_instance_kind(&mut self, schema: InstanceKindSchema) -> Result<(), InstanceError> {
        schema.validate()?;
        let previous = self.instances.kinds.insert(schema.kind.clone(), schema.clone());
        // A (re)registered kind may carry a new `:view`: every bound view
        // buffer of its instances re-renders through it.
        self.mark_instance_views_of_kind_dirty(&schema.kind);
        if let Some(previous) = previous {
            let ids: Vec<InstanceId> = self
                .instances
                .live
                .iter()
                .filter(|(_, record)| record.kind == schema.kind)
                .map(|(id, _)| *id)
                .collect();
            for id in ids {
                let Some(record) = self.instances.live.get_mut(&id) else {
                    continue;
                };
                let old_state = std::mem::take(&mut record.state);
                record.state = schema
                    .fields
                    .iter()
                    .map(|(name, default)| {
                        previous
                            .index_of(name)
                            .and_then(|old| old_state.get(old).cloned())
                            .unwrap_or_else(|| default.deep_clone())
                    })
                    .collect();
                // A newly declared field may already have (error-state)
                // readers; bring every retained source up to date.
                for (index, (name, _)) in schema.fields.iter().enumerate() {
                    let value = self.instances.value(id, &schema.kind, FieldSlot::State(index));
                    self.publish_instance_field(id, name, value);
                }
            }
        }
        Ok(())
    }

    pub fn instance_kind_schema(&self, kind: &str) -> Option<&InstanceKindSchema> {
        self.instances.kinds.get(kind)
    }

    /// Every kind id this VM holds a schema for, sorted.
    pub fn instance_kind_ids(&self) -> Vec<String> {
        let mut kinds: Vec<String> = self.instances.kinds.keys().cloned().collect();
        kinds.sort_unstable();
        kinds
    }

    /// Create the cells of a new instance with its kind's defaults and return
    /// the Lisp handle. Reusing a dropped id revives it with fresh defaults.
    pub fn create_instance(&mut self, id: InstanceId, kind: &str) -> Result<Value, InstanceError> {
        if self.instances.live.contains_key(&id) {
            return Err(InstanceError::DuplicateInstance(id));
        }
        let schema = self
            .instances
            .kinds
            .get(kind)
            .ok_or_else(|| InstanceError::UnknownKind(kind.to_string()))?;
        let record = InstanceRecord {
            kind: kind.to_string(),
            state: schema
                .fields
                .iter()
                .map(|(_, default)| default.deep_clone())
                .collect(),
            owner: Value::Nil,
            label: Value::Nil,
        };
        self.instances.dropped.remove(&id);
        self.instances.live.insert(id, record);
        // Readers of a previously dropped id see the revived values.
        self.republish_instance_sources(id);
        Ok(Value::Instance(id))
    }

    /// Drop an instance. Its handle turns stale: readers are dirtied and see
    /// defaults, writes become no-ops. Returns whether it was live.
    pub fn drop_instance(&mut self, id: InstanceId) -> bool {
        let Some(record) = self.instances.live.remove(&id) else {
            return false;
        };
        self.instances.dropped.insert(id, record.kind);
        self.republish_instance_sources(id);
        true
    }

    pub fn instance_is_live(&self, id: InstanceId) -> bool {
        self.instances.live.contains_key(&id)
    }

    pub fn instance_kind(&self, id: InstanceId) -> Option<&str> {
        self.instances.kind_of(id)
    }

    /// Live instance ids, sorted.
    pub fn live_instances(&self) -> Vec<InstanceId> {
        let mut ids: Vec<InstanceId> = self.instances.live.keys().copied().collect();
        ids.sort_unstable();
        ids
    }

    /// Push a host-owned field value (`owner`, `label`) and dirty its readers.
    pub fn set_instance_host_field(
        &mut self,
        id: InstanceId,
        field: InstanceHostField,
        value: Value,
    ) -> Result<(), InstanceError> {
        let record = self
            .instances
            .live
            .get_mut(&id)
            .ok_or(InstanceError::UnknownInstance(id))?;
        let stored = value.deep_clone();
        match field {
            InstanceHostField::Owner => record.owner = stored,
            InstanceHostField::Label => record.label = stored,
        }
        self.publish_instance_field(id, field.name(), value);
        Ok(())
    }

    /// Route Lisp label writes to the host (`None` = write the label cell
    /// locally).
    pub fn set_instance_label_hook(&mut self, hook: Option<InstanceLabelHook>) {
        self.instances.label_hook = hook;
    }

    /// Untracked read of any field, for the host.
    pub fn instance_field(&self, id: InstanceId, field: &str) -> Result<Value, InstanceError> {
        let (kind, slot) = self.instances.resolve(id, field)?;
        Ok(self.instances.value(id, &kind, slot))
    }

    /// Write a field exactly as Lisp `(set! x.field v)` would (for example to
    /// seed per-instance evaluated defaults).
    pub fn set_instance_field(
        &mut self,
        id: InstanceId,
        field: &str,
        value: Value,
    ) -> Result<(), InstanceError> {
        self.write_instance_field(id, field, value)
    }

    /// The kind's `:view` to render a live instance with (spec §7): `None`
    /// for a stale or never-created instance (its bound buffer renders
    /// empty until the host unbinds it), an error when the kind has no
    /// `:view` or its schema is gone.
    pub(super) fn instance_view_callable(
        &self,
        id: InstanceId,
    ) -> Result<Option<Value>, InstanceError> {
        let Some(record) = self.instances.live.get(&id) else {
            return Ok(None);
        };
        let schema = self
            .instances
            .kinds
            .get(&record.kind)
            .ok_or_else(|| InstanceError::UnknownKind(record.kind.clone()))?;
        match &schema.view {
            Some(view) => Ok(Some(view.clone())),
            None => Err(InstanceError::InvalidSchema(format!(
                "kind '{}' has no :view",
                record.kind
            ))),
        }
    }

    /// Live instance ids of `kind`, sorted.
    pub(super) fn live_instances_of_kind(&self, kind: &str) -> Vec<InstanceId> {
        let mut ids: Vec<InstanceId> = self
            .instances
            .live
            .iter()
            .filter(|(_, record)| record.kind == kind)
            .map(|(id, _)| *id)
            .collect();
        ids.sort_unstable();
        ids
    }

    // ---- VM internals ---------------------------------------------------

    /// `GetField` on an instance: resolve, read, and record a dependency of
    /// the running effect/derived on this one (instance, field) cell.
    pub(super) fn read_instance_field_tracked(
        &mut self,
        id: InstanceId,
        field: &str,
    ) -> Result<Value, VMError> {
        if self.active_expander.is_some() {
            return Err(self.expansion_error("instance field read"));
        }
        let (kind, slot) = self.instances.resolve(id, field)?;
        let value = self.instances.value(id, &kind, slot);
        let namespace = instance_namespace(id);
        self.record_reactive_read(&namespace, field);
        if let Some(ctx_id) = self.tracking_stack.last().copied() {
            let source_id = self.get_or_create_instance_source_node(&namespace, field, &value);
            self.dag.add_edge(source_id, ctx_id);
        }
        Ok(value)
    }

    /// `StoreField` on an instance.
    pub(super) fn write_instance_field(
        &mut self,
        id: InstanceId,
        field: &str,
        value: Value,
    ) -> Result<(), InstanceError> {
        let (kind, slot) = self.instances.resolve(id, field)?;
        if !self.instances.live.contains_key(&id) {
            // Stale handle: an event handler outliving its instance.
            return Ok(());
        }
        match slot {
            FieldSlot::Id | FieldSlot::Kind | FieldSlot::Owner => {
                return Err(InstanceError::ReadOnlyField {
                    kind,
                    field: field.to_string(),
                });
            }
            FieldSlot::Label => {
                if let Some(hook) = self.instances.label_hook.clone() {
                    hook(id, &value);
                    return Ok(());
                }
                return self.set_instance_host_field(id, InstanceHostField::Label, value);
            }
            FieldSlot::State(index) => {
                if let Some(cell) = self
                    .instances
                    .live
                    .get_mut(&id)
                    .and_then(|record| record.state.get_mut(index))
                {
                    *cell = value.deep_clone();
                }
            }
        }
        self.publish_instance_field(id, field, value);
        Ok(())
    }

    fn get_or_create_instance_source_node(
        &mut self,
        namespace: &str,
        field: &str,
        value: &Value,
    ) -> super::NodeId {
        if let Some(id) = self.dag.find_namespace_field_source_node(namespace, field) {
            return id;
        }
        let id = self.dag.alloc_id();
        self.dag.add_node(ReactiveNode::Source {
            id,
            source: ReactiveSource::NamespaceField {
                namespace: namespace.to_string(),
                field: field.to_string(),
            },
            value: value.deep_clone(),
            dependents: std::collections::HashSet::new(),
        });
        id
    }

    /// Advance one field's source (if anything ever read it) and dirty only
    /// its readers. Unread fields allocate nothing.
    fn publish_instance_field(&mut self, id: InstanceId, field: &str, value: Value) {
        let namespace = instance_namespace(id);
        if let Some(source_id) = self.dag.find_namespace_field_source_node(&namespace, field) {
            self.mark_source_dependents_dirty(source_id, value);
        }
    }

    /// Re-publish every retained source of an instance from the store (after
    /// create/drop changed what each field answers).
    fn republish_instance_sources(&mut self, id: InstanceId) {
        let namespace = instance_namespace(id);
        let Some(fields) = self.dag.namespace_field_sources.get(&namespace) else {
            return;
        };
        let fields: Vec<String> = fields.keys().cloned().collect();
        for field in fields {
            let value = match self.instances.resolve(id, &field) {
                Ok((kind, slot)) => self.instances.value(id, &kind, slot),
                Err(_) => Value::Nil,
            };
            self.publish_instance_field(id, &field, value);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::super::{EffectTarget, PendingUiUpdate, VM, VMError, Value};
    use super::{InstanceHostField, InstanceKindSchema};

    const KIND: &str = "test/pkg:probe";

    fn instance_vm() -> VM {
        let mut vm = VM::new(Vec::new());
        super::super::register_core_natives(&mut vm);
        crate::widgets::register_widget_natives(&mut vm);
        vm.register_instance_kind(
            InstanceKindSchema::new(KIND)
                .field("x", Value::Number(1.0))
                .field("y", Value::Number(2.0)),
        )
        .expect("register kind");
        vm
    }

    fn bind(vm: &mut VM, name: &str, id: u64) {
        let handle = vm.create_instance(id, KIND).expect("create instance");
        vm.set_global_value(name, handle);
    }

    /// Named effect-buffer targets rendered since the last clear, sorted.
    fn rendered_targets(vm: &mut VM) -> Vec<String> {
        let mut targets: Vec<String> = vm
            .pending_widget_trees
            .drain(..)
            .filter_map(|update| match update {
                PendingUiUpdate::FullTree(tree) => match tree.target {
                    EffectTarget::BufferName(name) => Some(name),
                    _ => None,
                },
                PendingUiUpdate::ReplaceSubtree { target, .. } => match target {
                    EffectTarget::BufferName(name) => Some(name),
                    _ => None,
                },
            })
            .collect();
        targets.sort();
        targets
    }

    fn eval(vm: &mut VM, code: &str) -> Option<Value> {
        vm.eval_str(code).unwrap_or_else(|e| panic!("{code}: {e:?}"))
    }

    #[test]
    fn tracked_field_write_rerenders_only_the_dependent_effect() {
        let mut vm = instance_vm();
        bind(&mut vm, "a", 1);
        eval(
            &mut vm,
            r#"
            (def show-x (s) (box :width s.x :height 1))
            (effect-buffer "*x*" (show-x a))
            (effect-buffer "*y*" (box :width a.y :height 1))
            "#,
        );
        assert_eq!(rendered_targets(&mut vm), vec!["*x*", "*y*"]);

        // A Lisp write through the existing dotted set! path.
        assert_eq!(eval(&mut vm, "(set! a.x 5)"), Some(Value::Number(5.0)));
        assert_eq!(rendered_targets(&mut vm), vec!["*x*"]);
        assert_eq!(eval(&mut vm, "a.x"), Some(Value::Number(5.0)));

        // Writing an unchanged value dirties nothing.
        eval(&mut vm, "(set! a.x 5)");
        assert!(rendered_targets(&mut vm).is_empty());

        // A host write reaches the same cell and only its reader.
        vm.set_instance_field(1, "y", Value::Number(7.0))
            .expect("host write");
        vm.process_dirty_reactive().expect("process");
        assert_eq!(rendered_targets(&mut vm), vec!["*y*"]);
        assert_eq!(eval(&mut vm, "a.y"), Some(Value::Number(7.0)));
    }

    #[test]
    fn a_closure_captures_its_instance_and_writes_it_later() {
        let mut vm = instance_vm();
        bind(&mut vm, "a", 1);
        eval(
            &mut vm,
            r#"
            (def make-setter (s) (lambda (v) (set! s.y v)))
            (def setter (make-setter a))
            (effect-buffer "*y*" (box :width a.y :height 1))
            "#,
        );
        rendered_targets(&mut vm);
        eval(&mut vm, "(setter 9)");
        assert_eq!(rendered_targets(&mut vm), vec!["*y*"]);
        assert_eq!(vm.instance_field(1, "y"), Ok(Value::Number(9.0)));
    }

    #[test]
    fn two_instances_of_one_kind_are_isolated() {
        let mut vm = instance_vm();
        bind(&mut vm, "a", 1);
        bind(&mut vm, "b", 2);
        eval(
            &mut vm,
            r#"
            (effect-buffer "*a*" (box :width a.x :height 1))
            (effect-buffer "*b*" (box :width b.x :height 1))
            "#,
        );
        rendered_targets(&mut vm);
        eval(&mut vm, "(set! a.x 3)");
        assert_eq!(rendered_targets(&mut vm), vec!["*a*"]);
        assert_eq!(eval(&mut vm, "a.x"), Some(Value::Number(3.0)));
        assert_eq!(eval(&mut vm, "b.x"), Some(Value::Number(1.0)));
        eval(&mut vm, "(set! b.x 4)");
        assert_eq!(rendered_targets(&mut vm), vec!["*b*"]);
        assert_eq!(eval(&mut vm, "a.x"), Some(Value::Number(3.0)));
        assert_eq!(eval(&mut vm, "(= a b)"), Some(Value::Bool(false)));
    }

    #[test]
    fn unknown_field_errors_name_the_kind_and_its_fields() {
        let mut vm = instance_vm();
        bind(&mut vm, "a", 1);
        let expected = format!(
            "kind '{KIND}' has no field 'nope'; fields: id, kind, owner, label, x, y"
        );
        assert_eq!(
            vm.eval_str("a.nope"),
            Err(VMError::Instance(expected.clone()))
        );
        assert_eq!(
            vm.eval_str("(set! a.nope 1)"),
            Err(VMError::Instance(expected))
        );
        for field in ["id", "kind", "owner"] {
            let Err(VMError::Instance(message)) = vm.eval_str(&format!("(set! a.{field} 1)"))
            else {
                panic!("{field} must be read-only");
            };
            assert!(message.contains("read-only"), "{message}");
            assert!(message.contains(KIND), "{message}");
        }
    }

    #[test]
    fn stale_instance_reads_defaults_and_ignores_writes() {
        let mut vm = instance_vm();
        bind(&mut vm, "a", 1);
        eval(
            &mut vm,
            r#"
            (def poke (v) (set! a.x v))
            (effect-buffer "*x*" (box :width a.x :height 1))
            "#,
        );
        eval(&mut vm, "(set! a.x 8)");
        vm.set_instance_host_field(1, InstanceHostField::Label, Value::String("Kit A".into()))
            .expect("label");
        rendered_targets(&mut vm);

        assert!(vm.drop_instance(1));
        assert!(!vm.drop_instance(1));
        vm.process_dirty_reactive().expect("process");
        // Readers are dirtied back to the default.
        assert_eq!(rendered_targets(&mut vm), vec!["*x*"]);
        assert_eq!(eval(&mut vm, "a.x"), Some(Value::Number(1.0)));
        assert_eq!(eval(&mut vm, "a.label"), Some(Value::Nil));
        assert_eq!(eval(&mut vm, "a.kind"), Some(Value::String(KIND.into())));
        // A late event handler's write is a silent no-op.
        eval(&mut vm, "(poke 9)");
        assert!(rendered_targets(&mut vm).is_empty());
        assert_eq!(eval(&mut vm, "a.x"), Some(Value::Number(1.0)));
        // Unknown fields still error on a stale handle.
        assert!(matches!(vm.eval_str("a.nope"), Err(VMError::Instance(_))));
        // A never-created id is an error, not a default.
        vm.set_global_value("ghost", Value::Instance(99));
        assert!(matches!(vm.eval_str("ghost.x"), Err(VMError::Instance(_))));
    }

    #[test]
    fn instance_ref_returns_live_handles_only() {
        let mut vm = instance_vm();
        bind(&mut vm, "a", 3);
        assert_eq!(eval(&mut vm, "(instance-ref 3)"), Some(Value::Instance(3)));
        assert_eq!(eval(&mut vm, "(let ((i (instance-ref 3))) i.y)"), Some(Value::Number(2.0)));
        assert_eq!(eval(&mut vm, "(instance-ref 4)"), Some(Value::Nil), "never created");
        assert_eq!(eval(&mut vm, "(instance-ref 3.5)"), Some(Value::Nil));
        assert!(vm.drop_instance(3));
        assert_eq!(eval(&mut vm, "(instance-ref 3)"), Some(Value::Nil), "dropped");
    }

    #[test]
    fn host_fields_are_tracked_and_label_writes_route_through_the_hook() {
        let mut vm = instance_vm();
        bind(&mut vm, "a", 4);
        assert_eq!(eval(&mut vm, "a.id"), Some(Value::Number(4.0)));
        assert_eq!(eval(&mut vm, "a.kind"), Some(Value::String(KIND.into())));
        assert_eq!(eval(&mut vm, "a.owner"), Some(Value::Nil));
        eval(
            &mut vm,
            r#"(effect-buffer "*label*" (label a.label))
               (effect-buffer "*owner*" (label a.owner))"#,
        );
        rendered_targets(&mut vm);

        vm.set_instance_host_field(4, InstanceHostField::Owner, Value::String("Kit A".into()))
            .expect("owner");
        vm.process_dirty_reactive().expect("process");
        assert_eq!(rendered_targets(&mut vm), vec!["*owner*"]);

        // No hook: the label cell is written locally.
        eval(&mut vm, r#"(set! a.label "first")"#);
        assert_eq!(rendered_targets(&mut vm), vec!["*label*"]);
        assert_eq!(vm.instance_field(4, "label"), Ok(Value::String("first".into())));

        // With a hook, the write goes to the host, which pushes it back.
        let renames: Rc<RefCell<Vec<(u64, Value)>>> = Rc::default();
        let sink = renames.clone();
        vm.set_instance_label_hook(Some(Rc::new(move |id, value| {
            sink.borrow_mut().push((id, value.clone()));
        })));
        eval(&mut vm, r#"(set! a.label "second")"#);
        assert_eq!(*renames.borrow(), vec![(4, Value::String("second".into()))]);
        assert!(rendered_targets(&mut vm).is_empty());
        assert_eq!(vm.instance_field(4, "label"), Ok(Value::String("first".into())));
        vm.set_instance_host_field(4, InstanceHostField::Label, Value::String("second".into()))
            .expect("push label");
        vm.process_dirty_reactive().expect("process");
        assert_eq!(rendered_targets(&mut vm), vec!["*label*"]);
    }

    #[test]
    fn re_registering_a_kind_keeps_values_by_name() {
        let mut vm = instance_vm();
        bind(&mut vm, "a", 1);
        eval(&mut vm, "(set! a.y 5)");
        vm.register_instance_kind(
            InstanceKindSchema::new(KIND)
                .field("z", Value::Number(3.0))
                .field("y", Value::Number(0.0)),
        )
        .expect("re-register");
        assert_eq!(eval(&mut vm, "a.y"), Some(Value::Number(5.0)));
        assert_eq!(eval(&mut vm, "a.z"), Some(Value::Number(3.0)));
        assert!(matches!(vm.eval_str("a.x"), Err(VMError::Instance(_))));
    }

    #[test]
    fn schema_rejects_host_field_names_and_duplicates() {
        let mut vm = instance_vm();
        assert!(
            vm.register_instance_kind(InstanceKindSchema::new("k").field("label", Value::Nil))
                .is_err()
        );
        assert!(
            vm.register_instance_kind(
                InstanceKindSchema::new("k")
                    .field("a", Value::Nil)
                    .field("a", Value::Nil)
            )
            .is_err()
        );
        assert!(vm.create_instance(1, "missing").is_err());
        vm.create_instance(1, KIND).expect("create");
        assert!(vm.create_instance(1, KIND).is_err());
    }
}

/// `def-kind` compilation, the native-context kind registration and the
/// host label route, driven through a whole `Runtime` (instance-kinds spec
/// §3, §4).
#[cfg(test)]
mod runtime_tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use crate::host::HostCommand;
    use crate::runtime::Runtime;
    use crate::vm::Value;

    type Captured = Rc<RefCell<Vec<Vec<Value>>>>;

    /// A stand-in `def-kind` native that records its evaluated arguments and
    /// registers the `:state`/`:view` schema like the host's does.
    fn runtime_with_def_kind() -> (Runtime, Captured) {
        let mut runtime = Runtime::new();
        let captured: Captured = Rc::new(RefCell::new(Vec::new()));
        let sink = captured.clone();
        runtime.register_native("def-kind", move |args, ctx| {
            sink.borrow_mut().push(args.clone());
            let name = match args.first() {
                Some(Value::Symbol(name)) => name.clone(),
                other => return Err(format!("expected a symbol name, got {other:?}")),
            };
            let mut schema = super::InstanceKindSchema::new(format!("test/pkg:{name}"));
            let mut idx = 1;
            while idx + 1 < args.len() {
                match (&args[idx], &args[idx + 1]) {
                    (Value::Keyword(key), Value::List(pairs)) if key == "state" => {
                        for pair in pairs {
                            let Value::List(pair) = &*pair.borrow() else {
                                return Err("state pair".to_string());
                            };
                            let Value::Symbol(field) = &*pair[0].borrow() else {
                                return Err("state field".to_string());
                            };
                            schema = schema.field(field.clone(), pair[1].borrow().clone());
                        }
                    }
                    (Value::Keyword(key), view) if key == "view" => {
                        schema = schema.with_view(Some(view.clone()));
                    }
                    _ => {}
                }
                idx += 2;
            }
            ctx.register_instance_kind(schema);
            Ok(Value::String(format!("test/pkg:{name}")))
        });
        (runtime, captured)
    }

    fn list_items(value: &Value) -> Vec<Value> {
        match value {
            Value::List(items) => items.iter().map(|item| item.borrow().clone()).collect(),
            other => panic!("expected a list, got {other:?}"),
        }
    }

    #[test]
    fn def_kind_captures_sequencer_as_data_and_evaluates_state_defaults() {
        let (mut runtime, captured) = runtime_with_def_kind();
        runtime
            .eval_str(
                "(def base 3)
                 (def gvr-panel (lambda (self) self.sel))
                 (def-kind neural
                   :sequencer (:shape (line 4) :max-poly ,(+ base 1)
                               (def-node nrn :update (if (> 1 0) (emit :note 1) nil))
                               (edges :from nrn :to nrn))
                   :state ((sel (- base 4)) (open) (name \"x\"))
                   :view gvr-panel)",
            )
            .expect("def-kind evaluates");
        let calls = captured.borrow();
        assert_eq!(calls.len(), 1);
        let args = &calls[0];
        assert_eq!(args[0], Value::Symbol("neural".to_string()));
        assert_eq!(args[1], Value::Keyword("sequencer".to_string()));
        let body = list_items(&args[2]);
        // A top-level `,x` escapes to evaluation (as in graph-mode
        // def-sequencer); everything else arrives as data, never called here
        // (`def-node`, `line`, `if` would fail if evaluated).
        assert_eq!(body[0], Value::Keyword("shape".to_string()));
        let shape = list_items(&body[1]);
        assert_eq!(shape[0], Value::Symbol("line".to_string()));
        assert_eq!(shape[1], Value::Number(4.0));
        assert_eq!(body[2], Value::Keyword("max-poly".to_string()));
        assert_eq!(body[3], Value::Number(4.0));
        let node = list_items(&body[4]);
        assert_eq!(node[0], Value::Symbol("def-node".to_string()));
        let edges = list_items(&body[5]);
        assert_eq!(edges[0], Value::Symbol("edges".to_string()));
        assert_eq!(args[3], Value::Keyword("state".to_string()));
        let state = list_items(&args[4]);
        assert_eq!(
            list_items(&state[0]),
            vec![Value::Symbol("sel".to_string()), Value::Number(-1.0)]
        );
        assert_eq!(list_items(&state[1]), vec![Value::Symbol("open".to_string()), Value::Nil]);
        assert_eq!(args[5], Value::Keyword("view".to_string()));
        drop(calls);

        let schema = runtime
            .instance_kind_schema("test/pkg:neural")
            .expect("the native registered the schema in the evaluating VM");
        assert_eq!(
            schema.fields.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>(),
            vec!["sel", "open", "name"]
        );
        assert!(schema.view.is_some());

        let handle = runtime.create_instance(7, "test/pkg:neural").expect("create");
        runtime.set_global_value("inst", handle);
        assert_eq!(
            runtime.eval_str("(gvr-panel inst)").expect("view reads a field"),
            Some(Value::Number(-1.0))
        );
    }

    #[test]
    fn def_kind_rejects_malformed_state() {
        let (mut runtime, _) = runtime_with_def_kind();
        assert!(runtime.eval_str("(def-kind broken :state ((1 2)))").is_err());
    }

    #[test]
    fn label_writes_route_to_a_host_command_when_asked() {
        let (mut runtime, _) = runtime_with_def_kind();
        runtime.eval_str("(def-kind probe :state ((x 1)))").expect("kind");
        let handle = runtime.create_instance(3, "test/pkg:probe").expect("create");
        runtime.set_global_value("inst", handle);
        runtime.route_instance_labels_to_host_command("instance-rename");
        runtime.eval_str("(set! inst.label \"Kit A\")").expect("rename");
        let renames: Vec<_> = runtime
            .drain_host_commands()
            .into_iter()
            .filter_map(|command| match command {
                HostCommand::Custom { name, payload } if name == "instance-rename" => {
                    Some(payload)
                }
                _ => None,
            })
            .collect();
        assert_eq!(renames.len(), 1);
        let Value::Map(payload) = &renames[0] else {
            panic!("rename payload is a map");
        };
        assert_eq!(*payload["id"].borrow(), Value::Number(3.0));
        assert_eq!(*payload["label"].borrow(), Value::String("Kit A".to_string()));
        // The host owns the label: nothing changes until it pushes one back.
        assert_eq!(runtime.instance_field(3, "label"), Ok(Value::Nil));
    }
}
