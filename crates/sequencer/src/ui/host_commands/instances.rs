use crate::*;

/// Instance lifecycle commands (docs/instance-kinds-spec.md §5, §8.3). The
/// Packages tab and the rack panel call these; each is one recorded,
/// undoable project edit.
///
/// - `instance-create` `{:kind "alez/neural:neural" [:group-id gid] [:label s]}`
///   (a `:group-id` makes the rack the owner)
/// - `instance-delete` `{:id n}`
/// - `instance-duplicate` `{:id n}`
/// - `instance-rename` `{:id n :label s}` (also what `(set! self.label v)`
///   queues)
/// - `instance-move` `{:id n [:group-id gid]}`: "Move to rack" with a
///   `:group-id`, "Give back to project" without one
/// - `instance-open` `{:id n}`: show the instance's view tab (reopening it
///   if its × closed it); not an edit
///
/// Create and duplicate also open the new instance's tab.
pub(super) const COMMANDS: &[&str] = &[
    "instance-create",
    "instance-delete",
    "instance-duplicate",
    "instance-rename",
    "instance-move",
    "instance-open",
];

pub(super) fn handle(
    name: &str,
    payload: Value,
    app: &mut app::App,
    editor: &mut Editor,
    _ctx: &mut LoopCtx<'_>,
) {
    apply_on_editor(name, payload, app, editor);
}

/// [`handle`] without the event loop: apply one instance command, mirror
/// the list into the UI VM (records, view buffers, tabs), run a fresh
/// instance's `:on-create`, and open a new instance's tab. Headless capture
/// runs fixture-queued instance commands through here too.
pub(crate) fn apply_on_editor(name: &str, payload: Value, app: &mut app::App, editor: &mut Editor) {
    if name == "instance-open" {
        let result = payload_id(&payload, name).and_then(|id| {
            sync_instances_to_editor(app, editor);
            open_instance_view(editor, id)
        });
        if let Err(error) = result {
            editor.handle_host_event(HostEvent::Status(error));
        }
        return;
    }
    let before = instance_ids(app);
    match apply_instance_command(name, &payload, app) {
        Ok(status) => {
            after_instance_command(name, &before, app, editor);
            if !status.is_empty() {
                editor.handle_host_event(HostEvent::Status(status));
            }
        }
        Err(error) => editor.handle_host_event(HostEvent::Status(error)),
    }
}

/// The ids of every project instance, to tell which ones a command created.
pub(crate) fn instance_ids(app: &app::App) -> std::collections::HashSet<u64> {
    app.instances.list.iter().map(|instance| instance.id).collect()
}

/// What follows an applied instance command `name` (`before` = the ids
/// before it): mirror the list into the UI VM, run each fresh instance's
/// `:on-create` (a duplicate already carries its source's document), and
/// open the new instances' tabs.
pub(crate) fn after_instance_command(
    name: &str,
    before: &std::collections::HashSet<u64>,
    app: &mut app::App,
    editor: &mut Editor,
) {
    sync_instances_to_editor(app, editor);
    if !matches!(name, "instance-create" | "instance-duplicate") {
        return;
    }
    let created: Vec<u64> = app
        .instances
        .list
        .iter()
        .map(|instance| instance.id)
        .filter(|id| !before.contains(id))
        .collect();
    for id in created {
        if name == "instance-create" {
            run_on_create(editor, id);
        }
        // A kind without a :view has no tab; nothing to show.
        let _ = open_instance_view(editor, id);
    }
}

fn payload_id(payload: &Value, command: &str) -> Result<u64, String> {
    extract_usize_from_payload(payload, "id")
        .map(|id| id as u64)
        .ok_or_else(|| format!("{command} needs an instance :id"))
}

/// Apply one instance command to the project and return its status line
/// (empty for a quiet no-op, such as a rename to the current label).
/// Split from [`handle`] so tests drive it without an event loop.
pub(crate) fn apply_instance_command(
    name: &str,
    payload: &Value,
    app: &mut app::App,
) -> Result<String, String> {
    match name {
        "instance-create" => {
            let kind = extract_string_from_payload(payload, "kind")
                .map(|kind| kind.trim().to_string())
                .filter(|kind| !kind.is_empty())
                .ok_or("instance-create needs a :kind")?;
            let owner = match extract_usize_from_payload(payload, "group-id") {
                Some(group_id) => sequencer::project::ProjectInstanceOwner::Rack(group_id as u64),
                None => sequencer::project::ProjectInstanceOwner::Project,
            };
            let label = extract_string_from_payload(payload, "label");
            let id = app.create_instance_recorded(&kind, owner, label)?;
            Ok(format!("Created {}", instance_label(app, id)))
        }
        "instance-delete" => {
            let id = payload_id(payload, name)?;
            let removed = app.delete_instance_recorded(id)?;
            Ok(format!("Deleted {}", removed.label))
        }
        "instance-duplicate" => {
            let id = payload_id(payload, name)?;
            let new_id = app.duplicate_instance_recorded(id)?;
            Ok(format!("Duplicated as {}", instance_label(app, new_id)))
        }
        "instance-rename" => {
            let id = payload_id(payload, name)?;
            let label = extract_string_from_payload(payload, "label")
                .ok_or("instance-rename needs a :label string")?;
            let unchanged = app
                .instances
                .get(id)
                .is_some_and(|instance| instance.label == label.trim());
            app.rename_instance_recorded(id, &label)?;
            if unchanged {
                return Ok(String::new());
            }
            Ok(format!("Renamed to {}", label.trim()))
        }
        "instance-move" => {
            let id = payload_id(payload, name)?;
            let owner = match extract_usize_from_payload(payload, "group-id") {
                Some(group_id) => sequencer::project::ProjectInstanceOwner::Rack(group_id as u64),
                None => sequencer::project::ProjectInstanceOwner::Project,
            };
            move_instance_status(app, id, owner)
        }
        _ => Err(format!("Unknown instance command {name}")),
    }
}

/// Move instance `id` to `owner` and describe it; empty for a move to the
/// current owner. Shared with the rack menu's attach/detach entries, which
/// route an instance's sequencer id here.
pub(crate) fn move_instance_status(
    app: &mut app::App,
    id: u64,
    owner: sequencer::project::ProjectInstanceOwner,
) -> Result<String, String> {
    let unchanged = app.instances.get(id).is_some_and(|instance| instance.owner == owner);
    app.move_instance_owner_recorded(id, owner)?;
    if unchanged {
        return Ok(String::new());
    }
    let label = instance_label(app, id);
    Ok(match owner.rack() {
        Some(group_id) => format!(
            "{} now owns {label}; its routes are rack members",
            super::drum_rack_v2::group_name(app, group_id).unwrap_or_else(|| "The rack".to_string())
        ),
        None => format!("{label} belongs to the project; its routes are plain tracks"),
    })
}

/// One row of `SEQ.instances`: `{:id :kind :label :owner-rack :owner-label
/// :registered?}` per project instance, in list order. `:owner-rack` is nil
/// for a project-owned instance; `:owner-label` is "project" or the rack's
/// name; `:registered?` is false for a placeholder whose kind has not
/// registered (package missing or not attached yet, spec §5).
struct InstanceRow {
    id: u64,
    kind: String,
    label: String,
    owner_rack: Option<u64>,
    owner_label: String,
    registered: bool,
}

fn instance_rows(app: &app::App) -> Vec<InstanceRow> {
    app.instances
        .list
        .iter()
        .map(|instance| {
            let owner_rack = instance.owner.rack();
            InstanceRow {
                id: instance.id,
                kind: instance.kind.clone(),
                label: instance.label.clone(),
                owner_rack,
                owner_label: match owner_rack {
                    Some(group_id) => super::drum_rack_v2::group_name(app, group_id)
                        .unwrap_or_else(|| format!("rack {group_id}")),
                    None => "project".to_string(),
                },
                registered: sequencer::lisp_host::kind_is_registered(&instance.kind),
            }
        })
        .collect()
}

/// `SEQ.instances` as a Lisp list of dicts.
pub(crate) fn build_instances_value(app: &app::App) -> Value {
    instances_value(&instance_rows(app))
}

fn instances_value(rows: &[InstanceRow]) -> Value {
    Value::List(
        rows.iter()
            .map(|row| {
                std::rc::Rc::new(std::cell::RefCell::new(crate::values::map_value(vec![
                    ("id", Value::Number(row.id as f64)),
                    ("kind", Value::String(row.kind.clone())),
                    ("label", Value::String(row.label.clone())),
                    (
                        "owner-rack",
                        row.owner_rack.map(|gid| Value::Number(gid as f64)).unwrap_or(Value::Nil),
                    ),
                    ("owner-label", Value::String(row.owner_label.clone())),
                    ("registered?", Value::Bool(row.registered)),
                ])))
            })
            .collect(),
    )
}

/// The `SEQ.instances` value when it differs from the one `last`
/// fingerprints (updating `last`), else `None`. Runs every reactive tick,
/// so the fingerprint borrows instead of building rows: each instance's
/// fields, its owner rack's name, and ONE registry version read (which moves
/// whenever any kind's registration does) stand in for `:registered?`.
/// Rows are only built on a change.
pub(crate) fn instances_value_if_changed(app: &app::App, last: &mut u64) -> Option<Value> {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    sequencer::lisp_host::kind_registry_version().hash(&mut hasher);
    for instance in &app.instances.list {
        let owner_rack = instance.owner.rack();
        let owner_name = owner_rack.and_then(|group_id| {
            app.groups.iter().find(|group| group.id == group_id).map(|group| group.name.as_str())
        });
        (instance.id, &instance.kind, &instance.label, owner_rack, owner_name).hash(&mut hasher);
    }
    app.instances.list.len().hash(&mut hasher);
    let fingerprint = hasher.finish();
    if fingerprint == *last {
        return None;
    }
    *last = fingerprint;
    Some(instances_value(&instance_rows(app)))
}

fn instance_label(app: &app::App, id: u64) -> String {
    app.instances
        .get(id)
        .map(|instance| instance.label.clone())
        .unwrap_or_else(|| format!("instance {id}"))
}

/// Publish instance sequencers and mirror the list into the UI VM's
/// records and view buffers right away, instead of waiting for the next
/// reactive tick.
pub(crate) fn sync_instances_to_editor(app: &app::App, editor: &mut Editor) {
    app.publish_instance_sequencers();
    let mut changed =
        sequencer::lisp_host::sync_instance_records(editor.runtime_mut(), &app.instances);
    changed |= sync_instance_views(editor, &app.instances, false);
    if changed {
        editor.runtime_mut().run_reactive_cycle();
        editor.refresh_runtime_side_effects();
    }
    editor.mark_needs_redraw();
}

/// Run the kind's `:on-create` for the fresh instance `id` in the UI VM
/// (spec §11), then let the views re-render the defaults it wrote. A
/// failing hook leaves the instance in place and reports it.
pub(crate) fn run_on_create(editor: &mut Editor, id: u64) {
    match sequencer::lisp_host::run_instance_on_create(editor.runtime_mut(), id) {
        Ok(false) => {}
        Ok(true) => {
            editor.runtime_mut().run_reactive_cycle();
            editor.refresh_runtime_side_effects();
            editor.mark_needs_redraw();
        }
        Err(error) => editor.handle_host_event(HostEvent::Status(error)),
    }
}

/// Host-created per-instance buffers and tabs (docs/instance-kinds-spec.md
/// §7): reconcile the UI VM's bound view buffers with the instance list,
/// then mirror each change into the editor and the step-tab registry. A new
/// binding gets its buffer (with the kind's `:keymap`) and a tab; a rename
/// renames the buffer in place and updates the tab (a closed tab stays
/// closed); a removal drops the tab, then the buffer. `reset` (project open
/// / new project) starts every view afresh. Run after
/// `sync_instance_records`, whose records the views render. Returns whether
/// anything changed.
pub(crate) fn sync_instance_views(
    editor: &mut Editor,
    instances: &sequencer::project::ProjectInstances,
    reset: bool,
) -> bool {
    use sequencer::lisp_host::InstanceViewChange;
    let (changes, views) =
        sequencer::lisp_host::sync_instance_view_buffers(editor.runtime_mut(), instances, reset);
    let mut removed = Vec::new();
    for change in &changes {
        match change {
            InstanceViewChange::Removed { id, buffer } => {
                eval_step_tabs(editor, &format!("(eseq.seq-step-tabs/seq-unregister-instance-tab {id})"));
                removed.push(buffer.clone());
            }
            InstanceViewChange::Renamed { old, view } => {
                editor.rename_buffer(old, &view.buffer);
                eval_step_tabs(
                    editor,
                    &format!(
                        "(eseq.seq-step-tabs/seq-update-instance-tab {} \"{}\" \"{}\")",
                        view.id,
                        crate::edit_sessions::escape_lisp_string(&view.label),
                        crate::edit_sessions::escape_lisp_string(&view.buffer),
                    ),
                );
            }
            InstanceViewChange::Added(view) => {
                editor.ensure_view_buffer(&view.buffer, None);
                eval_step_tabs(
                    editor,
                    &format!(
                        "(eseq.seq-step-tabs/seq-register-instance-tab {} \"{}\" \"{}\")",
                        view.id,
                        crate::edit_sessions::escape_lisp_string(&view.label),
                        crate::edit_sessions::escape_lisp_string(&view.buffer),
                    ),
                );
            }
        }
    }
    // The kind's :keymap may have changed with a reload; reapply it to
    // every view (cheap: this runs only when the instance key moves).
    for view in &views {
        if let Some(keymap) = view.keymap.as_deref() {
            editor.ensure_view_buffer(&view.buffer, Some(keymap));
        }
    }
    if !removed.is_empty() {
        // The tab unregistration queued `set-window-buffer-for` onto the
        // factory grid; apply it before the buffers go away.
        editor.refresh_runtime_side_effects();
        for buffer in removed {
            if !views.iter().any(|view| view.buffer == buffer) {
                editor.remove_buffer_by_name(&buffer);
            }
        }
    }
    !changes.is_empty()
}

/// Show instance `id`'s view tab, reopening it if needed.
pub(crate) fn open_instance_view(editor: &mut Editor, id: u64) -> Result<(), String> {
    let Some((buffer, label)) = editor
        .runtime()
        .bound_view_buffers()
        .into_iter()
        .find_map(|(target, view)| match view {
            eseqlisp::vm::BoundView::Instance(bound) if bound == id => {
                let label = match editor.runtime().instance_field(id, "label") {
                    Ok(Value::String(label)) => label,
                    _ => format!("instance {id}"),
                };
                Some((target, label))
            }
            _ => None,
        })
    else {
        return Err(format!("instance {id} has no view"));
    };
    eval_step_tabs(
        editor,
        &format!(
            "(eseq.seq-step-tabs/seq-open-instance-tab {id} \"{}\" \"{}\")",
            crate::edit_sessions::escape_lisp_string(&label),
            crate::edit_sessions::escape_lisp_string(&buffer),
        ),
    );
    editor.refresh_runtime_side_effects();
    editor.mark_needs_redraw();
    Ok(())
}

/// Run a step-tab registry call. A runtime without the UI modules (a
/// headless test editor) has no registry; the view buffers still work.
fn eval_step_tabs(editor: &mut Editor, source: &str) {
    if let Err(error) = editor.runtime_mut().eval_str(source) {
        if std::env::var("ESEQ_INSTANCE_TRACE").is_ok_and(|value| value != "0") {
            eprintln!("[instance-views] {source}: {error:?}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_app() -> app::App {
        let state = std::sync::Arc::new(sequencer::sequencer::SequencerState::new(
            1,
            vec![sequencer::sequencer::default_empty_effect_chain()],
        ));
        let (keyboard_tx, _keyboard_rx) = std::sync::mpsc::channel();
        let mut app = app::App::new(
            state,
            sequencer::audiograph::LiveGraphPtr(std::ptr::null_mut()),
            44_100,
            app::AudioBuses {
                bus_l_id: 0,
                bus_r_id: 0,
                default_bus_nodes: Vec::new(),
                bus_effect_runtime: std::sync::Arc::new(std::sync::Mutex::new(
                    std::sync::Arc::new(Vec::new()),
                )),
                reverb_bus_id: 0,
                reverb_node_id: 0,
            },
            std::sync::Arc::new(sequencer::recorder::MasterRecorder::new(44_100, 2)),
            keyboard_tx,
        );
        app.tracks = vec!["Track 1".to_string()];
        app.track_registry =
            sequencer::sequencer::TrackRegistry::for_legacy_track_count(1).unwrap();
        app
    }

    fn payload(fields: Vec<(&'static str, Value)>) -> Value {
        crate::values::map_value(fields)
    }

    #[test]
    fn instance_commands_round_trip_through_dispatch() {
        sequencer::lisp_host::clear_kind_registry();
        sequencer::lisp_host::register_kind(sequencer::lisp_host::KindDefinition {
            id: "scratch:tst".to_string(),
            name: "tst".to_string(),
            package: None,
            module: None,
            sequencer: None,
            state_fields: Vec::new(),
            has_view: false,
            keymap: None,
        });
        let mut app = test_app();

        let status = apply_instance_command(
            "instance-create",
            &payload(vec![("kind", Value::String("scratch:tst".into()))]),
            &mut app,
        )
        .expect("create");
        assert_eq!(status, "Created tst 1");
        let id = app.instances.list[0].id;
        let by_id = |extra: Vec<(&'static str, Value)>| {
            let mut fields = vec![("id", Value::Number(id as f64))];
            fields.extend(extra);
            payload(fields)
        };

        let status = apply_instance_command(
            "instance-rename",
            &by_id(vec![("label", Value::String(" Kit A ".into()))]),
            &mut app,
        )
        .expect("rename");
        assert_eq!(status, "Renamed to Kit A");
        let status = apply_instance_command(
            "instance-rename",
            &by_id(vec![("label", Value::String("Kit A".into()))]),
            &mut app,
        )
        .expect("writing the current label back is not an error");
        assert!(status.is_empty(), "and shows no status: {status:?}");

        // Moving to the current owner is quiet; a group that is not a rack
        // is refused.
        let status = apply_instance_command("instance-move", &by_id(Vec::new()), &mut app)
            .expect("already project-owned");
        assert!(status.is_empty(), "{status:?}");
        assert!(apply_instance_command(
            "instance-move",
            &by_id(vec![("group-id", Value::Number(99.0))]),
            &mut app
        )
        .is_err());

        let status = apply_instance_command("instance-duplicate", &by_id(Vec::new()), &mut app)
            .expect("duplicate");
        assert_eq!(status, "Duplicated as tst 1");
        assert_eq!(app.instances.list.len(), 2);

        let status =
            apply_instance_command("instance-delete", &by_id(Vec::new()), &mut app).expect("delete");
        assert_eq!(status, "Deleted Kit A");
        assert!(!app.instances.contains(id));

        assert!(apply_instance_command("instance-delete", &by_id(Vec::new()), &mut app).is_err());
        assert!(apply_instance_command("instance-create", &payload(Vec::new()), &mut app).is_err());
        assert!(apply_instance_command("instance-bogus", &by_id(Vec::new()), &mut app).is_err());
    }
}
