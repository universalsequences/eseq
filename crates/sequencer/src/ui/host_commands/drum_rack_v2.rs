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
                source: None,
                    track,
                    transpose: 0.0,
                    velocity: 1.0,
                    note_off: false,
                }))
                .is_ok()
            {
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
            match app.save_rack_as_kit(group_id, &name, overwrite) {
                Ok(path) => {
                    let rt = editor.runtime_mut();
                    rt.set_reactive("SEQ", "kit-presets", build_kit_presets_value());
                    rt.run_reactive_cycle();
                    editor.refresh_runtime_side_effects();
                    editor.handle_host_event(HostEvent::Status(format!(
                        "Saved kit '{name}' to {}",
                        path.display()
                    )));
                }
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
                        sync_after_rack_structure_change(app, editor, ctx, None);
                        editor.handle_host_event(HostEvent::Status(format!(
                            "Auditioned kit '{name}'"
                        )));
                    }
                    Err(error) => editor.handle_host_event(HostEvent::Status(error)),
                }
            } else {
                let tracks_before = app.tracks.len();
                match app.load_kit_as_rack(Path::new(&path)) {
                    Ok((group_id, failures)) => {
                        let name = group_name(app, group_id).unwrap_or_else(|| "Kit".to_string());
                        let focus = (app.tracks.len() > tracks_before)
                            .then(|| app.tracks.len() - 1);
                        sync_after_rack_structure_change(app, editor, ctx, focus);
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

/// A rack group's display name by its stable id, for status messages and kit
/// naming. Groups are addressed by `GroupId`, never by index.
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
    rt.clear_subtree_effects_for_named_target("*sequencer*");
    rt.run_reactive_cycle();
    editor.refresh_runtime_side_effects();
    refresh_visible_track_topology_layouts(editor);
    ctx.frame.prev_track_playheads = track_playheads_snapshot(&state, app);
    ctx.frame.prev_track_button_states = track_button_state_snapshot(&state);
    ctx.shared.ui_epoch.fetch_add(1, Ordering::Relaxed);
}
