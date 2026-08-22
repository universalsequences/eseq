mod agent;
mod dispatch;
mod drum_rack_v2;
mod effects;
mod instrument_authoring;
mod instrument_params;
mod learn;
mod misc;
mod project;
mod rack;
mod routing;
mod samples;
mod sampler_slices;
mod scenes;
mod scripts;
mod song;
mod step_history;
mod tracks;

pub(crate) use dispatch::dispatch_custom_host_command;
#[cfg(test)]
pub(crate) use learn::open_patch_learn_buffer;
#[cfg(test)]
pub(crate) use tracks::apply_rename_group_host_command;
pub(crate) use song::{apply_song_edit_command, apply_sound_palette_view_command};

use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use eseqlisp::vm::Value;
use eseqlisp::{Editor, HostEvent};
use sequencer::sequencer::SequencerState;
use sequencer::app;

use super::natives;
use super::state_values::{
    build_accumulator_names, build_effects_value, build_instrument_panel_value,
    build_midi_effects_value, build_step_has_plocks, build_steps_value, build_track_ids,
    build_track_names, push_solo_mutes, set_current_track_reactive, sync_all_track_sequencer_state,
    sync_fx_param_binding_fields, sync_groups_bindings, sync_step_param_lists,
    sync_track_mixer_state, sync_track_name_state, sync_track_params, sync_track_peak_fields,
};
use super::{map_number, map_string, map_u32, map_usize};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MacroHostCommandOutcome {
    NotMacro,
    Ignored,
    Applied,
}

pub(crate) fn handle_macro_host_command(
    name: &str,
    payload: &Value,
    app: &mut app::App,
    state: &SequencerState,
    current_track: usize,
) -> MacroHostCommandOutcome {
    use MacroHostCommandOutcome::{Applied, Ignored, NotMacro};

    let is_macro_command = matches!(
        name,
        "macro-create"
            | "macro-ensure"
            | "macro-create-scene"
            | "macro-scene-config"
            | "macro-delete"
            | "macro-rename"
            | "macro-set-value"
            | "macro-release"
            | "scene-push-begin"
            | "scene-push-set-value"
            | "scene-push-end"
            | "macro-map-param"
            | "macro-set-range"
            | "macro-set-curve"
            | "macro-unmap"
    );
    if !is_macro_command {
        return NotMacro;
    }
    let Value::Map(map) = payload else {
        return Ignored;
    };

    let command = match name {
        "macro-create" => {
            let Some(name) = map_string(map, "name") else {
                return Ignored;
            };
            app::AppCommand::MacroCreate { name }
        }
        "macro-create-scene" => {
            let (Some(name), Some(target_scene)) =
                (map_string(map, "name"), map_usize(map, "target-scene"))
            else {
                return Ignored;
            };
            app::AppCommand::MacroCreateScene { name, target_scene }
        }
        "macro-scene-config" => {
            let Some(id) = map_u32(map, "id") else {
                return Ignored;
            };
            let Some(existing) = app.macro_engine.scene_config(id).cloned() else {
                return Ignored;
            };
            let quantize = map_string(map, "quantize")
                .map(|value| match value.as_str() {
                    "off" => sequencer::macro_engine::StealQuantize::Off,
                    "sixteenth" | "1/16" => sequencer::macro_engine::StealQuantize::Sixteenth,
                    _ => sequencer::macro_engine::StealQuantize::Bar,
                })
                .unwrap_or(existing.quantize);
            let bool_value = |key: &str, fallback: bool| {
                map.get(key)
                    .and_then(|cell| match &*cell.borrow() {
                        Value::Bool(value) => Some(*value),
                        _ => None,
                    })
                    .unwrap_or(fallback)
            };
            let track_mask = map
                .get("track-mask")
                .and_then(|cell| match &*cell.borrow() {
                    Value::Nil => Some(None),
                    Value::List(items) => Some(Some(
                        items
                            .iter()
                            .map(|item| matches!(&*item.borrow(), Value::Bool(true)))
                            .collect(),
                    )),
                    _ => None,
                })
                .unwrap_or(existing.track_mask);
            app::AppCommand::MacroSceneConfig {
                id,
                config: sequencer::macro_engine::SceneMacroConfig {
                    target_scene: map_usize(map, "target-scene").unwrap_or(existing.target_scene),
                    morph_params: bool_value("morph-params", existing.morph_params),
                    steal_patterns: bool_value("steal-patterns", existing.steal_patterns),
                    quantize,
                    track_mask,
                },
            }
        }
        "macro-ensure" => {
            let (Some(key), Some(name)) = (map_string(map, "key"), map_string(map, "name")) else {
                return Ignored;
            };
            app::AppCommand::MacroEnsure { key, name }
        }
        "macro-delete" => {
            let Some(id) = map_u32(map, "id") else {
                return Ignored;
            };
            app::AppCommand::MacroDelete { id }
        }
        "macro-rename" => {
            let (Some(id), Some(name)) = (map_u32(map, "id"), map_string(map, "name")) else {
                return Ignored;
            };
            app::AppCommand::MacroRename { id, name }
        }
        "macro-set-value" => {
            let (Some(id), Some(value)) = (map_u32(map, "id"), map_number(map, "value")) else {
                return Ignored;
            };
            app::AppCommand::MacroSetValue {
                id,
                value: value as f32,
            }
        }
        "macro-release" => {
            let Some(id) = map_u32(map, "id") else {
                return Ignored;
            };
            app::AppCommand::MacroRelease { id }
        }
        "scene-push-begin" => {
            let Some(target_scene) = map_usize(map, "target-scene") else {
                return Ignored;
            };
            let value = map_number(map, "value").unwrap_or(1.0) as f32;
            app::AppCommand::ScenePushBegin {
                target_scene,
                value,
            }
        }
        "scene-push-set-value" => {
            let Some(value) = map_number(map, "value") else {
                return Ignored;
            };
            app::AppCommand::ScenePushSetValue {
                value: value as f32,
            }
        }
        "scene-push-end" => app::AppCommand::ScenePushEnd,
        "macro-map-param" => {
            let Some(id) = map_u32(map, "id") else {
                return Ignored;
            };
            let track = map_number(map, "track")
                .map(|track| track as usize)
                .unwrap_or(current_track);
            let target = match natives::param_target_from_value(state, track, payload) {
                Ok(target) => target,
                Err(error) => {
                    eprintln!("macro-map-param failed: {error}");
                    return Ignored;
                }
            };
            app::AppCommand::MacroMapParam { id, track, target }
        }
        "macro-set-range" => {
            let (Some(id), Some(mapping_idx), Some(min), Some(max)) = (
                map_u32(map, "id"),
                map_usize(map, "mapping-idx"),
                map_number(map, "min"),
                map_number(map, "max"),
            ) else {
                return Ignored;
            };
            app::AppCommand::MacroSetRange {
                id,
                mapping_idx,
                min: min as f32,
                max: max as f32,
            }
        }
        "macro-set-curve" => {
            let (Some(id), Some(mapping_idx), Some(curve)) = (
                map_u32(map, "id"),
                map_usize(map, "mapping-idx"),
                map_string(map, "curve"),
            ) else {
                return Ignored;
            };
            let curve = match curve.as_str() {
                "linear" => sequencer::macro_engine::MacroCurve::Linear,
                "exp" | "exponential" => sequencer::macro_engine::MacroCurve::Exp,
                "log" | "logarithmic" => sequencer::macro_engine::MacroCurve::Log,
                _ => return Ignored,
            };
            app::AppCommand::MacroSetCurve {
                id,
                mapping_idx,
                curve,
            }
        }
        "macro-unmap" => {
            let (Some(id), Some(mapping_idx)) = (map_u32(map, "id"), map_usize(map, "mapping-idx"))
            else {
                return Ignored;
            };
            app::AppCommand::MacroUnmap { id, mapping_idx }
        }
        _ => unreachable!("macro command set and command dispatch must stay in sync"),
    };

    app::apply_command(app, command);
    Applied
}

pub(crate) struct AddTrackInstrumentCtx<'a> {
    pub(crate) app: &'a mut app::App,
    pub(crate) editor: &'a mut Editor,
    pub(crate) state: &'a Arc<SequencerState>,
    pub(crate) current_track: &'a Arc<AtomicUsize>,
    pub(crate) track_names: &'a mut Vec<String>,
    pub(crate) track_pan_ids: &'a Arc<Mutex<Vec<i32>>>,
    pub(crate) record_armed: &'a Arc<Mutex<Vec<bool>>>,
    pub(crate) selected_steps: &'a Arc<Mutex<HashSet<usize>>>,
    pub(crate) accumulator_names: &'a Arc<Mutex<Vec<String>>>,
    pub(crate) cached_track_peak_levels: &'a [f64],
    pub(crate) group_id: Option<u64>,
    /// Pad note the new member claims when the drop landed on an empty
    /// drum-rack pad cell (docs/drum-rack-v2-spec.md, "Track budget").
    pub(crate) pad_note: Option<i32>,
    pub(crate) track_groups: &'a Arc<Mutex<Vec<sequencer::project::ProjectTrackGroup>>>,
    pub(crate) ui_epoch: &'a Arc<AtomicUsize>,
    pub(crate) lg_raw: *mut sequencer::audiograph::LiveGraph,
}

pub(crate) fn handle_add_track_instrument_command(payload: &Value, ctx: AddTrackInstrumentCtx<'_>) {
    let Some(name) = payload_name(payload) else {
        return;
    };

    match ctx.app.add_saved_instrument_track_sync(&name) {
        Ok(idx) => finish_added_instrument_track(idx, ctx),
        Err(e) => {
            ctx.editor.handle_host_event(HostEvent::Status(format!(
                "Error adding instrument track: {e}"
            )));
        }
    }
}

pub(crate) fn finish_added_instrument_track(idx: usize, ctx: AddTrackInstrumentCtx<'_>) {
    let AddTrackInstrumentCtx {
        app,
        editor,
        state,
        current_track,
        track_names,
        track_pan_ids,
        record_armed,
        selected_steps,
        accumulator_names,
        cached_track_peak_levels,
        group_id,
        pad_note,
        track_groups,
        ui_epoch,
        lg_raw,
    } = ctx;

    let groups_before = app.groups.clone();
    let attach = add_new_track_to_group(app, idx, group_id, pad_note);
    if let Some(status) = attach.rejection_status("Error adding instrument track") {
        // The attach failed. `add_new_track_to_group` already dropped the new
        // track when it could, so there is nothing to commit — reporting
        // success here is what used to strand a loose, padless track.
        *track_groups.lock().unwrap() = app.groups.clone();
        editor.handle_host_event(HostEvent::Status(status));
        return;
    }
    if let Err(error) = app.commit_created_track(idx, "Add instrument track") {
        app.groups = groups_before;
        *track_groups.lock().unwrap() = app.groups.clone();
        editor.handle_host_event(HostEvent::Status(format!(
            "Error adding instrument track: {error}"
        )));
        return;
    }
    *track_groups.lock().unwrap() = app.groups.clone();

    let selected = selection_after_added_track(idx, pad_note, current_track, app.tracks.len());
    current_track.store(selected, Ordering::Relaxed);
    let new_name = app.tracks[idx].clone();
    track_names.push(new_name.clone());

    {
        let mut pan_ids = track_pan_ids.lock().unwrap();
        pan_ids.push(app.graph.track_node_ids[idx].pan_id);
        push_solo_mutes(lg_raw, state);
    }
    record_armed.lock().unwrap().push(false);

    let rt = editor.runtime_mut();
    rt.set_reactive("SEQ", "num-tracks", Value::Number(track_names.len() as f64));
    rt.set_reactive("SEQ", "track-ids", build_track_ids(app));
    set_current_track_reactive(rt, app.tracks.len(), selected);
    rt.set_reactive("SEQ", "track-names", build_track_names(track_names));
    sync_all_track_sequencer_state(rt, state, app, selected, selected_steps);
    rt.set_reactive("SEQ", "steps", build_steps_value(state, selected));
    sync_step_param_lists(rt, state, selected);
    sync_track_mixer_state(rt, app, state);
    sync_groups_bindings(rt, &app.groups);
    sync_track_peak_fields(rt, cached_track_peak_levels);
    rt.set_reactive(
        "SEQ",
        "effects",
        build_effects_value(
            state,
            selected,
            &app.graph.effect_descriptors,
            selected_steps,
        ),
    );
    rt.set_reactive(
        "SEQ",
        "midi-effects",
        build_midi_effects_value(state, selected, selected_steps),
    );
    rt.set_reactive(
        "SEQ",
        "instrument-panel",
        build_instrument_panel_value(app, selected, selected_steps),
    );
    *accumulator_names.lock().unwrap() = build_accumulator_names(app);
    sync_track_params(rt, app, state, selected, selected_steps);
    sync_fx_param_binding_fields(rt, app, state, selected, selected_steps);
    rt.set_reactive(
        "SEQ",
        "step-has-plocks",
        build_step_has_plocks(state, selected, &app.graph.effect_descriptors),
    );
    rt.run_reactive_cycle();
    editor.refresh_runtime_side_effects();
    ui_epoch.fetch_add(1, Ordering::Relaxed);
    editor.handle_host_event(HostEvent::Status(format!(
        "Added instrument track {}: {new_name}",
        idx + 1
    )));
}

pub(crate) struct SwapTrackInstrumentCtx<'a> {
    pub(crate) app: &'a mut app::App,
    pub(crate) editor: &'a mut Editor,
    pub(crate) state: &'a Arc<SequencerState>,
    pub(crate) current_track: &'a Arc<AtomicUsize>,
    pub(crate) track_names: &'a mut Vec<String>,
    pub(crate) selected_steps: &'a Arc<Mutex<HashSet<usize>>>,
    pub(crate) fx_epoch: &'a Arc<AtomicUsize>,
    pub(crate) ui_epoch: &'a Arc<AtomicUsize>,
}

pub(crate) fn finish_swapped_instrument_track(
    name: &str,
    track: usize,
    summary: sequencer::sequencer::InstrumentSlotResetSummary,
    preserve_track_selection: bool,
    ctx: SwapTrackInstrumentCtx<'_>,
) {
    let SwapTrackInstrumentCtx {
        app,
        editor,
        state,
        current_track,
        track_names,
        selected_steps,
        fx_epoch,
        ui_epoch,
    } = ctx;
    let selected_track = selection_after_track_apply(
        track,
        preserve_track_selection,
        current_track,
        app.tracks.len(),
    );
    current_track.store(selected_track, Ordering::Relaxed);
    app.ui.cursor_track = selected_track;
    if !app.tracks.is_empty() {
        let rt = editor.runtime_mut();
        set_current_track_reactive(rt, app.tracks.len(), selected_track);
        sync_track_name_state(rt, track_names, app);
        sync_all_track_sequencer_state(rt, state, app, selected_track, selected_steps);
        rt.set_reactive("SEQ", "steps", build_steps_value(state, selected_track));
        sync_step_param_lists(rt, state, selected_track);
        rt.set_reactive(
            "SEQ",
            "effects",
            build_effects_value(
                state,
                selected_track,
                &app.graph.effect_descriptors,
                selected_steps,
            ),
        );
        rt.set_reactive(
            "SEQ",
            "midi-effects",
            build_midi_effects_value(state, selected_track, selected_steps),
        );
        rt.set_reactive(
            "SEQ",
            "instrument-panel",
            build_instrument_panel_value(app, selected_track, selected_steps),
        );
        sync_track_params(rt, app, state, selected_track, selected_steps);
        sync_fx_param_binding_fields(rt, app, state, selected_track, selected_steps);
        rt.set_reactive(
            "SEQ",
            "step-has-plocks",
            build_step_has_plocks(state, selected_track, &app.graph.effect_descriptors),
        );
        rt.run_reactive_cycle();
        editor.refresh_runtime_side_effects();
        editor.refresh_visible_layouts_for_buffer_named("*fx*");
    }
    fx_epoch.fetch_add(1, Ordering::Relaxed);
    ui_epoch.fetch_add(1, Ordering::Relaxed);
    editor.handle_host_event(HostEvent::Status(instrument_swap_status(name, summary)));
}

pub(crate) fn instrument_swap_status(
    name: &str,
    summary: sequencer::sequencer::InstrumentSlotResetSummary,
) -> String {
    let mut details = Vec::new();
    if summary.patterns_with_cleared_locks > 0 {
        let count = summary.patterns_with_cleared_locks;
        details.push(format!(
            "cleared instrument p-locks in {count} {}",
            if count == 1 { "pattern" } else { "patterns" }
        ));
    }
    if summary.process_bindings_dropped > 0 {
        let count = summary.process_bindings_dropped;
        details.push(format!(
            "dropped {count} stale process {}",
            if count == 1 { "binding" } else { "bindings" }
        ));
    }
    if summary.neural_overrides_dropped > 0 {
        let count = summary.neural_overrides_dropped;
        details.push(format!(
            "dropped {count} stale neural {}",
            if count == 1 { "override" } else { "overrides" }
        ));
    }
    let base = format!("Swapped → {name}");
    if details.is_empty() {
        base
    } else {
        format!("{base} ({})", details.join(", "))
    }
}

/// Track the panel should follow after applying an instrument to a track.
///
/// A drum-pad drop edits a rack member without navigating away from the rack;
/// direct track drops navigate to the changed track.
pub(crate) fn selection_after_track_apply(
    applied_track: usize,
    preserve_track_selection: bool,
    current_track: &AtomicUsize,
    track_count: usize,
) -> usize {
    if !preserve_track_selection {
        return applied_track;
    }
    let current = current_track.load(Ordering::Relaxed);
    if current < track_count {
        current
    } else {
        applied_track
    }
}

/// Track the panel should follow once `new_track` has been added.
///
/// Filling an empty drum-rack pad is an in-place edit of the rack the user is
/// already looking at, so the selection stays put. Every other add-track path
/// (mixer drop, Add Track, loose sample drop, non-pad browser drop) follows
/// the newly created track.
pub(crate) fn selection_after_added_track(
    new_track: usize,
    pad_note: Option<i32>,
    current_track: &AtomicUsize,
    track_count: usize,
) -> usize {
    selection_after_track_apply(
        new_track,
        pad_note.is_some(),
        current_track,
        track_count,
    )
}

/// What became of a freshly created track that was offered to a group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NewTrackGroupOutcome {
    /// No group was asked for: the new track stays loose, as intended.
    Loose,
    /// The track joined the group (and, for a rack, claimed its pad).
    Attached,
    /// The group refused the track — an out-of-domain pad note, an occupied
    /// pad, a full rack or a missing group. `rolled_back` reports whether the
    /// just-created track was deleted again; when it is, the caller must
    /// abandon the rest of the add-track flow because `track` no longer exists.
    Rejected { reason: String, rolled_back: bool },
}

impl NewTrackGroupOutcome {
    /// User-facing status for a rejection, or `None` when nothing went wrong.
    pub(crate) fn rejection_status(&self, action: &str) -> Option<String> {
        match self {
            NewTrackGroupOutcome::Loose | NewTrackGroupOutcome::Attached => None,
            NewTrackGroupOutcome::Rejected { reason, rolled_back } => Some(if *rolled_back {
                format!("{action}: {reason}")
            } else {
                format!("{action}: {reason} (the new track was left ungrouped)")
            }),
        }
    }

    pub(crate) fn is_rejected(&self) -> bool {
        matches!(self, NewTrackGroupOutcome::Rejected { .. })
    }
}

/// Adds a freshly created track to `group_id`. With `pad_note`, the group must
/// be a drum rack and the track becomes the member backing that pad — this is
/// the lazy-pad path: a pad claims a track only when a sound lands on it.
///
/// The attach legitimately fails (out-of-domain pad note, occupied pad, full
/// rack), and by then the track already exists. It cannot be pre-checked before
/// creation — `attach_track_to_group` validates against a live track index — so
/// a rejection rolls the track back here instead, before any caller commits it
/// to history. Callers get the reason to show the user.
pub(crate) fn add_new_track_to_group(
    app: &mut app::App,
    track: usize,
    group_id: Option<u64>,
    pad_note: Option<i32>,
) -> NewTrackGroupOutcome {
    add_new_track_to_group_with_rollback(app, track, group_id, pad_note, |app, track| {
        app.graph_controller().delete_track(track).map(|_| ())
    })
}

/// Rollback-injectable core of [`add_new_track_to_group`] so the rejection
/// paths are testable without a live audio graph.
fn add_new_track_to_group_with_rollback<R>(
    app: &mut app::App,
    track: usize,
    group_id: Option<u64>,
    pad_note: Option<i32>,
    rollback: R,
) -> NewTrackGroupOutcome
where
    R: FnOnce(&mut app::App, usize) -> Result<(), String>,
{
    let Some(group_id) = group_id else {
        return NewTrackGroupOutcome::Loose;
    };
    let attached = if new_track_group_target(&app.groups, track, Some(group_id)).is_some() {
        app.attach_track_to_group(track, group_id, pad_note)
    } else {
        Err(format!(
            "Track group {group_id} cannot take track {}",
            track + 1
        ))
    };
    let Err(reason) = attached else {
        return NewTrackGroupOutcome::Attached;
    };
    // Nothing has been committed to history yet — every caller commits the
    // created track after this returns — so dropping the track here leaves no
    // half-transaction on the undo stack.
    match rollback(app, track) {
        Ok(()) => NewTrackGroupOutcome::Rejected {
            reason,
            rolled_back: true,
        },
        Err(rollback_error) => NewTrackGroupOutcome::Rejected {
            reason: format!("{reason}; rolling the new track back also failed ({rollback_error})"),
            rolled_back: false,
        },
    }
}

fn new_track_group_target(
    groups: &[sequencer::project::ProjectTrackGroup],
    track: usize,
    group_id: Option<u64>,
) -> Option<(usize, u64)> {
    let group_id = group_id?;
    groups
        .iter()
        .enumerate()
        .find(|(_, group)| group.id == group_id && !group.members.contains(&track))
        .map(|(index, group)| (index, group.bus_id))
}

fn payload_name(payload: &Value) -> Option<String> {
    let Value::Map(map) = payload else {
        return None;
    };
    let cell = map.get("name")?;
    let value = cell.borrow();
    match &*value {
        Value::String(name) => Some(name.clone()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(id: u64, bus_id: u64, members: Vec<usize>) -> sequencer::project::ProjectTrackGroup {
        sequencer::project::ProjectTrackGroup {
            id,
            name: format!("Group {id}"),
            color: [0.5; 3],
            collapsed: false,
            members,
            bus_id,
            rack: None,
            rack_members: Vec::new(),
        }
    }

    #[test]
    fn occupied_pad_replacement_keeps_the_current_track_selected() {
        let current = AtomicUsize::new(1);
        assert_eq!(selection_after_track_apply(4, true, &current, 5), 1);
    }

    #[test]
    fn direct_track_replacement_selects_the_applied_track() {
        let current = AtomicUsize::new(1);
        assert_eq!(selection_after_track_apply(4, false, &current, 5), 4);
    }

    #[test]
    fn occupied_pad_replacement_falls_back_when_selection_is_out_of_range() {
        let current = AtomicUsize::new(9);
        assert_eq!(selection_after_track_apply(4, true, &current, 5), 4);
    }

    #[test]
    fn empty_pad_drop_keeps_the_current_track_selected() {
        let current = AtomicUsize::new(1);
        assert_eq!(selection_after_added_track(4, Some(36), &current, 5), 1);
    }

    #[test]
    fn non_pad_add_selects_the_new_track() {
        let current = AtomicUsize::new(1);
        assert_eq!(selection_after_added_track(4, None, &current, 5), 4);
    }

    #[test]
    fn empty_pad_drop_falls_back_when_the_current_track_is_out_of_range() {
        let current = AtomicUsize::new(9);
        assert_eq!(selection_after_added_track(4, Some(36), &current, 5), 4);
    }

    #[test]
    fn new_track_group_target_resolves_stable_group_id() {
        let groups = vec![group(12, 4, vec![0, 1]), group(27, 9, vec![2, 3])];
        assert_eq!(new_track_group_target(&groups, 4, Some(27)), Some((1, 9)));
        assert_eq!(new_track_group_target(&groups, 4, Some(99)), None);
        assert_eq!(new_track_group_target(&groups, 4, None), None);
        assert_eq!(new_track_group_target(&groups, 3, Some(27)), None);
    }

    /// A drum rack whose only pad (note 36) is already taken by member track 0.
    fn rack_group() -> sequencer::project::ProjectTrackGroup {
        sequencer::project::ProjectTrackGroup {
            id: 7,
            name: "Kit".to_string(),
            color: [0.5; 3],
            collapsed: false,
            members: vec![0],
            bus_id: 2,
            rack: Some(sequencer::project::ProjectRackConfig {
                pads: vec![sequencer::project::ProjectRackPad {
                    pad_note: 36,
                    member: 0,
                }],
                choke_groups: vec![None],
            }),
            rack_members: Vec::new(),
        }
    }

    /// Two tracks: track 0 already backs the rack pad, track 1 stands in for a
    /// track that was just created and is about to be offered to the rack.
    fn rack_attach_test_app() -> app::App {
        let state = Arc::new(sequencer::sequencer::SequencerState::new(
            2,
            vec![
                sequencer::sequencer::default_empty_effect_chain(),
                sequencer::sequencer::default_empty_effect_chain(),
            ],
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
                bus_effect_runtime: Arc::new(Mutex::new(Arc::new(Vec::new()))),
                reverb_bus_id: 0,
                reverb_node_id: 0,
            },
            Arc::new(sequencer::recorder::MasterRecorder::new(44_100, 2)),
            keyboard_tx,
        );
        app.tracks = vec!["Kick".to_string(), "New".to_string()];
        app.track_registry = sequencer::sequencer::TrackRegistry::for_legacy_track_count(2)
            .expect("test track registry");
        app.groups = vec![rack_group()];
        app
    }

    /// Stands in for `graph_controller().delete_track`, which needs a live
    /// audio graph: drop the last track the way a real rollback would.
    fn stub_rollback(app: &mut app::App, track: usize) -> Result<(), String> {
        assert_eq!(track, app.tracks.len() - 1, "only the new track rolls back");
        app.tracks.remove(track);
        Ok(())
    }

    fn is_stranded(app: &app::App, track: usize) -> bool {
        app.tracks.len() > track
            && !app.groups.iter().any(|group| group.members.contains(&track))
    }

    #[test]
    fn attaching_a_new_track_to_an_occupied_pad_rolls_the_track_back() {
        let mut app = rack_attach_test_app();
        let outcome = add_new_track_to_group_with_rollback(
            &mut app,
            1,
            Some(7),
            Some(36),
            stub_rollback,
        );

        assert!(outcome.is_rejected(), "occupied pad must not report success");
        let status = outcome
            .rejection_status("Error adding instrument track")
            .expect("a rejection has a user-facing status");
        assert!(
            status.contains("already occupied"),
            "status should say why the pad refused the track: {status}"
        );
        assert_eq!(app.tracks.len(), 1, "the created track was rolled back");
        assert!(!is_stranded(&app, 1), "no loose ungrouped track is left behind");
        assert_eq!(
            app.groups[0].rack.as_ref().expect("rack").pads.len(),
            1,
            "the occupied pad still belongs to its original member",
        );
    }

    #[test]
    fn attaching_a_new_track_to_an_out_of_domain_pad_note_rolls_the_track_back() {
        let mut app = rack_attach_test_app();
        let outcome = add_new_track_to_group_with_rollback(
            &mut app,
            1,
            Some(7),
            Some(200),
            stub_rollback,
        );

        assert!(outcome.is_rejected(), "an out-of-domain note must not report success");
        let status = outcome
            .rejection_status("Error adding instrument track")
            .expect("a rejection has a user-facing status");
        assert!(
            status.contains("200"),
            "status should name the rejected pad note: {status}"
        );
        assert_eq!(app.tracks.len(), 1, "the created track was rolled back");
        assert!(!is_stranded(&app, 1), "no loose ungrouped track is left behind");
        assert!(
            app.groups[0].members == vec![0],
            "the rack keeps exactly its original member",
        );
    }

    #[test]
    fn a_failed_rollback_is_reported_as_a_left_behind_track() {
        let mut app = rack_attach_test_app();
        let outcome = add_new_track_to_group_with_rollback(&mut app, 1, Some(7), Some(36), |_, _| {
            Err("Cannot delete the last remaining track".to_string())
        });

        let status = outcome
            .rejection_status("Error adding instrument track")
            .expect("a rejection has a user-facing status");
        assert!(status.contains("already occupied"), "{status}");
        assert!(status.contains("left ungrouped"), "{status}");
        assert!(
            is_stranded(&app, 1),
            "the stranded track is exactly what the status now admits to",
        );
    }

    #[test]
    fn attaching_a_new_track_to_a_free_pad_succeeds() {
        let mut app = rack_attach_test_app();
        let outcome =
            add_new_track_to_group_with_rollback(&mut app, 1, Some(7), Some(38), stub_rollback);

        assert_eq!(outcome, NewTrackGroupOutcome::Attached);
        assert_eq!(outcome.rejection_status("Error adding instrument track"), None);
        assert_eq!(app.tracks.len(), 2, "the new track survives a good attach");
        assert_eq!(app.groups[0].members, vec![0, 1]);
    }

    #[test]
    fn no_group_requested_leaves_the_track_loose_without_an_error() {
        let mut app = rack_attach_test_app();
        let outcome = add_new_track_to_group_with_rollback(&mut app, 1, None, None, stub_rollback);

        assert_eq!(outcome, NewTrackGroupOutcome::Loose);
        assert_eq!(outcome.rejection_status("Error adding instrument track"), None);
        assert_eq!(app.tracks.len(), 2);
    }

    #[test]
    fn instrument_swap_status_reports_destructive_cleanup() {
        assert_eq!(
            instrument_swap_status(
                "core/drift",
                sequencer::sequencer::InstrumentSlotResetSummary::default(),
            ),
            "Swapped → core/drift"
        );
        assert_eq!(
            instrument_swap_status(
                "core/drift",
                sequencer::sequencer::InstrumentSlotResetSummary {
                    patterns_reset: 4,
                    patterns_with_cleared_locks: 3,
                    process_bindings_dropped: 1,
                    neural_overrides_dropped: 2,
                },
            ),
            "Swapped → core/drift (cleared instrument p-locks in 3 patterns, dropped 1 stale process binding, dropped 2 stale neural overrides)"
        );
    }
}
