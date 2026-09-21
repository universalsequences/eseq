use crate::*;

/// Drum rack v2 polish commands (docs/drum-rack-v2-spec.md, "Polish"): the
/// per-member-row pad chrome (pad note + choke group), the pad grid's live
/// hits, and kits as browser objects. Everything here addresses a rack by its
/// stable `GroupId` and a pad by its note — never by track index, which moves
/// under track delete/reindex.
pub(super) const COMMANDS: &[&str] = &[
    "set-rack-pad-note",
    "set-rack-pad-choke-group",
    "trigger-rack-pad",
    "save-rack-as-kit",
    "load-kit",
    "audition-sound-on-rack",
    "attach-rack-sequencer",
    "detach-rack-sequencer",
    "move-sequencer-into-rack",
    // Rack clips (docs/rack-clips-and-break-kits-spec.md §4.2-§4.4, §6.3).
    "convert-rack-to-clips",
    "launch-rack-clip",
    "save-rack-clip-as",
    "delete-rack-clip",
    "rename-rack-clip",
];

/// How long a pad-grid hit sounds before its note-off. The pad grid is a
/// performance view, not a latch: a click is a hit, exactly as a key press is.
const PAD_HIT_DURATION: Duration = Duration::from_millis(180);

pub(super) fn handle(
    name: &str,
    payload: Value,
    app: &mut app::App,
    editor: &mut Editor,
    ctx: &mut LoopCtx<'_>,
) {
    let ui_epoch = ctx.shared.ui_epoch.clone();
    let track_groups = ctx.shared.track_groups.clone();
    let keyboard_tx = ctx.shared.keyboard_tx.clone();
    match name {
        // Move a pad to another note on the pad keyboard. The pad keeps its
        // grid position, member track and choke group.
        "set-rack-pad-note" => {
            let group_id = extract_usize_from_payload(&payload, "group-id").map(|id| id as u64);
            let pad_note = extract_i32_from_payload(&payload, "pad-note");
            let note = extract_i32_from_payload(&payload, "note");
            let (Some(group_id), Some(pad_note), Some(note)) = (group_id, pad_note, note) else {
                editor.handle_host_event(HostEvent::Status(
                    "set-rack-pad-note needs a group id, pad note and note".to_string(),
                ));
                return;
            };
            match app.set_rack_pad_note_recorded(group_id, pad_note, note) {
                Ok(()) => sync_rack_pad_map(app, editor, &track_groups, &ui_epoch),
                Err(error) => editor.handle_host_event(HostEvent::Status(error)),
            }
        }
        // Choke group of a pad; `value` 0 clears it (choke groups start at 1,
        // because 0 is the packed "unassigned" runtime key).
        "set-rack-pad-choke-group" => {
            let group_id = extract_usize_from_payload(&payload, "group-id").map(|id| id as u64);
            let pad_note = extract_i32_from_payload(&payload, "pad-note");
            let value = extract_i32_from_payload(&payload, "value");
            let (Some(group_id), Some(pad_note), Some(value)) = (group_id, pad_note, value) else {
                editor.handle_host_event(HostEvent::Status(
                    "set-rack-pad-choke-group needs a group id, pad note and value".to_string(),
                ));
                return;
            };
            let choke = u8::try_from(value).ok().filter(|value| *value > 0);
            match app.set_rack_pad_choke_group_recorded(group_id, pad_note, choke) {
                Ok(()) => sync_rack_pad_map(app, editor, &track_groups, &ui_epoch),
                Err(error) => editor.handle_host_event(HostEvent::Status(error)),
            }
        }
        // Pad grid hit: the same live path a pad key takes — the pad's member
        // track at base pitch (transpose 0), so choke groups and the member's
        // own fx chain apply exactly as they do from the keyboard.
        "trigger-rack-pad" => {
            let group_id = extract_usize_from_payload(&payload, "group-id").map(|id| id as u64);
            let pad_note = extract_i32_from_payload(&payload, "pad-note");
            let (Some(group_id), Some(pad_note)) = (group_id, pad_note) else {
                return;
            };
            let Some(track) = app
                .groups
                .iter()
                .find(|group| group.id == group_id)
                .and_then(|group| group.rack_pad_track(pad_note))
            else {
                return;
            };
            release_matching_key_lock_auditions(
                &mut ctx.sessions.pending_key_lock_auditions,
                &keyboard_tx,
                track,
                0.0,
            );
            if keyboard_tx
                .send(sequencer::sequencer::LiveInputEvent::Note(KeyboardTrigger {
                    generation: 0,
                source: None,
                    track,
                    transpose: 0.0,
                    velocity: 1.0,
                    note_off: false,
                }))
                .is_ok()
            {
                if let Some(id) = app.track_registry.id_at(track) {
                    app.retrospective.trig(id, 0.0, Instant::now(), PAD_HIT_DURATION);
                }
                ctx.sessions
                    .pending_key_lock_auditions
                    .push(PendingKeyLockAudition {
                        track,
                        transpose: 0.0,
                        release_at: Instant::now() + PAD_HIT_DURATION,
                    });
            }
        }
        // Save the rack as a kit browser object: group config + one Sound per
        // pad, no patterns.
        "save-rack-as-kit" => {
            let group_id = extract_usize_from_payload(&payload, "group-id").map(|id| id as u64);
            let Some(group_id) = group_id else {
                editor.handle_host_event(HostEvent::Status(
                    "save-rack-as-kit needs a group id".to_string(),
                ));
                return;
            };
            let name = extract_string_from_payload(&payload, "name")
                .map(|name| name.trim().to_string())
                .filter(|name| !name.is_empty())
                .or_else(|| group_name(app, group_id))
                .unwrap_or_else(|| "Kit".to_string());
            let overwrite = extract_bool_from_payload(&payload, "overwrite");
            // The scene checklist (§7.2). Absent/empty = today's kit: pads and
            // bus chain only, no clips.
            let scenes = crate::state_values::extract_usize_list_from_payload(&payload, "scenes");
            match app.save_rack_as_kit(group_id, &name, overwrite, &scenes) {
                Ok((path, warnings)) => {
                    let rt = editor.runtime_mut();
                    rt.set_reactive("SEQ", "kit-presets", build_kit_presets_value());
                    rt.run_reactive_cycle();
                    editor.refresh_runtime_side_effects();
                    sync_rack_pad_map(app, editor, &track_groups, &ui_epoch);
                    let mut status = format!("Saved kit '{name}' to {}", path.display());
                    if !warnings.is_empty() {
                        status = format!("{status} ({})", warnings.join("; "));
                    }
                    editor.handle_host_event(HostEvent::Status(status));
                }
                Err(error) => editor.handle_host_event(HostEvent::Status(error)),
            }
        }
        // Evaluate a Lisp form (`(load "path")`, `(import pkg)` or script
        // text) with every graph def-sequencer it publishes owned by the rack,
        // and record each new instance so it comes back on project open
        // (docs/rack-clips-and-break-kits-spec.md §5.1).
        "attach-rack-sequencer" => {
            let group_id = extract_usize_from_payload(&payload, "group-id").map(|id| id as u64);
            let source = extract_string_from_payload(&payload, "source")
                .map(|source| source.trim().to_string())
                .filter(|source| !source.is_empty());
            let (Some(group_id), Some(source)) = (group_id, source) else {
                editor.handle_host_event(HostEvent::Status(
                    "attach-rack-sequencer needs a group id and a source form".to_string(),
                ));
                return;
            };
            if !app.groups.iter().any(|group| group.id == group_id && group.rack.is_some()) {
                editor.handle_host_event(HostEvent::Status(
                    format!("Track group {group_id} is not a drum rack"),
                ));
                return;
            }
            let attached = match evaluate_rack_sequencer_source(editor, app, group_id, &source) {
                Ok(attached) => attached,
                Err(error) => {
                    editor.handle_host_event(HostEvent::Status(error));
                    return;
                }
            };
            if attached.is_empty() {
                editor.handle_host_event(HostEvent::Status(
                    "The source published no graph sequencer; nothing attached".to_string(),
                ));
                return;
            }
            let mut names = Vec::new();
            for (id, name) in attached {
                if let Err(error) = app.attach_rack_sequencer_recorded(group_id, id, &name, &source) {
                    editor.handle_host_event(HostEvent::Status(error));
                    return;
                }
                names.push(name);
            }
            sync_rack_pad_map(app, editor, &track_groups, &ui_epoch);
            editor.handle_host_event(HostEvent::Status(format!(
                "Attached {} to {}",
                names.join(", "),
                group_name(app, group_id).unwrap_or_else(|| "rack".to_string())
            )));
        }
        "detach-rack-sequencer" => {
            let group_id = extract_usize_from_payload(&payload, "group-id").map(|id| id as u64);
            let sequencer_id = extract_usize_from_payload(&payload, "sequencer-id").map(|id| id as u64);
            let (Some(group_id), Some(sequencer_id)) = (group_id, sequencer_id) else {
                editor.handle_host_event(HostEvent::Status(
                    "detach-rack-sequencer needs a group id and a sequencer id".to_string(),
                ));
                return;
            };
            match app.detach_rack_sequencer_recorded(group_id, sequencer_id) {
                Ok((name, source)) => {
                    sync_rack_pad_map(app, editor, &track_groups, &ui_epoch);
                    let mut status = format!("Detached '{name}'; its routes are plain tracks again");
                    // Re-run the module now that no rack owns it, so the
                    // project-owned instance and its UI come straight back.
                    if !source.trim().is_empty() {
                        if let Err(error) = reevaluate_sequencer_source(editor, &source) {
                            status = format!("{status}; re-running the script failed: {error}");
                        }
                    }
                    editor.handle_host_event(HostEvent::Status(status));
                }
                Err(error) => editor.handle_host_event(HostEvent::Status(error)),
            }
        }
        "move-sequencer-into-rack" => {
            let group_id = extract_usize_from_payload(&payload, "group-id").map(|id| id as u64);
            let sequencer_id = extract_usize_from_payload(&payload, "sequencer-id").map(|id| id as u64);
            // The UI passes the script's source PATH (from the step-tab
            // registry); a declared, resolvable module is recorded as an
            // import, while a plain script keeps its load semantics.
            let source_path = extract_string_from_payload(&payload, "source-path")
                .map(|path| path.trim().to_string())
                .unwrap_or_default();
            let source = source_form_for_script_path(&source_path);
            let (Some(group_id), Some(sequencer_id)) = (group_id, sequencer_id) else {
                editor.handle_host_event(HostEvent::Status(
                    "move-sequencer-into-rack needs a group id and a sequencer id".to_string(),
                ));
                return;
            };
            match app.move_sequencer_into_rack_recorded(group_id, sequencer_id, &source) {
                Ok(_) => {
                    sync_rack_pad_map(app, editor, &track_groups, &ui_epoch);
                    let rack = group_name(app, group_id).unwrap_or_else(|| "rack".to_string());
                    let mut status = format!("{rack} now owns the sequencer; routes are its members");
                    // Re-run the module now that the rack owns it (the owner
                    // map is published), so its tab and route dropdown switch
                    // over right away. A fresh eval pass re-imports.
                    if !source.is_empty() {
                        if let Err(error) = reevaluate_sequencer_source(editor, &source) {
                            status = format!("{status}; re-running the script failed: {error}");
                        }
                    }
                    editor.handle_host_event(HostEvent::Status(status));
                }
                Err(error) => editor.handle_host_event(HostEvent::Status(error)),
            }
        }
        // Legacy rack -> clips, once (§4.3). One undoable patch, no audible
        // change: every scene keeps playing exactly what it played.
        "convert-rack-to-clips" => {
            let Some(group_id) = extract_usize_from_payload(&payload, "group-id").map(|id| id as u64)
            else {
                editor.handle_host_event(HostEvent::Status(
                    "convert-rack-to-clips needs a group id".to_string(),
                ));
                return;
            };
            match app.convert_rack_to_clips_recorded(group_id) {
                Ok(created) => {
                    sync_rack_pad_map(app, editor, &track_groups, &ui_epoch);
                    editor.handle_host_event(HostEvent::Status(format!(
                        "{} now has {created} clip(s)",
                        group_name(app, group_id).unwrap_or_else(|| "Rack".to_string())
                    )));
                }
                Err(error) => editor.handle_host_event(HostEvent::Status(error)),
            }
        }
        // Clip launch (§4.4): set the pointer in the current scene, then
        // relaunch the current scene through the ordinary scene-launch path,
        // so it rides quantized launch with no new boundary code.
        "launch-rack-clip" => {
            let group_id = extract_usize_from_payload(&payload, "group-id").map(|id| id as u64);
            let clip_id = extract_usize_from_payload(&payload, "clip-id").map(|id| id as u64);
            let Some(group_id) = group_id else {
                editor.handle_host_event(HostEvent::Status(
                    "launch-rack-clip needs a group id".to_string(),
                ));
                return;
            };
            // clip-id < 0 (absent) means "silence this rack in this scene".
            let clip = clip_id.filter(|id| *id > 0);
            if let Err(error) = app.set_current_rack_clip_recorded(group_id, clip) {
                editor.handle_host_event(HostEvent::Status(error));
                return;
            }
            let quantize = extract_string_from_payload(&payload, "quantize")
                .unwrap_or_else(|| "off".to_string());
            let scene = ctx.shared.state.current_scene_index();
            let relaunch = Value::Map(
                [
                    ("idx".to_string(), Value::Number(scene as f64)),
                    ("quantize".to_string(), Value::String(quantize)),
                ]
                .into_iter()
                .map(|(key, value)| (key, std::rc::Rc::new(std::cell::RefCell::new(value))))
                .collect(),
            );
            super::scenes::handle("switch-pattern", relaunch, app, editor, ctx);
            sync_rack_pad_map(app, editor, &track_groups, &ui_epoch);
        }
        "save-rack-clip-as" => {
            let group_id = extract_usize_from_payload(&payload, "group-id").map(|id| id as u64);
            let name = extract_string_from_payload(&payload, "name").unwrap_or_default();
            let Some(group_id) = group_id else {
                editor.handle_host_event(HostEvent::Status(
                    "save-rack-clip-as needs a group id".to_string(),
                ));
                return;
            };
            match app.save_rack_clip_as_recorded(group_id, &name) {
                Ok(_) => {
                    sync_rack_pad_map(app, editor, &track_groups, &ui_epoch);
                    editor.handle_host_event(HostEvent::Status("Saved rack clip".to_string()));
                }
                Err(error) => editor.handle_host_event(HostEvent::Status(error)),
            }
        }
        "delete-rack-clip" => {
            let group_id = extract_usize_from_payload(&payload, "group-id").map(|id| id as u64);
            let clip_id = extract_usize_from_payload(&payload, "clip-id").map(|id| id as u64);
            let (Some(group_id), Some(clip_id)) = (group_id, clip_id) else {
                editor.handle_host_event(HostEvent::Status(
                    "delete-rack-clip needs a group id and a clip id".to_string(),
                ));
                return;
            };
            match app.delete_rack_clip_recorded(group_id, clip_id) {
                Ok(()) => sync_rack_pad_map(app, editor, &track_groups, &ui_epoch),
                Err(error) => editor.handle_host_event(HostEvent::Status(error)),
            }
        }
        "rename-rack-clip" => {
            let group_id = extract_usize_from_payload(&payload, "group-id").map(|id| id as u64);
            let clip_id = extract_usize_from_payload(&payload, "clip-id").map(|id| id as u64);
            let name = extract_string_from_payload(&payload, "name").unwrap_or_default();
            let (Some(group_id), Some(clip_id)) = (group_id, clip_id) else {
                editor.handle_host_event(HostEvent::Status(
                    "rename-rack-clip needs a group id and a clip id".to_string(),
                ));
                return;
            };
            match app.rename_rack_clip_recorded(group_id, clip_id, &name) {
                Ok(()) => sync_rack_pad_map(app, editor, &track_groups, &ui_epoch),
                Err(error) => editor.handle_host_event(HostEvent::Status(error)),
            }
        }
        // With a selected rack, activating a kit swaps that rack's complete
        // pad/sound assignment in one undo entry. With no addressed rack the
        // browser's create behavior remains: append a new rack.
        "load-kit" => {
            let Some(path) = extract_path_from_payload(&payload) else {
                editor.handle_host_event(HostEvent::Status(
                    "Kit drop is missing a path".to_string(),
                ));
                return;
            };
            let selected_rack = extract_usize_from_payload(&payload, "group-id")
                .map(|id| id as u64);
            if let Some(group_id) = selected_rack {
                match app.load_kit_onto_rack(group_id, Path::new(&path)) {
                    Ok(name) => {
                        // Publish the rebuilt rack to the UI runtime BEFORE
                        // running its scripts: a rack-owned script reads
                        // `SEQ.groups` (its tab wears the rack's name) and
                        // must see the members it now has.
                        sync_after_rack_structure_change(app, editor, ctx, None);
                        let failures = evaluate_rack_sequencers(editor, app, group_id);
                        refresh_after_rack_scripts(editor);
                        let mut status = format!("Auditioned kit '{name}'");
                        if !failures.is_empty() {
                            status = format!("{status} ({})", failures.join("; "));
                        }
                        editor.handle_host_event(HostEvent::Status(status));
                    }
                    Err(error) => editor.handle_host_event(HostEvent::Status(error)),
                }
            } else {
                let tracks_before = app.tracks.len();
                match app.load_kit_as_rack(Path::new(&path)) {
                    Ok((group_id, mut failures)) => {
                        let name = group_name(app, group_id).unwrap_or_else(|| "Kit".to_string());
                        let focus = (app.tracks.len() > tracks_before)
                            .then(|| app.tracks.len() - 1);
                        // The App recorded the kit's sequencers but cannot run
                        // Lisp; evaluating each recorded source under the new
                        // rack is the host half of the import (§7.3). A module
                        // that is not installed is reported and its entry stays
                        // recorded, so a later re-import brings it back.
                        //
                        // The new rack is published to the UI runtime FIRST:
                        // a rack-owned script reads `SEQ.groups` on evaluation
                        // (its tab is named after the rack), and a group the
                        // runtime has not heard of yet fails that eval, which
                        // rolls the script's panel and tab back while the
                        // def-sequencer it already published stays behind.
                        sync_after_rack_structure_change(app, editor, ctx, focus);
                        failures.extend(evaluate_rack_sequencers(editor, app, group_id));
                        refresh_after_rack_scripts(editor);
                        let status = if failures.is_empty() {
                            format!("Loaded kit '{name}'")
                        } else {
                            format!("Loaded kit '{name}' ({})", failures.join("; "))
                        };
                        editor.handle_host_event(HostEvent::Status(status));
                    }
                    Err(error) => editor.handle_host_event(HostEvent::Status(error)),
                }
            }
        }
        "audition-sound-on-rack" => {
            let group_id = extract_usize_from_payload(&payload, "group-id").map(|id| id as u64);
            let path = extract_path_from_payload(&payload);
            let (Some(group_id), Some(path)) = (group_id, path) else {
                editor.handle_host_event(HostEvent::Status(
                    "Sound audition needs a drum rack and path".to_string(),
                ));
                return;
            };
            match app.replace_rack_with_sound(group_id, Path::new(&path)) {
                Ok(track) => {
                    super::initialize_loaded_rack_view(app, editor, track);
                    // The selected entity is now an ordinary track, not the
                    // removed rack's bus. Move the shared Lisp selection onto
                    // that track before rebuilding browser/sidebar state.
                    if let Err(error) = editor.runtime_mut().eval_str(
                        "(set! eseq.seq-core-state/selected-bus -1)",
                    ) {
                        editor.handle_host_event(HostEvent::Status(format!(
                            "Sound loaded, but rack selection could not be cleared: {error:?}"
                        )));
                    }
                    sync_after_rack_structure_change(app, editor, ctx, Some(track));
                    editor.handle_host_event(HostEvent::Status(
                        "Auditioned Sound on drum rack".to_string(),
                    ));
                }
                Err(error) => editor.handle_host_event(HostEvent::Status(error)),
            }
        }
        _ => {}
    }
}

/// Preserve the source's actual loading contract. Only named modules that
/// resolve to this file can be imported; living under a load root alone does
/// not make a plain script a module. Factory loads use portable `@/` paths.
pub(crate) fn source_form_for_script_path(path: &str) -> String {
    if path.is_empty() {
        return String::new();
    }
    match module_name_for_script_path(path) {
        Some(module) => format!("(import {module})"),
        None => {
            let factory = sequencer::app_paths::app_paths().factory_root();
            let load_path = Path::new(path).strip_prefix(&factory)
                .map(|relative| format!("@/{}", relative.display()))
                .unwrap_or_else(|_| path.to_string());
            format!("(load {load_path:?})")
        }
    }
}

/// Evaluate a recorded sequencer source in a fresh pass. Imports are
/// load-once per pass, so this re-runs the module even though the scratch
/// imported it earlier; the current owner map decides who owns the result.
pub(crate) fn reevaluate_sequencer_source(editor: &mut Editor, source: &str) -> Result<(), String> {
    let overlays = editor.snapshot_file_backed_sources();
    let report = editor.runtime_mut().eval_source_transactional(None, source, overlays);
    if report.success {
        editor.refresh_runtime_side_effects();
        Ok(())
    } else {
        Err(report.failure_message())
    }
}

/// The declared module name, provided the production resolver selects this
/// file. This also respects scoped package prefixes and module shadowing.
pub(crate) fn module_name_for_script_path(path: &str) -> Option<String> {
    let script = std::fs::canonicalize(path).ok()?;
    let source = std::fs::read_to_string(&script).ok()?;
    let (module, _) = eseqlisp::modules::inspect_exports(&source).ok()?;
    let module = module?;
    let mut sources = eseqlisp::hot_reload::SourceManager::new();
    sources.set_scoped_module_load_roots(sequencer::app_paths::app_paths().module_load_roots().0);
    let resolved = sources.load_module_source(
        &module, &eseqlisp::modules::module_relative_file_candidates(&module),
    )?.ok()?;
    (resolved.path == script).then_some(module)
}

/// Evaluate `source` on the UI runtime with every graph def-sequencer it
/// publishes owned by `group_id`. Returns the (id, name) of each graph
/// sequencer the evaluation newly published under that owner.
pub(crate) fn evaluate_rack_sequencer_source(
    editor: &mut Editor,
    app: &app::App,
    group_id: u64,
    source: &str,
) -> Result<Vec<(u64, String)>, String> {
    let before: std::collections::HashSet<u64> = app
        .state
        .published_sequencers()
        .iter()
        .map(|published| published.id)
        .collect();
    let overlays = editor.snapshot_file_backed_sources();
    let report = sequencer::lisp_host::with_graph_owner_rack(Some(group_id), || {
        editor.runtime_mut().eval_source_transactional(None, source, overlays)
    });
    let failure = (!report.success).then(|| report.failure_message());
    // Project reopen can restore only rack sources (with empty scratch).
    // Materialize their buffers and tab operations here, independent of any
    // later scratch report or reactive tick, and retain reload diagnostics.
    editor.process_lisp_reload_report(report);
    if let Some(failure) = failure {
        return Err(format!("Rack sequencer source failed: {failure}"));
    }
    Ok(app
        .state
        .published_sequencers()
        .into_iter()
        .filter(|published| !before.contains(&published.id))
        .filter(|published| {
            published
                .graph
                .as_ref()
                .is_some_and(|manifest| manifest.owner_rack == Some(group_id))
        })
        .map(|published| (published.id, published.name))
        .collect())
}

/// Evaluate every source a rack has recorded, under that rack, and report the
/// ones that failed by name. This is the host half of a break-kit import
/// (§7.3) and of a break-kit audition: the App records the entries, only the
/// host can run Lisp. The recorded entry survives a failure on purpose, so a
/// project that later installs the missing package gets the instance back.
fn evaluate_rack_sequencers(
    editor: &mut Editor,
    app: &app::App,
    group_id: u64,
) -> Vec<String> {
    let recorded = app.rack_sequencers(group_id);
    let mut failures = Vec::new();
    for sequencer in recorded {
        if sequencer.source.trim().is_empty() {
            continue;
        }
        if let Err(error) =
            evaluate_rack_sequencer_source(editor, app, group_id, &sequencer.source)
        {
            let what = sequencer::app::rack_sequencer_module(&sequencer.source)
                .unwrap_or_else(|| sequencer.sequencer_name.clone());
            failures.push(format!("{what}: {error}"));
        }
    }
    failures
}

/// Settle the UI runtime after rack scripts were evaluated as a follow-up to a
/// topology sync: the panels and tabs those scripts registered need a
/// reactive cycle and a side-effect flush to appear.
fn refresh_after_rack_scripts(editor: &mut Editor) {
    editor.runtime_mut().run_reactive_cycle();
    editor.refresh_runtime_side_effects();
}

pub(super) fn group_name(app: &app::App, group_id: u64) -> Option<String> {
    app.groups
        .iter()
        .find(|group| group.id == group_id)
        .map(|group| group.name.clone())
}

/// Republishes what a pad-map edit can change: the group value the grid reads
/// its pad badges and choke selectors from, and the groups snapshot the live
/// keyboard's pad routing reads.
fn sync_rack_pad_map(
    app: &mut app::App,
    editor: &mut Editor,
    track_groups: &Arc<Mutex<Vec<sequencer::project::ProjectTrackGroup>>>,
    ui_epoch: &Arc<AtomicUsize>,
) {
    *track_groups.lock().unwrap() = app.groups.clone();
    let rt = editor.runtime_mut();
    sync_groups_bindings(rt, &app.groups);
    // Clip bank edits (create/rename/delete/convert/launch) do not bump the
    // pattern epoch, so the clip run's source is republished here explicitly.
    rt.set_reactive("SEQ", "rack-clips", build_rack_clips_value(&app.state));
    rt.run_reactive_cycle();
    editor.refresh_runtime_side_effects();
    ui_epoch.fetch_add(1, Ordering::Relaxed);
}

/// Republishes everything a rack-shaped topology edit can touch: groups,
/// buses, track vectors, meters and the current sidebar. Kit creation only
/// grows topology, while rack audition can both add and delete member tracks,
/// so rebuilding from `App` is the single safe synchronization path.
/// `focus` is an explicit resulting track (a rack-to-Sound swap); otherwise
/// the existing current track is retained and clamped after reindexing.
pub(super) fn sync_after_rack_structure_change(
    app: &mut app::App,
    editor: &mut Editor,
    ctx: &mut LoopCtx<'_>,
    focus: Option<usize>,
) {
    let state = ctx.shared.state.clone();
    let current = ctx.shared.current_track.load(Ordering::Relaxed);
    let current = focus.unwrap_or(current)
        .min(app.tracks.len().saturating_sub(1));
    ctx.shared.current_track.store(current, Ordering::Relaxed);
    *ctx.shared.track_pan_ids.lock().unwrap() = app.graph.track_node_ids.iter()
        .map(|ids| ids.pan_id).collect();
    *ctx.shared.record_armed.lock().unwrap() = app.graph.record_armed.clone();
    *ctx.shared.bus_state.lock().unwrap() = app.buses.clone();
    *ctx.shared.bus_node_ids.lock().unwrap() = app.graph.bus_node_ids.clone();
    *ctx.shared.track_groups.lock().unwrap() = app.groups.clone();
    natives::prune_stale_group_references(
        &ctx.shared.armed_rack,
        &ctx.shared.active_delete_target,
        &ctx.shared.active_delete_target_version,
        &app.groups,
    );
    push_solo_mutes(ctx.shared.lg_raw, app, &state);
    ctx.meters.cached_track_peak_levels =
        read_track_peak_levels(app.graph.lg, &app.graph.track_node_ids);
    ctx.meters.cached_bus_peak_levels =
        read_bus_peak_levels(app.graph.lg, &app.graph.bus_node_ids);
    (ctx.meters.cached_modulator_phases, ctx.meters.cached_modulator_levels) =
        read_modulator_display_values(app.graph.lg, app);
    ctx.meters.last_meter_poll_at = Instant::now();

    let rt = editor.runtime_mut();
    sync_track_topology_state(
        rt,
        app,
        &state,
        ctx.track_names,
        current,
        &ctx.shared.selected_steps,
        &ctx.shared.piano_roll_selection,
        &ctx.shared.accumulator_names,
        &ctx.shared.record_armed,
        &ctx.meters.cached_track_peak_levels,
    );
    sync_groups_bindings(rt, &app.groups);
    sync_bus_mixer_state(rt, app);
    sync_bus_peak_fields(rt, &ctx.meters.cached_bus_peak_levels);
    sync_modulator_phase_fields(rt, &ctx.meters.cached_modulator_phases);
    sync_modulator_level_fields(rt, &ctx.meters.cached_modulator_levels);
    sync_mod_port_level_fields(rt, &ctx.meters.cached_mod_port_levels);
    rt.clear_subtree_effects_for_named_target("*sequencer*");
    rt.run_reactive_cycle();
    editor.refresh_runtime_side_effects();
    refresh_visible_track_topology_layouts(editor);
    ctx.frame.prev_track_playheads = track_playheads_snapshot(&state, app);
    ctx.frame.prev_track_button_states = track_button_state_snapshot(&state);
    ctx.shared.ui_epoch.fetch_add(1, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::{module_name_for_script_path, source_form_for_script_path};

    #[test]
    fn package_scripts_record_their_import_and_plain_files_record_a_load() {
        let factory = sequencer::app_paths::app_paths().factory_root();
        let module = factory.join("packages/alez.tracker/src/ui.lisp");
        let module = module.to_str().unwrap();
        assert_eq!(source_form_for_script_path(module), "(import alez.tracker.ui)");
        assert_eq!(
            module_name_for_script_path(module).as_deref(), Some("alez.tracker.ui")
        );
        let script = factory.join("scripts/sequencers/graph-neural-variable-reset-demo.lisp");
        assert_eq!(module_name_for_script_path(script.to_str().unwrap()), None);
        assert_eq!(
            source_form_for_script_path(script.to_str().unwrap()),
            "(load \"@/scripts/sequencers/graph-neural-variable-reset-demo.lisp\")"
        );
        assert_eq!(
            source_form_for_script_path("/elsewhere/scripts/x.lisp"),
            "(load \"/elsewhere/scripts/x.lisp\")"
        );
        assert_eq!(module_name_for_script_path("/elsewhere/scripts/x.lisp"), None);
        assert_eq!(source_form_for_script_path(""), "");
    }
}
