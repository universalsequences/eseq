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
//! Declared fields are typed (docs/kind-bindings-spec.md §3.3, [`FieldType`]):
//! the type is inferred from the default or declared, and every write is
//! checked against it. A singleton kind (`:key ()`, §3.1) has exactly one
//! instance, created by `def-kind` and bound to the kind's name; its
//! built-in fields are only `id` and `kind`, and host enumerations
//! (`live_instances`, `instance_kind_ids`) leave it out.
//!
//! A dropped instance keeps a tombstone naming its kind: reads answer the
//! kind's defaults, writes are silent no-ops (spec §4 "stale self").
//!
//! A kind's `:document` fields (docs/jaki-kind-spec.md §3) read and write
//! with the same syntax but are stored by the host, per pattern: a read calls
//! the host native [`INSTANCE_DOC_READ_NATIVE`] and a write
//! [`INSTANCE_DOC_WRITE_NATIVE`], which carry their own reactive edge and
//! history. A VM without those natives keeps document fields in local cells,
//! exactly like `:state`.

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

/// Host native `(__instance-doc-read id field default)` backing document
/// field reads. It tracks its own reactive dependency.
pub const INSTANCE_DOC_READ_NATIVE: &str = "__instance-doc-read";

/// Host native `(__instance-doc-write id field value)` backing document
/// field writes. It dirties its own readers and records history.
pub const INSTANCE_DOC_WRITE_NATIVE: &str = "__instance-doc-write";

/// The native `(def-kind name :key () ...)` compiles to (kind-bindings
/// spec §3.1); it returns the singleton instance the compiler then binds to
/// the kind's name.
pub const DEF_SINGLETON_KIND_NATIVE: &str = "__def-singleton-kind";

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

/// The built-in fields of a singleton kind (kind-bindings spec §3.1): no
/// `owner`/`label`, which only host-created instances have.
pub(crate) const SINGLETON_BUILTIN_FIELDS: [&str; 2] = ["id", "kind"];

/// Singleton instances take ids from here up, one per singleton kind, so
/// they never collide with host-assigned instance ids (which count up from
/// 1) and stay exact as a Lisp number. Only allocation uses it: whether an
/// instance is a singleton is its kind's [`InstanceKindSchema::singleton`].
pub(crate) const SINGLETON_INSTANCE_ID_BASE: InstanceId = 1 << 48;

/// Kind id prefix for kinds defined in headerless (scratch) code.
pub const SCRATCH_KIND_PACKAGE: &str = "scratch";

/// `<package name>:<kind name>`. Kinds outside any installed package fall
/// back to their module (`<module>:<kind>`), and headerless code to
/// `scratch:<kind>`, so a kind id is never ambiguous with a package's.
pub fn kind_id(package: Option<&str>, module: Option<&str>, name: &str) -> String {
    match (package, module) {
        (Some(package), _) => format!("{package}:{name}"),
        (None, Some(module)) => format!("{module}:{name}"),
        (None, None) => format!("{SCRATCH_KIND_PACKAGE}:{name}"),
    }
}

/// The kind name part of a kind id (`alez/neural:neural` -> `neural`).
pub fn kind_name_of(kind_id: &str) -> &str {
    kind_id
        .rsplit_once(':')
        .map(|(_, name)| name)
        .unwrap_or(kind_id)
}

pub(crate) const FIELD_TYPES_HINT: &str =
    "types are :number :int :bool :rgb :point :string :any, a kind name, or (list-of type)";

/// The error for a `:state`/`:document` entry with an untyped nil default
/// (kind-bindings spec §3.3): nil says nothing about the field's type.
pub(crate) fn nil_default_message(kind: &str, slot: &str, field: &str) -> String {
    format!(
        "def-kind {kind}: :{slot} field '{field}' defaults to nil; declare its type: \
         ({field} <type> :default nil)"
    )
}

/// The error for a malformed `:state`/`:document` entry.
pub(crate) fn state_entry_shape_message(kind: &str, slot: &str) -> String {
    format!("def-kind {kind}: each :{slot} entry is (field default) or (field type :default d)")
}

/// The declared type of a `:state`/`:document` field (kind-bindings spec
/// §3.3). Writes are checked against it: `(set! x.f v)` with a value of
/// another type is an error naming the kind, the field and its type.
#[derive(Clone, Debug, PartialEq)]
pub enum FieldType {
    Number,
    /// A number with no fractional part.
    Int,
    Bool,
    /// `(rgb r g b)`, the tagged list the `rgb` native builds.
    Rgb,
    /// `(dict :col c :row r)`.
    Point,
    String,
    Any,
    /// An instance of the named kind: a qualified name (`pkg:scene`) is
    /// the exact kind id, a bare one (`scene`) matches the kind name part
    /// of the instance's kind id ([`kind_name_of`]).
    Kind(String),
    ListOf(Box<FieldType>),
}

/// Answers an instance's kind id, for checking kind-typed fields.
type KindOf<'a, 'k> = Option<&'a dyn Fn(InstanceId) -> Option<&'k str>>;

impl FieldType {
    /// The type an untyped `(field default)` entry declares: `false`/`true`
    /// → `:bool`, a number → `:number`, a string → `:string`, a list →
    /// `(list-of :any)`, anything else → `:any`.
    pub fn infer(default: &Value) -> Self {
        match default {
            Value::Bool(_) => Self::Bool,
            Value::Number(_) => Self::Number,
            Value::String(_) => Self::String,
            Value::List(_) => Self::ListOf(Box::new(Self::Any)),
            _ => Self::Any,
        }
    }

    /// The type a keyword names (`number` for `:number`), if any.
    pub fn from_keyword(name: &str) -> Option<Self> {
        Some(match name {
            "number" => Self::Number,
            "int" => Self::Int,
            "bool" => Self::Bool,
            "rgb" => Self::Rgb,
            "point" => Self::Point,
            "string" => Self::String,
            "any" => Self::Any,
            _ => return None,
        })
    }

    /// Parse a type spelled as data: `:bool`, `scene`, `(list-of :number)`.
    pub fn from_value(value: &Value) -> Result<Self, String> {
        match value {
            Value::Keyword(name) => {
                let name = name.trim_start_matches(':');
                Self::from_keyword(name)
                    .ok_or_else(|| format!("unknown field type :{name}; {FIELD_TYPES_HINT}"))
            }
            Value::Symbol(name) if !matches!(name.as_str(), "" | "nil" | "true" | "false") => {
                Ok(Self::Kind(name.clone()))
            }
            Value::List(items) if items.len() == 2 => {
                match (&*items[0].borrow(), &*items[1].borrow()) {
                    (Value::Symbol(head), inner) if head == "list-of" => {
                        Ok(Self::ListOf(Box::new(Self::from_value(inner)?)))
                    }
                    _ => Err(format!("invalid field type; {FIELD_TYPES_HINT}")),
                }
            }
            _ => Err(format!("invalid field type; {FIELD_TYPES_HINT}")),
        }
    }

    /// Whether `value` has this type. Without `kind_of` (no store to ask) a
    /// kind type accepts any instance.
    fn accepts(&self, value: &Value, kind_of: KindOf<'_, '_>) -> bool {
        let number = |item: &Rc<std::cell::RefCell<Value>>| matches!(&*item.borrow(), Value::Number(_));
        match (self, value) {
            (Self::Any, _) => true,
            (Self::Number, Value::Number(_)) => true,
            (Self::Int, Value::Number(n)) => n.is_finite() && n.fract() == 0.0,
            (Self::Bool, Value::Bool(_)) => true,
            (Self::String, Value::String(_)) => true,
            (Self::Rgb, Value::List(items)) => {
                items.len() == 4
                    && matches!(&*items[0].borrow(), Value::Symbol(head) if head == "rgb")
                    && items[1..].iter().all(number)
            }
            (Self::Point, Value::Map(map)) => {
                map.get("col").is_some_and(number) && map.get("row").is_some_and(number)
            }
            (Self::Kind(name), Value::Instance(id)) => match kind_of {
                None => true,
                Some(kind_of) => kind_of(*id).is_some_and(|kind| {
                    if name.contains(':') {
                        kind == name
                    } else {
                        kind_name_of(kind) == name
                    }
                }),
            },
            (Self::ListOf(item), Value::List(_)) if **item == Self::Any => true,
            (Self::ListOf(item), Value::List(items)) => {
                items.iter().all(|value| item.accepts(&value.borrow(), kind_of))
            }
            _ => false,
        }
    }
}

impl std::fmt::Display for FieldType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Number => write!(f, ":number"),
            Self::Int => write!(f, ":int"),
            Self::Bool => write!(f, ":bool"),
            Self::Rgb => write!(f, ":rgb"),
            Self::Point => write!(f, ":point"),
            Self::String => write!(f, ":string"),
            Self::Any => write!(f, ":any"),
            Self::Kind(name) => write!(f, "{name}"),
            Self::ListOf(item) => write!(f, "(list-of {item})"),
        }
    }
}

/// One declared `:state`/`:document` field: its name, default and type.
#[derive(Clone, Debug)]
pub struct KindField {
    pub name: String,
    pub default: Value,
    pub ty: FieldType,
}

impl KindField {
    /// An untyped `(field default)` entry: the type is inferred.
    pub fn new(name: impl Into<String>, default: Value) -> Self {
        let ty = FieldType::infer(&default);
        Self { name: name.into(), default, ty }
    }

    pub fn typed(name: impl Into<String>, ty: FieldType, default: Value) -> Self {
        Self { name: name.into(), default, ty }
    }

    /// Parse one entry as the compiler hands it over: `(field default)` or
    /// `(field default type)` (from `(field type :default d)`). An untyped
    /// entry whose default is nil is an error: nil says nothing about the
    /// type. `kind`/`slot` only label the error.
    pub fn from_entry(kind: &str, slot: &str, entry: &Value) -> Result<Self, String> {
        let Value::List(items) = entry else {
            return Err(state_entry_shape_message(kind, slot));
        };
        let items: Vec<Value> = items.iter().map(|item| item.borrow().clone()).collect();
        let name = match items.first() {
            Some(Value::Symbol(name) | Value::String(name)) if !name.is_empty() => name.clone(),
            _ => return Err(format!("def-kind {kind}: :{slot} field names must be symbols")),
        };
        let default = items.get(1).cloned().unwrap_or(Value::Nil);
        let field = match items.get(2) {
            None if matches!(default, Value::Nil) => {
                return Err(nil_default_message(kind, slot, &name));
            }
            None => Self::new(name, default),
            Some(ty) => {
                let ty = FieldType::from_value(ty)
                    .map_err(|error| format!("def-kind {kind}: :{slot} field '{name}': {error}"))?;
                Self::typed(name, ty, default)
            }
        };
        if !field.accepts(&field.default, None) {
            return Err(format!(
                "def-kind {kind}: :{slot} field '{}' is {}; its default {} is not",
                field.name,
                field.ty,
                describe_value(&field.default)
            ));
        }
        Ok(field)
    }

    /// Whether the field can hold `value`: a value of its type, or nil when
    /// the field defaults to nil (an optional field) or is a list (nil is
    /// the empty list).
    fn accepts(&self, value: &Value, kind_of: KindOf<'_, '_>) -> bool {
        self.ty.accepts(value, kind_of)
            || (matches!(value, Value::Nil)
                && (matches!(self.default, Value::Nil) || matches!(self.ty, FieldType::ListOf(_))))
    }
}

/// A value as an error message shows it, cut short.
fn describe_value(value: &Value) -> String {
    let text = super::format_lisp_source(value);
    if text.chars().count() > 48 {
        format!("{}…", text.chars().take(48).collect::<String>())
    } else {
        text
    }
}

/// The position of the field named `name` in `fields`.
fn index_in(fields: &[KindField], name: &str) -> Option<usize> {
    fields.iter().position(|declared| declared.name == name)
}

/// Ordered `:state` fields of one kind with their default values.
#[derive(Clone, Debug)]
pub struct InstanceKindSchema {
    /// Kind id, `<package name>:<kind name>` (spec §5).
    pub kind: String,
    /// `:state` fields, in declaration order.
    pub fields: Vec<KindField>,
    /// `:document` fields, in declaration order: stored by the host per
    /// pattern (docs/jaki-kind-spec.md §3).
    pub document: Vec<KindField>,
    /// `:key ()` (kind-bindings spec §3.1): exactly one instance, created by
    /// `def-kind` itself and bound to the kind's name. Its built-in fields
    /// are `id` and `kind` only, and the host never sees it (no project
    /// instance, tab, view or Packages row): [`VM::live_instances`] and
    /// [`VM::instance_kind_ids`] leave singletons out.
    pub singleton: bool,
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
            document: Vec::new(),
            singleton: false,
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

    pub fn singleton(mut self) -> Self {
        self.singleton = true;
        self
    }

    /// An untyped `:state` field; its type is inferred from the default.
    pub fn field(self, name: impl Into<String>, default: Value) -> Self {
        self.with_field(KindField::new(name, default))
    }

    pub fn with_field(mut self, field: KindField) -> Self {
        self.fields.push(field);
        self
    }

    /// An untyped `:document` field; its type is inferred from the default.
    pub fn document_field(self, name: impl Into<String>, default: Value) -> Self {
        self.with_document_field(KindField::new(name, default))
    }

    pub fn with_document_field(mut self, field: KindField) -> Self {
        self.document.push(field);
        self
    }

    /// The fields every instance of this kind answers before its own.
    pub fn builtin_fields(&self) -> &'static [&'static str] {
        if self.singleton { &SINGLETON_BUILTIN_FIELDS } else { &INSTANCE_HOST_FIELDS }
    }

    fn index_of(&self, field: &str) -> Option<usize> {
        index_in(&self.fields, field)
    }

    fn document_index_of(&self, field: &str) -> Option<usize> {
        index_in(&self.document, field)
    }

    fn document_default_of(&self, index: usize) -> Value {
        self.document
            .get(index)
            .map(|declared| declared.default.deep_clone())
            .unwrap_or(Value::Nil)
    }

    fn default_of(&self, index: usize) -> Value {
        self.fields
            .get(index)
            .map(|declared| declared.default.deep_clone())
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
        for KindField { name, .. } in self.fields.iter().chain(self.document.iter()) {
            if self.builtin_fields().contains(&name.as_str()) {
                return Err(InstanceError::InvalidSchema(format!(
                    "kind '{}' declares state field '{name}', which is a {} field",
                    self.kind,
                    if self.singleton { "built-in" } else { "host" }
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

    /// Every readable field name, built-in fields first.
    pub fn all_field_names(&self) -> Vec<String> {
        self.builtin_fields()
            .iter()
            .map(|name| (*name).to_string())
            .chain(self.fields.iter().map(|declared| declared.name.clone()))
            .chain(self.document.iter().map(|declared| declared.name.clone()))
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
    /// A write of a value the field's declared type does not admit.
    TypeMismatch {
        kind: String,
        field: String,
        expected: String,
        got: String,
    },
    /// The host asked to create an instance of a singleton kind.
    SingletonKind(String),
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
            Self::TypeMismatch {
                kind,
                field,
                expected,
                got,
            } => write!(f, "field '{field}' of kind '{kind}' is {expected}; got {got}"),
            Self::SingletonKind(kind) => write!(
                f,
                "kind '{kind}' is a singleton (:key ()); its one instance comes from def-kind"
            ),
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
    /// Local document cells, one per `:document` field, used only when the
    /// host has no document natives.
    document: Vec<Value>,
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
    Document(usize),
}

#[derive(Default)]
pub(crate) struct InstanceStore {
    kinds: HashMap<String, InstanceKindSchema>,
    live: HashMap<InstanceId, InstanceRecord>,
    /// Dropped instance -> its kind, so a stale handle still reads defaults.
    dropped: HashMap<InstanceId, String>,
    /// Singleton kind id -> its one instance. Entries are never removed, so
    /// a re-evaluated `def-kind` finds the same instance (and ids are
    /// allocated as `SINGLETON_INSTANCE_ID_BASE + len`).
    singletons: HashMap<String, InstanceId>,
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
                    for field in schema.fields.iter_mut().chain(schema.document.iter_mut()) {
                        field.default = clone_value_for_snapshot(&field.default);
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
                            document: record.document.iter().map(clone_value_for_snapshot).collect(),
                            owner: clone_value_for_snapshot(&record.owner),
                            label: clone_value_for_snapshot(&record.label),
                        },
                    )
                })
                .collect(),
            dropped: self.dropped.clone(),
            singletons: self.singletons.clone(),
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
        let (kind, slot, _) = self.resolve_declared(id, field)?;
        Ok((kind.to_string(), slot))
    }

    /// [`Self::resolve`], borrowing the kind id, plus the declared field
    /// behind a `:state`/`:document` slot.
    fn resolve_declared(
        &self,
        id: InstanceId,
        field: &str,
    ) -> Result<(&str, FieldSlot, Option<&KindField>), InstanceError> {
        let kind = self.kind_of(id).ok_or(InstanceError::UnknownInstance(id))?;
        let schema = self.kinds.get(kind);
        // A singleton has no owner/label: those names are its own fields
        // (or unknown).
        let singleton = schema.is_some_and(|schema| schema.singleton);
        let slot = match field {
            "id" => FieldSlot::Id,
            "kind" => FieldSlot::Kind,
            "owner" if !singleton => FieldSlot::Owner,
            "label" if !singleton => FieldSlot::Label,
            _ => {
                let schema = schema.ok_or_else(|| InstanceError::UnknownKind(kind.to_string()))?;
                return match (schema.index_of(field), schema.document_index_of(field)) {
                    (Some(index), _) => Ok((kind, FieldSlot::State(index), schema.fields.get(index))),
                    (None, Some(index)) => {
                        Ok((kind, FieldSlot::Document(index), schema.document.get(index)))
                    }
                    (None, None) => Err(InstanceError::UnknownField {
                        kind: kind.to_string(),
                        field: field.to_string(),
                        fields: schema.all_field_names(),
                    }),
                };
            }
        };
        Ok((kind, slot, None))
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
            FieldSlot::Document(index) => match record.and_then(|r| r.document.get(index)) {
                Some(value) => value.clone(),
                None => self.document_default(kind, index),
            },
        }
    }

    fn document_default(&self, kind: &str, index: usize) -> Value {
        self.kinds
            .get(kind)
            .map(|schema| schema.document_default_of(index))
            .unwrap_or(Value::Nil)
    }

    /// Whether `field` can hold `value`, checking a kind-typed field
    /// against the instance's actual kind.
    fn field_accepts(&self, field: &KindField, value: &Value) -> bool {
        let kind_of = |id: InstanceId| self.kind_of(id);
        field.accepts(value, Some(&kind_of))
    }

    /// Reject a write the declared field's type does not admit.
    fn check_write(
        &self,
        kind: &str,
        field: &str,
        declared: &KindField,
        value: &Value,
    ) -> Result<(), InstanceError> {
        if self.field_accepts(declared, value) {
            return Ok(());
        }
        Err(InstanceError::TypeMismatch {
            kind: kind.to_string(),
            field: field.to_string(),
            expected: declared.ty.to_string(),
            got: describe_value(value),
        })
    }

    /// Cells for `fields` from an instance's cells under `previous`: a value
    /// survives by name while the new type still admits it; anything else
    /// starts at its default.
    fn retained_cells(&self, previous: &[KindField], fields: &[KindField], old: &[Value]) -> Vec<Value> {
        fields
            .iter()
            .map(|field| {
                index_in(previous, &field.name)
                    .and_then(|index| old.get(index).cloned())
                    .filter(|value| self.field_accepts(field, value))
                    .unwrap_or_else(|| field.default.deep_clone())
            })
            .collect()
    }
}

pub(crate) fn instance_namespace(id: InstanceId) -> String {
    format!("{INSTANCE_NAMESPACE_PREFIX}{id}")
}

impl VM {
    // ---- host API -------------------------------------------------------

    /// Register (or re-register, on hot reload) a kind's `:state` schema.
    /// Live instances of the kind keep their values by field name while the
    /// field's (new) type admits them; new fields start at their default and
    /// removed fields are dropped (spec §5). A kind cannot switch between
    /// created and singleton (`:key ()`) while the VM holds it.
    pub fn register_instance_kind(&mut self, schema: InstanceKindSchema) -> Result<(), InstanceError> {
        schema.validate()?;
        if let Some(previous) = self.instances.kinds.get(&schema.kind)
            && previous.singleton != schema.singleton
        {
            return Err(InstanceError::InvalidSchema(format!(
                "kind '{}' is already defined {}; restart to change its :key",
                schema.kind,
                if previous.singleton { "as a singleton (:key ())" } else { "without :key" }
            )));
        }
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
                let Some(record) = self.instances.live.get(&id) else {
                    continue;
                };
                let document =
                    self.instances
                        .retained_cells(&previous.document, &schema.document, &record.document);
                let state = self
                    .instances
                    .retained_cells(&previous.fields, &schema.fields, &record.state);
                if let Some(record) = self.instances.live.get_mut(&id) {
                    record.document = document;
                    record.state = state;
                }
                // A newly declared field may already have (error-state)
                // readers; bring every retained source up to date.
                for (index, field) in schema.fields.iter().enumerate() {
                    let value = self.instances.value(id, &schema.kind, FieldSlot::State(index));
                    self.publish_instance_field(id, &field.name, value);
                }
            }
        }
        Ok(())
    }

    pub fn instance_kind_schema(&self, kind: &str) -> Option<&InstanceKindSchema> {
        self.instances.kinds.get(kind)
    }

    /// Every created (non-singleton) kind id this VM holds a schema for,
    /// sorted: the kinds the host can hold instances of.
    pub fn instance_kind_ids(&self) -> Vec<String> {
        let mut kinds: Vec<String> = self
            .instances
            .kinds
            .values()
            .filter(|schema| !schema.singleton)
            .map(|schema| schema.kind.clone())
            .collect();
        kinds.sort_unstable();
        kinds
    }

    /// Create the cells of a new instance with its kind's defaults and return
    /// the Lisp handle. Reusing a dropped id revives it with fresh defaults.
    /// A singleton kind's one instance comes from `def-kind`, never here.
    pub fn create_instance(&mut self, id: InstanceId, kind: &str) -> Result<Value, InstanceError> {
        if self
            .instances
            .kinds
            .get(kind)
            .is_some_and(|schema| schema.singleton)
        {
            return Err(InstanceError::SingletonKind(kind.to_string()));
        }
        self.insert_instance_record(id, kind)
    }

    fn insert_instance_record(&mut self, id: InstanceId, kind: &str) -> Result<Value, InstanceError> {
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
                .map(|field| field.default.deep_clone())
                .collect(),
            document: schema
                .document
                .iter()
                .map(|field| field.default.deep_clone())
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

    /// `(def-kind name :key () ...)`: register the singleton kind and return
    /// its one instance, creating it on first definition. Re-evaluating the
    /// `def-kind` (hot reload) keeps the same instance and its values, as
    /// [`Self::register_instance_kind`] does for any kind.
    pub fn define_singleton_kind(&mut self, schema: InstanceKindSchema) -> Result<Value, InstanceError> {
        let schema = schema.singleton();
        let kind = schema.kind.clone();
        self.register_instance_kind(schema)?;
        let next = SINGLETON_INSTANCE_ID_BASE + self.instances.singletons.len() as InstanceId;
        let id = *self.instances.singletons.entry(kind.clone()).or_insert(next);
        if !self.instances.live.contains_key(&id) {
            self.insert_instance_record(id, &kind)?;
        }
        Ok(Value::Instance(id))
    }

    /// The [`DEF_SINGLETON_KIND_NATIVE`] call `(def-kind name :key () :state
    /// (...))` compiles to: the name as a symbol, then `:state` with the
    /// entries the compiler built. The kind id is `<module>:<name>`
    /// (`scratch:<name>` in headerless code), the fallback the host uses for
    /// kinds outside a package; singletons are never saved, so it only has to
    /// be unique within the VM.
    pub(super) fn def_singleton_kind_from_args(&mut self, args: Vec<Value>) -> Result<Value, VMError> {
        let Some(Value::Symbol(name)) = args.first() else {
            return Err(VMError::Instance("def-kind expects a kind name".to_string()));
        };
        let module = Some(self.current_module_name())
            .filter(|module| *module != crate::modules::IMPLICIT_MODULE);
        let mut schema = InstanceKindSchema::new(kind_id(None, module, name));
        for pair in args[1..].chunks(2) {
            match pair {
                [Value::Keyword(key), Value::List(entries)] if key == "state" => {
                    for entry in entries {
                        let field = KindField::from_entry(name, "state", &entry.borrow())
                            .map_err(VMError::Instance)?;
                        schema = schema.with_field(field);
                    }
                }
                [Value::Keyword(key), Value::Nil] if key == "state" => {}
                _ => {
                    return Err(VMError::Instance(format!(
                        "def-kind {name}: a singleton (:key ()) takes only :state"
                    )));
                }
            }
        }
        Ok(self.define_singleton_kind(schema)?)
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

    /// Live host-created instance ids, sorted. Singleton instances are the
    /// VM's own and left out, so host syncs never drop them.
    pub fn live_instances(&self) -> Vec<InstanceId> {
        let mut ids: Vec<InstanceId> = self
            .instances
            .live
            .iter()
            .filter(|(_, record)| {
                !self
                    .instances
                    .kinds
                    .get(&record.kind)
                    .is_some_and(|schema| schema.singleton)
            })
            .map(|(id, _)| *id)
            .collect();
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
        if let FieldSlot::Document(index) = slot {
            if let Some(read) = self.instance_doc_native(INSTANCE_DOC_READ_NATIVE) {
                // The host resolves the current pattern's value and injects
                // its own reactive edge (the scene-slot source).
                let default = self.instances.document_default(&kind, index);
                return Ok(read(
                    vec![Value::Number(id as f64), Value::String(field.to_string()), default],
                    self,
                ));
            }
        }
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
        let (kind, slot, declared) = self.instances.resolve_declared(id, field)?;
        if !self.instances.live.contains_key(&id) {
            // Stale handle: an event handler outliving its instance.
            return Ok(());
        }
        if let Some(declared) = declared {
            self.instances.check_write(kind, field, declared, &value)?;
        }
        let kind = kind.to_string();
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
            FieldSlot::Document(index) => {
                if let Some(write) = self.instance_doc_native(INSTANCE_DOC_WRITE_NATIVE) {
                    write(
                        vec![Value::Number(id as f64), Value::String(field.to_string()), value],
                        self,
                    );
                    return Ok(());
                }
                if let Some(cell) = self
                    .instances
                    .live
                    .get_mut(&id)
                    .and_then(|record| record.document.get_mut(index))
                {
                    *cell = value.deep_clone();
                }
            }
        }
        self.publish_instance_field(id, field, value);
        Ok(())
    }

    /// The host's document native `name`, if registered.
    fn instance_doc_native(&self, name: &str) -> Option<super::NativeFn> {
        match self.global_value(name)? {
            Value::NativeFunction(native) => Some(native.callable.clone()),
            _ => None,
        }
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
    fn document_fields_use_local_cells_without_host_natives() {
        let mut vm = VM::new(Vec::new());
        super::super::register_core_natives(&mut vm);
        vm.register_instance_kind(
            InstanceKindSchema::new(KIND)
                .field("x", Value::Number(1.0))
                .document_field("rows", Value::Number(8.0)),
        )
        .expect("register kind");
        bind(&mut vm, "a", 1);
        bind(&mut vm, "b", 2);
        assert_eq!(eval(&mut vm, "a.rows"), Some(Value::Number(8.0)));
        eval(&mut vm, "(set! a.rows 3)");
        assert_eq!(eval(&mut vm, "a.rows"), Some(Value::Number(3.0)));
        assert_eq!(eval(&mut vm, "b.rows"), Some(Value::Number(8.0)));
    }

    #[test]
    fn document_fields_route_through_host_natives() {
        let mut vm = VM::new(Vec::new());
        super::super::register_core_natives(&mut vm);
        vm.register_instance_kind(
            InstanceKindSchema::new(KIND).document_field("rows", Value::Number(8.0)),
        )
        .expect("register kind");
        bind(&mut vm, "a", 7);
        let calls: Rc<RefCell<Vec<Vec<Value>>>> = Rc::new(RefCell::new(Vec::new()));
        let reads = calls.clone();
        vm.register_native_with_vm(super::INSTANCE_DOC_READ_NATIVE, move |args, _vm| {
            reads.borrow_mut().push(args.clone());
            Value::Number(42.0)
        });
        let writes = calls.clone();
        vm.register_native_with_vm(super::INSTANCE_DOC_WRITE_NATIVE, move |args, _vm| {
            writes.borrow_mut().push(args.clone());
            Value::Nil
        });
        assert_eq!(eval(&mut vm, "a.rows"), Some(Value::Number(42.0)));
        eval(&mut vm, "(set! a.rows 5)");
        let calls = calls.borrow();
        assert_eq!(
            calls[0],
            vec![Value::Number(7.0), Value::String("rows".into()), Value::Number(8.0)]
        );
        assert_eq!(
            calls[1],
            vec![Value::Number(7.0), Value::String("rows".into()), Value::Number(5.0)]
        );
    }

    #[test]
    fn schema_rejects_a_document_field_shadowing_a_state_field() {
        let schema = InstanceKindSchema::new(KIND)
            .field("rows", Value::Nil)
            .document_field("rows", Value::Nil);
        assert!(schema.validate().is_err());
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

    // ---- singleton kinds and typed fields (kind-bindings spec §3.1, §3.3) --

    const SINGLETON_ID: u64 = super::SINGLETON_INSTANCE_ID_BASE;

    /// The message of an eval that must fail with an instance error.
    fn instance_error(vm: &mut VM, code: &str) -> String {
        match vm.eval_str(code) {
            Err(VMError::Instance(message)) => message,
            other => panic!("{code}: expected an instance error, got {other:?}"),
        }
    }

    /// The source-load errors of an eval that must fail to compile.
    fn compile_errors(vm: &mut VM, code: &str) -> String {
        assert_eq!(vm.eval_str(code), Err(VMError::CompileError), "{code}");
        vm.take_source_load_errors().join("\n")
    }

    #[test]
    fn singleton_fields_read_write_and_dirty_only_their_readers() {
        let mut vm = instance_vm();
        assert_eq!(
            eval(&mut vm, "(def-kind menu :key () :state ((open false) (count 0)))"),
            Some(Value::Instance(SINGLETON_ID)),
            "the form's value is the instance"
        );
        assert_eq!(eval(&mut vm, "menu"), Some(Value::Instance(SINGLETON_ID)));
        assert_eq!(eval(&mut vm, "menu.open"), Some(Value::Bool(false)));
        assert_eq!(eval(&mut vm, "menu.kind"), Some(Value::String("scratch:menu".into())));
        assert_eq!(eval(&mut vm, "menu.id"), Some(Value::Number(SINGLETON_ID as f64)));
        eval(
            &mut vm,
            r#"
            (effect-buffer "*open*" (label (if menu.open "open" "closed")))
            (effect-buffer "*count*" (box :width menu.count :height 1))
            "#,
        );
        assert_eq!(rendered_targets(&mut vm), vec!["*count*", "*open*"]);

        assert_eq!(eval(&mut vm, "(set! menu.open true)"), Some(Value::Bool(true)));
        assert_eq!(rendered_targets(&mut vm), vec!["*open*"]);
        assert_eq!(eval(&mut vm, "menu.open"), Some(Value::Bool(true)));
        eval(&mut vm, "(set! menu.count 3)");
        assert_eq!(rendered_targets(&mut vm), vec!["*count*"]);
        // A closure reaches the same instance through the binding.
        eval(&mut vm, "(def close-menu () (set! menu.open false))");
        eval(&mut vm, "(close-menu)");
        assert_eq!(rendered_targets(&mut vm), vec!["*open*"]);
    }

    #[test]
    fn singleton_built_in_fields_are_id_and_kind_only() {
        let mut vm = instance_vm();
        eval(&mut vm, "(def-kind menu :key () :state ((open false) (label \"File\")))");
        // `label` is the singleton's own field, not a host rename.
        assert_eq!(eval(&mut vm, "menu.label"), Some(Value::String("File".into())));
        eval(&mut vm, "(set! menu.label \"Edit\")");
        assert_eq!(eval(&mut vm, "menu.label"), Some(Value::String("Edit".into())));
        assert_eq!(
            instance_error(&mut vm, "menu.owner"),
            "kind 'scratch:menu' has no field 'owner'; fields: id, kind, open, label"
        );
        assert!(instance_error(&mut vm, "(set! menu.id 2)").contains("read-only"));
        // `id`/`kind` cannot be declared.
        assert!(
            instance_error(&mut vm, "(def-kind bad :key () :state ((kind 1)))")
                .contains("built-in field")
        );
    }

    #[test]
    fn singletons_are_invisible_to_host_instance_enumeration() {
        let mut vm = instance_vm();
        bind(&mut vm, "a", 1);
        eval(&mut vm, "(def-kind menu :key () :state ((open false)))");
        assert_eq!(vm.live_instances(), vec![1]);
        assert_eq!(vm.instance_kind_ids(), vec![KIND.to_string()]);
        assert!(vm.instance_is_live(SINGLETON_ID));
        assert_eq!(
            vm.create_instance(2, "scratch:menu"),
            Err(super::InstanceError::SingletonKind("scratch:menu".into()))
        );
        // A created kind cannot turn into a singleton (or back) in place.
        let error = vm
            .register_instance_kind(InstanceKindSchema::new("scratch:menu").field("open", Value::Bool(false)))
            .expect_err("kind kept its :key");
        assert!(error.to_string().contains("singleton"), "{error}");
    }

    #[test]
    fn re_evaluating_a_singleton_def_kind_keeps_its_instance_and_values() {
        let mut vm = instance_vm();
        eval(&mut vm, "(def-kind menu :key () :state ((open false) (count 0) (gone 1)))");
        eval(&mut vm, "(def-kind other :key () :state ((x 1)))");
        eval(&mut vm, "(set! menu.open true) (set! menu.count 4)");
        eval(&mut vm, r#"(effect-buffer "*open*" (label (if menu.open "open" "closed")))"#);
        rendered_targets(&mut vm);
        // Hot reload: `count` changes type (its value no longer fits, so it
        // restarts at the new default), `gone` goes, `extra` arrives.
        assert_eq!(
            eval(
                &mut vm,
                "(def-kind menu :key () :state ((open false) (count \"none\") (extra 2)))"
            ),
            Some(Value::Instance(SINGLETON_ID))
        );
        assert_eq!(eval(&mut vm, "menu.open"), Some(Value::Bool(true)));
        assert_eq!(eval(&mut vm, "menu.count"), Some(Value::String("none".into())));
        assert_eq!(eval(&mut vm, "menu.extra"), Some(Value::Number(2.0)));
        assert!(matches!(vm.eval_str("menu.gone"), Err(VMError::Instance(_))));
        assert_eq!(eval(&mut vm, "other"), Some(Value::Instance(SINGLETON_ID + 1)));
        assert!(rendered_targets(&mut vm).is_empty(), "an unchanged field dirties nothing");
    }

    #[test]
    fn field_types_are_inferred_or_declared_and_checked_on_write() {
        let mut vm = instance_vm();
        eval(
            &mut vm,
            "(def-kind t :key ()
               :state ((n 0) (flag false) (name \"\") (items (list)) (mode :loop)
                       (i :int :default 0)
                       (color :rgb :default (rgb 1 0 0))
                       (at :point :default nil)
                       (nums (list-of :number) :default (list))
                       (other t :default nil)
                       (anything :any :default nil)))
             (def-kind u :key () :state ((x 1)))",
        );
        let types: Vec<String> = vm
            .instance_kind_schema("scratch:t")
            .expect("schema")
            .fields
            .iter()
            .map(|field| field.ty.to_string())
            .collect();
        assert_eq!(
            types,
            vec![
                ":number", ":bool", ":string", "(list-of :any)", ":any", ":int", ":rgb", ":point",
                "(list-of :number)", "t", ":any"
            ]
        );
        for ok in [
            "(set! t.n 2.5)",
            "(set! t.flag true)",
            "(set! t.name \"x\")",
            "(set! t.items (list 1 \"a\"))",
            "(set! t.items nil)",
            "(set! t.mode 3)",
            "(set! t.i -2)",
            "(set! t.color (rgb 0 0.5 1))",
            "(set! t.at (dict :col 1 :row 2))",
            "(set! t.at nil)",
            "(set! t.nums (list 1 2))",
            "(set! t.other t)",
            "(set! t.other nil)",
            "(set! t.anything (dict :a 1))",
        ] {
            eval(&mut vm, ok);
        }
        assert_eq!(
            instance_error(&mut vm, "(set! t.n \"x\")"),
            "field 'n' of kind 'scratch:t' is :number; got \"x\""
        );
        for (bad, expected) in [
            ("(set! t.n nil)", ":number"),
            ("(set! t.flag 1)", ":bool"),
            ("(set! t.name :x)", ":string"),
            ("(set! t.i 2.5)", ":int"),
            ("(set! t.color (list 1 0 0))", ":rgb"),
            ("(set! t.color (list 'rgb 1 0))", ":rgb"),
            ("(set! t.at 3)", ":point"),
            ("(set! t.at (dict :col 1))", ":point"),
            ("(set! t.nums (list 1 \"a\"))", "(list-of :number)"),
            ("(set! t.other u)", "is t;"),
        ] {
            let message = instance_error(&mut vm, bad);
            assert!(message.contains(expected), "{bad}: {message}");
        }
        assert_eq!(eval(&mut vm, "t.i"), Some(Value::Number(-2.0)), "failed writes change nothing");
    }

    #[test]
    fn a_nil_default_needs_a_declared_type() {
        let mut vm = instance_vm();
        for code in [
            "(def-kind a :key () :state ((x nil)))",
            "(def-kind a :key () :state ((x)))",
            "(def-kind a :key () :state (x))",
        ] {
            let errors = compile_errors(&mut vm, code);
            assert!(
                errors.contains("field 'x' defaults to nil; declare its type: (x <type> :default nil)"),
                "{code}: {errors}"
            );
        }
        // A default that evaluates to nil is caught when def-kind runs.
        assert!(
            instance_error(&mut vm, "(def-kind a :key () :state ((x (if false 1 nil))))")
                .contains("field 'x' defaults to nil")
        );
        assert!(
            instance_error(&mut vm, "(def-kind a :key () :state ((x :bool :default 3)))")
                .contains("field 'x' is :bool; its default 3 is not")
        );
        assert!(
            compile_errors(&mut vm, "(def-kind a :key () :state ((x :float :default 0)))")
                .contains("unknown field type :float")
        );
        assert!(vm.eval_str("a").is_err(), "nothing was defined");
    }

    #[test]
    fn keyed_kinds_and_created_kind_slots_on_singletons_are_errors() {
        let mut vm = instance_vm();
        assert!(
            compile_errors(&mut vm, "(def-kind track :key (index) :state ((x 0)))")
                .contains("keyed kinds (:key (index ...)) are not supported yet")
        );
        assert!(
            compile_errors(&mut vm, "(def-kind menu :key () :state ((x 0)) :view show)")
                .contains("a singleton (:key ()) has no :view")
        );
        assert!(
            compile_errors(&mut vm, "(def-kind menu :key () :document ((x 0)))")
                .contains("has no :document")
        );
    }

    #[test]
    fn a_singleton_binding_is_a_module_definition_reachable_by_import() {
        let mut vm = VM::new(Vec::new());
        super::super::register_core_natives(&mut vm);
        let module = format!("test.singleton-kind-{}", std::process::id());
        let helper = std::env::temp_dir().join(format!("{module}.lisp"));
        std::fs::write(
            &helper,
            format!(
                "(module {module})\n(export menu toggle-menu)\n\
                 (def-kind menu :key () :state ((open false)))\n\
                 (def toggle-menu () (set! menu.open (not menu.open)))"
            ),
        )
        .expect("write helper module");
        let main = helper.with_file_name(format!("eseqlisp-singleton-main-{}.lisp", std::process::id()));
        let result = vm.eval_module_source(
            main.clone(),
            &format!("(import {module} :refer (menu toggle-menu))\n(toggle-menu)\nmenu.open"),
            1,
        );
        assert_eq!(result, Ok(Some(Value::Bool(true))), "{:?}", vm.take_source_load_errors());
        assert_eq!(
            vm.eval_module_source(
                main,
                &format!("(import {module} :refer (menu))\n(set! menu.open false)\nmenu.open"),
                1
            ),
            Ok(Some(Value::Bool(false)))
        );
        assert_eq!(
            vm.instance_field(SINGLETON_ID, "kind"),
            Ok(Value::String(format!("{module}:menu")))
        );
        let _ = std::fs::remove_file(helper);
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
                    (Value::Keyword(key), Value::List(entries)) if key == "state" => {
                        for entry in entries {
                            schema = schema
                                .with_field(super::KindField::from_entry(&name, key, &entry.borrow())?);
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
                   :state ((sel (- base 4)) (open :bool :default nil) (name \"x\"))
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
        assert_eq!(
            list_items(&state[1]),
            vec![Value::Symbol("open".to_string()), Value::Nil, Value::Keyword("bool".to_string())]
        );
        assert_eq!(args[5], Value::Keyword("view".to_string()));
        drop(calls);

        let schema = runtime
            .instance_kind_schema("test/pkg:neural")
            .expect("the native registered the schema in the evaluating VM");
        assert_eq!(
            schema.fields.iter().map(|field| field.name.as_str()).collect::<Vec<_>>(),
            vec!["sel", "open", "name"]
        );
        assert_eq!(
            schema.fields.iter().map(|field| field.ty.to_string()).collect::<Vec<_>>(),
            vec![":number", ":bool", ":string"]
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
    fn created_kinds_take_typed_fields_and_singletons_skip_the_host_native() {
        let (mut runtime, captured) = runtime_with_def_kind();
        runtime
            .eval_str("(def-kind probe :state ((x 1) (tags (list-of :string) :default (list))))")
            .expect("kind");
        let handle = runtime.create_instance(3, "test/pkg:probe").expect("create");
        runtime.set_global_value("inst", handle);
        runtime.eval_str("(set! inst.tags (list \"a\"))").expect("typed write");
        assert!(runtime.eval_str("(set! inst.tags (list 1))").is_err());
        assert!(runtime.eval_str("(set! inst.x \"one\")").is_err());
        assert_eq!(
            runtime.instance_field(3, "tags"),
            Ok(Value::List(vec![Rc::new(RefCell::new(Value::String("a".into())))]))
        );

        // A singleton never reaches the host's `def-kind` native.
        runtime
            .eval_str("(def-kind menu :key () :state ((open false)))")
            .expect("singleton");
        assert_eq!(
            runtime.eval_str("(set! menu.open true) menu.open").expect("write"),
            Some(Value::Bool(true))
        );
        assert_eq!(captured.borrow().len(), 1);
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
