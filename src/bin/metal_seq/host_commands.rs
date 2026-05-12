use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use eseqlisp::vm::Value;
use eseqlisp::{Editor, HostEvent};
use sequencer::sequencer::SequencerState;
use sequencer::ui;

use super::state_values::{
    build_accumulator_names, build_effects_value, build_instrument_panel_value,
    build_midi_effects_value, build_step_has_plocks, build_steps_value, build_track_ids,
    build_track_names, push_solo_mutes, sync_all_track_sequencer_state,
    sync_fx_param_binding_fields, sync_step_param_lists, sync_track_mixer_state, sync_track_params,
    sync_track_peak_fields,
};

pub(crate) struct AddTrackInstrumentCtx<'a> {
    pub(crate) app: &'a mut ui::App,
    pub(crate) editor: &'a mut Editor,
    pub(crate) state: &'a Arc<SequencerState>,
    pub(crate) current_track: &'a Arc<AtomicUsize>,
    pub(crate) track_names: &'a mut Vec<String>,
    pub(crate) track_pan_ids: &'a Arc<Mutex<Vec<i32>>>,
    pub(crate) record_armed: &'a Arc<Mutex<Vec<bool>>>,
    pub(crate) selected_steps: &'a Arc<Mutex<HashSet<usize>>>,
    pub(crate) accumulator_names: &'a Arc<Mutex<Vec<String>>>,
    pub(crate) cached_track_peak_levels: &'a [f64],
    pub(crate) ui_epoch: &'a Arc<AtomicUsize>,
    pub(crate) lg_raw: *mut sequencer::audiograph::LiveGraph,
}

pub(crate) fn handle_add_track_instrument_command(payload: &Value, ctx: AddTrackInstrumentCtx<'_>) {
    let Some(name) = payload_name(payload) else {
        return;
    };

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
        ui_epoch,
        lg_raw,
    } = ctx;

    match app.add_saved_instrument_track_sync(&name) {
        Ok(idx) => {
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
            rt.set_reactive("SEQ", "current-track", Value::Number(idx as f64));
            rt.set_reactive("SEQ", "track-names", build_track_names(track_names));
            sync_all_track_sequencer_state(rt, state, app, idx, selected_steps);
            rt.set_reactive("SEQ", "steps", build_steps_value(state, idx));
            sync_step_param_lists(rt, state, idx);
            sync_track_mixer_state(rt, app, state);
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
            sync_fx_param_binding_fields(rt, app, state, idx);
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
        Err(e) => {
            editor.handle_host_event(HostEvent::Status(format!(
                "Error adding instrument track: {e}"
            )));
        }
    }
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
