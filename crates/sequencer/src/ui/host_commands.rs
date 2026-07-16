use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use eseqlisp::vm::Value;
use eseqlisp::{Editor, HostEvent};
use sequencer::sequencer::SequencerState;
use sequencer::tui;

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
    app: &mut tui::App,
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
            tui::AppCommand::MacroCreate { name }
        }
        "macro-create-scene" => {
            let (Some(name), Some(target_scene)) =
                (map_string(map, "name"), map_usize(map, "target-scene"))
            else {
                return Ignored;
            };
            tui::AppCommand::MacroCreateScene { name, target_scene }
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
            tui::AppCommand::MacroSceneConfig {
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
            tui::AppCommand::MacroEnsure { key, name }
        }
        "macro-delete" => {
            let Some(id) = map_u32(map, "id") else {
                return Ignored;
            };
            tui::AppCommand::MacroDelete { id }
        }
        "macro-rename" => {
            let (Some(id), Some(name)) = (map_u32(map, "id"), map_string(map, "name")) else {
                return Ignored;
            };
            tui::AppCommand::MacroRename { id, name }
        }
        "macro-set-value" => {
            let (Some(id), Some(value)) = (map_u32(map, "id"), map_number(map, "value")) else {
                return Ignored;
            };
            tui::AppCommand::MacroSetValue {
                id,
                value: value as f32,
            }
        }
        "macro-release" => {
            let Some(id) = map_u32(map, "id") else {
                return Ignored;
            };
            tui::AppCommand::MacroRelease { id }
        }
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
            tui::AppCommand::MacroMapParam { id, track, target }
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
            tui::AppCommand::MacroSetRange {
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
            tui::AppCommand::MacroSetCurve {
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
            tui::AppCommand::MacroUnmap { id, mapping_idx }
        }
        _ => unreachable!("macro command set and command dispatch must stay in sync"),
    };

    tui::apply_command(app, command);
    Applied
}

pub(crate) struct AddTrackInstrumentCtx<'a> {
    pub(crate) app: &'a mut tui::App,
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
        track_groups,
        ui_epoch,
        lg_raw,
    } = ctx;

    add_new_track_to_group(app, idx, group_id);
    *track_groups.lock().unwrap() = app.groups.clone();

    current_track.store(idx, Ordering::Relaxed);
    let new_name = app.tracks[idx].clone();
    track_names.push(new_name.clone());

    {
        let mut pan_ids = track_pan_ids.lock().unwrap();
        pan_ids.push(app.graph.track_node_ids[idx].pan_id);
        push_solo_mutes(lg_raw, state, &pan_ids);
    }
    record_armed.lock().unwrap().push(false);

    let rt = editor.runtime_mut();
    rt.set_reactive("SEQ", "num-tracks", Value::Number(track_names.len() as f64));
    rt.set_reactive("SEQ", "track-ids", build_track_ids(app));
    set_current_track_reactive(rt, app.tracks.len(), idx);
    rt.set_reactive("SEQ", "track-names", build_track_names(track_names));
    sync_all_track_sequencer_state(rt, state, app, idx, selected_steps);
    rt.set_reactive("SEQ", "steps", build_steps_value(state, idx));
    sync_step_param_lists(rt, state, idx);
    sync_track_mixer_state(rt, app, state);
    sync_groups_bindings(rt, &app.groups);
    sync_track_peak_fields(rt, cached_track_peak_levels);
    rt.set_reactive(
        "SEQ",
        "effects",
        build_effects_value(state, idx, &app.graph.effect_descriptors, selected_steps),
    );
    rt.set_reactive(
        "SEQ",
        "midi-effects",
        build_midi_effects_value(state, idx, selected_steps),
    );
    rt.set_reactive(
        "SEQ",
        "instrument-panel",
        build_instrument_panel_value(app, idx, selected_steps),
    );
    *accumulator_names.lock().unwrap() = build_accumulator_names(app);
    sync_track_params(rt, app, state, idx, selected_steps);
    sync_fx_param_binding_fields(rt, app, state, idx, selected_steps);
    rt.set_reactive(
        "SEQ",
        "step-has-plocks",
        build_step_has_plocks(state, idx, &app.graph.effect_descriptors),
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
    pub(crate) app: &'a mut tui::App,
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
    summary: sequencer::sequencer::InstrumentSlotResetSummary,
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
    let selected_track = current_track
        .load(Ordering::Relaxed)
        .min(app.tracks.len().saturating_sub(1));
    if !app.tracks.is_empty() {
        let rt = editor.runtime_mut();
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

pub(crate) fn add_new_track_to_group(
    app: &mut tui::App,
    track: usize,
    group_id: Option<u64>,
) -> bool {
    let Some((group_index, bus_id)) = new_track_group_target(&app.groups, track, group_id) else {
        return false;
    };
    if track >= app.tracks.len() {
        return false;
    }

    app.set_track_output_all_scenes(
        track,
        sequencer::sequencer::TrackOutput::Bus(sequencer::sequencer::BusId(bus_id)),
    );
    let members = &mut app.groups[group_index].members;
    members.push(track);
    members.sort_unstable();
    members.dedup();
    true
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
        }
    }

    #[test]
    fn new_track_group_target_resolves_stable_group_id() {
        let groups = vec![group(12, 4, vec![0, 1]), group(27, 9, vec![2, 3])];
        assert_eq!(new_track_group_target(&groups, 4, Some(27)), Some((1, 9)));
        assert_eq!(new_track_group_target(&groups, 4, Some(99)), None);
        assert_eq!(new_track_group_target(&groups, 4, None), None);
        assert_eq!(new_track_group_target(&groups, 3, Some(27)), None);
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
