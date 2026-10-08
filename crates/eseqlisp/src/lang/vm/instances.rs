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
//! Fields are either built-in fields (for a created kind `id`, `kind`,
//! `owner`, `label`) or the kind's declared fields. `id` and `kind` come
//! from the record, `owner` and `label` are pushed by the host
//! (`VM::set_instance_builtin_field`). Only `label` is writable from Lisp:
//! with a label hook installed the write is forwarded to the host (a rename
//! is an undoable project edit, the host pushes the accepted label back);
//! without one the label cell is written locally.
//!
//! Declared fields are typed (docs/kind-bindings-spec.md §3.3, [`FieldType`]):
//! the type is inferred from the default or declared, and every write is
//! checked against it. A singleton kind (`:key ()`, §3.1) has exactly one
//! instance, created by `def-kind` and bound to the kind's name; its
//! built-in fields are only `id` and `kind` (keyed kinds add `key`), and
//! host enumerations (`live_instances`, `instance_kind_ids`) leave every
//! kind with a `:key` out.
//!
//! A keyed kind (`:key (index)` or `:key (parent index)`, kind-bindings
//! spec §3.1, §4) projects host things: the host registers an instance per
//! key ([`VM::register_keyed_instance`]), re-keys it on reorder (the id, and
//! so every captured handle, keeps meaning the same thing) and drops it.
//! `def-kind` binds an index-keyed kind's name to a constructor, `(track 3)`,
//! that answers the instance under that key (or nil) and depends on that one
//! key's source under `%keys/<kind>`. Kinds with a key carry `:host` fields
//! ([`HostField`], §3.2): cells the host pushes with
//! [`VM::set_instance_field`]; a Lisp write calls the field's `:set`
//! function and leaves the cell to the host, and without `:set` it is
//! read-only.
//!
//! A view-local kind (a `:key` of names and no `:host`, kind-bindings spec
//! §3.1, eseq-0l17.62) is keyed state Lisp owns: its constructor,
//! `(adsr-gesture scope section)`, answers the instance under that key and
//! creates it on first call; `(drop-instance g)` drops one. Key parts are
//! plain values ([`LocalKeyPart`]); a part that is an instance makes the
//! view-local instance its child, dropped with it.
//!
//! A dropped instance keeps a tombstone naming its kind: reads answer the
//! kind's defaults, writes are silent no-ops (spec §4 "stale self").
//!
//! Slot-backed fields (`:number :int :bool :rgb`, kind-bindings spec §3.3)
//! can be bound with `#'x.field` ([`FIELD_REF_NATIVE`], §7.1): the binding
//! is a `Value::ReactiveRef` over a float slot (three for `:rgb`) in the
//! VM's binding store, under the field's `%instance/<id>` namespace (also
//! its DAG source), with a `BindingKind::Instance*` kind naming the
//! instance. Slots are created on the first binding, seeded from the field,
//! and from then on every change of the field writes them and queues a
//! repaint of the widgets bound to them
//! ([`VM::take_pending_binding_repaints`]).
//! Dropping an instance writes its defaults and frees its slots.
//!
//! A kind's `:document` fields (docs/jaki-kind-spec.md §3) read and write
//! with the same syntax but are stored by the host, per pattern: a read calls
//! the host native [`INSTANCE_DOC_READ_NATIVE`] and a write
//! [`INSTANCE_DOC_WRITE_NATIVE`], which carry their own reactive edge and
//! history. A VM without those natives keeps document fields in local cells,
//! exactly like `:state`.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use super::{
    BindingKind, ReactiveNode, ReactiveSource, VM, VMError, Value, clone_value_for_snapshot,
};

/// Host-assigned instance id, stable within a project.
pub type InstanceId = u64;

/// The observed mask [`VM::host_fields_observed`] returns: bit `i` is the
/// `i`th field asked about (kind-bindings spec §9, D3).
pub type ObservedMask = u64;

/// How many fields one [`VM::host_fields_observed`] call (and so one kind's
/// live field list on the host) can cover: the bits of [`ObservedMask`].
pub const MAX_OBSERVED_FIELDS: usize = ObservedMask::BITS as usize;

/// Receives a Lisp `(set! x.label v)` when the host wants renames routed
/// through its own (undoable) edit path. The host is expected to push the
/// accepted label back with `VM::set_instance_builtin_field`.
pub type InstanceLabelHook = Rc<dyn Fn(InstanceId, &Value)>;

/// Answers a by-value read of a `:host` field nothing observes (kind-bindings
/// spec §9, D3): the host skips computing unobserved fields, so their cells
/// may be stale; a read of one asks the host for the current value instead.
/// `None` keeps the cell's value. A `Some` is type-checked and written to the
/// cell like a host push. The hook may register keyed instances (a lazy
/// `t.steps`, D2) but must not read fields through Lisp.
pub type HostFieldReader = Rc<dyn Fn(&mut VM, InstanceId, &str) -> Option<Value>>;

/// Reserved DAG namespace prefix for instance field sources.
pub const INSTANCE_NAMESPACE_PREFIX: &str = "%instance/";

/// The native `#'x.field` compiles to: `(__field-ref x "field")` returns a
/// binding to the field (kind-bindings spec §7.1).
pub const FIELD_REF_NATIVE: &str = "__field-ref";

/// Host native `(__instance-doc-read id field default)` backing document
/// field reads. It tracks its own reactive dependency.
pub const INSTANCE_DOC_READ_NATIVE: &str = "__instance-doc-read";

/// Host native `(__instance-doc-write id field value)` backing document
/// field writes. It dirties its own readers and records history.
pub const INSTANCE_DOC_WRITE_NATIVE: &str = "__instance-doc-write";

/// The native every `(def-kind name :key (...) ...)` compiles to
/// (kind-bindings spec §3.1). For a singleton (`:key ()`) it returns the one
/// instance and for an index-keyed kind (`:key (index)`) the constructor,
/// either of which the compiler binds to the kind's name; a parent-keyed
/// kind (`:key (track index)`) gets no binding and returns its kind id.
pub const DEF_KEYED_KIND_NATIVE: &str = "__def-keyed-kind";

/// Reserved DAG namespace prefix for an index-keyed kind's key map: field
/// `"3"` of `%keys/<kind id>` is the source a constructor call `(track 3)`
/// depends on, advanced whenever that key starts or stops naming an
/// instance (registration, drop, re-key). Parent-keyed kinds have no
/// constructor, so no such sources.
pub const KIND_KEYS_NAMESPACE_PREFIX: &str = "%keys/";

/// The built-in fields every instance of a created kind (no `:key`)
/// answers, in this order.
pub const CREATED_BUILTIN_FIELDS: [&str; 4] = ["id", "kind", "owner", "label"];

/// The built-in fields of a created kind whose value the host pushes
/// ([`VM::set_instance_builtin_field`]; `id`/`kind` are fixed by the
/// record). Not to be confused with a keyed kind's `:host` fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstanceBuiltinField {
    Owner,
    Label,
}

impl InstanceBuiltinField {
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

/// The built-in fields of a keyed kind: `key` is the instance's current key
/// as a list (`(3)`, `(41 12)`), re-published on a re-key.
pub(crate) const KEYED_BUILTIN_FIELDS: [&str; 3] = ["id", "kind", "key"];

/// A keyed instance's key (kind-bindings spec §4): one component for
/// `:key (index)`; for `:key (parent index)` the parent instance's id, then
/// the index (§12 D2: re-keying a parent leaves its children alone).
pub type InstanceKey = Vec<u64>;

/// The parent instance a key names: the first part of a two-part
/// (`:key (parent index)`) key.
fn key_parent(key: &[u64]) -> Option<InstanceId> {
    match key {
        [parent, _] => Some(*parent),
        _ => None,
    }
}

/// A key as messages and printing show it: `3`, `41 12`.
fn key_text(key: &[u64]) -> String {
    key.iter().map(u64::to_string).collect::<Vec<_>>().join(" ")
}

/// One part of a view-local instance's key (kind-bindings spec §3.1,
/// eseq-0l17.62): the values a constructor call `(adsr-gesture "core" -1)`
/// may key by. Numbers compare by value (`-0` is `0`); an instance part
/// compares by id.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum LocalKeyPart {
    Number(u64),
    Bool(bool),
    String(String),
    Keyword(String),
    Symbol(String),
    Instance(InstanceId),
}

impl LocalKeyPart {
    /// The part for `value`, or `None` for a value that cannot key (nil, a
    /// non-finite number, a list, a map, a function, a binding).
    fn from_value(value: &Value) -> Option<Self> {
        Some(match value {
            Value::Number(n) if n.is_finite() => Self::Number((n + 0.0).to_bits()),
            Value::Bool(b) => Self::Bool(*b),
            Value::String(s) => Self::String(s.clone()),
            Value::Keyword(k) => Self::Keyword(k.clone()),
            Value::Symbol(s) => Self::Symbol(s.clone()),
            Value::Instance(id) => Self::Instance(*id),
            _ => return None,
        })
    }

    fn to_value(&self) -> Value {
        match self {
            Self::Number(bits) => Value::Number(f64::from_bits(*bits)),
            Self::Bool(b) => Value::Bool(*b),
            Self::String(s) => Value::String(s.clone()),
            Self::Keyword(k) => Value::Keyword(k.clone()),
            Self::Symbol(s) => Value::Symbol(s.clone()),
            Self::Instance(id) => Value::Instance(*id),
        }
    }
}

/// A view-local key as messages, printing and its key source show it:
/// `"core" -1`.
fn local_key_text(key: &[LocalKeyPart]) -> String {
    key.iter()
        .map(|part| super::format_lisp_source(&part.to_value()))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Every instance a view-local key names: dropping any of them drops the
/// view-local instance.
fn local_key_parents(key: &[LocalKeyPart]) -> impl Iterator<Item = InstanceId> + '_ {
    key.iter().filter_map(|part| match part {
        LocalKeyPart::Instance(id) => Some(*id),
        _ => None,
    })
}

/// A kind's `:key` (kind-bindings spec §3.1).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum KindKey {
    /// No `:key`: the host creates (and saves) the instances.
    #[default]
    Created,
    /// `:key ()`: exactly one instance, created by `def-kind` itself and
    /// bound to the kind's name.
    Singleton,
    /// `:key (index)`: the host registers an instance per integer key
    /// ([`VM::register_keyed_instance`]); the kind's name is bound to the
    /// constructor `(name i)`.
    Indexed { index: String },
    /// `:key (parent index)`: an instance per (parent instance id, index),
    /// reached through the parent, an instance of the keyed kind `parent`.
    /// `:key ((p1 p2 …) index)` names several parent kinds: the parent is
    /// an instance of any one of them (a device under a track or a bus).
    Under { parents: Vec<String>, index: String },
    /// `:key (name …)` with no `:host` group (eseq-0l17.62): view-local
    /// state Lisp owns, an instance per tuple of key values, created by the
    /// constructor `(name v …)` on first call and dropped by
    /// `(drop-instance x)`. The host never registers these.
    Local { names: Vec<String> },
}

impl KindKey {
    /// How many parts a registered key has (0 for kinds the host does not
    /// register by key).
    pub fn arity(&self) -> usize {
        match self {
            Self::Created | Self::Singleton => 0,
            Self::Indexed { .. } => 1,
            Self::Under { .. } => 2,
            Self::Local { names } => names.len(),
        }
    }

    /// Whether a re-registration keeps the key's shape: the same variant
    /// and, under a parent, the same parent kind names (in any order).
    /// Index names may change.
    fn same_shape(&self, other: &Self) -> bool {
        match (self, other) {
            // The parent list is a set: `((track bus) did)` and
            // `((bus track) did)` are one shape.
            (Self::Under { parents: a, .. }, Self::Under { parents: b, .. }) => {
                a.len() == b.len() && a.iter().all(|parent| b.contains(parent))
            }
            // Key names may change; their number may not (live keys).
            (Self::Local { names: a }, Self::Local { names: b }) => a.len() == b.len(),
            _ => std::mem::discriminant(self) == std::mem::discriminant(other),
        }
    }

    /// How the key reads in messages.
    fn describe(&self) -> String {
        match self {
            Self::Created => "without :key".to_string(),
            Self::Singleton => "as a singleton (:key ())".to_string(),
            Self::Indexed { index } => format!("keyed (:key ({index}))"),
            Self::Under { parents, index } => match parents.as_slice() {
                [parent] => format!("keyed (:key ({parent} {index}))"),
                parents => format!("keyed (:key (({}) {index}))", parents.join(" ")),
            },
            Self::Local { names } => format!("view-local (:key ({}))", names.join(" ")),
        }
    }

    /// The parent kind names of a `:key (parent index)` key, as messages
    /// show them: `track`, `track or bus`.
    fn parents_text(parents: &[String]) -> String {
        parents.join(" or ")
    }
}

/// Keyed instances take ids from here up. They are ordinary instance ids
/// (same store, equality, tombstones), allocated by the VM in a range of
/// their own: they are never saved, so they must not collide with the ids
/// the host assigns and saves for created instances (counting up from 1).
pub(crate) const KEYED_INSTANCE_ID_BASE: InstanceId = 1 << 32;

/// Singleton instances take ids from here up, one per singleton kind, so
/// they never collide with host-assigned instance ids (which count up from
/// 1) and stay exact as a Lisp number. Only allocation uses it: whether an
/// instance is a singleton is its kind's [`InstanceKindSchema::is_singleton`].
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

/// The built-in fields of an instance of a kind with this key: what
/// [`InstanceKindSchema::builtin_fields`] answers.
pub(crate) fn builtin_fields_for(key: &KindKey) -> &'static [&'static str] {
    match key {
        KindKey::Created => &CREATED_BUILTIN_FIELDS,
        KindKey::Singleton => &SINGLETON_BUILTIN_FIELDS,
        KindKey::Indexed { .. } | KindKey::Under { .. } | KindKey::Local { .. } => {
            &KEYED_BUILTIN_FIELDS
        }
    }
}

/// The error for a declared field that a kind with this key already has as
/// a built-in field (`key` on a keyed kind; eseq-0l17.60): rejected when
/// `def-kind` compiles, before the form can run.
pub(crate) fn builtin_field_message(kind: &str, slot: &str, field: &str, key: &KindKey) -> String {
    let what = match key {
        KindKey::Created => "a created kind",
        KindKey::Singleton => "a singleton (:key ())",
        KindKey::Indexed { .. } | KindKey::Under { .. } | KindKey::Local { .. } => "a keyed kind",
    };
    format!(
        "def-kind {kind}: :{slot} field '{field}' is a built-in field of {what} ({}); rename it",
        builtin_fields_for(key).join(", ")
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

    /// The binding a field of this type on instance `id` supports: a float
    /// slot for the numeric types, three for `:rgb`, none for value-only
    /// types (kind-bindings spec §3.3).
    pub fn binding_kind(&self, id: InstanceId) -> Option<BindingKind> {
        match self {
            Self::Number | Self::Int | Self::Bool => Some(BindingKind::InstanceFloat(id)),
            Self::Rgb => Some(BindingKind::InstanceRgb(id)),
            _ => None,
        }
    }

    /// The value a field of this type answers before anything is written
    /// (a `:host` field before the host pushes one, or on a stale
    /// instance): `0`, `false`, `""`, `(rgb 0 0 0)`, `()`, or nil.
    pub fn default_value(&self) -> Value {
        match self {
            Self::Number | Self::Int => Value::Number(0.0),
            Self::Bool => Value::Bool(false),
            Self::String => Value::String(String::new()),
            Self::Rgb => super::tagged_list("rgb", vec![Value::Number(0.0); 3]),
            Self::ListOf(_) => Value::List(Vec::new()),
            Self::Point | Self::Any | Self::Kind(_) => Value::Nil,
        }
    }

    /// Whether `#'` can bind a field of this type.
    pub fn is_bindable(&self) -> bool {
        self.binding_kind(0).is_some()
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

/// One `:host` field (kind-bindings spec §3.2): host-owned, always typed,
/// starting at its type's default until the host pushes a value. With
/// `:set f`, `(set! t.field v)` calls `(f t v)` instead of writing the cell;
/// without it the field is read-only from Lisp.
#[derive(Clone, Debug)]
pub struct HostField {
    pub field: KindField,
    /// `:set f`: a function of the instance and the new value.
    pub set: Option<Value>,
    /// `:range (lo hi)`: metadata for widgets that scale themselves.
    pub range: Option<(f64, f64)>,
    /// `:doc "…"`.
    pub doc: Option<String>,
}

impl HostField {
    pub fn new(name: impl Into<String>, ty: FieldType) -> Self {
        let default = ty.default_value();
        Self {
            field: KindField::typed(name, ty, default),
            set: None,
            range: None,
            doc: None,
        }
    }

    pub fn with_set(mut self, set: Value) -> Self {
        self.set = Some(set);
        self
    }

    pub fn with_range(mut self, lo: f64, hi: f64) -> Self {
        self.range = Some((lo, hi));
        self
    }

    pub fn with_doc(mut self, doc: impl Into<String>) -> Self {
        self.doc = Some(doc.into());
        self
    }

    /// Parse one `(field type option…)` entry as the compiler hands it over
    /// (the type as data, `:set` evaluated, `:range` as data).
    pub fn from_entry(kind: &str, entry: &Value) -> Result<Self, String> {
        let shape = || host_entry_shape_message(kind);
        let Value::List(items) = entry else {
            return Err(shape());
        };
        let items: Vec<Value> = items.iter().map(|item| item.borrow().clone()).collect();
        let (Some(Value::Symbol(name)), Some(ty)) = (items.first(), items.get(1)) else {
            return Err(shape());
        };
        let ty = FieldType::from_value(ty)
            .map_err(|error| format!("def-kind {kind}: :host field '{name}': {error}"))?;
        let mut field = Self::new(name.clone(), ty);
        for pair in items[2..].chunks(2) {
            match pair {
                [Value::Keyword(option), value] => match (option.as_str(), value) {
                    ("set", Value::Nil) => {}
                    ("set", setter) => field = field.with_set(setter.clone()),
                    ("range", Value::List(bounds)) if bounds.len() == 2 => {
                        let bound = |index: usize| match &*bounds[index].borrow() {
                            Value::Number(n) => Some(*n),
                            _ => None,
                        };
                        let (Some(lo), Some(hi)) = (bound(0), bound(1)) else {
                            return Err(host_option_message(kind, name, "range"));
                        };
                        field = field.with_range(lo, hi);
                    }
                    ("doc", Value::String(doc)) => field = field.with_doc(doc.clone()),
                    (option, _) => return Err(host_option_message(kind, name, option)),
                },
                _ => return Err(shape()),
            }
        }
        Ok(field)
    }
}

/// The error for a malformed `:host` entry.
pub(crate) fn host_entry_shape_message(kind: &str) -> String {
    format!(
        "def-kind {kind}: each :host entry is (field type option…) with options \
         :set f, :range (lo hi), :doc \"…\""
    )
}

/// The error for a bad `:host` field option.
pub(crate) fn host_option_message(kind: &str, field: &str, option: &str) -> String {
    match option {
        "set" => format!("def-kind {kind}: :host field '{field}': :set takes a function"),
        "range" => {
            format!("def-kind {kind}: :host field '{field}': :range takes (lo hi), two numbers")
        }
        "doc" => format!("def-kind {kind}: :host field '{field}': :doc takes a string"),
        other => format!(
            "def-kind {kind}: :host field '{field}': unknown option :{other}; options are :set, :range, :doc"
        ),
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
    /// `:host` fields, in declaration order: pushed by the host
    /// (kind-bindings spec §3.2); only on kinds with a `:key`.
    pub host: Vec<HostField>,
    /// The kind's `:key` (kind-bindings spec §3.1). A kind with a key
    /// opts out of what only created kinds have: its built-in fields are
    /// `id` and `kind` (plus `key` when keyed), and the host never sees it
    /// as a project instance (no tab, view or Packages row):
    /// [`VM::live_instances`] and [`VM::instance_kind_ids`] leave it out.
    pub key: KindKey,
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
            host: Vec::new(),
            key: KindKey::Created,
            view: None,
            keymap: None,
            on_create: None,
        }
    }

    /// Whether `other` declares the same kind as `self` up to the identity
    /// of its function values: the same fields (names, types, defaults),
    /// host fields (and which are settable), key, keymap and whether it has
    /// a `:view` / `:on-create`. Re-evaluating an unchanged `def-kind` (a
    /// later pass re-importing its module) re-registers such a schema with
    /// fresh closures; that is no schema change (eseq-0l17.59).
    pub fn same_declaration(&self, other: &Self) -> bool {
        let fields = |a: &[KindField], b: &[KindField]| {
            a.len() == b.len()
                && a.iter()
                    .zip(b)
                    .all(|(a, b)| a.name == b.name && a.ty == b.ty && a.default == b.default)
        };
        self.kind == other.kind
            && fields(&self.fields, &other.fields)
            && fields(&self.document, &other.document)
            && self.host.len() == other.host.len()
            && self.host.iter().zip(&other.host).all(|(a, b)| {
                fields(
                    std::slice::from_ref(&a.field),
                    std::slice::from_ref(&b.field),
                ) && a.set.is_some() == b.set.is_some()
                    && a.range == b.range
                    && a.doc == b.doc
            })
            && self.key == other.key
            && self.keymap == other.keymap
            && self.view.is_some() == other.view.is_some()
            && self.on_create.is_some() == other.on_create.is_some()
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
        self.key = KindKey::Singleton;
        self
    }

    /// `:key (index)`.
    pub fn indexed(mut self, index: impl Into<String>) -> Self {
        self.key = KindKey::Indexed {
            index: index.into(),
        };
        self
    }

    /// `:key (parent index)`, under the keyed kind `parent`.
    pub fn under(self, parent: impl Into<String>, index: impl Into<String>) -> Self {
        self.under_any(vec![parent.into()], index)
    }

    /// `:key ((p1 p2 …) index)`, under an instance of any of the keyed
    /// kinds `parents`.
    pub fn under_any(mut self, parents: Vec<String>, index: impl Into<String>) -> Self {
        self.key = KindKey::Under {
            parents,
            index: index.into(),
        };
        self
    }

    pub fn with_host_field(mut self, field: HostField) -> Self {
        self.host.push(field);
        self
    }

    /// `:key ()`.
    pub fn is_singleton(&self) -> bool {
        self.key == KindKey::Singleton
    }

    /// A view-local kind (`:key (name …)` without `:host`): Lisp creates
    /// its instances.
    pub fn is_local(&self) -> bool {
        matches!(self.key, KindKey::Local { .. })
    }

    /// `:key (index)` or `:key (parent index)`: the host registers the
    /// instances (a view-local kind is not keyed in this sense).
    pub fn is_keyed(&self) -> bool {
        matches!(self.key, KindKey::Indexed { .. } | KindKey::Under { .. })
    }

    /// The parent kind name of a `:key (parent index)` kind (`track`;
    /// `track or bus` for several).
    pub fn parent_kind_name(&self) -> Option<String> {
        match &self.key {
            KindKey::Under { parents, .. } => Some(KindKey::parents_text(parents)),
            _ => None,
        }
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
        builtin_fields_for(&self.key)
    }

    fn index_of(&self, field: &str) -> Option<usize> {
        index_in(&self.fields, field)
    }

    fn host_index_of(&self, field: &str) -> Option<usize> {
        self.host
            .iter()
            .position(|declared| declared.field.name == field)
    }

    fn host_default_of(&self, index: usize) -> Value {
        self.host
            .get(index)
            .map(|declared| declared.field.default.deep_clone())
            .unwrap_or(Value::Nil)
    }

    /// Declared fields of every group: `:host`, `:state`, `:document`.
    fn declared_fields(&self) -> impl Iterator<Item = &KindField> {
        self.host
            .iter()
            .map(|declared| &declared.field)
            .chain(self.fields.iter())
            .chain(self.document.iter())
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
    /// never a built-in field. Hosts run it before recording a kind anywhere
    /// else, so a schema the VM would reject is never half-registered.
    pub fn validate(&self) -> Result<(), InstanceError> {
        if self.kind.is_empty() {
            return Err(InstanceError::InvalidSchema("empty kind id".to_string()));
        }
        if !self.host.is_empty() && self.key == KindKey::Created {
            // kind-bindings spec §12 D5.
            return Err(InstanceError::InvalidSchema(format!(
                "kind '{}': :host fields need :key",
                self.kind
            )));
        }
        if self.key != KindKey::Created && !self.document.is_empty() {
            return Err(InstanceError::InvalidSchema(format!(
                "kind '{}' has a :key, so it has no :document",
                self.kind
            )));
        }
        let mut seen = std::collections::HashSet::new();
        for KindField { name, .. } in self.declared_fields() {
            if self.builtin_fields().contains(&name.as_str()) {
                return Err(InstanceError::InvalidSchema(format!(
                    "kind '{}' declares field '{name}', which is a built-in field",
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

    /// Every readable field name, built-in fields first.
    pub fn all_field_names(&self) -> Vec<String> {
        self.builtin_fields()
            .iter()
            .map(|name| (*name).to_string())
            .chain(self.declared_fields().map(|declared| declared.name.clone()))
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
    /// A Lisp write of a `:host` field without `:set`.
    ReadOnlyHostField {
        kind: String,
        field: String,
    },
    /// The host asked to create an instance of a keyed kind: those are
    /// registered by key ([`VM::register_keyed_instance`]). `parent` is the
    /// parent kind name of a `:key (parent index)` kind.
    KeyedKind {
        kind: String,
        parent: Option<String>,
    },
    /// A keyed registration or re-key the kind's `:key` does not admit.
    InvalidKey(String),
    /// The host asked to create (or register) an instance of a view-local
    /// kind: Lisp creates those with the kind's constructor.
    LocalKind(String),
    /// The host asked to create an instance with an id from the range the
    /// VM allocates keyed and singleton instances in.
    ReservedId(InstanceId),
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
            Self::ReadOnlyHostField { kind, field } => {
                write!(f, "{}.{field} is read-only", kind_name_of(kind))
            }
            Self::KeyedKind { kind, parent: None } => {
                let name = kind_name_of(kind);
                write!(f, "{name} instances come from the project; use ({name} i)")
            }
            Self::KeyedKind {
                kind,
                parent: Some(parent),
            } => write!(
                f,
                "{} instances come from the project; reach them through their {parent}",
                kind_name_of(kind)
            ),
            Self::InvalidKey(message) => write!(f, "{message}"),
            Self::LocalKind(kind) => {
                let name = kind_name_of(kind);
                write!(
                    f,
                    "{name} instances are view-local; create them with ({name} key …)"
                )
            }
            Self::ReservedId(id) => write!(
                f,
                "instance id {id} is reserved for keyed and singleton instances"
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
    /// The kind id, shared with the store's kind table.
    kind: Rc<str>,
    /// One value per schema field, in schema order.
    state: Vec<Value>,
    /// Local document cells, one per `:document` field, used only when the
    /// host has no document natives.
    document: Vec<Value>,
    owner: Value,
    label: Value,
    /// One value per `:host` field, pushed by the host.
    host: Vec<Value>,
    /// A keyed instance's current key.
    key: Option<InstanceKey>,
    /// A view-local instance's key (its constructor's arguments).
    local_key: Option<Rc<[LocalKeyPart]>>,
}

/// Which cell a field name resolves to.
#[derive(Clone, Copy)]
enum FieldSlot {
    Id,
    Kind,
    Owner,
    Label,
    /// A keyed instance's built-in `key`.
    Key,
    State(usize),
    Document(usize),
    Host(usize),
}

/// A field name resolved against an instance: its kind, its cell, the
/// declared field behind a `:host`/`:state`/`:document` cell, and the
/// record when the instance is live (`None` for a stale one).
struct Resolved<'s> {
    kind: &'s str,
    slot: FieldSlot,
    declared: Option<&'s KindField>,
    record: Option<&'s InstanceRecord>,
}

impl Resolved<'_> {
    /// A `:host` field of a live instance (what the host pushes).
    fn is_live_host(&self) -> bool {
        matches!(self.slot, FieldSlot::Host(_)) && self.record.is_some()
    }
}

/// The live instances of one keyed kind by key (kind-bindings spec §4).
#[derive(Clone, Default)]
struct KeyRegistry {
    ids: HashMap<InstanceKey, InstanceId>,
    /// `%keys/<kind id>` for an index-keyed kind, whose constructor reads
    /// one source per key; `None` for a parent-keyed kind (no constructor).
    namespace: Option<Rc<str>>,
}

/// `%keys/<kind id>`, the namespace of an index-keyed kind's key sources.
fn key_namespace(kind: &str) -> Rc<str> {
    format!("{KIND_KEYS_NAMESPACE_PREFIX}{kind}").into()
}

#[derive(Default)]
pub(crate) struct InstanceStore {
    kinds: HashMap<Rc<str>, InstanceKindSchema>,
    live: HashMap<InstanceId, InstanceRecord>,
    /// Dropped instance -> its kind, so a stale handle still reads defaults.
    dropped: HashMap<InstanceId, Rc<str>>,
    /// Singleton kind id -> its one instance. Entries are never removed, so
    /// a re-evaluated `def-kind` finds the same instance (and ids are
    /// allocated as `SINGLETON_INSTANCE_ID_BASE + len`).
    singletons: HashMap<String, InstanceId>,
    /// Keyed kind id -> its live instances by key.
    keyed: HashMap<Rc<str>, KeyRegistry>,
    /// View-local kind id -> its live instances by key (eseq-0l17.62).
    local: HashMap<Rc<str>, HashMap<Rc<[LocalKeyPart]>, InstanceId>>,
    /// `:key (parent index)` kind id -> its parent kind ids (one per name
    /// its key gives), resolved when the first instance registers and then
    /// fixed, so a kind defined later (one that would make a parent name
    /// ambiguous) never re-parents live children.
    parent_kinds: HashMap<Rc<str>, Rc<[Rc<str>]>>,
    /// Parent instance -> its live children (host keys naming it as parent,
    /// or view-local keys naming it in any part), dropped with it.
    children: HashMap<InstanceId, HashSet<InstanceId>>,
    /// Keyed instance ids allocated so far (from [`KEYED_INSTANCE_ID_BASE`]);
    /// never reused, so a stale handle never comes back as another thing.
    /// A rollback keeps it.
    keyed_allocated: u64,
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
                    for host in &mut schema.host {
                        host.field.default = clone_value_for_snapshot(&host.field.default);
                        if let Some(set) = &mut host.set {
                            *set = clone_value_for_snapshot(set);
                        }
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
                            host: record.host.iter().map(clone_value_for_snapshot).collect(),
                            key: record.key.clone(),
                            local_key: record.local_key.clone(),
                        },
                    )
                })
                .collect(),
            dropped: self.dropped.clone(),
            singletons: self.singletons.clone(),
            keyed: self.keyed.clone(),
            local: self.local.clone(),
            parent_kinds: self.parent_kinds.clone(),
            children: self.children.clone(),
            keyed_allocated: self.keyed_allocated,
            label_hook: self.label_hook.clone(),
        }
    }

    /// Roll back to `snapshot`, keeping the host's current label hook and
    /// every keyed id allocated since (ids are never reused).
    pub(crate) fn restore_from(&mut self, snapshot: Self) {
        let hook = self.label_hook.take();
        let keyed_allocated = self.keyed_allocated.max(snapshot.keyed_allocated);
        *self = snapshot;
        self.label_hook = hook;
        self.keyed_allocated = keyed_allocated;
    }

    fn kind_of(&self, id: InstanceId) -> Option<&str> {
        self.live
            .get(&id)
            .map(|record| &*record.kind)
            .or_else(|| self.dropped.get(&id).map(|kind| &**kind))
    }

    /// The shared kind id the store holds for `kind`.
    fn kind_rc(&self, kind: &str) -> Option<Rc<str>> {
        self.kinds.get_key_value(kind).map(|(kind, _)| kind.clone())
    }

    /// Resolve `field` of instance `id` (live or stale) with one lookup.
    fn resolve(&self, id: InstanceId, field: &str) -> Result<Resolved<'_>, InstanceError> {
        let record = self.live.get(&id);
        let kind = match record {
            Some(record) => &*record.kind,
            None => self
                .dropped
                .get(&id)
                .map(|kind| &**kind)
                .ok_or(InstanceError::UnknownInstance(id))?,
        };
        let schema = self.kinds.get(kind);
        // A kind with a :key has no owner/label (those names are its own
        // fields, or unknown); a keyed kind has `key`.
        let created = schema.is_none_or(|schema| schema.key == KindKey::Created);
        let keyed = schema.is_some_and(|schema| schema.is_keyed() || schema.is_local());
        let resolved = |slot, declared| Resolved {
            kind,
            slot,
            declared,
            record,
        };
        let slot = match field {
            "id" => FieldSlot::Id,
            "kind" => FieldSlot::Kind,
            "owner" if created => FieldSlot::Owner,
            "label" if created => FieldSlot::Label,
            "key" if keyed => FieldSlot::Key,
            _ => {
                let schema = schema.ok_or_else(|| InstanceError::UnknownKind(kind.to_string()))?;
                if let Some(index) = schema.host_index_of(field) {
                    return Ok(resolved(
                        FieldSlot::Host(index),
                        Some(&schema.host[index].field),
                    ));
                }
                return match (schema.index_of(field), schema.document_index_of(field)) {
                    (Some(index), _) => {
                        Ok(resolved(FieldSlot::State(index), schema.fields.get(index)))
                    }
                    (None, Some(index)) => Ok(resolved(
                        FieldSlot::Document(index),
                        schema.document.get(index),
                    )),
                    (None, None) => Err(InstanceError::UnknownField {
                        kind: kind.to_string(),
                        field: field.to_string(),
                        fields: schema.all_field_names(),
                    }),
                };
            }
        };
        Ok(resolved(slot, None))
    }

    /// Current value of a resolved field: the live cell, or the default for
    /// a dropped instance.
    fn value_of(&self, id: InstanceId, resolved: &Resolved<'_>) -> Value {
        let record = resolved.record;
        let kind = resolved.kind;
        match resolved.slot {
            FieldSlot::Id => Value::Number(id as f64),
            FieldSlot::Kind => Value::String(kind.to_string()),
            FieldSlot::Owner => record.map(|r| r.owner.clone()).unwrap_or(Value::Nil),
            FieldSlot::Label => record.map(|r| r.label.clone()).unwrap_or(Value::Nil),
            FieldSlot::Key => match record.and_then(|r| r.local_key.as_deref()) {
                Some(key) => super::list_from_values(key.iter().map(LocalKeyPart::to_value)),
                None => key_value(record.and_then(|r| r.key.as_deref()).unwrap_or(&[])),
            },
            FieldSlot::Host(index) => match record.and_then(|r| r.host.get(index)) {
                Some(value) => value.clone(),
                None => self
                    .kinds
                    .get(kind)
                    .map(|schema| schema.host_default_of(index))
                    .unwrap_or(Value::Nil),
            },
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

    /// Resolve and read one field.
    fn field_value(&self, id: InstanceId, field: &str) -> Result<Value, InstanceError> {
        let resolved = self.resolve(id, field)?;
        Ok(self.value_of(id, &resolved))
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

    /// The fields of `schema` `#'` (or a shader) can bind, for error
    /// messages: slot-backed `:host` and `:state` fields, and `:document`
    /// fields while they are local cells.
    fn bindable_fields(schema: &InstanceKindSchema, host_documents: bool) -> String {
        let names: Vec<&str> = schema
            .declared_fields()
            .filter(|field| {
                field.ty.is_bindable()
                    && !(host_documents && schema.document_index_of(&field.name).is_some())
            })
            .map(|field| field.name.as_str())
            .collect();
        if names.is_empty() {
            "(none)".to_string()
        } else {
            names.join(", ")
        }
    }

    /// The binding `field` of `schema` supports on instance `id`
    /// (kind-bindings spec §7.1, §7.3): a declared, slot-backed field
    /// (`:number :int :bool :rgb`), not a `:document` field the host
    /// stores. `Err` is the reason, naming the kind, the field and the
    /// bindable fields; the caller prefixes who asked (`#'`, a widget).
    pub(super) fn field_binding(
        schema: &InstanceKindSchema,
        id: InstanceId,
        field: &str,
        host_documents: bool,
    ) -> Result<BindingKind, String> {
        let kind = &schema.kind;
        let unbindable = |what: &str| {
            format!(
                "field '{field}' of kind '{kind}' {what}; bindable fields: {}",
                Self::bindable_fields(schema, host_documents)
            )
        };
        if schema.builtin_fields().contains(&field) {
            return Err(unbindable("is a built-in field"));
        }
        let Some(declared) = schema
            .declared_fields()
            .find(|declared| declared.name == field)
        else {
            return Err(format!(
                "kind '{kind}' has no field '{field}'; bindable fields: {}",
                Self::bindable_fields(schema, host_documents)
            ));
        };
        let Some(binding) = declared.ty.binding_kind(id) else {
            return Err(unbindable(&format!(
                "is {}, which is not bindable",
                declared.ty
            )));
        };
        if host_documents
            && schema.host_index_of(field).is_none()
            && schema.index_of(field).is_none()
        {
            return Err(unbindable("is a host-stored :document field"));
        }
        Ok(binding)
    }

    /// The kinds of any key shape whose name part is `name` (`track` for
    /// `eseq.kinds:track`).
    pub(super) fn kinds_named<'s, 'n>(
        &'s self,
        name: &'n str,
    ) -> impl Iterator<Item = (&'s Rc<str>, &'s InstanceKindSchema)> + use<'s, 'n> {
        self.kinds
            .iter()
            .filter(move |(_, schema)| kind_name_of(&schema.kind) == name)
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

    /// The keyed kind `kind` names: its exact id, or a bare name (`track`)
    /// that exactly one keyed kind has.
    fn resolve_keyed_kind(
        &self,
        kind: &str,
    ) -> Result<(&Rc<str>, &InstanceKindSchema), InstanceError> {
        let not_keyed = |schema: &InstanceKindSchema| {
            InstanceError::InvalidKey(format!(
                "kind '{}' is not keyed; it is defined {}",
                schema.kind,
                schema.key.describe()
            ))
        };
        if let Some((id, schema)) = self.kinds.get_key_value(kind) {
            return if schema.is_keyed() {
                Ok((id, schema))
            } else if schema.is_local() {
                Err(InstanceError::LocalKind(id.to_string()))
            } else {
                Err(not_keyed(schema))
            };
        }
        let mut matches = self
            .kinds_named(kind)
            .filter(|(_, schema)| schema.is_keyed());
        match (matches.next(), matches.next()) {
            (Some(found), None) => Ok(found),
            (Some((first, _)), Some((second, _))) => Err(InstanceError::InvalidKey(format!(
                "keyed kind name '{kind}' is ambiguous ('{first}', '{second}'); use its kind id"
            ))),
            (None, _) => match self.kinds_named(kind).next() {
                Some((id, schema)) if schema.is_local() => {
                    Err(InstanceError::LocalKind(id.to_string()))
                }
                Some((_, schema)) => Err(not_keyed(schema)),
                None => Err(InstanceError::UnknownKind(kind.to_string())),
            },
        }
    }

    /// The parent kind ids of the `:key (parent index)` kind `kind`, one
    /// per parent name: the parent named in its own module first (`m:track`
    /// for `m:step`), else the one keyed kind with that name. Resolved when
    /// the first instance registers (so kinds can be declared in any order)
    /// and then fixed.
    fn parent_kind(&mut self, kind: &str) -> Result<Rc<[Rc<str>]>, InstanceError> {
        if let Some(parents) = self.parent_kinds.get(kind) {
            return Ok(parents.clone());
        }
        let Some((kind, schema)) = self.kinds.get_key_value(kind) else {
            return Err(InstanceError::UnknownKind(kind.to_string()));
        };
        let KindKey::Under { parents, .. } = &schema.key else {
            return Err(InstanceError::InvalidKey(format!(
                "kind '{kind}' is not keyed under a parent"
            )));
        };
        let mut resolved = Vec::with_capacity(parents.len());
        for parent in parents {
            let local = kind.rsplit_once(':').and_then(|(module, _)| {
                self.kinds
                    .get_key_value(format!("{module}:{parent}").as_str())
            });
            let found = match local {
                Some(found) => Ok(found),
                None => self.resolve_keyed_kind(parent),
            };
            match found {
                Ok((parent_kind, parent_schema)) if parent_schema.is_keyed() => {
                    resolved.push(parent_kind.clone());
                }
                _ => {
                    return Err(InstanceError::InvalidKey(format!(
                        "kind '{kind}' is keyed under '{parent}', which is not a keyed kind"
                    )));
                }
            }
        }
        let resolved: Rc<[Rc<str>]> = resolved.into();
        let kind = kind.clone();
        self.parent_kinds.insert(kind, resolved.clone());
        Ok(resolved)
    }

    /// Whether `key` fits keyed kind `kind`: its length, and for a
    /// parent-keyed kind a live parent instance as its first part, which is
    /// returned.
    fn check_key(&mut self, kind: &str, key: &[u64]) -> Result<Option<InstanceId>, InstanceError> {
        let schema = self
            .kinds
            .get(kind)
            .ok_or_else(|| InstanceError::UnknownKind(kind.to_string()))?;
        if key.len() != schema.key.arity() {
            return Err(InstanceError::InvalidKey(format!(
                "kind '{kind}' is {}; got key ({})",
                schema.key.describe(),
                key_text(key)
            )));
        }
        let Some(parent) = key_parent(key) else {
            return Ok(None);
        };
        let parent_kinds = self.parent_kind(kind)?;
        let live_parent = self
            .live
            .get(&parent)
            .is_some_and(|record| parent_kinds.contains(&record.kind));
        if !live_parent {
            let names: Vec<&str> = parent_kinds.iter().map(|kind| &**kind).collect();
            return Err(InstanceError::InvalidKey(format!(
                "kind '{kind}': key ({}) names parent {parent}, which is not a live '{}' instance",
                key_text(key),
                names.join("' or '")
            )));
        }
        Ok(Some(parent))
    }

    /// The live instance of keyed kind id `kind` under `key`.
    fn keyed_id(&self, kind: &str, key: &[u64]) -> Option<InstanceId> {
        self.keyed.get(kind)?.ids.get(key).copied()
    }

    /// Record `child` under `parent` (or forget it, for `None`).
    fn set_parent(&mut self, child: InstanceId, from: Option<InstanceId>, to: Option<InstanceId>) {
        if from == to {
            return;
        }
        if let Some(from) = from
            && let Some(siblings) = self.children.get_mut(&from)
        {
            siblings.remove(&child);
            if siblings.is_empty() {
                self.children.remove(&from);
            }
        }
        if let Some(to) = to {
            self.children.entry(to).or_default().insert(child);
        }
    }

    /// How an instance prints when its kind has a `:key` (kind-bindings
    /// spec §4): `<track#41 [3]>`, `<step#902 [41 12]>`, `<transport>`; a
    /// dropped keyed instance has no key (`<track#41>`). `None` for a
    /// created kind's instance, which prints as `<instance:id>`.
    fn display(&self, id: InstanceId) -> Option<String> {
        let kind = self.kind_of(id)?;
        let schema = self.kinds.get(kind)?;
        let name = kind_name_of(kind);
        match &schema.key {
            KindKey::Created => None,
            KindKey::Singleton => Some(format!("<{name}>")),
            KindKey::Indexed { .. } | KindKey::Under { .. } => Some(
                match self.live.get(&id).and_then(|record| record.key.as_deref()) {
                    Some(key) => format!("<{name}#{id} [{}]>", key_text(key)),
                    None => format!("<{name}#{id}>"),
                },
            ),
            KindKey::Local { .. } => Some(
                match self
                    .live
                    .get(&id)
                    .and_then(|record| record.local_key.as_deref())
                {
                    Some(key) => format!("<{name}#{id} [{}]>", local_key_text(key)),
                    None => format!("<{name}#{id}>"),
                },
            ),
        }
    }
}

pub(crate) fn instance_namespace(id: InstanceId) -> String {
    format!("{INSTANCE_NAMESPACE_PREFIX}{id}")
}

/// A key as Lisp sees it: `(41 12)`.
fn key_value(key: &[u64]) -> Value {
    super::list_from_values(key.iter().map(|part| Value::Number(*part as f64)))
}

impl VM {
    // ---- host API -------------------------------------------------------

    /// Register (or re-register, on hot reload) a kind's `:state` schema.
    /// Live instances of the kind keep their values by field name while the
    /// field's (new) type admits them; new fields start at their default and
    /// removed fields are dropped (spec §5). A kind cannot change the shape
    /// of its `:key` (created, singleton, keyed, keyed under which parent)
    /// while the VM holds it.
    pub fn register_instance_kind(&mut self, schema: InstanceKindSchema) -> Result<(), InstanceError> {
        schema.validate()?;
        let name = kind_name_of(&schema.kind);
        if let Some(owner) = self.reserved_kind_names.get(name)
            && schema.kind.rsplit_once(':').map(|(module, _)| module) != Some(owner.as_str())
        {
            return Err(InstanceError::InvalidSchema(format!(
                "kind name '{name}' is reserved for the host kinds of {owner}; \
                 use (import {owner} :refer ({name})) or pick another name"
            )));
        }
        if let Some(previous) = self.instances.kinds.get(schema.kind.as_str())
            && !previous.key.same_shape(&schema.key)
        {
            return Err(InstanceError::InvalidSchema(format!(
                "kind '{}' is already defined {}; restart to change its :key",
                schema.kind,
                previous.key.describe()
            )));
        }
        let kind = self
            .instances
            .kind_rc(&schema.kind)
            .unwrap_or_else(|| Rc::from(schema.kind.as_str()));
        let unchanged = (self.instances.kinds.get(&kind))
            .is_some_and(|previous| previous.same_declaration(&schema));
        let previous = self.instances.kinds.insert(kind.clone(), schema);
        if !unchanged {
            self.kind_schema_generation += 1;
        }
        // A (re)registered kind may carry a new `:view`: every bound view
        // buffer of its instances re-renders through it.
        self.mark_instance_views_of_kind_dirty(&kind);
        let Some(previous) = previous else {
            return Ok(());
        };
        let schema = &self.instances.kinds[&kind];
        let host_fields = |schema: &InstanceKindSchema| -> Vec<KindField> {
            schema
                .host
                .iter()
                .map(|declared| declared.field.clone())
                .collect()
        };
        let (previous_host, host) = (host_fields(&previous), host_fields(schema));
        let (fields, document) = (schema.fields.clone(), schema.document.clone());
        for id in self.live_instances_of_kind(&kind) {
            let Some(record) = self.instances.live.get(&id) else {
                continue;
            };
            let document =
                self.instances
                    .retained_cells(&previous.document, &document, &record.document);
            let state = self
                .instances
                .retained_cells(&previous.fields, &fields, &record.state);
            let host = self
                .instances
                .retained_cells(&previous_host, &host, &record.host);
            if let Some(record) = self.instances.live.get_mut(&id) {
                record.document = document;
                record.state = state;
                record.host = host;
            }
            // A newly declared field may already have (error-state)
            // readers; bring every read source up to date.
            self.republish_instance_sources(id, true);
            self.sync_bound_slots(Some(id));
        }
        Ok(())
    }

    pub fn instance_kind_schema(&self, kind: &str) -> Option<&InstanceKindSchema> {
        self.instances.kinds.get(kind)
    }

    /// Every created kind id (no `:key`) this VM holds a schema for,
    /// sorted: the kinds the host can hold project instances of.
    pub fn instance_kind_ids(&self) -> Vec<String> {
        let mut kinds: Vec<String> = self
            .instances
            .kinds
            .values()
            .filter(|schema| schema.key == KindKey::Created)
            .map(|schema| schema.kind.clone())
            .collect();
        kinds.sort_unstable();
        kinds
    }

    /// Create the cells of a new instance with its kind's defaults and return
    /// the Lisp handle. Reusing a dropped id revives it with fresh defaults.
    /// A singleton kind's one instance comes from `def-kind` and a keyed
    /// kind's from [`Self::register_keyed_instance`], never here, and ids
    /// from [`KEYED_INSTANCE_ID_BASE`] up are the VM's to allocate.
    pub fn create_instance(&mut self, id: InstanceId, kind: &str) -> Result<Value, InstanceError> {
        if id >= KEYED_INSTANCE_ID_BASE {
            return Err(InstanceError::ReservedId(id));
        }
        if let Some(schema) = self.instances.kinds.get(kind) {
            if schema.is_singleton() {
                return Err(InstanceError::SingletonKind(kind.to_string()));
            }
            if schema.is_local() {
                return Err(InstanceError::LocalKind(kind.to_string()));
            }
            if schema.is_keyed() {
                return Err(InstanceError::KeyedKind {
                    kind: kind.to_string(),
                    parent: schema.parent_kind_name(),
                });
            }
        }
        self.insert_instance_record(id, kind, None)
    }

    fn insert_instance_record(
        &mut self,
        id: InstanceId,
        kind: &str,
        key: Option<InstanceKey>,
    ) -> Result<Value, InstanceError> {
        if self.instances.live.contains_key(&id) {
            return Err(InstanceError::DuplicateInstance(id));
        }
        let (kind, schema) = self
            .instances
            .kinds
            .get_key_value(kind)
            .ok_or_else(|| InstanceError::UnknownKind(kind.to_string()))?;
        let defaults = |fields: &[KindField]| -> Vec<Value> {
            fields
                .iter()
                .map(|field| field.default.deep_clone())
                .collect()
        };
        let record = InstanceRecord {
            kind: kind.clone(),
            state: defaults(&schema.fields),
            document: defaults(&schema.document),
            owner: Value::Nil,
            label: Value::Nil,
            host: schema
                .host
                .iter()
                .map(|declared| declared.field.default.deep_clone())
                .collect(),
            key,
            local_key: None,
        };
        self.instances.live.insert(id, record);
        // Readers of a previously dropped id see the revived values (an id
        // never live before has no readers: reading it is an error).
        if self.instances.dropped.remove(&id).is_some() {
            self.republish_instance_sources(id, false);
        }
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
            self.insert_instance_record(id, &kind, None)?;
        }
        Ok(Value::Instance(id))
    }

    /// The [`DEF_KEYED_KIND_NATIVE`] call every `(def-kind name :key (...)
    /// ...)` compiles to: the name as a symbol, then `:key` with the key
    /// names as data (the compiler checked their shape) and `:host`/`:state`
    /// with the entries the compiler built. The kind id is `<module>:<name>`
    /// (`scratch:<name>` in headerless code), the fallback the host uses for
    /// kinds outside a package; these kinds are never saved, so it only has
    /// to be unique within the VM. Returns what the compiler binds to the
    /// name: the singleton's instance, an index-keyed kind's constructor,
    /// or (for a parent-keyed kind, which gets no binding) the kind id.
    pub(super) fn def_keyed_kind_from_args(&mut self, args: Vec<Value>) -> Result<Value, VMError> {
        let Some(Value::Symbol(name)) = args.first() else {
            return Err(VMError::Instance("def-kind expects a kind name".to_string()));
        };
        let name = name.clone();
        let module = Some(self.current_module_name())
            .filter(|module| *module != crate::modules::IMPLICIT_MODULE);
        let mut schema = InstanceKindSchema::new(kind_id(None, module, &name));
        let malformed_key = || VMError::Instance(format!("def-kind {name}: malformed :key"));
        for pair in args[1..].chunks(2) {
            match pair {
                [Value::Keyword(slot), Value::Nil] if slot == "key" => schema = schema.singleton(),
                [Value::Keyword(slot), Value::List(names)] if slot == "key" => {
                    let symbol = |item: &Rc<std::cell::RefCell<Value>>| match &*item.borrow() {
                        Value::Symbol(name) => Ok(name.clone()),
                        _ => Err(malformed_key()),
                    };
                    schema = match names.as_slice() {
                        [index] => schema.indexed(symbol(index)?),
                        // `((p1 p2 …) index)`: several parent kinds.
                        [parents, index] => {
                            let parents = match &*parents.borrow() {
                                Value::List(parents) if !parents.is_empty() => {
                                    parents.iter().map(symbol).collect::<Result<Vec<_>, _>>()?
                                }
                                Value::Symbol(parent) => vec![parent.clone()],
                                _ => return Err(malformed_key()),
                            };
                            if let Some(duplicate) = crate::compiler::first_duplicate(&parents) {
                                return Err(VMError::Instance(format!(
                                    "def-kind {name}: :key names parent {duplicate} twice"
                                )));
                            }
                            schema.under_any(parents, symbol(index)?)
                        }
                        _ => return Err(malformed_key()),
                    };
                }
                // `:key (name …)` without `:host`: the compiler emits a
                // view-local kind's key as `:local-key` (eseq-0l17.62).
                [Value::Keyword(slot), Value::List(names)] if slot == "local-key" => {
                    let names = names
                        .iter()
                        .map(|item| match &*item.borrow() {
                            Value::Symbol(name) => Ok(name.clone()),
                            _ => Err(malformed_key()),
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    if names.is_empty() {
                        return Err(malformed_key());
                    }
                    schema.key = KindKey::Local { names };
                }
                [Value::Keyword(slot), Value::Nil] if slot == "state" || slot == "host" => {}
                [Value::Keyword(slot), Value::List(entries)] if slot == "state" => {
                    for entry in entries {
                        let field = KindField::from_entry(&name, "state", &entry.borrow())
                            .map_err(VMError::Instance)?;
                        schema = schema.with_field(field);
                    }
                }
                [Value::Keyword(slot), Value::List(entries)] if slot == "host" => {
                    for entry in entries {
                        let field = HostField::from_entry(&name, &entry.borrow())
                            .map_err(VMError::Instance)?;
                        if field
                            .set
                            .as_ref()
                            .is_some_and(|set| !super::is_callable(set))
                        {
                            return Err(VMError::Instance(host_option_message(
                                &name,
                                &field.field.name,
                                "set",
                            )));
                        }
                        schema = schema.with_host_field(field);
                    }
                }
                _ => {
                    return Err(VMError::Instance(format!(
                        "def-kind {name}: a kind with :key takes only :key, :host and :state"
                    )));
                }
            }
        }
        if let Some(message) = crate::compiler::kind_name_widget_collision(&name, &schema.key) {
            return Err(VMError::Instance(message));
        }
        if schema.is_singleton() {
            return Ok(self.define_singleton_kind(schema)?);
        }
        let kind = schema.kind.clone();
        let indexed = matches!(schema.key, KindKey::Indexed { .. });
        let local = schema.is_local();
        self.register_instance_kind(schema)?;
        let namespace = key_namespace(&kind);
        if local {
            // `(adsr-gesture scope section)`: the instance under that key,
            // created on first call.
            let constructor = super::NativeFunction::new(name.clone(), move |args, vm| {
                match vm.local_constructor_call(&kind, &namespace, &name, &args) {
                    Ok(instance) => instance,
                    Err(error) => {
                        vm.fail_native_call(error);
                        Value::Nil
                    }
                }
            });
            return Ok(Value::NativeFunction(constructor));
        }
        if !indexed {
            return Ok(Value::String(kind));
        }
        // `(track 3)`: the instance registered under key [3], or nil.
        let constructor = super::NativeFunction::new(name.clone(), move |args, vm| {
            match vm.keyed_constructor_call(&kind, &namespace, &name, &args) {
                Ok(instance) => instance,
                Err(error) => {
                    vm.fail_native_call(error);
                    Value::Nil
                }
            }
        });
        Ok(Value::NativeFunction(constructor))
    }

    /// `(track i)`: the instance under key `[i]` of `kind`, or nil, with a
    /// dependency on that key's source (field `i` of `namespace`, the
    /// kind's `%keys/<kind>`) so the reader re-runs when a track `i`
    /// appears, goes or moves.
    fn keyed_constructor_call(
        &mut self,
        kind: &str,
        namespace: &str,
        name: &str,
        args: &[Value],
    ) -> Result<Value, VMError> {
        if self.active_expander.is_some() {
            return Err(self.expansion_error("keyed instance lookup"));
        }
        let index = match args {
            [Value::Number(index)]
                if *index >= 0.0 && index.fract() == 0.0 && index.is_finite() =>
            {
                *index as u64
            }
            _ => {
                return Err(VMError::Instance(format!(
                    "({name} i) takes one non-negative integer; got ({name}{})",
                    args.iter()
                        .map(|arg| format!(" {}", describe_value(arg)))
                        .collect::<String>()
                )));
            }
        };
        let value = self
            .instances
            .keyed_id(kind, &[index])
            .map_or(Value::Nil, Value::Instance);
        self.track_instance_source_read(namespace, &index.to_string(), &value);
        Ok(value)
    }

    /// `(adsr-gesture scope section)` (eseq-0l17.62): the instance of
    /// view-local kind `kind` under the key the arguments make, created at
    /// its fields' defaults on first call, with a dependency on that key's
    /// source (field `<key text>` of `namespace`) so the reader re-runs when
    /// the instance is dropped. A key part that is a dropped instance answers
    /// nil (its view-local children went with it).
    fn local_constructor_call(
        &mut self,
        kind: &str,
        namespace: &str,
        name: &str,
        args: &[Value],
    ) -> Result<Value, VMError> {
        if self.active_expander.is_some() {
            return Err(self.expansion_error("view-local instance lookup"));
        }
        let names = match self.instances.kinds.get(kind).map(|schema| &schema.key) {
            Some(KindKey::Local { names }) => names.clone(),
            _ => {
                return Err(VMError::Instance(format!(
                    "{name}: kind '{kind}' is not view-local"
                )));
            }
        };
        let shape = || {
            VMError::Instance(format!(
                "({name} {}) takes {} key value{} (numbers, strings, keywords, symbols, booleans \
                 or instances); got ({name}{})",
                names.join(" "),
                names.len(),
                if names.len() == 1 { "" } else { "s" },
                args.iter()
                    .map(|arg| format!(" {}", describe_value(arg)))
                    .collect::<String>()
            ))
        };
        if args.len() != names.len() {
            return Err(shape());
        }
        let key: Rc<[LocalKeyPart]> = args
            .iter()
            .map(LocalKeyPart::from_value)
            .collect::<Option<Vec<_>>>()
            .ok_or_else(shape)?
            .into();
        let field = local_key_text(&key);
        let live_parents = local_key_parents(&key).all(|id| self.instances.live.contains_key(&id));
        let existing = self
            .instances
            .local
            .get(kind)
            .and_then(|ids| ids.get(&key))
            .copied();
        let value = match existing {
            _ if !live_parents => Value::Nil,
            Some(id) => Value::Instance(id),
            None => {
                let id = KEYED_INSTANCE_ID_BASE + self.instances.keyed_allocated;
                self.instances.keyed_allocated += 1;
                self.insert_instance_record(id, kind, None)?;
                if let Some(record) = self.instances.live.get_mut(&id) {
                    record.local_key = Some(key.clone());
                }
                let kind_rc = self.instances.kind_rc(kind).unwrap_or_else(|| kind.into());
                self.instances
                    .local
                    .entry(kind_rc)
                    .or_default()
                    .insert(key.clone(), id);
                for parent in local_key_parents(&key) {
                    self.instances.set_parent(id, None, Some(parent));
                }
                self.dirty_namespace_field(namespace, &field, Value::Instance(id));
                Value::Instance(id)
            }
        };
        self.track_instance_source_read(namespace, &field, &value);
        Ok(value)
    }

    /// The live instances of view-local kind `kind` (its kind id or a
    /// unique bare name), sorted (eseq-0l17.62).
    pub fn local_instances(&self, kind: &str) -> Vec<InstanceId> {
        let kind = match self.instances.kinds.get_key_value(kind) {
            Some((kind, _)) => kind.clone(),
            None => match self
                .instances
                .kinds_named(kind)
                .find(|(_, schema)| schema.is_local())
            {
                Some((kind, _)) => kind.clone(),
                None => return Vec::new(),
            },
        };
        let mut ids: Vec<InstanceId> = self
            .instances
            .local
            .get(&kind)
            .map(|ids| ids.values().copied().collect())
            .unwrap_or_default();
        ids.sort_unstable();
        ids
    }

    /// `(drop-instance x)` (eseq-0l17.62): drop a view-local instance (as
    /// [`Self::drop_instance`]). Whether it was live; any other value is an
    /// error, since host and singleton instances are not Lisp's to drop.
    pub(super) fn drop_local_instance(&mut self, value: &Value) -> Result<bool, VMError> {
        let local = match value {
            Value::Instance(id) => self
                .instances
                .kind_of(*id)
                .and_then(|kind| self.instances.kinds.get(kind))
                .is_some_and(InstanceKindSchema::is_local),
            _ => false,
        };
        match value {
            Value::Instance(id) if local => Ok(self.drop_instance(*id)),
            other => Err(VMError::Instance(format!(
                "(drop-instance x) takes an instance of a view-local kind (a :key without \
                 :host); got {}",
                self.instance_display_or_value(other)
            ))),
        }
    }

    fn instance_display_or_value(&self, value: &Value) -> String {
        match value {
            Value::Instance(id) => self
                .instances
                .display(*id)
                .unwrap_or_else(|| format!("<instance:{id}>")),
            other => describe_value(other),
        }
    }

    /// `(describe-kind 'track)` (kind-bindings spec §10, eseq-0l17.23): the
    /// kind `name` names (its kind id, or a bare name: the current module's
    /// kind first, else the one kind with that name) as text, one line for
    /// the kind (id, how its instances come, built-in fields) and one per
    /// declared field: group, name, type, then `:default`, `:set`,
    /// `:range` and `:doc` where it has them.
    pub fn describe_kind(&self, name: &str) -> Result<String, String> {
        let module = self.current_module_name();
        let local = kind_id(None, Some(module), name);
        let schema = match self
            .instances
            .kinds
            .get(name)
            .or_else(|| self.instances.kinds.get(local.as_str()))
        {
            Some(schema) => schema,
            None => {
                let mut found: Vec<&InstanceKindSchema> = self
                    .instances
                    .kinds_named(name)
                    .map(|(_, schema)| schema)
                    .collect();
                found.sort_by(|a, b| a.kind.cmp(&b.kind));
                match found.as_slice() {
                    [schema] => *schema,
                    [] => return Err(format!("describe-kind: no kind named '{name}'")),
                    several => {
                        return Err(format!(
                            "describe-kind: kind name '{name}' is ambiguous ({}); use its kind id",
                            several
                                .iter()
                                .map(|schema| schema.kind.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        ));
                    }
                }
            }
        };
        let how = match &schema.key {
            KindKey::Created => "created (no :key)".to_string(),
            key => key.describe(),
        };
        let mut lines = vec![format!(
            "kind {}: {how}; built-in fields: {}",
            schema.kind,
            schema.builtin_fields().join(", ")
        )];
        let mut extras = Vec::new();
        if let Some(keymap) = &schema.keymap {
            extras.push(format!(":keymap {keymap}"));
        }
        if schema.view.is_some() {
            extras.push(":view".to_string());
        }
        if schema.on_create.is_some() {
            extras.push(":on-create".to_string());
        }
        if !extras.is_empty() {
            lines.push(format!("  {}", extras.join(" ")));
        }
        let field_line = |group: &str, field: &KindField, options: &[String]| {
            let mut line = format!("  {group:<9} {:<14} {}", field.name, field.ty);
            for option in options {
                line.push_str("  ");
                line.push_str(option);
            }
            line
        };
        for host in &schema.host {
            let mut options = Vec::new();
            if let Some(set) = &host.set {
                options.push(format!(":set {}", self.callable_name(set)));
            }
            if let Some((lo, hi)) = host.range {
                options.push(format!(
                    ":range ({} {})",
                    super::format_lisp_source(&Value::Number(lo)),
                    super::format_lisp_source(&Value::Number(hi))
                ));
            }
            if let Some(doc) = &host.doc {
                options.push(format!(":doc {doc:?}"));
            }
            lines.push(field_line(":host", &host.field, &options));
        }
        for (group, fields) in [(":state", &schema.fields), (":document", &schema.document)] {
            for field in fields {
                let default = format!(":default {}", self.format_value(&field.default));
                lines.push(field_line(group, field, &[default]));
            }
        }
        Ok(lines.join("\n"))
    }

    /// How a `:set` function reads in [`Self::describe_kind`]: its name.
    fn callable_name(&self, value: &Value) -> String {
        match value {
            Value::NativeFunction(native) => native.name.clone(),
            Value::Closure(chunk, _) | Value::Function(chunk) => self
                .chunks
                .get(*chunk)
                .and_then(|chunk| chunk.source_symbol.clone())
                .unwrap_or_else(|| "(lambda)".to_string()),
            Value::HostHandle { kind, id, .. } => format!("<{kind}:{id}>"),
            other => self.format_value(other),
        }
    }

    // ---- keyed registry (kind-bindings spec §4, §9) --------------------

    /// Register the instance of keyed kind `kind` (its kind id, or a bare
    /// name exactly one keyed kind has) under `key`, or return the one
    /// already there. A `:key (parent index)` key is the parent instance's
    /// id and the index: the parent must be a live instance of the parent
    /// kind, which is resolved at the kind's first registration (so kinds
    /// may be declared in any order). The instance starts with its `:host`
    /// fields at their type defaults until the host pushes values
    /// ([`Self::set_instance_field`]).
    pub fn register_keyed_instance(
        &mut self,
        kind: &str,
        key: &[u64],
    ) -> Result<InstanceId, InstanceError> {
        let (kind, schema) = self.instances.resolve_keyed_kind(kind)?;
        if let Some(id) = self.instances.keyed_id(kind, key) {
            return Ok(id);
        }
        let indexed = matches!(schema.key, KindKey::Indexed { .. });
        let kind = kind.clone();
        let parent = self.instances.check_key(&kind, key)?;
        let id = KEYED_INSTANCE_ID_BASE + self.instances.keyed_allocated;
        self.instances.keyed_allocated += 1;
        self.insert_instance_record(id, &kind, Some(key.to_vec()))?;
        self.instances
            .keyed
            .entry(kind.clone())
            .or_insert_with(|| KeyRegistry {
                ids: HashMap::new(),
                namespace: indexed.then(|| key_namespace(&kind)),
            })
            .ids
            .insert(key.to_vec(), id);
        self.instances.set_parent(id, None, parent);
        self.publish_key_source(&kind, key);
        Ok(id)
    }

    /// The live instance of keyed kind `kind` (its kind id or unique bare
    /// name) under `key`, untracked.
    pub fn keyed_instance(&self, kind: &str, key: &[u64]) -> Option<InstanceId> {
        let (kind, _) = self.instances.resolve_keyed_kind(kind).ok()?;
        self.instances.keyed_id(kind, key)
    }

    /// The one instance of singleton kind `kind` (its kind id), once its
    /// `def-kind` has run.
    pub fn singleton_instance(&self, kind: &str) -> Option<InstanceId> {
        self.instances
            .singletons
            .get(kind)
            .copied()
            .filter(|id| self.instances.live.contains_key(id))
    }

    /// The live children of `parent` of kind `kind` (a kind id, as
    /// [`Self::keyed_instance`] takes) with their keys, in no particular
    /// order and without allocating (for a host's drop loops).
    pub fn keyed_children_of_kind<'s>(
        &'s self,
        parent: InstanceId,
        kind: &'s str,
    ) -> impl Iterator<Item = (InstanceId, &'s [u64])> + 's {
        self.instances
            .children
            .get(&parent)
            .into_iter()
            .flatten()
            .filter_map(move |id| {
                let record = self.instances.live.get(id)?;
                (&*record.kind == kind).then_some((*id, record.key.as_deref()?))
            })
    }

    /// The live children of `parent` (instances of `:key (parent index)`
    /// kinds under it, and view-local instances keyed by it), sorted.
    pub fn keyed_children(&self, parent: InstanceId) -> Vec<InstanceId> {
        let mut ids: Vec<InstanceId> = self
            .instances
            .children
            .get(&parent)
            .map(|children| children.iter().copied().collect())
            .unwrap_or_default();
        ids.sort_unstable();
        ids
    }

    /// The current key of a live keyed instance.
    pub fn instance_key(&self, id: InstanceId) -> Option<&[u64]> {
        self.instances.live.get(&id)?.key.as_deref()
    }

    /// Drop the instance of keyed kind `kind` under `key` (as
    /// [`Self::drop_instance`]). Returns whether there was one.
    pub fn drop_keyed_instance(&mut self, kind: &str, key: &[u64]) -> bool {
        match self.keyed_instance(kind, key) {
            Some(id) => self.drop_instance(id),
            None => false,
        }
    }

    /// Re-key one keyed instance (a reorder): its id, and so every handle
    /// to it, keeps meaning the same thing; `(track i)` answers change.
    pub fn rekey_instance(&mut self, id: InstanceId, key: &[u64]) -> Result<(), InstanceError> {
        self.rekey_instances(&[(id, key.to_vec())])
    }

    /// Re-key several keyed instances at once, so a reorder can swap keys
    /// (`[(a, [4]), (b, [3])]`). Nothing changes unless every move fits:
    /// each id a live keyed instance named once, each key valid for its
    /// kind, and no two instances of a kind left under one key. A child
    /// moved to another parent (`[(step, [t2, 0])]`) is dropped with its
    /// new parent from then on.
    pub fn rekey_instances(
        &mut self,
        moves: &[(InstanceId, InstanceKey)],
    ) -> Result<(), InstanceError> {
        // Each move's kind, old key and new parent.
        let mut plan: Vec<(Rc<str>, InstanceKey, Option<InstanceId>)> =
            Vec::with_capacity(moves.len());
        let mut moved = HashSet::with_capacity(moves.len());
        for (id, key) in moves {
            if !moved.insert(*id) {
                return Err(InstanceError::InvalidKey(format!(
                    "instance {id} is re-keyed twice in one move"
                )));
            }
            let (kind, old) = self
                .instances
                .live
                .get(id)
                .and_then(|record| Some((record.kind.clone(), record.key.clone()?)))
                .ok_or(InstanceError::UnknownInstance(*id))?;
            let parent = self.instances.check_key(&kind, key)?;
            plan.push((kind, old, parent));
        }
        // Conflicts, against the registry overlaid with the moves: a key
        // vacated by a mover is free, and no two movers share a target.
        let vacated: HashSet<(&str, &[u64])> = plan
            .iter()
            .map(|(kind, old, _)| (&**kind, old.as_slice()))
            .collect();
        let mut targets: HashMap<(&str, &[u64]), InstanceId> = HashMap::with_capacity(moves.len());
        for ((id, key), (kind, _, _)) in moves.iter().zip(&plan) {
            let target = (&**kind, key.as_slice());
            let held = match targets.insert(target, *id) {
                Some(other) => Some(other),
                None => self
                    .instances
                    .keyed_id(kind, key)
                    .filter(|held| held != id && !vacated.contains(&target)),
            };
            if let Some(held) = held {
                return Err(InstanceError::InvalidKey(format!(
                    "kind '{kind}': key ({}) is already held by instance {held}",
                    key_text(key)
                )));
            }
        }
        // Apply: vacate every old key first, so swaps land.
        for ((id, _), (kind, old, _)) in moves.iter().zip(&plan) {
            if let Some(registry) = self.instances.keyed.get_mut(kind)
                && registry.ids.get(old) == Some(id)
            {
                registry.ids.remove(old);
            }
        }
        for ((id, key), (kind, old, parent)) in moves.iter().zip(&plan) {
            if let Some(registry) = self.instances.keyed.get_mut(kind) {
                registry.ids.insert(key.clone(), *id);
            }
            if let Some(record) = self.instances.live.get_mut(id) {
                record.key = Some(key.clone());
            }
            self.instances.set_parent(*id, key_parent(old), *parent);
        }
        for ((id, key), (kind, old, _)) in moves.iter().zip(&plan) {
            if old != key {
                self.publish_key_source(kind, old);
                self.publish_key_source(kind, key);
                self.publish_instance_field(*id, "key", key_value(key));
            }
        }
        Ok(())
    }

    /// Advance the source of one key of an index-keyed `kind` to the
    /// instance it names now (parent-keyed kinds have no key sources).
    fn publish_key_source(&mut self, kind: &str, key: &[u64]) {
        let Some(registry) = self.instances.keyed.get(kind) else {
            return;
        };
        let (Some(namespace), [index]) = (&registry.namespace, key) else {
            return;
        };
        let value = registry
            .ids
            .get(key)
            .map_or(Value::Nil, |id| Value::Instance(*id));
        let namespace = namespace.clone();
        self.dirty_namespace_field(&namespace, &index.to_string(), value);
    }

    /// `<track#41 [3]>`-style printing for an instance of a kind with a
    /// `:key`; `None` for a created kind's instance.
    pub(crate) fn instance_display(&self, id: InstanceId) -> Option<String> {
        self.instances.display(id)
    }

    /// Drop an instance. Its handle turns stale: readers are dirtied and see
    /// defaults, writes become no-ops. Returns whether it was live.
    /// A keyed instance leaves its key (readers of `(track i)` re-run), its
    /// children (instances of `:key (parent index)` kinds under it) are
    /// dropped with it, and its field sources nothing reads any more are
    /// freed (its id is never reused).
    pub fn drop_instance(&mut self, id: InstanceId) -> bool {
        let Some(record) = self.instances.live.remove(&id) else {
            return false;
        };
        let keyed = record.key.is_some() || record.local_key.is_some();
        if let Some(key) = &record.local_key {
            if let Some(ids) = self.instances.local.get_mut(&record.kind)
                && ids.get(key) == Some(&id)
            {
                ids.remove(key);
            }
            for parent in local_key_parents(key) {
                self.instances.set_parent(id, Some(parent), None);
            }
            let field = local_key_text(key);
            self.dirty_namespace_field(&key_namespace(&record.kind), &field, Value::Nil);
        }
        if let Some(key) = &record.key {
            if let Some(registry) = self.instances.keyed.get_mut(&record.kind)
                && registry.ids.get(key) == Some(&id)
            {
                registry.ids.remove(key);
            }
            self.instances.set_parent(id, key_parent(key), None);
            self.publish_key_source(&record.kind, key);
        }
        let children = self.instances.children.remove(&id);
        self.instances.dropped.insert(id, record.kind);
        self.republish_instance_sources(id, false);
        // Held bindings read the stale defaults; the store forgets the slots
        // (a revived id binds fresh ones).
        self.sync_bound_slots(Some(id));
        if let Some(fields) = self.bound_instance_fields.remove(&id) {
            let namespace = instance_namespace(id);
            for field in fields.keys() {
                self.reactive_float_slots
                    .remove_field_slots(&namespace, field);
            }
        }
        if keyed {
            self.free_unread_instance_sources(id);
        }
        if let Some(children) = children {
            let mut children: Vec<InstanceId> = children.into_iter().collect();
            children.sort_unstable();
            for child in children {
                self.drop_instance(child);
            }
        }
        true
    }

    /// Remove the field sources of instance `id` that no reader depends on.
    fn free_unread_instance_sources(&mut self, id: InstanceId) {
        let Some(fields) = self
            .dag
            .namespace_field_sources
            .get(&instance_namespace(id))
        else {
            return;
        };
        let unread: Vec<super::NodeId> = fields
            .values()
            .copied()
            .filter(|node| {
                matches!(
                    self.dag.nodes.get(node),
                    Some(ReactiveNode::Source { dependents, .. }) if dependents.is_empty()
                )
            })
            .collect();
        for node in unread {
            self.dag.remove_node(node);
        }
    }

    pub fn instance_is_live(&self, id: InstanceId) -> bool {
        self.instances.live.contains_key(&id)
    }

    pub fn instance_kind(&self, id: InstanceId) -> Option<&str> {
        self.instances.kind_of(id)
    }

    /// Live host-created instance ids, sorted. Instances of kinds with a
    /// `:key` (singletons, keyed projections) are not project instances and
    /// are left out, so host syncs never drop them.
    pub fn live_instances(&self) -> Vec<InstanceId> {
        let mut ids: Vec<InstanceId> = self
            .instances
            .live
            .iter()
            .filter(|(_, record)| {
                self.instances
                    .kinds
                    .get(&record.kind)
                    .is_none_or(|schema| schema.key == KindKey::Created)
            })
            .map(|(id, _)| *id)
            .collect();
        ids.sort_unstable();
        ids
    }

    /// Push a created kind's host-owned built-in field (`owner`, `label`)
    /// and dirty its readers.
    pub fn set_instance_builtin_field(
        &mut self,
        id: InstanceId,
        field: InstanceBuiltinField,
        value: Value,
    ) -> Result<(), InstanceError> {
        let record = self
            .instances
            .live
            .get_mut(&id)
            .ok_or(InstanceError::UnknownInstance(id))?;
        let stored = value.deep_clone();
        match field {
            InstanceBuiltinField::Owner => record.owner = stored,
            InstanceBuiltinField::Label => record.label = stored,
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
        self.instances.field_value(id, field)
    }

    /// The host's write of a field: a `:host` field's push (kind-bindings
    /// spec §9: the cell, its readers, and its slot when bound), or any other
    /// field exactly as Lisp `(set! x.field v)` would write it (for example
    /// to seed per-instance evaluated defaults). Type-checked like every
    /// write; a stale instance ignores it.
    pub fn set_instance_field(
        &mut self,
        id: InstanceId,
        field: &str,
        value: Value,
    ) -> Result<(), InstanceError> {
        match self.checked_write_slot(id, field, &value)? {
            Some(slot) => self.write_slot(id, field, slot, value),
            None => Ok(()),
        }
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
            .ok_or_else(|| InstanceError::UnknownKind(record.kind.to_string()))?;
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
            .filter(|(_, record)| &*record.kind == kind)
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
        let resolved = self.instances.resolve(id, field)?;
        if let FieldSlot::Document(index) = resolved.slot
            && let Some(read) = self.instance_doc_native(INSTANCE_DOC_READ_NATIVE)
        {
            // The host resolves the current pattern's value and injects
            // its own reactive edge (the scene-slot source).
            let default = self.instances.document_default(resolved.kind, index);
            return Ok(read(
                vec![
                    Value::Number(id as f64),
                    Value::String(field.to_string()),
                    default,
                ],
                self,
            ));
        }
        let value = if resolved.is_live_host() {
            self.refresh_cold_host_field(id, field)?;
            self.instances.field_value(id, field)?
        } else {
            self.instances.value_of(id, &resolved)
        };
        self.track_instance_source_read(&instance_namespace(id), field, &value);
        Ok(value)
    }

    /// Before a read of a live instance's `:host` field nothing observes:
    /// the host skips computing unobserved fields (kind-bindings spec D3),
    /// so ask its [`HostFieldReader`] for the current value and store it.
    /// Returns whether the cell may have changed.
    fn refresh_cold_host_field(&mut self, id: InstanceId, field: &str) -> Result<bool, VMError> {
        let Some(reader) = self.host_field_reader.clone() else {
            return Ok(false);
        };
        if self.host_field_observed(id, field) {
            return Ok(false);
        }
        let Some(value) = reader(self, id, field) else {
            return Ok(false);
        };
        if self
            .instances
            .field_value(id, field)
            .is_ok_and(|cell| cell == value)
        {
            return Ok(false);
        }
        self.set_instance_field(id, field, value)?;
        Ok(true)
    }

    /// Whether anything observes `field` of instance `id` (kind-bindings
    /// spec §9, D3): its DAG source has readers, or a `#'` binding to it is
    /// held outside the slot store (by a widget or a Lisp value). The host
    /// computes and pushes only observed `:host` fields.
    pub fn host_field_observed(&self, id: InstanceId, field: &str) -> bool {
        self.host_fields_observed(id, &[field]) != 0
    }

    /// [`Self::host_field_observed`] for several fields of one instance at
    /// once: bit `i` of the result is set when `fields[i]` is observed. The
    /// namespace is formatted once and the slot store locked once. At most
    /// [`MAX_OBSERVED_FIELDS`] fields.
    pub fn host_fields_observed(&self, id: InstanceId, fields: &[&str]) -> ObservedMask {
        assert!(
            fields.len() <= MAX_OBSERVED_FIELDS,
            "host_fields_observed takes at most {MAX_OBSERVED_FIELDS} fields, got {}",
            fields.len()
        );
        let namespace = instance_namespace(id);
        let sources = self.dag.namespace_field_sources.get(&namespace);
        let mut mask: ObservedMask = 0;
        for (bit, field) in fields.iter().enumerate() {
            let read = sources
                .and_then(|sources| sources.get(*field))
                .is_some_and(|node| {
                    matches!(
                        self.dag.nodes.get(node),
                        Some(ReactiveNode::Source { dependents, .. }) if !dependents.is_empty()
                    )
                });
            if read {
                mask |= 1 << bit;
            }
        }
        if let Some(bound) = self.bound_instance_fields.get(&id) {
            let unread = fields
                .iter()
                .enumerate()
                .filter_map(|(bit, field)| {
                    (mask & (1 << bit) == 0)
                        .then(|| bound.get(*field).map(|kind| (bit, *field, *kind)))
                        .flatten()
                });
            mask |= self
                .reactive_float_slots
                .binding_slots_held(&namespace, unread);
        }
        mask
    }

    /// Whether any live child of `parent` of kind `kind` has one of
    /// `fields` observed ([`Self::host_fields_observed`]).
    #[cfg(test)]
    pub fn keyed_children_observed(&self, parent: InstanceId, kind: &str, fields: &[&str]) -> bool {
        self.keyed_children_of_kind(parent, kind)
            .any(|(child, _)| self.host_fields_observed(child, fields) != 0)
    }

    /// Bumped whenever an instance field may have gained an observer (a
    /// tracked read adds a reader edge, or a `#'` binding is handed out).
    /// Losing one does not bump it, so a host caching "nothing observes
    /// these" until it moves never misses a new observer.
    pub fn instance_observer_epoch(&self) -> u64 {
        self.instance_observer_epoch
    }

    /// Bumped whenever a kind schema is registered or changed by a
    /// re-registration (not an identical one,
    /// [`InstanceKindSchema::same_declaration`]) or rolled back, so a host
    /// can re-check the kinds it publishes after a hot reload.
    pub fn instance_kind_schema_generation(&self) -> u64 {
        self.kind_schema_generation
    }

    /// Install (or remove) the host's answer to reads of unobserved `:host`
    /// fields ([`HostFieldReader`]).
    pub fn set_host_field_reader(&mut self, reader: Option<HostFieldReader>) {
        self.host_field_reader = reader;
    }

    /// Reserve kind names for `module` (kind-bindings spec §3.4: the host
    /// kinds of `eseq.kinds`): defining a kind with one of these names in
    /// any other module is an error. Existing kinds are not checked.
    pub fn reserve_kind_names(&mut self, module: &str, names: &[&str]) {
        for name in names {
            self.reserved_kind_names
                .insert((*name).to_string(), module.to_string());
        }
    }

    /// Record a read of the store-backed source `(namespace, field)`, whose
    /// current value is `value`: the effect/subtree read set and, while
    /// something is being tracked, a DAG edge from the source to it.
    fn track_instance_source_read(&mut self, namespace: &str, field: &str, value: &Value) {
        self.record_reactive_read(namespace, field);
        if let Some(ctx_id) = self.tracking_stack.last().copied() {
            let source_id = self.get_or_create_instance_source_node(namespace, field, value);
            self.dag.add_edge(source_id, ctx_id);
            self.note_rerender_read_site(source_id, ctx_id);
            self.instance_observer_epoch += 1;
        }
    }

    /// `StoreField` on an instance: `(set! x.field v)`. A `:host` field
    /// with `:set f` calls `(f x v)` and leaves the cell to the host, which
    /// pushes the accepted value back (kind-bindings spec §6); without one
    /// it is read-only.
    pub(super) fn write_instance_field(
        &mut self,
        id: InstanceId,
        field: &str,
        value: Value,
    ) -> Result<(), VMError> {
        let checked = match self.checked_write_slot(id, field, &value) {
            // A numeric `:host` field's `:set` takes true/false too (an on/off
            // param's `(set! p.base true)`): the setter converts it.
            Err(InstanceError::TypeMismatch { .. })
                if matches!(value, Value::Bool(_)) && self.numeric_host_setter(id, field) =>
            {
                let resolved = self.instances.resolve(id, field)?;
                resolved.record.map(|_| resolved.slot)
            }
            checked => checked?,
        };
        let Some(slot) = checked else {
            return Ok(());
        };
        let FieldSlot::Host(index) = slot else {
            return Ok(self.write_slot(id, field, slot, value)?);
        };
        let kind = self.instances.kind_of(id).unwrap_or_default();
        let setter = self
            .instances
            .kinds
            .get(kind)
            .and_then(|schema| schema.host.get(index))
            .and_then(|declared| declared.set.clone());
        let Some(setter) = setter else {
            return Err(InstanceError::ReadOnlyHostField {
                kind: kind.to_string(),
                field: field.to_string(),
            }
            .into());
        };
        self.invoke(setter, vec![Value::Instance(id), value])?;
        Ok(())
    }

    /// Whether `field` of `id` is a `:number`/`:int` `:host` field with a
    /// `:set` function.
    fn numeric_host_setter(&self, id: InstanceId, field: &str) -> bool {
        let Ok(resolved) = self.instances.resolve(id, field) else {
            return false;
        };
        let FieldSlot::Host(index) = resolved.slot else {
            return false;
        };
        let numeric = resolved
            .declared
            .is_some_and(|declared| matches!(declared.ty, FieldType::Number | FieldType::Int));
        let setter = (self.instances.kinds.get(resolved.kind))
            .and_then(|schema| schema.host.get(index))
            .is_some_and(|declared| declared.set.is_some());
        numeric && setter
    }

    /// Resolve a write of `value` to `field` and type-check it: the cell to
    /// write, or `None` for a stale instance (writes to it are no-ops: an
    /// event handler outliving its instance).
    fn checked_write_slot(
        &self,
        id: InstanceId,
        field: &str,
        value: &Value,
    ) -> Result<Option<FieldSlot>, InstanceError> {
        let resolved = self.instances.resolve(id, field)?;
        if resolved.record.is_none() {
            return Ok(None);
        }
        if let Some(declared) = resolved.declared {
            self.instances
                .check_write(resolved.kind, field, declared, value)?;
        }
        Ok(Some(resolved.slot))
    }

    /// Store a checked write in a live instance's cell (a `:host` cell is
    /// the host's push) and publish it.
    fn write_slot(
        &mut self,
        id: InstanceId,
        field: &str,
        slot: FieldSlot,
        value: Value,
    ) -> Result<(), InstanceError> {
        let index = match slot {
            FieldSlot::Id | FieldSlot::Kind | FieldSlot::Owner | FieldSlot::Key => {
                return Err(InstanceError::ReadOnlyField {
                    kind: self.instances.kind_of(id).unwrap_or_default().to_string(),
                    field: field.to_string(),
                });
            }
            FieldSlot::Label => {
                match self.instances.label_hook.clone() {
                    Some(hook) => hook(id, &value),
                    None => {
                        self.set_instance_builtin_field(id, InstanceBuiltinField::Label, value)?
                    }
                }
                return Ok(());
            }
            FieldSlot::Host(index) | FieldSlot::State(index) | FieldSlot::Document(index) => index,
        };
        if matches!(slot, FieldSlot::Document(_))
            && let Some(write) = self.instance_doc_native(INSTANCE_DOC_WRITE_NATIVE)
        {
            write(
                vec![
                    Value::Number(id as f64),
                    Value::String(field.to_string()),
                    value,
                ],
                self,
            );
            return Ok(());
        }
        if let Some(record) = self.instances.live.get_mut(&id) {
            let cells = match slot {
                FieldSlot::Host(_) => &mut record.host,
                FieldSlot::State(_) => &mut record.state,
                _ => &mut record.document,
            };
            if let Some(cell) = cells.get_mut(index) {
                *cell = value.deep_clone();
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
    /// its readers, and write its binding slot if it has one. Unread,
    /// unbound fields allocate nothing but the namespace name.
    fn publish_instance_field(&mut self, id: InstanceId, field: &str, value: Value) {
        let namespace = instance_namespace(id);
        self.write_bound_slot(&namespace, id, field, &value);
        self.dirty_namespace_field(&namespace, field, value);
    }

    /// Advance the source `(namespace, field)`, if anything ever read it,
    /// to `value`, dirtying its readers when that changes it.
    fn dirty_namespace_field(&mut self, namespace: &str, field: &str, value: Value) {
        if let Some(source_id) = self.dag.find_namespace_field_source_node(namespace, field) {
            self.mark_source_dependents_dirty(source_id, value);
        }
    }

    /// Write a bound field's slot(s) (`namespace` is the instance's); queue
    /// a repaint of its widgets when that changed them.
    fn write_bound_slot(&mut self, namespace: &str, id: InstanceId, field: &str, value: &Value) {
        let Some(&kind) = self
            .bound_instance_fields
            .get(&id)
            .and_then(|fields| fields.get(field))
        else {
            return;
        };
        let (_, changed) = self
            .reactive_float_slots
            .write_binding(namespace, field, kind, value);
        if changed {
            self.pending_binding_repaints
                .insert((id, field.to_string()));
        }
    }

    /// Rewrite the bound slots of instance `id` (every instance for `None`)
    /// from the store, after a kind re-registration, a drop or a rollback
    /// changed what its fields answer. A field whose new type is no longer
    /// slot-backed loses its slot.
    pub(super) fn sync_bound_slots(&mut self, id: Option<InstanceId>) {
        let ids: Vec<InstanceId> = match id {
            Some(id) if self.bound_instance_fields.contains_key(&id) => vec![id],
            Some(_) => return,
            None => self.bound_instance_fields.keys().copied().collect(),
        };
        for id in ids {
            let namespace = instance_namespace(id);
            let fields: Vec<String> = self
                .bound_instance_fields
                .get(&id)
                .map(|fields| fields.keys().cloned().collect())
                .unwrap_or_default();
            for field in fields {
                let resolved = self
                    .instances
                    .resolve(id, &field)
                    .ok()
                    .and_then(|resolved| {
                        let binding = resolved.declared?.ty.binding_kind(id)?;
                        Some((self.instances.value_of(id, &resolved), binding))
                    });
                let Some(bound) = self.bound_instance_fields.get_mut(&id) else {
                    continue;
                };
                match resolved {
                    Some((value, binding)) => {
                        bound.insert(field.clone(), binding);
                        self.write_bound_slot(&namespace, id, &field, &value);
                    }
                    None => {
                        bound.remove(&field);
                        if bound.is_empty() {
                            self.bound_instance_fields.remove(&id);
                        }
                        self.reactive_float_slots
                            .remove_field_slots(&namespace, &field);
                    }
                }
            }
        }
    }

    /// Whether a bound slot changed since the last
    /// [`Self::take_pending_binding_repaints`].
    pub fn has_pending_binding_repaints(&self) -> bool {
        !self.pending_binding_repaints.is_empty()
    }

    /// `(namespace, field)` of every bound slot that changed since the last
    /// call: the host repaints the widgets bound to them (no re-render).
    pub fn take_pending_binding_repaints(&mut self) -> Vec<(String, String)> {
        self.pending_binding_repaints
            .drain()
            .map(|(id, field)| (instance_namespace(id), field))
            .collect()
    }

    /// `#'x.field` (kind-bindings spec §7.1): a binding to a slot-backed
    /// field of instance `id`. An unknown field or a type `#'` cannot bind
    /// is an error naming the kind, the field and the bindable fields.
    pub(super) fn instance_field_ref(
        &mut self,
        id: InstanceId,
        field: &str,
    ) -> Result<Value, VMError> {
        let namespace = instance_namespace(id);
        Ok(match self.prepare_field_binding(id, field, "#'")? {
            PreparedBinding::Stale { binding, value } => {
                crate::reactive::detached_binding_ref(namespace, field, binding, &value)
            }
            PreparedBinding::Bound { binding, seed } => {
                let slot = match seed {
                    Some(value) => {
                        self.reactive_float_slots
                            .write_binding(&namespace, field, binding, &value)
                            .0
                    }
                    None => self
                        .reactive_float_slots
                        .binding_slot(&namespace, field, binding),
                };
                Value::ReactiveRef {
                    namespace,
                    field: field.to_string(),
                    index: None,
                    kind: binding,
                    slot,
                }
            }
        })
    }

    /// [`Self::instance_field_ref`] with one ref per slot: one for a float
    /// field, the r, g and b slots of an `:rgb` field (a shader reads each
    /// as a uniform, kind-bindings spec §7.3). A stale instance gets
    /// detached slots, so nothing enters the store under its namespace.
    /// `who` prefixes an error (the widget).
    pub(super) fn instance_field_refs(
        &mut self,
        id: InstanceId,
        field: &str,
        who: &str,
    ) -> Result<Vec<Value>, VMError> {
        let namespace = instance_namespace(id);
        let (binding, slots) = match self.prepare_field_binding(id, field, who)? {
            PreparedBinding::Stale { binding, value } => {
                return Ok(crate::reactive::detached_binding_refs(
                    &namespace, field, binding, &value,
                ));
            }
            PreparedBinding::Bound { binding, seed } => (
                binding,
                self.reactive_float_slots
                    .binding_slots(&namespace, field, binding, seed.as_ref()),
            ),
        };
        Ok(slots
            .into_iter()
            .map(|slot| Value::ReactiveRef {
                namespace: namespace.clone(),
                field: field.to_string(),
                index: None,
                kind: binding,
                slot,
            })
            .collect())
    }

    /// What `#'` checks and refreshes before handing out a field's slots:
    /// the field must be bindable; a cold `:host` field of a live instance
    /// is read from the host first. A field already bound keeps its slots
    /// current on every change, so it is not read again (no seed).
    fn prepare_field_binding(
        &mut self,
        id: InstanceId,
        field: &str,
        who: &str,
    ) -> Result<PreparedBinding, VMError> {
        let host_documents = self.instance_doc_native(INSTANCE_DOC_READ_NATIVE).is_some();
        let (binding, stale, live_host) = {
            let resolved = match self.instances.resolve(id, field) {
                Ok(resolved) => resolved,
                // Reported below with the bindable fields.
                Err(InstanceError::UnknownField { .. }) => {
                    let kind = self.instances.kind_of(id).unwrap_or_default();
                    let reason = self
                        .instances
                        .kinds
                        .get(kind)
                        .map(|schema| {
                            InstanceStore::field_binding(schema, id, field, host_documents)
                        })
                        .and_then(Result::err)
                        .unwrap_or_else(|| format!("kind '{kind}' has no field '{field}'"));
                    return Err(VMError::Instance(format!("{who}: {reason}")));
                }
                Err(error) => return Err(error.into()),
            };
            let schema = self
                .instances
                .kinds
                .get(resolved.kind)
                .ok_or_else(|| InstanceError::UnknownKind(resolved.kind.to_string()))?;
            let binding = InstanceStore::field_binding(schema, id, field, host_documents)
                .map_err(|reason| VMError::Instance(format!("{who}: {reason}")))?;
            (binding, resolved.record.is_none(), resolved.is_live_host())
        };
        if stale {
            // A stale instance: detached slots holding the default.
            let value = self.instances.field_value(id, field)?;
            return Ok(PreparedBinding::Stale { binding, value });
        }
        if live_host {
            // Returns at once when a reader observes it or a held ref keeps
            // it current.
            self.refresh_cold_host_field(id, field)?;
        }
        // A new held ref may observe a field nothing observed before.
        self.instance_observer_epoch += 1;
        let bound = self.bound_instance_fields.entry(id).or_default();
        if bound.contains_key(field) {
            // Already bound: every change of the field (a cold refresh
            // included) keeps its slots current; no value to read.
            return Ok(PreparedBinding::Bound {
                binding,
                seed: None,
            });
        }
        bound.insert(field.to_string(), binding);
        let value = self.instances.field_value(id, field)?;
        Ok(PreparedBinding::Bound {
            binding,
            seed: Some(value),
        })
    }

    /// Re-publish every read source of an instance from the store (after
    /// create/drop/re-registration changed what each field answers). A
    /// field it no longer has publishes nil, unless `keep_removed` (a hot
    /// reload leaves readers of a removed field alone rather than re-running
    /// them into an error mid-reload).
    fn republish_instance_sources(&mut self, id: InstanceId, keep_removed: bool) {
        let namespace = instance_namespace(id);
        let Some(fields) = self.dag.namespace_field_sources.get(&namespace) else {
            return;
        };
        let fields: Vec<String> = fields.keys().cloned().collect();
        for field in fields {
            let value = match self.instances.field_value(id, &field) {
                Ok(value) => value,
                Err(_) if keep_removed => continue,
                Err(_) => Value::Nil,
            };
            self.write_bound_slot(&namespace, id, &field, &value);
            self.dirty_namespace_field(&namespace, &field, value);
        }
    }
}

/// A field binding [`VM::prepare_field_binding`] checked.
enum PreparedBinding {
    /// A dropped instance's field: detached slots holding `value`.
    Stale { binding: BindingKind, value: Value },
    /// A live instance's field, bound in the store; `seed` is its value
    /// when the slots need writing (first bind).
    Bound {
        binding: BindingKind,
        seed: Option<Value>,
    },
}

#[path = "widget_state.rs"]
mod widget_state;
pub use widget_state::SdfStatePlan;

#[cfg(test)]
#[path = "field_binding_tests.rs"]
mod field_binding_tests;

#[cfg(test)]
#[path = "keyed_kind_tests.rs"]
mod keyed_kind_tests;

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::super::{EffectTarget, PendingUiUpdate, VM, VMError, Value};
    use super::{InstanceBuiltinField, InstanceKindSchema};

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
        vm.set_instance_builtin_field(
            1,
            InstanceBuiltinField::Label,
            Value::String("Kit A".into()),
        )
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

        vm.set_instance_builtin_field(
            4,
            InstanceBuiltinField::Owner,
            Value::String("Kit A".into()),
        )
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
        assert_eq!(
            vm.instance_field(4, "label"),
            Ok(Value::String("first".into()))
        );
        vm.set_instance_builtin_field(
            4,
            InstanceBuiltinField::Label,
            Value::String("second".into()),
        )
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
        // `id`/`kind` cannot be declared (a compile error since eseq-0l17.60).
        assert!(
            compile_errors(&mut vm, "(def-kind bad :key () :state ((kind 1)))")
                .contains("def-kind bad: :state field 'kind' is a built-in field")
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
    fn created_kind_slots_on_singletons_are_errors() {
        let mut vm = instance_vm();
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
