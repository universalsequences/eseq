/*!
Instance kinds (`docs/instance-kinds-spec.md` §3, §5, §8.1).

A package defines **kinds** with `def-kind`; the host owns **instances** of
them. This module holds the Lisp side of that split:

- the `def-kind` native, which the compiler hands the kind name, the
  `:sequencer` body captured as data (exactly like graph-mode
  `def-sequencer`), the `:state` `(field default)` pairs with their defaults
  already evaluated, and the `:view` function;
- the host **kind registry**: one [`KindDefinition`] per kind id, process-wide
  (like the graph owner cells) because the UI runtime and the scheduler
  runtime both evaluate the same modules but only the host publishes;
- kind ids (`<package name>:<kind name>`), resolved from the package whose
  owned namespace contains the defining module;
- the manifest `kinds` check the Packages tab runs on attach.

Importing a module registers its kinds and creates nothing. The instance list
itself lives in the project model (`crate::project::ProjectInstances`); the
app publishes each instance's sequencer with [`instance_published_sequencer`].
*/

use super::super::*;
use crate::graph::GraphManifest;

pub const DEF_KIND_SIGNATURE: &str =
    "(def-kind name :sequencer (graph-body ...) :state ((field default) ...) :view f :keymap mode :on-create f)";
pub const DEF_KIND_DOCS: &str = "Define an instance kind. The host owns instances of it: each one publishes the :sequencer graph body under its own id, carries its own :state cells, and renders (view instance) in its own buffer and step tab (whose keymap is the optional :keymap mode). The optional :on-create function runs once with a freshly created instance (not a duplicate, kit load or reopened project) to write document defaults the :sequencer body cannot express. Returns the kind id \"<package>:<name>\".";
pub const DEF_KIND_KEYWORDS: &[&str] = &["sequencer", "state", "view", "keymap", "on-create"];

/// Kind id prefix for kinds defined in headerless (scratch) code.
pub const SCRATCH_KIND_PACKAGE: &str = "scratch";

/// One registered kind, as the host sees it. Everything here is `Send`; the
/// `:view` closure and the evaluated `:state` defaults live in the defining
/// VM's instance-kind schema instead.
#[derive(Clone, Debug, PartialEq)]
pub struct KindDefinition {
    /// `<package name>:<kind name>` (spec §5).
    pub id: String,
    /// The authored kind name (`neural`), used for default labels.
    pub name: String,
    /// The package whose module defined the kind, if any.
    pub package: Option<String>,
    /// The module whose `def-kind` registered the kind (`None` = scratch).
    pub module: Option<String>,
    /// The `:sequencer` resource slot as a parsed graph manifest template.
    /// Each instance publishes a copy with its own id, name and owner.
    pub sequencer: Option<GraphManifest>,
    /// Declared `:state` field names, in order.
    pub state_fields: Vec<String>,
    pub has_view: bool,
    /// `:keymap`: the mode every instance's view buffer gets (spec §7).
    pub keymap: Option<String>,
}

struct KindRegistry {
    kinds: BTreeMap<String, KindDefinition>,
    version: u64,
}

static KIND_REGISTRY: Mutex<KindRegistry> = Mutex::new(KindRegistry {
    kinds: BTreeMap::new(),
    version: 0,
});

fn with_registry<T>(body: impl FnOnce(&mut KindRegistry) -> T) -> T {
    let mut registry = KIND_REGISTRY.lock().unwrap_or_else(|error| error.into_inner());
    body(&mut registry)
}

/// Bumped whenever a kind is registered or changes; the host re-publishes
/// the instances of changed kinds when it moves.
pub fn kind_registry_version() -> u64 {
    with_registry(|registry| registry.version)
}

/// Insert or replace a kind. Returns whether anything changed (an identical
/// re-registration, e.g. the scheduler runtime evaluating the same module,
/// leaves the version alone).
pub fn register_kind(definition: KindDefinition) -> bool {
    with_registry(|registry| {
        if registry.kinds.get(&definition.id) == Some(&definition) {
            return false;
        }
        registry.kinds.insert(definition.id.clone(), definition);
        registry.version += 1;
        true
    })
}

pub fn registered_kind(id: &str) -> Option<KindDefinition> {
    with_registry(|registry| registry.kinds.get(id).cloned())
}

/// [`registered_kind`]`.is_some()` without cloning the definition (and its
/// parsed graph template): for per-frame checks.
pub fn kind_is_registered(id: &str) -> bool {
    with_registry(|registry| registry.kinds.contains_key(id))
}

pub fn registered_kinds() -> Vec<KindDefinition> {
    with_registry(|registry| registry.kinds.values().cloned().collect())
}

/// The kinds a module's `def-kind`s registered.
pub fn kinds_defined_in_module(module: &str) -> Vec<KindDefinition> {
    with_registry(|registry| {
        registry
            .kinds
            .values()
            .filter(|kind| kind.module.as_deref() == Some(module))
            .cloned()
            .collect()
    })
}

/// Forget the kinds `module` registered (an attach the manifest check
/// refused). Returns how many were removed.
pub fn unregister_module_kinds(module: &str) -> usize {
    with_registry(|registry| {
        let before = registry.kinds.len();
        registry.kinds.retain(|_, kind| kind.module.as_deref() != Some(module));
        let removed = before - registry.kinds.len();
        if removed > 0 {
            registry.version += 1;
        }
        removed
    })
}

/// Forget every registered kind. Tests only: the registry is process-wide.
pub fn clear_kind_registry() {
    with_registry(|registry| {
        registry.kinds.clear();
        registry.version += 1;
    });
}

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

/// The installed package (`author/name`) whose owned namespace holds `module`.
pub fn package_name_for_module(module: &str) -> Option<String> {
    crate::app_paths::app_paths()
        .package_catalog()
        .package_for_module(module)
        .map(|package| package.manifest.name.clone())
}

/// One kind a module declares in its installed package's manifest, as
/// project migration (spec §10) sees it: no code is evaluated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeclaredKind {
    /// `<package name>:<kind name>`.
    pub id: String,
    /// `PackageKind::legacy_sequencer`: the def-sequencer name the module
    /// published before it declared the kind.
    pub legacy_sequencer: Option<String>,
}

/// The kinds `module`'s installed package declares for that module, in
/// manifest order. Empty when the module is in no installed package or its
/// package declares no kind there.
pub fn declared_kinds_for_module(module: &str) -> Vec<DeclaredKind> {
    crate::app_paths::app_paths()
        .package_catalog()
        .package_for_module(module)
        .map(|package| {
            package
                .manifest
                .kinds
                .iter()
                .filter(|kind| kind.module == module)
                .map(|kind| DeclaredKind {
                    id: package.manifest.kind_id(&kind.name),
                    legacy_sequencer: kind.legacy_sequencer.clone(),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The module that defines kind `kind_id` according to its installed
/// package's manifest, or `None` when no installed package declares it
/// (the package is missing: an instance of it stays a placeholder, spec §5).
/// A kit load attaches that module so the kind registers (spec §9).
pub fn declared_module_for_kind(kind_id: &str) -> Option<String> {
    crate::app_paths::app_paths()
        .package_catalog()
        .ordered()
        .find_map(|package| {
            package
                .manifest
                .kinds
                .iter()
                .find(|kind| package.manifest.kind_id(&kind.name) == kind_id)
                .map(|kind| kind.module.clone())
        })
}

/// The kind name part of a kind id (`alez/neural:neural` -> `neural`).
pub fn kind_name_of(kind_id: &str) -> &str {
    kind_id.rsplit_once(':').map(|(_, name)| name).unwrap_or(kind_id)
}

/// The package/module part of a kind id (`alez/neural:neural` -> `alez/neural`).
pub fn kind_package_of(kind_id: &str) -> &str {
    kind_id.rsplit_once(':').map(|(package, _)| package).unwrap_or("")
}

/// The published sequencer name of an instance. Unique per instance on
/// purpose: `GraphManifest::matches_overrides` falls back to a name match
/// within one owner (for legacy full-width ids), and two instances of one
/// kind sharing the kind's name would then read and write each other's
/// overrides. Views address an instance by value, never by this name.
pub fn instance_sequencer_name(kind_name: &str, id: u64) -> String {
    format!("{kind_name}#{id}")
}

/// What publishing instance `id` of `kind` hands the scheduler: the kind's
/// `:sequencer` manifest with the sequencer id == the instance id (spec §5).
/// `None` for a kind without a `:sequencer` slot.
pub fn instance_published_sequencer(
    kind: &KindDefinition,
    id: u64,
    owner_rack: Option<u64>,
) -> Option<PublishedSequencer> {
    let mut manifest = kind.sequencer.clone()?;
    manifest.id = id;
    manifest.name = instance_sequencer_name(&kind.name, id);
    manifest.owner_rack = owner_rack;
    Some(PublishedSequencer {
        id,
        name: manifest.name.clone(),
        resolution: Timebase::Sixteenth as u8,
        tick_source: String::new(),
        requires: Vec::new(),
        graph: Some(manifest),
    })
}

/// Attach-time check of a package manifest's `kinds` against what `module`
/// actually registered (spec §8.1). A declared kind of this module that it
/// never `def-kind`s is an error; a `def-kind` missing from the manifest is
/// a warning (it works once loaded, but cannot be offered before attach).
/// Returns the warnings.
pub fn check_manifest_kinds(
    package: &eseqlisp::package::PackageManifest,
    module: &str,
) -> Result<Vec<String>, String> {
    let defined = kinds_defined_in_module(module);
    let missing: Vec<&str> = package
        .kinds
        .iter()
        .filter(|kind| kind.module == module)
        .filter(|kind| {
            let id = package.kind_id(&kind.name);
            !defined.iter().any(|defined| defined.id == id)
        })
        .map(|kind| kind.name.as_str())
        .collect();
    if !missing.is_empty() {
        return Err(format!(
            "{} declares kind {} in {module}, but the module never def-kinds {}",
            package.name,
            missing
                .iter()
                .map(|name| format!("'{name}'"))
                .collect::<Vec<_>>()
                .join(", "),
            if missing.len() == 1 { "it" } else { "them" },
        ));
    }
    Ok(defined
        .iter()
        .filter(|defined| {
            !package
                .kinds
                .iter()
                .any(|kind| package.kind_id(&kind.name) == defined.id)
        })
        .map(|defined| {
            format!(
                "def-kind '{}' in {module} is missing from {}'s manifest kinds; it cannot be offered before attach",
                defined.name, package.name
            )
        })
        .collect())
}

fn def_kind_symbol(value: &EValue) -> Option<String> {
    match value {
        EValue::Symbol(name) | EValue::String(name) | EValue::Keyword(name) => {
            Some(name.trim_start_matches(':').to_string())
        }
        _ => None,
    }
}

fn def_kind_list(value: &EValue) -> Option<Vec<EValue>> {
    match value {
        EValue::List(items) => Some(items.iter().map(|item| item.borrow().clone()).collect()),
        _ => None,
    }
}

/// Parse `def-kind` arguments into the host definition plus the VM schema.
/// `package` is the defining module's package name, if any.
pub fn parse_def_kind(
    args: &[EValue],
    module: Option<&str>,
    package: Option<&str>,
) -> Result<(KindDefinition, eseqlisp::vm::InstanceKindSchema), String> {
    let name = args
        .first()
        .and_then(def_kind_symbol)
        .filter(|name| !name.is_empty())
        .ok_or_else(|| "def-kind expects a kind name".to_string())?;
    if name.contains(':') {
        return Err(format!("def-kind {name}: a kind name cannot contain ':'"));
    }
    let id = kind_id(package, module, &name);
    let mut sequencer = None;
    let mut schema = eseqlisp::vm::InstanceKindSchema::new(id.clone());
    let mut view = None;
    let mut keymap = None;
    let mut on_create = None;
    let mut idx = 1;
    while idx < args.len() {
        let key = match &args[idx] {
            EValue::Keyword(key) => key.trim_start_matches(':').to_string(),
            _ => return Err(format!("def-kind {name}: expected :slot keyword/value pairs")),
        };
        let value = args
            .get(idx + 1)
            .ok_or_else(|| format!("def-kind {name}: missing value for :{key}"))?;
        match key.as_str() {
            "sequencer" => {
                if matches!(value, EValue::Nil) {
                    sequencer = None;
                } else {
                    let body = def_kind_list(value).ok_or_else(|| {
                        format!("def-kind {name}: :sequencer expects a def-sequencer body")
                    })?;
                    let mut sequencer_args = Vec::with_capacity(body.len() + 1);
                    sequencer_args.push(EValue::String(name.clone()));
                    sequencer_args.extend(body);
                    if !graph_mode_present(&sequencer_args) {
                        return Err(format!(
                            "def-kind {name}: :sequencer needs a graph body with a def-node"
                        ));
                    }
                    // The template never depends on where `def-kind` ran:
                    // parse with no owner (each instance sets its own id and
                    // owner at publish time), so a def-kind replayed inside a
                    // legacy rack scope registers the same definition as the
                    // scheduler's and the registry does not flip-flop.
                    let manifest = parse_graph_manifest_owned(&sequencer_args, None)
                        .map_err(|error| format!("def-kind {name} :sequencer: {error}"))?;
                    sequencer = Some(manifest);
                }
            }
            "state" => {
                for entry in def_kind_list(value).unwrap_or_default() {
                    let pair = def_kind_list(&entry).ok_or_else(|| {
                        format!("def-kind {name}: each :state entry is (field default)")
                    })?;
                    let field = pair
                        .first()
                        .and_then(def_kind_symbol)
                        .ok_or_else(|| format!("def-kind {name}: :state field names must be symbols"))?;
                    let default = pair.get(1).cloned().unwrap_or(EValue::Nil);
                    schema = schema.field(field, default);
                }
            }
            "view" => {
                view = (!matches!(value, EValue::Nil)).then(|| value.clone());
            }
            "on-create" => {
                on_create = (!matches!(value, EValue::Nil)).then(|| value.clone());
            }
            "keymap" => {
                keymap = match value {
                    EValue::Nil => None,
                    EValue::String(mode) | EValue::Symbol(mode) | EValue::Keyword(mode) => {
                        Some(mode.trim_start_matches(':').to_string())
                            .filter(|mode| !mode.is_empty())
                    }
                    _ => {
                        return Err(format!(
                            "def-kind {name}: :keymap expects a mode name (string or symbol)"
                        ));
                    }
                };
            }
            other => return Err(format!("def-kind {name}: unknown slot :{other}")),
        }
        idx += 2;
    }
    // Reject what the evaluating VM would reject before anything reaches
    // the process-wide registry, so a bad `:state` never leaves a kind the
    // host can instantiate but no VM can hold a record for.
    schema.validate().map_err(|error| format!("def-kind {name}: {error}"))?;
    let definition = KindDefinition {
        id,
        name,
        package: package.map(str::to_string),
        module: module.map(str::to_string),
        sequencer,
        state_fields: schema.fields.iter().map(|(field, _)| field.clone()).collect(),
        has_view: view.is_some(),
        keymap: keymap.clone(),
    };
    Ok((
        definition,
        schema.with_view(view).with_keymap(keymap).with_on_create(on_create),
    ))
}

/// Install `def-kind`. It registers the kind with the host registry and its
/// `:state`/`:view` schema with the evaluating VM, and returns the kind id.
/// It publishes nothing: instances are the host's, and the host re-publishes
/// the instances of a (re)registered kind when the registry version moves.
pub fn register_def_kind_native(runtime: &mut Runtime) {
    runtime.register_native_with_docs_and_keywords(
        "def-kind",
        DEF_KIND_SIGNATURE,
        DEF_KIND_DOCS,
        DEF_KIND_KEYWORDS.iter().copied(),
        move |args, ctx| {
            let module = ctx.current_module();
            let package = module.as_deref().and_then(package_name_for_module);
            let (definition, schema) =
                parse_def_kind(&args, module.as_deref(), package.as_deref())?;
            let id = definition.id.clone();
            register_kind(definition);
            ctx.register_instance_kind(schema);
            Ok(EValue::String(id))
        },
    );
}

/// The `owner` host field an instance record carries: `:project`, or the
/// owning rack's group id.
pub fn instance_owner_value(owner: crate::project::ProjectInstanceOwner) -> EValue {
    match owner {
        crate::project::ProjectInstanceOwner::Project => EValue::Keyword("project".to_string()),
        crate::project::ProjectInstanceOwner::Rack(group_id) => EValue::Number(group_id as f64),
    }
}

/// Drop every live instance record in a VM (their handles turn stale). The
/// UI runs it when `ProjectInstances::generation` moves (project open / new
/// project), so :state cells of one project never survive into another that
/// happens to reuse the same id and kind. Returns whether anything dropped.
pub fn drop_all_instance_records(runtime: &mut Runtime) -> bool {
    let mut changed = false;
    for id in runtime.live_instances() {
        changed |= runtime.drop_instance(id);
    }
    changed
}

/// Mirror the project's instance list into a VM's instance records (the
/// chunk-1 record API): create the record of every instance whose kind this
/// VM has registered, push `owner` / `label`, and drop the records of
/// instances that are gone (their handles turn stale). An instance whose
/// kind is not registered here yet keeps no record until it is. Returns
/// whether anything changed; the caller runs its reactive cycle.
pub fn sync_instance_records(
    runtime: &mut Runtime,
    instances: &crate::project::ProjectInstances,
) -> bool {
    let mut changed = false;
    for id in runtime.live_instances() {
        let wanted = instances.get(id).is_some_and(|instance| {
            runtime.instance_kind(id).as_deref() == Some(instance.kind.as_str())
                && runtime.instance_kind_schema(&instance.kind).is_some()
        });
        if !wanted {
            changed |= runtime.drop_instance(id);
        }
    }
    for instance in &instances.list {
        if runtime.instance_kind_schema(&instance.kind).is_none() {
            continue;
        }
        if !runtime.instance_is_live(instance.id) {
            if let Err(error) = runtime.create_instance(instance.id, &instance.kind) {
                eprintln!("metal_seq: instance {} ({}): {error}", instance.id, instance.kind);
                continue;
            }
            changed = true;
        }
        let owner = instance_owner_value(instance.owner);
        if runtime.instance_field(instance.id, "owner").ok().as_ref() != Some(&owner) {
            let _ = runtime.set_instance_host_field(
                instance.id,
                eseqlisp::vm::InstanceHostField::Owner,
                owner,
            );
            changed = true;
        }
        let label = EValue::String(instance.label.clone());
        if runtime.instance_field(instance.id, "label").ok().as_ref() != Some(&label) {
            let _ = runtime.set_instance_host_field(
                instance.id,
                eseqlisp::vm::InstanceHostField::Label,
                label,
            );
            changed = true;
        }
    }
    changed
}

/// Run the kind's `:on-create` with the fresh instance `id` (spec §11):
/// the host calls it once, after a user-created instance's sequencer is
/// published and its record exists, so `graph-*` writes in the hook land on
/// that instance's overrides. Never for a duplicate, kit load, migration or
/// reopened project: those carry their document state already. Returns
/// whether a hook ran; an instance with no record here, or a kind without
/// `:on-create`, is a quiet no-op.
pub fn run_instance_on_create(runtime: &mut Runtime, id: u64) -> Result<bool, String> {
    let Some(kind) = runtime.instance_kind(id) else {
        return Ok(false);
    };
    let Some(hook) = runtime
        .instance_kind_schema(&kind)
        .and_then(|schema| schema.on_create.clone())
    else {
        return Ok(false);
    };
    runtime
        .invoke(hook, vec![EValue::Instance(id)])
        .map_err(|error| format!("{} :on-create: {error:?}", kind_name_of(&kind)))?;
    Ok(true)
}

/// The key scope an instance's view renders in (spec §7): widget and
/// subtree keys become `instance:<id>::<key>`.
pub fn instance_key_scope(id: u64) -> String {
    format!("instance:{id}")
}

/// The buffer an instance's view renders into: `*<kind> · <label>*`.
pub fn instance_view_buffer_name(kind_name: &str, label: &str) -> String {
    format!("*{kind_name} · {label}*")
}

/// One instance view buffer as the host shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstanceView {
    pub id: u64,
    pub buffer: String,
    /// The tab label: the instance's label.
    pub label: String,
    /// The kind's `:keymap` mode, if any.
    pub keymap: Option<String>,
}

/// What [`sync_instance_view_buffers`] changed; the UI mirrors these into
/// editor buffers and step tabs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstanceViewChange {
    /// A new binding (new instance, loaded project, or a kind that just
    /// gained a `:view`): create its buffer and tab.
    Added(InstanceView),
    /// A rename moved the binding from `old` to the view's buffer.
    Renamed { old: String, view: InstanceView },
    /// The instance is gone (or lost its record / `:view`).
    Removed { id: u64, buffer: String },
}

/// The view buffers every instance should have: each live record in this
/// runtime whose kind has a `:view`, named `*<kind> · <label>*`. Two
/// instances whose names would coincide (same kind and label) are told
/// apart by id.
pub fn desired_instance_views(
    runtime: &Runtime,
    instances: &crate::project::ProjectInstances,
) -> Vec<InstanceView> {
    let mut views: Vec<InstanceView> = instances
        .list
        .iter()
        .filter(|instance| runtime.instance_is_live(instance.id))
        .filter_map(|instance| {
            let schema = runtime.instance_kind_schema(&instance.kind)?;
            schema.view.as_ref()?;
            Some(InstanceView {
                id: instance.id,
                buffer: instance_view_buffer_name(kind_name_of(&instance.kind), &instance.label),
                label: instance.label.clone(),
                keymap: schema.keymap.clone(),
            })
        })
        .collect();
    let mut counts: HashMap<String, usize> = HashMap::new();
    for view in &views {
        *counts.entry(view.buffer.clone()).or_default() += 1;
    }
    for view in &mut views {
        if counts.get(&view.buffer).copied().unwrap_or(0) > 1 {
            let base = view.buffer.trim_end_matches('*').to_string();
            view.buffer = format!("{base} #{}*", view.id);
        }
    }
    views
}

/// Reconcile the runtime's bound instance view buffers with the instance
/// list (spec §7): bind a buffer per live instance whose kind has a
/// `:view`, retarget it on rename, unbind it when the instance, its record
/// or its kind's `:view` goes. `reset` unbinds every instance view first
/// (project open / new project: the new project's instances start with new
/// buffers and tabs even when ids coincide). Bindings that are not instance
/// views are left alone. Returns the changes plus every current view.
pub fn sync_instance_view_buffers(
    runtime: &mut Runtime,
    instances: &crate::project::ProjectInstances,
    reset: bool,
) -> (Vec<InstanceViewChange>, Vec<InstanceView>) {
    let mut changes = Vec::new();
    let mut bound: BTreeMap<u64, String> = BTreeMap::new();
    for (target, view) in runtime.bound_view_buffers() {
        if let eseqlisp::vm::BoundView::Instance(id) = view {
            if reset {
                runtime.unbind_view_buffer(&target);
                changes.push(InstanceViewChange::Removed { id, buffer: target });
            } else {
                bound.insert(id, target);
            }
        }
    }
    let desired = desired_instance_views(runtime, instances);
    let wanted: HashMap<u64, &InstanceView> = desired.iter().map(|view| (view.id, view)).collect();
    // Removals first, so a freed name can be taken by a rename below.
    for (id, target) in bound.clone() {
        if !wanted.contains_key(&id) {
            runtime.unbind_view_buffer(&target);
            bound.remove(&id);
            changes.push(InstanceViewChange::Removed { id, buffer: target });
        }
    }
    // Renames next. A rename onto a name another instance still holds (two
    // labels swapped) cannot retarget; it unbinds now and binds as new.
    let mut pending = Vec::new();
    for view in &desired {
        let Some(old) = bound.get(&view.id).cloned() else {
            pending.push(view);
            continue;
        };
        if old == view.buffer {
            continue;
        }
        if runtime.retarget_view_buffer(&old, &view.buffer) {
            bound.insert(view.id, view.buffer.clone());
            changes.push(InstanceViewChange::Renamed { old, view: view.clone() });
        } else {
            runtime.unbind_view_buffer(&old);
            bound.remove(&view.id);
            changes.push(InstanceViewChange::Removed { id: view.id, buffer: old });
            pending.push(view);
        }
    }
    for view in pending {
        runtime.bind_view_buffer(
            &view.buffer,
            eseqlisp::vm::BoundView::Instance(view.id),
            Some(instance_key_scope(view.id)),
        );
        changes.push(InstanceViewChange::Added(view.clone()));
    }
    (changes, desired)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(kinds: &[(&str, &str)]) -> eseqlisp::package::PackageManifest {
        serde_json::from_value(serde_json::json!({
            "name": "alec/neural",
            "version": "1",
            "kinds": kinds
                .iter()
                .map(|(name, module)| serde_json::json!({"name": name, "module": module}))
                .collect::<Vec<_>>(),
        }))
        .expect("manifest")
    }

    fn defined(name: &str, module: &str) -> KindDefinition {
        KindDefinition {
            id: kind_id(Some("alec/neural"), Some(module), name),
            name: name.to_string(),
            package: Some("alec/neural".to_string()),
            module: Some(module.to_string()),
            sequencer: None,
            state_fields: Vec::new(),
            has_view: false,
            keymap: None,
        }
    }

    #[test]
    fn kind_ids_prefer_the_package_then_the_module_then_scratch() {
        assert_eq!(kind_id(Some("alez/neural"), Some("alez.neural.x"), "neural"), "alez/neural:neural");
        assert_eq!(kind_id(None, Some("demos.graph"), "neural"), "demos.graph:neural");
        assert_eq!(kind_id(None, None, "neural"), "scratch:neural");
        assert_eq!(kind_name_of("alez/neural:neural"), "neural");
        assert_eq!(kind_package_of("alez/neural:neural"), "alez/neural");
    }

    #[test]
    fn manifest_kinds_missing_def_kind_is_an_error_and_undeclared_def_kind_a_warning() {
        clear_kind_registry();
        let module = "alec.neural.seq";
        let declared = manifest(&[("neural", module), ("other", "alec.neural.elsewhere")]);
        let error = check_manifest_kinds(&declared, module).expect_err("never def-kinded");
        assert!(error.contains("'neural'"), "{error}");
        assert!(!error.contains("'other'"), "only this module's kinds are checked: {error}");

        assert!(register_kind(defined("neural", module)));
        assert!(!register_kind(defined("neural", module)), "identical re-registration is a no-op");
        assert_eq!(check_manifest_kinds(&declared, module), Ok(Vec::new()));

        register_kind(defined("extra", module));
        let warnings = check_manifest_kinds(&declared, module).expect("warnings only");
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("'extra'"), "{warnings:?}");
    }
}
