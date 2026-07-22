use super::*;
use eseqlisp::runtime::ReactiveSetResult;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

fn sampler_modulation_depth_display_range(
    depth_desc: &sequencer::effects::ParamDescriptor,
    target: &sequencer::effects::InstrumentModulationTarget,
) -> (f32, f32) {
    (
        depth_desc.stored_to_user(target.depth_min),
        depth_desc.stored_to_user(target.depth_max),
    )
}

fn instrument_modulation_depth_display_range(
    target: &sequencer::effects::InstrumentModulationTarget,
) -> (f32, f32) {
    // Custom-instrument manifests define modulation depth ranges in display
    // units already; sampler ranges are stored in DSP units and scaled above.
    (target.depth_min, target.depth_max)
}

fn modulation_routing_param_indices(
    desc: &sequencer::effects::EffectDescriptor,
) -> std::collections::HashSet<usize> {
    let mut indices = std::collections::HashSet::new();
    for target in &desc.instrument_modulation_targets {
        indices.insert(target.depth_param_idx);
        if let Some(source_param_idx) = target.source_param_idx {
            indices.insert(source_param_idx);
        }
        if let Some(active_param_idx) = target.active_param_idx {
            indices.insert(active_param_idx);
        }
    }
    indices
}

/// Build a Lisp Value::List of bools from the step pattern for a given track.
pub(crate) fn build_steps_value(state: &Arc<SequencerState>, track: usize) -> Value {
    let items: Vec<Rc<RefCell<Value>>> = (0..MAX_STEPS)
        .map(|s| {
            Rc::new(RefCell::new(Value::Bool(
                state.pattern.patterns[track].is_active(s),
            )))
        })
        .collect();
    Value::List(items)
}

/// Build a list-of-lists of bools: one step list per track for the *sequencer* buffer.
pub(crate) fn build_all_track_steps_value(state: &Arc<SequencerState>, app: &app::App) -> Value {
    let tracks: Vec<Rc<RefCell<Value>>> = (0..app.tracks.len())
        .map(|t| {
            let steps: Vec<Rc<RefCell<Value>>> = (0..MAX_STEPS)
                .map(|s| {
                    Rc::new(RefCell::new(Value::Bool(
                        state.pattern.patterns[t].is_active(s),
                    )))
                })
                .collect();
            Rc::new(RefCell::new(Value::List(steps)))
        })
        .collect();
    Value::List(tracks)
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ReactiveSetStats {
    pub calls: usize,
    pub effects_dirty: usize,
    pub widgets_dirty: usize,
}

impl ReactiveSetStats {
    fn note(&mut self, result: ReactiveSetResult) {
        self.calls += 1;
        if result.effects_dirty {
            self.effects_dirty += 1;
        }
        if result.widgets_dirty {
            self.widgets_dirty += 1;
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct AllTrackStepBindingSyncProfile {
    pub elapsed: Duration,
    pub active_elapsed: Duration,
    pub duration_elapsed: Duration,
    pub plocked_elapsed: Duration,
    pub selected_elapsed: Duration,
    pub slider_elapsed: Duration,
    pub haptic_elapsed: Duration,
    pub active_sets: ReactiveSetStats,
    pub duration_sets: ReactiveSetStats,
    pub plocked_sets: ReactiveSetStats,
    pub selected_sets: ReactiveSetStats,
    pub slider_sets: ReactiveSetStats,
    pub haptic_sets: ReactiveSetStats,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct AllTrackSequencerSyncProfile {
    pub elapsed: Duration,
    pub track_steps: Duration,
    pub track_num_steps: Duration,
    pub track_timebases: Duration,
    pub track_duration_spans: Duration,
    pub track_step_has_plocks: Duration,
    pub track_playheads: Duration,
    pub track_velocities: Duration,
    pub track_durations: Duration,
    pub track_auxas: Duration,
    pub track_transposes: Duration,
    pub track_pans: Duration,
    pub track_syncs: Duration,
    pub track_delays: Duration,
    pub step_bindings: AllTrackStepBindingSyncProfile,
    pub playhead_fields: Duration,
}

pub(crate) fn build_track_pattern_cells_value(
    state: &Arc<SequencerState>,
    track_count: usize,
) -> Value {
    list_value((0..track_count).map(|track| {
        list_value(
            state
                .track_pattern_cells(track)
                .into_iter()
                .map(|cell| map_value([("id", Value::Number(cell.pattern_id.0 as f64))])),
        )
    }))
}

pub(crate) fn build_all_track_num_steps_value(
    state: &Arc<SequencerState>,
    app: &app::App,
) -> Value {
    let items: Vec<Rc<RefCell<Value>>> = (0..app.tracks.len())
        .map(|t| {
            Rc::new(RefCell::new(Value::Number(
                state.pattern.track_params[t].get_num_steps() as f64,
            )))
        })
        .collect();
    Value::List(items)
}

fn resolved_track_timebase_label(
    state: &Arc<SequencerState>,
    track: usize,
    current_track_idx: usize,
    selected_step: Option<usize>,
) -> String {
    let timebase = if track == current_track_idx {
        selected_step
            .and_then(|step| state.pattern.timebase_plocks[track].get(step))
            .unwrap_or_else(|| state.pattern.track_params[track].get_timebase())
    } else {
        state.pattern.track_params[track].get_timebase()
    };
    timebase.label().to_string()
}

fn build_track_timebase_labels_value(
    state: &Arc<SequencerState>,
    track_count: usize,
    current_track_idx: usize,
    selected_step: Option<usize>,
) -> Value {
    let items: Vec<Rc<RefCell<Value>>> = (0..track_count)
        .map(|track| {
            Rc::new(RefCell::new(Value::String(resolved_track_timebase_label(
                state,
                track,
                current_track_idx,
                selected_step,
            ))))
        })
        .collect();
    Value::List(items)
}

pub(crate) fn build_all_track_timebase_labels_value(
    state: &Arc<SequencerState>,
    app: &app::App,
    current_track_idx: usize,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
) -> Value {
    let selected_step = {
        let selected = selected_steps.lock().unwrap();
        selected.iter().copied().min()
    };
    build_track_timebase_labels_value(state, app.tracks.len(), current_track_idx, selected_step)
}

pub(crate) fn build_track_duration_spans_value(state: &Arc<SequencerState>, track: usize) -> Value {
    let num_steps = state.pattern.track_params[track]
        .get_num_steps()
        .min(MAX_STEPS);
    let spans: Vec<Rc<RefCell<Value>>> = (0..MAX_STEPS)
        .map(|target_step| {
            let covered = target_step < num_steps
                && (0..=target_step).any(|source_step| {
                    if !state.pattern.patterns[track].is_active(source_step) {
                        return false;
                    }
                    let duration = state.pattern.step_data[track]
                        .get(source_step, StepParam::Duration)
                        .max(0.0);
                    duration > (target_step - source_step) as f32
                });
            Rc::new(RefCell::new(Value::Bool(covered)))
        })
        .collect();
    Value::List(spans)
}

pub(crate) fn track_step_duration_covered(
    state: &Arc<SequencerState>,
    track: usize,
    target_step: usize,
) -> bool {
    let num_steps = state.pattern.track_params[track]
        .get_num_steps()
        .min(MAX_STEPS);
    target_step < num_steps
        && (0..=target_step).any(|source_step| {
            if !state.pattern.patterns[track].is_active(source_step) {
                return false;
            }
            let duration = state.pattern.step_data[track]
                .get(source_step, StepParam::Duration)
                .max(0.0);
            duration > (target_step - source_step) as f32
        })
}

pub(crate) fn track_step_active_field(track: usize, step: usize) -> String {
    format!("seq-track-step-active-{track}-{step}")
}

pub(crate) fn drum_lane_step_active_field(track: usize, pad_note: i32, step: usize) -> String {
    format!("drum-lane-step-active-{track}-{pad_note}-{step}")
}

pub(crate) fn drum_lane_step_selected_field(track: usize, pad_note: i32, step: usize) -> String {
    format!("drum-lane-step-selected-{track}-{pad_note}-{step}")
}

pub(crate) fn drum_lane_step_duration_field(track: usize, pad_note: i32, step: usize) -> String {
    format!("drum-lane-step-duration-{track}-{pad_note}-{step}")
}

/// Registry field caching a hex digest of all four per-step binding lanes for
/// a track. When it is unchanged, the per-step field writes are skipped
/// entirely; single-step sync paths invalidate it by writing Nil.
pub(crate) fn track_step_binding_rev_field(track: usize) -> String {
    format!("seq-track-step-binding-rev-{track}")
}

pub(crate) fn track_step_duration_field(track: usize, step: usize) -> String {
    format!("seq-track-step-duration-{track}-{step}")
}

pub(crate) fn track_step_plocked_field(track: usize, step: usize) -> String {
    format!("seq-track-step-plocked-{track}-{step}")
}

pub(crate) fn track_step_plock_kind_field(track: usize, step: usize) -> String {
    format!("seq-track-step-plock-kind-{track}-{step}")
}

pub(crate) fn track_step_variant_color_field(
    track: usize,
    step: usize,
    channel: char,
) -> String {
    format!("seq-track-step-variant-{channel}-{track}-{step}")
}

pub(crate) fn track_step_selected_field(track: usize, step: usize) -> String {
    format!("seq-track-step-selected-{track}-{step}")
}

pub(crate) fn track_selected_field(track: usize) -> String {
    format!("track-selected-{track}")
}

pub(crate) fn mixer_track_delete_target_field(track: usize) -> String {
    format!("mixer-track-delete-target-{track}")
}

pub(crate) fn rack_slot_delete_target_field(track: usize, slot: usize) -> String {
    format!("rack-slot-delete-target-{track}-{slot}")
}

pub(crate) fn track_pattern_cell_active_field(track: usize, pattern_id: u64) -> String {
    format!("track-pattern-cell-active-{track}-{pattern_id}")
}

pub(crate) fn track_pattern_cell_assigned_field(track: usize, pattern_id: u64) -> String {
    format!("track-pattern-cell-assigned-{track}-{pattern_id}")
}

pub(crate) fn track_pattern_cell_override_field(track: usize, pattern_id: u64) -> String {
    format!("track-pattern-cell-override-{track}-{pattern_id}")
}

pub(crate) fn track_pattern_cell_selected_field(track: usize, pattern_id: u64) -> String {
    format!("track-pattern-cell-selected-{track}-{pattern_id}")
}

fn mod_destination_kind_value(destination: sequencer::sequencer::ModDestination) -> Value {
    match destination {
        sequencer::sequencer::ModDestination::Track(_) => Value::String("track".to_string()),
        sequencer::sequencer::ModDestination::Bus(_) => Value::String("bus".to_string()),
    }
}

fn mod_destination_id_value(destination: sequencer::sequencer::ModDestination) -> Value {
    match destination {
        sequencer::sequencer::ModDestination::Track(track) => Value::Number(track as f64),
        sequencer::sequencer::ModDestination::Bus(bus) => Value::Number(bus.0 as f64),
    }
}

pub(crate) fn selected_mod_routes_value(
    active_delete_target: Option<&ActiveDeleteTarget>,
) -> Value {
    match active_delete_target {
        Some(ActiveDeleteTarget::ModRoute {
            source,
            destination,
            input,
        }) => list_value([map_value([
            ("source", Value::Number(*source as f64)),
            ("dest-kind", mod_destination_kind_value(*destination)),
            ("dest", mod_destination_id_value(*destination)),
            ("input", Value::Number(*input as f64)),
        ])]),
        _ => list_value(Vec::<Value>::new()),
    }
}

pub(crate) fn sync_track_pattern_cell_state_fields(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    track_count: usize,
) {
    for track in 0..track_count {
        for cell in state.track_pattern_cells(track) {
            let pattern_id = cell.pattern_id.0;
            rt.set_reactive(
                "SEQ",
                &track_pattern_cell_active_field(track, pattern_id),
                Value::Bool(cell.active_effective),
            );
            rt.set_reactive(
                "SEQ",
                &track_pattern_cell_assigned_field(track, pattern_id),
                Value::Bool(cell.assigned_to_current_scene),
            );
            rt.set_reactive(
                "SEQ",
                &track_pattern_cell_override_field(track, pattern_id),
                Value::Bool(cell.overridden),
            );
        }
    }
}

pub(crate) fn sync_track_pattern_cell_selected_fields(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    track_count: usize,
    active_delete_target: Option<&ActiveDeleteTarget>,
) {
    for track in 0..track_count {
        for cell in state.track_pattern_cells(track) {
            let pattern_id = cell.pattern_id.0;
            rt.set_reactive(
                "SEQ",
                &track_pattern_cell_selected_field(track, pattern_id),
                Value::Bool(matches!(
                    active_delete_target,
                    Some(ActiveDeleteTarget::TrackPattern {
                        track: selected_track,
                        pattern_id: selected_pattern_id,
                    }) if *selected_track == track && selected_pattern_id.0 == pattern_id
                )),
            );
        }
    }
}

pub(crate) fn sync_mixer_delete_target_binding_fields(
    rt: &mut Runtime,
    track_count: usize,
    state: &Arc<SequencerState>,
    active_delete_target: Option<&ActiveDeleteTarget>,
) {
    let rack_tracks = state.pattern.rack_tracks.lock().unwrap();
    for track in 0..track_count {
        rt.set_reactive(
            "SEQ",
            &mixer_track_delete_target_field(track),
            Value::Bool(matches!(
                active_delete_target,
                Some(ActiveDeleteTarget::MixerTrack { track: selected }) if *selected == track
            )),
        );
        let rack_slot_count = rack_tracks
            .get(track)
            .and_then(|rack| rack.as_ref())
            .map(|rack| rack.slots.len())
            .unwrap_or(0);
        for slot in 0..sequencer::sequencer::MAX_RACK_SLOTS {
            rt.set_reactive(
                "SEQ",
                &rack_slot_delete_target_field(track, slot),
                Value::Bool(matches!(
                    active_delete_target,
                    Some(ActiveDeleteTarget::RackSlot {
                        track: selected_track,
                        slot: selected_slot,
                    }) if *selected_track == track
                        && *selected_slot == slot
                        && slot < rack_slot_count
                )),
            );
        }
    }
    drop(rack_tracks);
    sync_track_pattern_cell_selected_fields(rt, state, track_count, active_delete_target);
    rt.set_reactive(
        "SEQ",
        "selected-mod-routes",
        selected_mod_routes_value(active_delete_target),
    );
}

pub(crate) fn sync_track_selection_binding_fields(
    rt: &mut Runtime,
    track_count: usize,
    current_track_idx: usize,
) {
    for track in 0..track_count {
        rt.set_reactive(
            "SEQ",
            &track_selected_field(track),
            Value::Bool(track == current_track_idx),
        );
    }
}

/// Builds the `SEQ.selected-tracks` reactive list (sorted track indices).
pub(crate) fn build_selected_tracks_value(selected: &HashSet<usize>) -> Value {
    let mut tracks: Vec<usize> = selected.iter().copied().collect();
    tracks.sort_unstable();
    list_value(tracks.into_iter().map(|t| Value::Number(t as f64)))
}

/// Lights `track-selected-{i}` for every track in the multi-select set (union
/// with the focused current track) and refreshes `SEQ.selected-tracks`.
pub(crate) fn sync_selected_tracks_bindings(
    rt: &mut Runtime,
    track_count: usize,
    current_track_idx: usize,
    selected: &HashSet<usize>,
) {
    for track in 0..track_count {
        let on = track == current_track_idx || selected.contains(&track);
        rt.set_reactive("SEQ", &track_selected_field(track), Value::Bool(on));
    }
    rt.set_reactive(
        "SEQ",
        "selected-tracks",
        build_selected_tracks_value(selected),
    );
}

/// Builds `SEQ.groups`: one map per group with id, name, color, bus-id, anchor
/// (lowest member index), collapsed flag, and ordered member indices.
pub(crate) fn build_groups_value(groups: &[sequencer::project::ProjectTrackGroup]) -> Value {
    list_value(groups.iter().map(|group| {
        let anchor = group.members.iter().copied().min().unwrap_or(0);
        map_value([
            ("id", Value::Number(group.id as f64)),
            ("name", Value::String(group.name.clone().into())),
            (
                "color",
                list_value(group.color.iter().map(|c| Value::Number(*c as f64))),
            ),
            ("bus-id", Value::Number(group.bus_id as f64)),
            ("anchor", Value::Number(anchor as f64)),
            ("collapsed", Value::Bool(group.collapsed)),
            (
                "members",
                list_value(group.members.iter().map(|m| Value::Number(*m as f64))),
            ),
        ])
    }))
}

/// Builds `SEQ.group-collapsed`: parallel list of collapsed flags per group.
pub(crate) fn build_group_collapsed_value(
    groups: &[sequencer::project::ProjectTrackGroup],
) -> Value {
    list_value(groups.iter().map(|g| Value::Bool(g.collapsed)))
}

pub(crate) fn sync_groups_bindings(
    rt: &mut Runtime,
    groups: &[sequencer::project::ProjectTrackGroup],
) {
    rt.set_reactive("SEQ", "groups", build_groups_value(groups));
    rt.set_reactive(
        "SEQ",
        "group-collapsed",
        build_group_collapsed_value(groups),
    );
}

pub(crate) fn set_current_track_reactive(
    rt: &mut Runtime,
    track_count: usize,
    current_track_idx: usize,
) {
    rt.set_reactive(
        "SEQ",
        "current-track",
        Value::Number(current_track_idx as f64),
    );
    sync_track_selection_binding_fields(rt, track_count, current_track_idx);
}

pub(crate) fn track_step_param_slider_field(track: usize, mode: usize, step: usize) -> String {
    format!("seq-track-step-param-slider-{track}-{mode}-{step}")
}

pub(crate) fn track_step_param_haptic_field(track: usize, mode: usize, step: usize) -> String {
    format!("seq-track-step-param-haptic-{track}-{mode}-{step}")
}

pub(crate) const PROCESS_LANE_MODE_OFFSET: usize = 7;

#[derive(Clone, Debug)]
struct ProcessLaneUiEntry {
    instance_id: sequencer::process::ProcessInstanceId,
    slot_index: usize,
    class_name: String,
    inlet_name: String,
    label: String,
    short_label: String,
    kind: String,
    min: f32,
    max: f32,
    default: f32,
    decimals: u8,
    target: String,
    map_ports: Vec<Value>,
    values: Vec<f32>,
    project: bool,
    forked: bool,
}

fn process_literal_as_f32(value: &sequencer::process::ProcessLiteral) -> Option<f32> {
    match value {
        sequencer::process::ProcessLiteral::Number(value) => Some(*value as f32),
        sequencer::process::ProcessLiteral::Bool(value) => Some(if *value { 1.0 } else { 0.0 }),
        _ => None,
    }
}

fn process_inlet_kind_name(kind: &sequencer::process::ProcessInletKind) -> &'static str {
    match kind {
        sequencer::process::ProcessInletKind::Float => "float",
        sequencer::process::ProcessInletKind::Int => "int",
        sequencer::process::ProcessInletKind::Gate => "gate",
        sequencer::process::ProcessInletKind::Track => "track",
        sequencer::process::ProcessInletKind::Field => "field",
        sequencer::process::ProcessInletKind::Any => "any",
    }
}

fn process_target_hint_label(target: Option<&sequencer::process::ProcessTargetHint>) -> String {
    match target {
        Some(sequencer::process::ProcessTargetHint::StepParam { param }) => {
            format!("step-param:{param}")
        }
        Some(sequencer::process::ProcessTargetHint::ParamTag { tag }) => {
            format!("param-tag:{tag}")
        }
        Some(sequencer::process::ProcessTargetHint::InstrumentParam { param }) => {
            format!("instrument-param:{param}")
        }
        Some(sequencer::process::ProcessTargetHint::EffectParam { effect, param }) => {
            format!("effect-param:{effect}:{param}")
        }
        Some(sequencer::process::ProcessTargetHint::MidiFxParam { fx, param }) => {
            format!("midi-fx-param:{fx}:{param}")
        }
        Some(sequencer::process::ProcessTargetHint::RackMacroParam { macro_id }) => {
            format!("rack-macro:macro_{}", macro_id + 1)
        }
        None => String::new(),
    }
}

fn process_param_target_label(target: &sequencer::process::ParamTarget) -> String {
    match target {
        sequencer::process::ParamTarget::StepParam { param } => {
            format!("step-param:{param}")
        }
        sequencer::process::ParamTarget::InstrumentParam { param, .. } => {
            format!("instrument:{param}")
        }
        sequencer::process::ParamTarget::EffectParam {
            slot,
            effect,
            param,
            ..
        } => format!("fx{}:{effect}:{param}", slot + 1),
        sequencer::process::ParamTarget::MidiFxParam { slot, fx, param } => {
            format!("midi-fx{}:{fx}:{param}", slot + 1)
        }
        sequencer::process::ParamTarget::ProcessInlet {
            process,
            inlet,
            instance_id,
        } => instance_id
            .map(|id| format!("process:{process}#{}:{inlet}", id.0))
            .unwrap_or_else(|| format!("process:{process}:{inlet}")),
        sequencer::process::ParamTarget::RackSlotParam { slot, param } => {
            format!("rack{}:{param}", slot + 1)
        }
        sequencer::process::ParamTarget::RackSlotInstrumentParam { slot, param, .. } => {
            format!("rack{}:instrument:{param}", slot + 1)
        }
        sequencer::process::ParamTarget::RackMacroParam { macro_id } => {
            format!("rack-macro:macro_{}", macro_id + 1)
        }
    }
}

fn macro_mapping_current_value(
    app: &app::App,
    mapping: &sequencer::macro_engine::MacroMapping,
) -> Option<f32> {
    match (mapping.scope, &mapping.target) {
        (
            sequencer::macro_engine::ParamScope::Track(track),
            sequencer::process::ParamTarget::EffectParam {
                slot,
                effect,
                param,
                ..
            },
        ) => {
            let param_idx = app
                .graph
                .effect_descriptors
                .get(track)?
                .get(*slot)
                .filter(|descriptor| descriptor.name.eq_ignore_ascii_case(effect))?
                .params
                .iter()
                .position(|descriptor| descriptor.has_tag_or_name(param))?;
            app.effective_slot_param_value(track, *slot, param_idx)
        }
        (
            sequencer::macro_engine::ParamScope::Track(track),
            sequencer::process::ParamTarget::InstrumentParam { param, .. },
        ) => {
            let param_idx = app
                .graph
                .instrument_descriptors
                .get(track)?
                .params
                .iter()
                .position(|descriptor| descriptor.has_tag_or_name(param))?;
            app.effective_instrument_param_value(track, param_idx)
        }
        (
            sequencer::macro_engine::ParamScope::Bus(bus_id),
            sequencer::process::ParamTarget::EffectParam {
                slot,
                effect,
                param,
                ..
            },
        ) => {
            let bus_idx = app.buses.iter().position(|bus| bus.id == bus_id)?;
            let param_idx = app.buses[bus_idx]
                .effect_descriptors
                .get(*slot)
                .filter(|descriptor| descriptor.name.eq_ignore_ascii_case(effect))?
                .params
                .iter()
                .position(|descriptor| descriptor.has_tag_or_name(param))?;
            app.effective_bus_slot_param_value(bus_idx, *slot, param_idx)
        }
        _ => None,
    }
}

fn macro_mapping_param_descriptor<'a>(
    app: &'a app::App,
    mapping: &sequencer::macro_engine::MacroMapping,
) -> Option<(
    &'a sequencer::effects::EffectDescriptor,
    &'a sequencer::effects::ParamDescriptor,
)> {
    match (mapping.scope, &mapping.target) {
        (
            sequencer::macro_engine::ParamScope::Track(track),
            sequencer::process::ParamTarget::EffectParam {
                slot,
                effect,
                param,
                ..
            },
        ) => {
            let device = app
                .graph
                .effect_descriptors
                .get(track)?
                .get(*slot)
                .filter(|descriptor| descriptor.name.eq_ignore_ascii_case(effect))?;
            let param = device
                .params
                .iter()
                .find(|descriptor| descriptor.has_tag_or_name(param))?;
            Some((device, param))
        }
        (
            sequencer::macro_engine::ParamScope::Track(track),
            sequencer::process::ParamTarget::InstrumentParam { param, .. },
        ) => {
            let device = app.graph.instrument_descriptors.get(track)?;
            let param = device
                .params
                .iter()
                .find(|descriptor| descriptor.has_tag_or_name(param))?;
            Some((device, param))
        }
        (
            sequencer::macro_engine::ParamScope::Bus(bus_id),
            sequencer::process::ParamTarget::EffectParam {
                slot,
                effect,
                param,
                ..
            },
        ) => {
            let bus = app.buses.iter().find(|bus| bus.id == bus_id)?;
            let device = bus
                .effect_descriptors
                .get(*slot)
                .filter(|descriptor| descriptor.name.eq_ignore_ascii_case(effect))?;
            let param = device
                .params
                .iter()
                .find(|descriptor| descriptor.has_tag_or_name(param))?;
            Some((device, param))
        }
        _ => None,
    }
}

fn macro_mapping_display_metadata(
    app: &app::App,
    mapping: &sequencer::macro_engine::MacroMapping,
) -> (String, String, f32, f32, f32, f32, f32, u8, String) {
    let Some((device, param)) = macro_mapping_param_descriptor(app, mapping) else {
        let scope_label = match mapping.scope {
            sequencer::macro_engine::ParamScope::Track(track) => format!("Track {}", track + 1),
            sequencer::macro_engine::ParamScope::Bus(bus_id) => app
                .buses
                .iter()
                .find(|bus| bus.id == bus_id)
                .map(|bus| bus.name.clone())
                .unwrap_or_else(|| format!("Bus {}", bus_id.0)),
        };
        return (
            scope_label,
            process_param_target_label(&mapping.target),
            mapping.range_min,
            mapping.range_max,
            mapping.range_min,
            mapping.range_max,
            1.0,
            2,
            String::new(),
        );
    };
    let scale = if param.is_percent() { 100.0 } else { 1.0 };
    let (decimals, unit) = match &param.kind {
        sequencer::effects::ParamKind::Boolean | sequencer::effects::ParamKind::Enum { .. } => {
            (0, String::new())
        }
        sequencer::effects::ParamKind::Continuous { unit } => {
            let decimals = if unit.as_deref() == Some("%") { 1 } else { 2 };
            (decimals, unit.clone().unwrap_or_default())
        }
    };
    let scope_label = match mapping.scope {
        sequencer::macro_engine::ParamScope::Track(track) => format!("T{}", track + 1),
        sequencer::macro_engine::ParamScope::Bus(bus_id) => app
            .buses
            .iter()
            .find(|bus| bus.id == bus_id)
            .map(|bus| bus.name.clone())
            .unwrap_or_else(|| format!("Bus {}", bus_id.0)),
    };
    (
        format!("{scope_label} · {}", device.name),
        param.name.clone(),
        param.stored_to_user(mapping.range_min),
        param.stored_to_user(mapping.range_max),
        param.stored_to_user(param.min),
        param.stored_to_user(param.max),
        scale,
        decimals,
        unit,
    )
}

pub(crate) fn build_macros_value(app: &app::App) -> Value {
    list_value(app.macro_engine.macros().iter().map(|macro_definition| {
        let kind = match macro_definition.kind {
            sequencer::macro_engine::MacroKind::Mapped => "mapped",
            sequencer::macro_engine::MacroKind::Scene(_) => "scene",
        };
        let mappings = list_value(macro_definition.mappings.iter().enumerate().map(
            |(mapping_idx, mapping)| {
                let (
                    path_label,
                    param_label,
                    display_min,
                    display_max,
                    domain_min,
                    domain_max,
                    display_scale,
                    display_decimals,
                    display_unit,
                ) = macro_mapping_display_metadata(app, mapping);
                let curve = match mapping.curve {
                    sequencer::macro_engine::MacroCurve::Linear => "linear",
                    sequencer::macro_engine::MacroCurve::Exp => "exp",
                    sequencer::macro_engine::MacroCurve::Log => "log",
                    sequencer::macro_engine::MacroCurve::LogDomain => "log-domain",
                };
                map_value([
                    ("mapping-idx", Value::Number(mapping_idx as f64)),
                    (
                        "track",
                        Value::Number(match mapping.scope {
                            sequencer::macro_engine::ParamScope::Track(track) => track as f64,
                            sequencer::macro_engine::ParamScope::Bus(_) => -1.0,
                        }),
                    ),
                    (
                        "scope",
                        Value::String(match mapping.scope {
                            sequencer::macro_engine::ParamScope::Track(_) => "track".to_string(),
                            sequencer::macro_engine::ParamScope::Bus(_) => "bus".to_string(),
                        }),
                    ),
                    ("target", macro_mapping_target_value(&mapping.target)),
                    (
                        "target-label",
                        Value::String(format!(
                            "{} · {}",
                            path_label,
                            process_param_target_label(&mapping.target)
                        )),
                    ),
                    ("min", Value::Number(mapping.range_min as f64)),
                    ("max", Value::Number(mapping.range_max as f64)),
                    ("path-label", Value::String(path_label)),
                    ("param-label", Value::String(param_label)),
                    ("display-min", Value::Number(display_min as f64)),
                    ("display-max", Value::Number(display_max as f64)),
                    ("domain-min", Value::Number(domain_min as f64)),
                    ("domain-max", Value::Number(domain_max as f64)),
                    ("display-scale", Value::Number(display_scale as f64)),
                    ("display-decimals", Value::Number(display_decimals as f64)),
                    ("display-unit", Value::String(display_unit)),
                    ("curve", Value::String(curve.to_string())),
                    (
                        "current",
                        macro_mapping_current_value(app, mapping)
                            .map(|value| Value::Number(value as f64))
                            .unwrap_or(Value::Nil),
                    ),
                    (
                        "display-current",
                        macro_mapping_current_value(app, mapping)
                            .and_then(|value| {
                                macro_mapping_param_descriptor(app, mapping)
                                    .map(|(_, param)| param.stored_to_user(value))
                            })
                            .map(|value| Value::Number(value as f64))
                            .unwrap_or(Value::Nil),
                    ),
                    ("suspended", Value::Bool(mapping.suspended)),
                ])
            },
        ));
        let (target_scene, morph_params, steal_patterns, quantize, track_mask, diff_count) =
            match &macro_definition.kind {
                sequencer::macro_engine::MacroKind::Mapped => (
                    Value::Nil,
                    Value::Nil,
                    Value::Nil,
                    Value::Nil,
                    Value::Nil,
                    Value::Nil,
                ),
                sequencer::macro_engine::MacroKind::Scene(config) => (
                    Value::Number(config.target_scene as f64),
                    Value::Bool(config.morph_params),
                    Value::Bool(config.steal_patterns),
                    Value::String(
                        match config.quantize {
                            sequencer::macro_engine::StealQuantize::Off => "off",
                            sequencer::macro_engine::StealQuantize::Sixteenth => "sixteenth",
                            sequencer::macro_engine::StealQuantize::Bar => "bar",
                        }
                        .to_string(),
                    ),
                    config
                        .track_mask
                        .as_ref()
                        .map(|mask| list_value(mask.iter().copied().map(Value::Bool)))
                        .unwrap_or(Value::Nil),
                    Value::Number(app.scene_macro_diff_count(config) as f64),
                ),
            };
        map_value([
            ("id", Value::Number(macro_definition.id as f64)),
            (
                "key",
                macro_definition
                    .key
                    .as_ref()
                    .map(|key| Value::String(key.clone()))
                    .unwrap_or(Value::Nil),
            ),
            ("name", Value::String(macro_definition.name.clone())),
            ("kind", Value::String(kind.to_string())),
            ("value", Value::Number(macro_definition.value as f64)),
            ("mappings", mappings),
            ("target-scene", target_scene),
            ("morph-params", morph_params),
            ("steal-patterns", steal_patterns),
            ("quantize", quantize),
            ("track-mask", track_mask),
            ("diff-count", diff_count),
        ])
    }))
}

pub(crate) fn sync_macro_state(rt: &mut Runtime, app: &app::App) {
    rt.set_reactive("SEQ", "macros", build_macros_value(app));
}

fn macro_mapping_target_value(target: &sequencer::process::ParamTarget) -> Value {
    use sequencer::process::ParamTarget;

    let mut entries = Vec::new();
    match target {
        ParamTarget::StepParam { param } => {
            entries.push(("kind", Value::String("step".to_string())));
            entries.push(("param", Value::String(param.clone())));
        }
        ParamTarget::InstrumentParam { param, .. } => {
            entries.push(("kind", Value::String("instrument".to_string())));
            entries.push(("param", Value::String(param.clone())));
        }
        ParamTarget::EffectParam {
            slot,
            effect,
            param,
            ..
        } => {
            entries.push(("kind", Value::String("effect".to_string())));
            entries.push(("slot-idx", Value::Number(*slot as f64)));
            entries.push(("effect", Value::String(effect.clone())));
            entries.push(("param", Value::String(param.clone())));
        }
        ParamTarget::MidiFxParam { slot, fx, param } => {
            entries.push(("kind", Value::String("midi-fx".to_string())));
            entries.push(("slot-idx", Value::Number(*slot as f64)));
            entries.push(("fx", Value::String(fx.clone())));
            entries.push(("param", Value::String(param.clone())));
        }
        ParamTarget::ProcessInlet { process, inlet, .. } => {
            entries.push(("kind", Value::String("process-inlet".to_string())));
            entries.push(("process", Value::String(process.clone())));
            entries.push(("inlet", Value::String(inlet.clone())));
        }
        ParamTarget::RackSlotParam { slot, param } => {
            entries.push(("kind", Value::String("rack-slot".to_string())));
            entries.push(("slot-idx", Value::Number(*slot as f64)));
            entries.push(("param", Value::String(param.clone())));
        }
        ParamTarget::RackSlotInstrumentParam { slot, param, .. } => {
            entries.push(("kind", Value::String("rack-slot-instrument".to_string())));
            entries.push(("slot-idx", Value::Number(*slot as f64)));
            entries.push(("param", Value::String(param.clone())));
        }
        ParamTarget::RackMacroParam { macro_id } => {
            entries.push(("kind", Value::String("rack-macro".to_string())));
            entries.push(("macro-id", Value::Number(*macro_id as f64)));
        }
    }
    map_value(entries)
}

fn process_param_target_is_bindable(target: &sequencer::process::ParamTarget) -> bool {
    !matches!(
        target,
        sequencer::process::ParamTarget::RackSlotParam { .. }
            | sequencer::process::ParamTarget::RackSlotInstrumentParam { .. }
    )
}

fn process_target_kind_label(kind: Option<sequencer::process::ProcessTargetKind>) -> String {
    kind.map(|kind| kind.as_str().to_string())
        .unwrap_or_default()
}

fn process_ports_label(ports: &[sequencer::process::ProcessPortDef]) -> String {
    match ports {
        [] => String::new(),
        [port] if port.name == sequencer::process::DEFAULT_PROCESS_PORT => {
            process_target_hint_label(port.target.as_ref())
        }
        _ => ports
            .iter()
            .map(|port| {
                let target = process_target_hint_label(port.target.as_ref());
                if target.is_empty() {
                    format!("{}:unbound", port.name)
                } else {
                    format!("{}:{target}", port.name)
                }
            })
            .collect::<Vec<_>>()
            .join(", "),
    }
}

fn process_slot_ports_value(
    slot: &sequencer::process::TrackProcessSlot,
    def: Option<&sequencer::process::PublishedProcessDef>,
) -> Value {
    let mut ports = def.map(|def| def.ports.clone()).unwrap_or_default();
    for name in slot.bindings.keys() {
        if !ports.iter().any(|port| &port.name == name) {
            ports.push(sequencer::process::ProcessPortDef {
                name: name.clone(),
                target: None,
                binding_mode: sequencer::process::ProcessPortBindingMode::Fixed,
                target_kind: None,
            });
        }
    }
    list_value(
        ports
            .into_iter()
            .map(|port| process_port_value(slot, &port)),
    )
}

fn process_mappable_port_values(
    slot: &sequencer::process::TrackProcessSlot,
    def: Option<&sequencer::process::PublishedProcessDef>,
) -> Vec<Value> {
    def.map(|def| {
        def.ports
            .iter()
            .filter(|port| port.is_mappable())
            .map(|port| process_port_value(slot, port))
            .collect()
    })
    .unwrap_or_default()
}

fn process_port_value(
    slot: &sequencer::process::TrackProcessSlot,
    port: &sequencer::process::ProcessPortDef,
) -> Value {
    let binding = slot.bindings.get(&port.name);
    let manual = matches!(binding, Some(Some(_)));
    let hint_label = process_target_hint_label(port.target.as_ref());
    let target_label = match binding {
        Some(Some(target)) => process_param_target_label(target),
        _ if !hint_label.is_empty() => hint_label.clone(),
        _ => "unbound".to_string(),
    };
    let status = match binding {
        Some(Some(_)) => "bound",
        Some(None) | None if port.target.is_some() => "hint",
        Some(None) | None => "unbound",
    };
    let bindable = port.is_mappable()
        && binding
            .and_then(|binding| binding.as_ref())
            .map(process_param_target_is_bindable)
            .unwrap_or(true);
    map_value([
        ("name", Value::String(port.name.clone())),
        (
            "label",
            Value::String(if port.name == sequencer::process::DEFAULT_PROCESS_PORT {
                "default".to_string()
            } else {
                port.name.clone()
            }),
        ),
        ("hint", Value::String(hint_label)),
        ("target", Value::String(target_label)),
        ("status", Value::String(status.to_string())),
        ("manual", Value::Bool(manual)),
        ("clearable", Value::Bool(manual)),
        ("mappable", Value::Bool(port.is_mappable())),
        ("connectable", Value::Bool(port.is_connectable())),
        ("bindable", Value::Bool(bindable)),
        (
            "target-kind",
            Value::String(process_target_kind_label(port.effective_target_kind())),
        ),
    ])
}

fn process_name_initials(name: &str) -> String {
    let initials = name
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|part| !part.is_empty())
        .filter_map(|part| part.chars().next())
        .take(4)
        .collect::<String>();
    if initials.len() >= 2 {
        initials
    } else {
        name.chars().take(4).collect::<String>()
    }
}

fn process_short_label(class_name: &str, inlet_name: &str) -> String {
    let class = process_name_initials(class_name);
    match (class.is_empty(), inlet_name.is_empty()) {
        (_, true) => "lane".to_string(),
        (true, false) => inlet_name.to_string(),
        (false, false) => format!("{class}/{inlet_name}"),
    }
}

fn process_inlet_decimals(kind: Option<&sequencer::process::ProcessInletKind>) -> u8 {
    match kind {
        Some(sequencer::process::ProcessInletKind::Int)
        | Some(sequencer::process::ProcessInletKind::Gate)
        | Some(sequencer::process::ProcessInletKind::Track) => 0,
        _ => 2,
    }
}

fn process_inlet_range(
    def: Option<&sequencer::process::PublishedProcessDef>,
    inlet: Option<&sequencer::process::PublishedProcessInletDef>,
    default: f32,
) -> (f32, f32) {
    if let (Some(min), Some(max)) = (
        inlet.and_then(|entry| entry.min),
        inlet.and_then(|entry| entry.max),
    ) {
        return (min, max);
    }
    match inlet.map(|entry| &entry.kind) {
        Some(sequencer::process::ProcessInletKind::Gate) => (0.0, 1.0),
        Some(sequencer::process::ProcessInletKind::Track) => (0.0, 127.0),
        Some(sequencer::process::ProcessInletKind::Int) => {
            let center = default.round();
            (center - 24.0, center + 24.0)
        }
        _ => def
            .and_then(|def| def.accumulator.as_ref())
            .and_then(|acc| acc.range)
            .unwrap_or((default - 1.0, default + 1.0)),
    }
}

fn process_lane_entries_for_track(
    state: &Arc<SequencerState>,
    track: usize,
) -> Vec<ProcessLaneUiEntry> {
    let Some(chain) = state.composed_track_process_chain(track) else {
        return Vec::new();
    };
    let published = state.published_process_authoring();
    let mut entries = Vec::new();
    for (slot_index, slot) in chain.slots.iter().enumerate() {
        let def = published
            .defs
            .iter()
            .find(|def| def.name == slot.class_name);
        let mut lane_names = def
            .map(|def| {
                def.inlets
                    .iter()
                    .filter(|inlet| inlet.lane)
                    .map(|inlet| inlet.name.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for name in slot.lanes.keys() {
            if !lane_names.iter().any(|entry| entry == name) {
                lane_names.push(name.clone());
            }
        }
        for inlet_name in lane_names {
            let inlet =
                def.and_then(|def| def.inlets.iter().find(|entry| entry.name == inlet_name));
            let default = slot
                .inlets
                .get(&inlet_name)
                .and_then(process_literal_as_f32)
                .or_else(|| inlet.and_then(|entry| process_literal_as_f32(&entry.default)))
                .unwrap_or(0.0);
            let (min, max) = process_inlet_range(def, inlet, default);
            let lane = slot.lanes.get(&inlet_name);
            let values = (0..MAX_STEPS)
                .map(|step| {
                    lane.map(|lane| lane.value_at(step, default))
                        .unwrap_or(default)
                })
                .collect::<Vec<_>>();
            let map_ports = process_mappable_port_values(slot, def);
            entries.push(ProcessLaneUiEntry {
                instance_id: slot.instance_id,
                slot_index,
                class_name: slot.class_name.clone(),
                inlet_name: inlet_name.clone(),
                label: format!("{} / {}", slot.class_name, inlet_name),
                short_label: process_short_label(&slot.class_name, &inlet_name),
                kind: inlet
                    .map(|entry| process_inlet_kind_name(&entry.kind).to_string())
                    .unwrap_or_else(|| "float".to_string()),
                min,
                max,
                default,
                decimals: process_inlet_decimals(inlet.map(|entry| &entry.kind)),
                target: def
                    .map(|def| process_ports_label(&def.ports))
                    .unwrap_or_default(),
                map_ports,
                values,
                project: slot.project_layer,
                forked: slot.project_layer
                    && state.has_project_process_lane_override(
                        track,
                        slot.instance_id,
                        &inlet_name,
                    ),
            });
        }
    }
    entries
}

fn process_lane_entry_value(entry: &ProcessLaneUiEntry, mode: usize) -> Value {
    map_value([
        ("mode", Value::Number(mode as f64)),
        (
            "lane-index",
            Value::Number((mode - PROCESS_LANE_MODE_OFFSET) as f64),
        ),
        ("slot-index", Value::Number(entry.slot_index as f64)),
        ("instance-id", Value::Number(entry.instance_id.0 as f64)),
        ("class", Value::String(entry.class_name.clone())),
        ("process", Value::String(entry.class_name.clone())),
        ("inlet", Value::String(entry.inlet_name.clone())),
        ("name", Value::String(entry.inlet_name.clone())),
        ("project", Value::Bool(entry.project)),
        ("forked", Value::Bool(entry.forked)),
        ("label", Value::String(entry.label.clone())),
        ("short-label", Value::String(entry.short_label.clone())),
        ("kind", Value::String(entry.kind.clone())),
        ("min", Value::Number(entry.min as f64)),
        ("max", Value::Number(entry.max as f64)),
        ("default", Value::Number(entry.default as f64)),
        ("decimals", Value::Number(entry.decimals as f64)),
        ("target", Value::String(entry.target.clone())),
        ("map-ports", list_value(entry.map_ports.iter().cloned())),
        (
            "values",
            list_value(
                entry
                    .values
                    .iter()
                    .map(|value| Value::Number(*value as f64)),
            ),
        ),
    ])
}

pub(crate) fn build_process_lanes_value(state: &Arc<SequencerState>, track: usize) -> Value {
    list_value(
        process_lane_entries_for_track(state, track)
            .iter()
            .enumerate()
            .map(|(lane_index, entry)| {
                process_lane_entry_value(entry, PROCESS_LANE_MODE_OFFSET + lane_index)
            }),
    )
}

pub(crate) fn build_all_track_process_lanes_value(
    state: &Arc<SequencerState>,
    track_count: usize,
) -> Value {
    list_value((0..track_count).map(|track| build_process_lanes_value(state, track)))
}

fn process_lane_value_for_mode(
    state: &Arc<SequencerState>,
    track: usize,
    mode: usize,
    step: usize,
) -> Option<f32> {
    let lane_index = mode.checked_sub(PROCESS_LANE_MODE_OFFSET)?;
    process_lane_entries_for_track(state, track)
        .get(lane_index)
        .and_then(|entry| entry.values.get(step).copied())
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ProcessLaneEditInfo {
    pub(crate) instance_id: sequencer::process::ProcessInstanceId,
    pub(crate) inlet_name: String,
    pub(crate) value: f32,
    pub(crate) min: f32,
    pub(crate) max: f32,
    pub(crate) decimals: u8,
}

pub(crate) fn process_lane_edit_info_for_mode(
    state: &Arc<SequencerState>,
    track: usize,
    mode: usize,
    step: usize,
) -> Option<ProcessLaneEditInfo> {
    let lane_index = mode.checked_sub(PROCESS_LANE_MODE_OFFSET)?;
    process_lane_entries_for_track(state, track)
        .get(lane_index)
        .and_then(|entry| process_lane_edit_info_from_entry(entry, step))
}

pub(crate) fn process_lane_edit_info_for_target(
    state: &Arc<SequencerState>,
    track: usize,
    instance_id: sequencer::process::ProcessInstanceId,
    inlet_name: &str,
    step: usize,
) -> Option<ProcessLaneEditInfo> {
    process_lane_entries_for_track(state, track)
        .iter()
        .find(|entry| entry.instance_id == instance_id && entry.inlet_name == inlet_name)
        .and_then(|entry| process_lane_edit_info_from_entry(entry, step))
}

fn process_lane_edit_info_from_entry(
    entry: &ProcessLaneUiEntry,
    step: usize,
) -> Option<ProcessLaneEditInfo> {
    Some(ProcessLaneEditInfo {
        instance_id: entry.instance_id,
        inlet_name: entry.inlet_name.clone(),
        value: *entry.values.get(step)?,
        min: entry.min,
        max: entry.max,
        decimals: entry.decimals,
    })
}

fn process_scalar_inlet_value(
    slot: &sequencer::process::TrackProcessSlot,
    inlet: Option<&sequencer::process::PublishedProcessInletDef>,
    inlet_name: &str,
) -> Option<f32> {
    slot.inlets
        .get(inlet_name)
        .and_then(process_literal_as_f32)
        .or_else(|| inlet.and_then(|entry| process_literal_as_f32(&entry.default)))
}

fn process_scalar_inlet_entry_value(
    slot: &sequencer::process::TrackProcessSlot,
    def: Option<&sequencer::process::PublishedProcessDef>,
    inlet_name: &str,
    inlet: Option<&sequencer::process::PublishedProcessInletDef>,
) -> Option<Value> {
    let value = process_scalar_inlet_value(slot, inlet, inlet_name)?;
    let default = inlet
        .and_then(|entry| process_literal_as_f32(&entry.default))
        .unwrap_or(value);
    let (min, max) = process_inlet_range(def, inlet, value);
    Some(map_value([
        ("name", Value::String(inlet_name.to_string())),
        ("label", Value::String(inlet_name.to_string())),
        (
            "kind",
            Value::String(
                inlet
                    .map(|entry| process_inlet_kind_name(&entry.kind))
                    .unwrap_or("float")
                    .to_string(),
            ),
        ),
        ("value", Value::Number(value as f64)),
        ("default", Value::Number(default as f64)),
        ("min", Value::Number(min as f64)),
        ("max", Value::Number(max as f64)),
        (
            "decimals",
            Value::Number(process_inlet_decimals(inlet.map(|entry| &entry.kind)) as f64),
        ),
        (
            "doc",
            Value::String(
                inlet
                    .and_then(|entry| entry.doc.clone())
                    .unwrap_or_default(),
            ),
        ),
    ]))
}

pub(crate) fn build_process_slots_value(state: &Arc<SequencerState>, track: usize) -> Value {
    let Some(chain) = state.composed_track_process_chain(track) else {
        return list_value(Vec::<Value>::new());
    };
    let published = state.published_process_authoring();
    list_value(chain.slots.iter().enumerate().map(|(slot_index, slot)| {
        let def = published
            .defs
            .iter()
            .find(|def| def.name == slot.class_name);
        let mut scalar_names = def
            .map(|def| {
                def.inlets
                    .iter()
                    .filter(|inlet| !inlet.lane)
                    .map(|inlet| inlet.name.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for name in slot.inlets.keys() {
            if !scalar_names.iter().any(|entry| entry == name) && !slot.lanes.contains_key(name) {
                scalar_names.push(name.clone());
            }
        }
        let inlet_values = scalar_names.into_iter().filter_map(|name| {
            let inlet = def.and_then(|def| def.inlets.iter().find(|entry| entry.name == name));
            process_scalar_inlet_entry_value(slot, def, &name, inlet)
        });
        map_value([
            ("slot-index", Value::Number(slot_index as f64)),
            ("instance-id", Value::Number(slot.instance_id.0 as f64)),
            ("class", Value::String(slot.class_name.clone())),
            ("process", Value::String(slot.class_name.clone())),
            ("label", Value::String(slot.class_name.clone())),
            (
                "doc",
                Value::String(def.and_then(|def| def.doc.clone()).unwrap_or_default()),
            ),
            (
                "source-path",
                Value::String(
                    def.and_then(|def| def.source_path.clone())
                        .unwrap_or_default(),
                ),
            ),
            ("enabled", Value::Bool(slot.enabled)),
            ("project", Value::Bool(slot.project_layer)),
            (
                "target",
                Value::String(
                    def.map(|def| process_ports_label(&def.ports))
                        .unwrap_or_default(),
                ),
            ),
            ("ports", process_slot_ports_value(slot, def)),
            ("inlets", list_value(inlet_values)),
        ])
    }))
}

pub(crate) fn build_all_track_process_slots_value(
    state: &Arc<SequencerState>,
    track_count: usize,
) -> Value {
    list_value((0..track_count).map(|track| build_process_slots_value(state, track)))
}

pub(crate) fn build_process_library_value(state: &Arc<SequencerState>) -> Value {
    let published = state.published_process_authoring();
    list_value(published.defs.iter().map(|def| {
        map_value([
            ("name", Value::String(def.name.clone())),
            ("label", Value::String(def.name.clone())),
            ("doc", Value::String(def.doc.clone().unwrap_or_default())),
            (
                "source-path",
                Value::String(def.source_path.clone().unwrap_or_default()),
            ),
            ("target", Value::String(process_ports_label(&def.ports))),
            (
                "ports",
                list_value(def.ports.iter().map(|port| {
                    map_value([
                        ("name", Value::String(port.name.clone())),
                        (
                            "label",
                            Value::String(
                                if port.name == sequencer::process::DEFAULT_PROCESS_PORT {
                                    "default".to_string()
                                } else {
                                    port.name.clone()
                                },
                            ),
                        ),
                        (
                            "hint",
                            Value::String(process_target_hint_label(port.target.as_ref())),
                        ),
                        ("mappable", Value::Bool(port.is_mappable())),
                        ("connectable", Value::Bool(port.is_connectable())),
                        (
                            "target-kind",
                            Value::String(process_target_kind_label(port.effective_target_kind())),
                        ),
                    ])
                })),
            ),
            (
                "lane-count",
                Value::Number(def.inlets.iter().filter(|inlet| inlet.lane).count() as f64),
            ),
        ])
    }))
}

pub(crate) fn sync_process_chain_state(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    track_count: usize,
    current_track: usize,
) {
    rt.set_reactive(
        "SEQ",
        "track-process-lanes",
        build_all_track_process_lanes_value(state, track_count),
    );
    rt.set_reactive(
        "SEQ",
        "process-lanes",
        build_process_lanes_value(state, current_track),
    );
    rt.set_reactive(
        "SEQ",
        "process-slots",
        build_process_slots_value(state, current_track),
    );
    rt.set_reactive(
        "SEQ",
        "track-process-slots",
        build_all_track_process_slots_value(state, track_count),
    );
    rt.set_reactive("SEQ", "process-library", build_process_library_value(state));
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ExpandedStepViewport {
    pub(crate) track: usize,
    pub(crate) track_id: usize,
    pub(crate) page: usize,
    pub(crate) mode: usize,
    pub(crate) cursor_step: usize,
}

#[derive(Debug, Default)]
pub(crate) struct ExpandedStepProjectionRegistry {
    viewports: Mutex<HashMap<usize, ExpandedStepViewport>>,
}

impl ExpandedStepProjectionRegistry {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn set_viewport(&self, viewport: ExpandedStepViewport) -> bool {
        let mut viewports = self.viewports.lock().unwrap();
        let changed = viewports.get(&viewport.track_id).copied() != Some(viewport);
        viewports.insert(viewport.track_id, viewport);
        changed
    }

    pub(crate) fn remove_viewport(&self, track_id: usize) -> bool {
        self.viewports.lock().unwrap().remove(&track_id).is_some()
    }

    pub(crate) fn viewport(&self, track_id: usize) -> Option<ExpandedStepViewport> {
        self.viewports.lock().unwrap().get(&track_id).copied()
    }

    pub(crate) fn viewports_for_track(&self, track: usize) -> Vec<ExpandedStepViewport> {
        self.viewports
            .lock()
            .unwrap()
            .values()
            .copied()
            .filter(|viewport| viewport.track == track)
            .collect()
    }

    pub(crate) fn all_viewports(&self) -> Vec<ExpandedStepViewport> {
        self.viewports.lock().unwrap().values().copied().collect()
    }

    pub(crate) fn clear(&self) {
        self.viewports.lock().unwrap().clear();
    }
}

pub(crate) fn expanded_step_slot_index_field(track_id: usize, slot: usize) -> String {
    format!("seqv-slot-step-index-{track_id}-{slot}")
}

pub(crate) fn expanded_step_slot_label_field(track_id: usize, slot: usize) -> String {
    format!("seqv-slot-step-label-{track_id}-{slot}")
}

pub(crate) fn expanded_step_slot_visible_field(track_id: usize, slot: usize) -> String {
    format!("seqv-slot-visible-{track_id}-{slot}")
}

pub(crate) fn expanded_step_slot_active_field(track_id: usize, slot: usize) -> String {
    format!("seqv-slot-active-{track_id}-{slot}")
}

pub(crate) fn expanded_step_slot_plocked_field(track_id: usize, slot: usize) -> String {
    format!("seqv-slot-plocked-{track_id}-{slot}")
}

pub(crate) fn expanded_step_slot_selected_field(track_id: usize, slot: usize) -> String {
    format!("seqv-slot-selected-{track_id}-{slot}")
}

pub(crate) fn expanded_step_slot_playhead_field(track_id: usize, slot: usize) -> String {
    format!("seqv-slot-playhead-active-{track_id}-{slot}")
}

pub(crate) fn expanded_step_slot_cursor_field(track_id: usize, slot: usize) -> String {
    format!("seqv-slot-cursor-active-{track_id}-{slot}")
}

pub(crate) fn expanded_step_slot_param_slider_field(
    track_id: usize,
    mode: usize,
    slot: usize,
) -> String {
    format!("seqv-slot-param-slider-{track_id}-{mode}-{slot}")
}

pub(crate) fn expanded_step_slot_param_haptic_field(
    track_id: usize,
    mode: usize,
    slot: usize,
) -> String {
    format!("seqv-slot-param-haptic-{track_id}-{mode}-{slot}")
}

pub(crate) fn expanded_step_page_active_field(track_id: usize, page: usize) -> String {
    format!("seqv-page-active-{track_id}-{page}")
}

pub(crate) fn visible_slot_for_step(viewport: ExpandedStepViewport, step: usize) -> Option<usize> {
    let first_step = viewport.page.saturating_mul(PAGE_SIZE);
    let slot = step.checked_sub(first_step)?;
    (slot < PAGE_SIZE).then_some(slot)
}

fn expanded_step_param_for_mode(mode: usize) -> Option<StepParam> {
    match mode {
        0 => Some(StepParam::Velocity),
        1 => Some(StepParam::Duration),
        2 => Some(StepParam::AuxA),
        3 => Some(StepParam::Transpose),
        4 => Some(StepParam::Pan),
        5 => Some(StepParam::Sync),
        6 => Some(StepParam::Delay),
        _ => None,
    }
}

fn expanded_step_param_slider_value(param: StepParam, value: f32) -> f32 {
    if param == StepParam::Duration {
        param.normalize(value)
    } else {
        value
    }
}

pub(crate) fn sync_expanded_step_param_slot(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    viewport: ExpandedStepViewport,
    mode: usize,
    slot: usize,
) -> bool {
    let step = viewport.page.saturating_mul(PAGE_SIZE).saturating_add(slot);
    let num_steps = state.pattern.track_params[viewport.track]
        .get_num_steps()
        .min(MAX_STEPS);
    let visible = step < num_steps;
    let param = expanded_step_param_for_mode(mode);
    let value = if visible {
        param
            .map(|param| state.pattern.step_data[viewport.track].get(step, param))
            .or_else(|| process_lane_value_for_mode(state, viewport.track, mode, step))
            .unwrap_or(0.0)
    } else {
        0.0
    };
    let slider_value = param
        .map(|param| expanded_step_param_slider_value(param, value))
        .unwrap_or(value);
    let mut dirty = false;
    dirty |= rt
        .set_reactive(
            "SEQ",
            &expanded_step_slot_param_slider_field(viewport.track_id, mode, slot),
            Value::Number(slider_value as f64),
        )
        .effects_dirty;
    dirty |= rt
        .set_reactive(
            "SEQ",
            &expanded_step_slot_param_haptic_field(viewport.track_id, mode, slot),
            Value::Number(value as f64),
        )
        .effects_dirty;
    dirty
}

pub(crate) fn sync_expanded_step_slot(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    app: &app::App,
    selected_steps: &HashSet<usize>,
    current_track_idx: usize,
    viewport: ExpandedStepViewport,
    slot: usize,
) -> bool {
    let step = viewport.page.saturating_mul(PAGE_SIZE).saturating_add(slot);
    let num_steps = state.pattern.track_params[viewport.track]
        .get_num_steps()
        .min(MAX_STEPS);
    let visible = step < num_steps && step < MAX_STEPS && viewport.track < app.tracks.len();
    let mut dirty = false;
    dirty |= rt
        .set_reactive(
            "SEQ",
            &expanded_step_slot_index_field(viewport.track_id, slot),
            Value::Number(if visible { step as f64 } else { -1.0 }),
        )
        .effects_dirty;
    dirty |= rt
        .set_reactive(
            "SEQ",
            &expanded_step_slot_label_field(viewport.track_id, slot),
            Value::Number((step + 1) as f64),
        )
        .effects_dirty;
    dirty |= rt
        .set_reactive(
            "SEQ",
            &expanded_step_slot_visible_field(viewport.track_id, slot),
            Value::Bool(visible),
        )
        .effects_dirty;
    dirty |= rt
        .set_reactive(
            "SEQ",
            &expanded_step_slot_active_field(viewport.track_id, slot),
            Value::Bool(visible && state.pattern.patterns[viewport.track].is_active(step)),
        )
        .effects_dirty;
    dirty |= rt
        .set_reactive(
            "SEQ",
            &expanded_step_slot_plocked_field(viewport.track_id, slot),
            Value::Bool(
                visible
                    && track_step_has_plock(
                        state,
                        viewport.track,
                        &app.graph.effect_descriptors,
                        step,
                    ),
            ),
        )
        .effects_dirty;
    dirty |= rt
        .set_reactive(
            "SEQ",
            &expanded_step_slot_selected_field(viewport.track_id, slot),
            Value::Bool(
                visible && viewport.track == current_track_idx && selected_steps.contains(&step),
            ),
        )
        .effects_dirty;
    dirty |= rt
        .set_reactive(
            "SEQ",
            &expanded_step_slot_playhead_field(viewport.track_id, slot),
            Value::Bool(visible && step == track_active_playhead_step(state, viewport.track)),
        )
        .effects_dirty;
    dirty |= rt
        .set_reactive(
            "SEQ",
            &expanded_step_slot_cursor_field(viewport.track_id, slot),
            Value::Bool(visible && step == viewport.cursor_step),
        )
        .effects_dirty;
    dirty |= sync_expanded_step_param_slot(rt, state, viewport, viewport.mode, slot);
    dirty
}

pub(crate) fn sync_expanded_step_viewport(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    app: &app::App,
    selected_steps: &HashSet<usize>,
    current_track_idx: usize,
    viewport: ExpandedStepViewport,
) -> bool {
    if viewport.track >= app.tracks.len() {
        return false;
    }
    let mut dirty = false;
    let page_count = track_playhead_row_count(state, viewport.track).max(1);
    for page in 0..((MAX_STEPS + PAGE_SIZE - 1) / PAGE_SIZE) {
        dirty |= rt
            .set_reactive(
                "SEQ",
                &expanded_step_page_active_field(viewport.track_id, page),
                Value::Bool(page == viewport.page && page < page_count),
            )
            .effects_dirty;
    }
    for slot in 0..PAGE_SIZE {
        dirty |= sync_expanded_step_slot(
            rt,
            state,
            app,
            selected_steps,
            current_track_idx,
            viewport,
            slot,
        );
    }
    dirty
}

pub(crate) fn sync_expanded_step_viewport_playhead(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    viewport: ExpandedStepViewport,
) -> bool {
    let active_step = track_active_playhead_step(state, viewport.track);
    let num_steps = state.pattern.track_params[viewport.track]
        .get_num_steps()
        .min(MAX_STEPS);
    let mut dirty = false;
    for slot in 0..PAGE_SIZE {
        let step = viewport.page.saturating_mul(PAGE_SIZE).saturating_add(slot);
        dirty |= rt
            .set_reactive(
                "SEQ",
                &expanded_step_slot_playhead_field(viewport.track_id, slot),
                Value::Bool(step < num_steps && step == active_step),
            )
            .effects_dirty;
    }
    dirty
}

pub(crate) fn sync_all_track_step_binding_fields(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    app: &app::App,
    current_track_idx: usize,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
    plock_masks: &[[u64; MAX_STEPS / 64]],
) {
    sync_all_track_step_binding_fields_inner(
        rt,
        state,
        app,
        current_track_idx,
        selected_steps,
        plock_masks,
        None,
    );
}

pub(crate) fn sync_all_track_step_binding_fields_profiled(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    app: &app::App,
    current_track_idx: usize,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
    plock_masks: &[[u64; MAX_STEPS / 64]],
) -> AllTrackStepBindingSyncProfile {
    let mut profile = AllTrackStepBindingSyncProfile::default();
    sync_all_track_step_binding_fields_inner(
        rt,
        state,
        app,
        current_track_idx,
        selected_steps,
        plock_masks,
        Some(&mut profile),
    );
    profile
}

fn sync_all_track_step_binding_fields_inner(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    app: &app::App,
    current_track_idx: usize,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
    plock_masks: &[[u64; MAX_STEPS / 64]],
    mut profile: Option<&mut AllTrackStepBindingSyncProfile>,
) {
    const WORDS: usize = MAX_STEPS / 64;
    let total_started = profile.as_ref().map(|_| Instant::now());
    let selected = selected_steps.lock().unwrap();
    for track in 0..app.tracks.len() {
        let num_steps = state.pattern.track_params[track]
            .get_num_steps()
            .min(MAX_STEPS);

        // Compute all four boolean lanes as bitmasks in one pass.
        let pattern_bits = state.pattern.patterns[track].load_bits();
        let plock_bits = plock_masks
            .get(track)
            .copied()
            .unwrap_or_else(|| track_step_plock_mask(state, track, &app.graph.effect_descriptors));
        let mut active_mask = [0u64; WORDS];
        let mut duration_mask = [0u64; WORDS];
        let mut plocked_mask = [0u64; WORDS];
        let mut selected_mask = [0u64; WORDS];
        let render_values = plock_variant_step_render_values(state, track);
        let mut max_reach = f64::NEG_INFINITY;
        for step in 0..MAX_STEPS {
            let word = step / 64;
            let bit = 1u64 << (step % 64);
            let visible = step < num_steps;
            let is_active = pattern_bits[word] & bit != 0;
            if is_active {
                // duration > (target - source) <=> source + duration > target;
                // computed in f64 where both forms are exact.
                let duration = state.pattern.step_data[track]
                    .get(step, StepParam::Duration)
                    .max(0.0) as f64;
                let reach = step as f64 + duration;
                if reach > max_reach {
                    max_reach = reach;
                }
            }
            if visible {
                if is_active {
                    active_mask[word] |= bit;
                }
                if max_reach > step as f64 {
                    duration_mask[word] |= bit;
                }
                if plock_bits[word] & bit != 0 {
                    plocked_mask[word] |= bit;
                }
                if track == current_track_idx && selected.contains(&step) {
                    selected_mask[word] |= bit;
                }
            }
        }

        // Skip all per-step writes when this track's lanes are unchanged.
        let mut rev = String::with_capacity(WORDS * 4 * 16 + 3);
        for mask in [&active_mask, &duration_mask, &plocked_mask, &selected_mask] {
            for word in mask.iter() {
                use std::fmt::Write as _;
                let _ = write!(rev, "{word:016x}");
            }
        }
        for render in &render_values {
            use std::fmt::Write as _;
            let _ = write!(rev, "{:02x}", render.kind);
            for channel in render.color {
                let _ = write!(rev, "{:08x}", channel.to_bits());
            }
        }
        let rev_changed = rt
            .set_reactive(
                "SEQ",
                &track_step_binding_rev_field(track),
                Value::String(rev),
            )
            .changed;
        if !rev_changed {
            continue;
        }

        for step in 0..MAX_STEPS {
            let word = step / 64;
            let bit = 1u64 << (step % 64);
            let started = profile.as_ref().map(|_| Instant::now());
            let result = rt.set_reactive(
                "SEQ",
                &track_step_active_field(track, step),
                Value::Bool(active_mask[word] & bit != 0),
            );
            if let Some(profile) = profile.as_deref_mut() {
                profile.active_elapsed += started.expect("profile timer").elapsed();
                profile.active_sets.note(result);
            }

            let started = profile.as_ref().map(|_| Instant::now());
            let result = rt.set_reactive(
                "SEQ",
                &track_step_duration_field(track, step),
                Value::Bool(duration_mask[word] & bit != 0),
            );
            if let Some(profile) = profile.as_deref_mut() {
                profile.duration_elapsed += started.expect("profile timer").elapsed();
                profile.duration_sets.note(result);
            }

            let started = profile.as_ref().map(|_| Instant::now());
            let result = rt.set_reactive(
                "SEQ",
                &track_step_plocked_field(track, step),
                Value::Bool(plocked_mask[word] & bit != 0),
            );
            if let Some(profile) = profile.as_deref_mut() {
                profile.plocked_elapsed += started.expect("profile timer").elapsed();
                profile.plocked_sets.note(result);
            }

            let started = profile.as_ref().map(|_| Instant::now());
            let result = rt.set_reactive(
                "SEQ",
                &track_step_selected_field(track, step),
                Value::Bool(selected_mask[word] & bit != 0),
            );
            if let Some(profile) = profile.as_deref_mut() {
                profile.selected_elapsed += started.expect("profile timer").elapsed();
                profile.selected_sets.note(result);
            }

            let render = render_values[step];
            let _ = rt.set_reactive(
                "SEQ",
                &track_step_plock_kind_field(track, step),
                Value::Number(render.kind as f64),
            );
            for (channel, value) in ['r', 'g', 'b'].into_iter().zip(render.color) {
                let _ = rt.set_reactive(
                    "SEQ",
                    &track_step_variant_color_field(track, step, channel),
                    Value::Number(value as f64),
                );
            }

            // Expanded-step param controls use the seqv-slot-param-* projection fields.
            // The legacy seq-track-step-param-* fields are intentionally not synced here;
            // no current Lisp UI binds to them, and writing them dominates scene switches.
        }
    }
    if let Some(profile) = profile.as_deref_mut() {
        profile.elapsed = total_started.expect("profile timer").elapsed();
    }
}

pub(crate) fn build_all_track_duration_spans_value(
    state: &Arc<SequencerState>,
    app: &app::App,
) -> Value {
    let tracks: Vec<Rc<RefCell<Value>>> = (0..app.tracks.len())
        .map(|track| Rc::new(RefCell::new(build_track_duration_spans_value(state, track))))
        .collect();
    Value::List(tracks)
}

pub(crate) fn build_all_track_playheads_value(
    state: &Arc<SequencerState>,
    app: &app::App,
) -> Value {
    let items: Vec<Rc<RefCell<Value>>> = (0..app.tracks.len())
        .map(|t| {
            Rc::new(RefCell::new(Value::Number(
                state.transport.track_playheads[t].load(Ordering::Relaxed) as f64,
            )))
        })
        .collect();
    Value::List(items)
}

pub(crate) fn build_all_track_step_has_plocks_from_masks(
    plock_masks: &[[u64; MAX_STEPS / 64]],
) -> Value {
    let tracks: Vec<Rc<RefCell<Value>>> = plock_masks
        .iter()
        .map(|mask| Rc::new(RefCell::new(build_step_has_plocks_from_mask(mask))))
        .collect();
    Value::List(tracks)
}

pub(crate) fn build_all_track_param_lists_value(
    state: &Arc<SequencerState>,
    app: &app::App,
    param: StepParam,
) -> Value {
    let tracks: Vec<Rc<RefCell<Value>>> = (0..app.tracks.len())
        .map(|track| Rc::new(RefCell::new(build_param_list(state, track, param))))
        .collect();
    Value::List(tracks)
}

pub(crate) fn build_all_active_track_param_lists_value(
    state: &Arc<SequencerState>,
    param: StepParam,
) -> Value {
    let tracks: Vec<Rc<RefCell<Value>>> = (0..state.active_track_count())
        .map(|track| Rc::new(RefCell::new(build_param_list(state, track, param))))
        .collect();
    Value::List(tracks)
}

pub(crate) fn track_playheads_snapshot(state: &Arc<SequencerState>, app: &app::App) -> Vec<u32> {
    (0..app.tracks.len())
        .map(|t| state.transport.track_playheads[t].load(Ordering::Relaxed))
        .collect()
}

fn track_playhead_row_field(track: usize, row: usize) -> String {
    format!("track-playhead-row-{track}-{row}")
}

pub(crate) fn track_playhead_active_field(track: usize, step: usize) -> String {
    format!("track-playhead-active-{track}-{step}")
}

pub(crate) fn track_playhead_page_field(track: usize) -> String {
    format!("track-playhead-page-{track}")
}

pub(crate) fn track_active_playhead_step(state: &Arc<SequencerState>, track: usize) -> usize {
    let num_steps = state.pattern.track_params[track]
        .get_num_steps()
        .max(1)
        .min(MAX_STEPS);
    let playhead = state.transport.track_playheads[track].load(Ordering::Relaxed) as usize;
    playhead.min(num_steps.saturating_sub(1))
}

pub(crate) fn selected_plock_step(selected_steps: &Arc<Mutex<HashSet<usize>>>) -> Option<usize> {
    selected_steps.lock().unwrap().iter().copied().min()
}

pub(crate) fn displayed_plock_step(
    state: &Arc<SequencerState>,
    track: usize,
    selected_step: Option<usize>,
) -> Option<usize> {
    selected_step.or_else(|| {
        state
            .transport
            .playing
            .load(Ordering::Relaxed)
            .then(|| track_active_playhead_step(state, track))
    })
}

fn track_playhead_row_count(state: &Arc<SequencerState>, track: usize) -> usize {
    let num_steps = state.pattern.track_params[track]
        .get_num_steps()
        .max(1)
        .min(MAX_STEPS);
    (num_steps + PAGE_SIZE - 1) / PAGE_SIZE
}

pub(crate) fn sync_all_track_playhead_fields(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    app: &app::App,
) {
    for track in 0..app.tracks.len() {
        let active_step = track_active_playhead_step(state, track);
        let active_row = active_step / PAGE_SIZE;
        let active_col = active_step % PAGE_SIZE;
        let row_count = track_playhead_row_count(state, track);
        let max_rows = (MAX_STEPS + PAGE_SIZE - 1) / PAGE_SIZE;
        for row in 0..max_rows {
            rt.set_reactive(
                "SEQ",
                &track_playhead_row_field(track, row),
                Value::Number(if row == active_row && row < row_count {
                    active_col as f64
                } else {
                    -1.0
                }),
            );
        }
    }
}

pub(crate) fn clear_all_track_playhead_fields(rt: &mut Runtime, app: &app::App) {
    let max_rows = (MAX_STEPS + PAGE_SIZE - 1) / PAGE_SIZE;
    for track in 0..app.tracks.len() {
        for row in 0..max_rows {
            rt.set_reactive(
                "SEQ",
                &track_playhead_row_field(track, row),
                Value::Number(-1.0),
            );
        }
    }
}

fn field_safe_name(name: &str) -> String {
    name.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

pub(crate) fn instrument_param_value_field(track: usize, param_idx: usize, name: &str) -> String {
    format!(
        "track-{track}-instrument-param-{param_idx}-{}",
        field_safe_name(name)
    )
}

pub(crate) fn instrument_tensor_value_field(track: usize, tensor_idx: usize, name: &str) -> String {
    format!(
        "track-{track}-instrument-tensor-{tensor_idx}-{}",
        field_safe_name(name)
    )
}

pub(crate) fn sampler_selection_time_field(track: usize, marker: &str) -> String {
    format!("track-{track}-sampler-selection-{marker}-time")
}

pub(crate) fn rack_slot_sampler_selection_time_field(
    track: usize,
    slot_idx: usize,
    marker: &str,
) -> String {
    format!("track-{track}-rack-slot-{slot_idx}-sampler-selection-{marker}-time")
}

pub(crate) fn instrument_base_note_value_field(track: usize) -> String {
    format!("track-{track}-instrument-base-note")
}

pub(crate) fn rack_macro_value_field(track: usize, macro_idx: usize) -> String {
    format!("track-{track}-rack-macro-{macro_idx}")
}

pub(crate) fn rack_macro_plock_active_field(track: usize, macro_idx: usize) -> String {
    format!("track-{track}-rack-macro-{macro_idx}-plock-active")
}

pub(crate) fn rack_macro_plock_default_field(track: usize, macro_idx: usize) -> String {
    format!("track-{track}-rack-macro-{macro_idx}-plock-default")
}

pub(crate) fn rack_slot_value_field(
    track: usize,
    slot_idx: usize,
    param: sequencer::sequencer::RackSlotParam,
) -> String {
    format!("track-{track}-rack-slot-{slot_idx}-{}", param.name())
}

pub(crate) fn rack_slot_selected_field(track: usize, slot_idx: usize) -> String {
    format!("track-{track}-rack-slot-{slot_idx}-selected")
}

pub(crate) fn rack_slot_instrument_param_value_field(
    track: usize,
    slot_idx: usize,
    param_idx: usize,
    name: &str,
) -> String {
    format!(
        "track-{track}-rack-slot-{slot_idx}-instrument-param-{param_idx}-{}",
        field_safe_name(name)
    )
}

pub(crate) fn rack_slot_effect_param_value_field(
    track: usize,
    slot_idx: usize,
    effect_slot: usize,
    param_idx: usize,
    name: &str,
) -> String {
    format!(
        "track-{track}-rack-slot-{slot_idx}-fx-{effect_slot}-param-{param_idx}-{}",
        field_safe_name(name)
    )
}

pub(crate) fn track_effect_param_value_field(
    track: usize,
    slot_idx: usize,
    param_idx: usize,
    name: &str,
) -> String {
    format!(
        "track-{track}-fx-{slot_idx}-param-{param_idx}-{}",
        field_safe_name(name)
    )
}

pub(crate) fn midi_fx_param_value_field(
    track: usize,
    slot_idx: usize,
    param_idx: usize,
    name: &str,
) -> String {
    format!(
        "track-{track}-midi-fx-{slot_idx}-param-{param_idx}-{}",
        field_safe_name(name)
    )
}

pub(crate) fn bus_effect_param_value_field(
    bus_idx: usize,
    slot_idx: usize,
    param_idx: usize,
    name: &str,
) -> String {
    format!(
        "bus-{bus_idx}-fx-{slot_idx}-param-{param_idx}-{}",
        field_safe_name(name)
    )
}

fn insert_string_prop(
    map: &mut HashMap<String, Rc<RefCell<Value>>>,
    key: &str,
    value: impl Into<String>,
) {
    map.insert(
        key.to_string(),
        Rc::new(RefCell::new(Value::String(value.into()))),
    );
}

fn insert_param_ui_metadata(
    map: &mut HashMap<String, Rc<RefCell<Value>>>,
    metadata: Option<&sequencer::effects::ParamUiMetadata>,
) {
    let Some(metadata) = metadata else { return };
    if let Some(group) = &metadata.group {
        insert_string_prop(map, "group", group);
    }
    if let Some(env) = &metadata.env {
        insert_string_prop(map, "env", env);
    }
    if let Some(role) = &metadata.role {
        insert_string_prop(map, "role", role);
    }
}

fn instrument_slot_param_value(
    slot: &sequencer::effects::EffectSlotState,
    desc: &sequencer::effects::EffectDescriptor,
    param_idx: usize,
    plock_step: Option<usize>,
) -> f32 {
    plock_step
        .and_then(|step| slot.plocks.get(step, param_idx))
        .unwrap_or_else(|| {
            if param_idx < slot.num_params.load(Ordering::Relaxed) as usize {
                slot.defaults.get(param_idx)
            } else {
                desc.params
                    .get(param_idx)
                    .map(|param| param.default)
                    .unwrap_or_default()
            }
        })
}

fn selected_voice_mod_source_indices(
    desc: &sequencer::effects::EffectDescriptor,
    slot: &sequencer::effects::EffectSlotState,
    plock_step: Option<usize>,
) -> Vec<usize> {
    sequencer::voice_modulator::selected_source_param_indices(&desc.params, |idx, _| {
        instrument_slot_param_value(slot, desc, idx, plock_step)
    })
}

fn selected_voice_mod_source_indices_for_optional_slot(
    desc: &sequencer::effects::EffectDescriptor,
    slot: Option<&sequencer::effects::EffectSlotState>,
    plock_step: Option<usize>,
) -> Vec<usize> {
    if let Some(slot) = slot {
        return selected_voice_mod_source_indices(desc, slot, plock_step);
    }
    sequencer::voice_modulator::selected_source_param_indices(&desc.params, |_, param| {
        param.default
    })
}

fn param_supports_value_binding(pdesc: &sequencer::effects::ParamDescriptor) -> bool {
    matches!(pdesc.kind, sequencer::effects::ParamKind::Continuous { .. })
        || matches!(pdesc.kind, sequencer::effects::ParamKind::Enum { .. })
        || pdesc.name.eq_ignore_ascii_case("enabled")
}

fn slot_param_stored_value(
    slot: &sequencer::effects::EffectSlotState,
    pdesc: &sequencer::effects::ParamDescriptor,
    param_idx: usize,
    display_step: Option<usize>,
) -> f32 {
    display_step
        .and_then(|step| slot.plocks.get(step, param_idx))
        .unwrap_or_else(|| {
            if param_idx < slot.num_params.load(Ordering::Relaxed) as usize {
                slot.defaults.get(param_idx)
            } else {
                pdesc.default
            }
        })
}

fn reactive_set_needs_ui(result: eseqlisp::runtime::ReactiveSetResult) -> bool {
    result.effects_dirty || result.widgets_dirty
}

pub(crate) fn sync_instrument_param_value_field(
    rt: &mut Runtime,
    app: &app::App,
    track: usize,
    param_idx: usize,
    display_step: Option<usize>,
) -> bool {
    if let Some((name, value)) = app
        .graph
        .instrument_descriptors
        .get(track)
        .and_then(|desc| desc.params.get(param_idx))
        .and_then(|pdesc| {
            app.state.pattern.instrument_slots.get(track).map(|slot| {
                let stored = display_step
                    .and_then(|step| slot.plocks.get(step, param_idx))
                    .or_else(|| app.effective_instrument_param_value(track, param_idx))
                    .unwrap_or_else(|| {
                        slot_param_stored_value(slot, pdesc, param_idx, display_step)
                    });
                (pdesc.name.clone(), pdesc.stored_to_user(stored))
            })
        })
    {
        return reactive_set_needs_ui(rt.set_reactive(
            "SEQ",
            &instrument_param_value_field(track, param_idx, &name),
            Value::Number(value as f64),
        ));
    }
    false
}

pub(crate) fn sync_instrument_tensor_value_field(
    rt: &mut Runtime,
    app: &app::App,
    track: usize,
    tensor_idx: usize,
    display_step: Option<usize>,
) -> bool {
    if let Some((name, values)) = app
        .graph
        .instrument_descriptors
        .get(track)
        .and_then(|desc| desc.tensor_params.get(tensor_idx))
        .and_then(|tdesc| {
            app.state.pattern.instrument_slots.get(track).map(|slot| {
                let values = slot
                    .tensor_params
                    .resolved_values(display_step, tensor_idx)
                    .unwrap_or_else(|| tdesc.default.clone());
                (tdesc.name.clone(), values)
            })
        })
    {
        let list = values
            .into_iter()
            .map(|value| Rc::new(RefCell::new(Value::Number(value as f64))))
            .collect();
        return reactive_set_needs_ui(rt.set_reactive(
            "SEQ",
            &instrument_tensor_value_field(track, tensor_idx, &name),
            Value::List(list),
        ));
    }
    false
}

pub(crate) fn sync_rack_macro_value_fields(
    rt: &mut Runtime,
    app: &app::App,
    track: usize,
    display_step: Option<usize>,
) -> bool {
    let rack_macros = {
        let racks = app.state.pattern.rack_tracks.lock().unwrap();
        let Some(Some(rack)) = racks.get(track) else {
            return false;
        };
        rack.macros
            .iter()
            .map(|rack_macro| {
                let plock_value = display_step
                    .and_then(|step| rack_macro.plocks.get(step))
                    .and_then(|value| *value);
                (rack_macro.id, rack_macro.value, plock_value)
            })
            .collect::<Vec<_>>()
    };
    let mut needs_ui = false;
    for (id, base_value, plock_value) in rack_macros {
        let value = app
            .effective_rack_macro_value(track, id, display_step)
            .unwrap_or(base_value);
        needs_ui |= reactive_set_needs_ui(rt.set_reactive(
            "SEQ",
            &rack_macro_value_field(track, id.index()),
            Value::Number(value as f64),
        ));
        needs_ui |= reactive_set_needs_ui(rt.set_reactive(
            "SEQ",
            &rack_macro_plock_active_field(track, id.index()),
            Value::Number(if plock_value.is_some() { 1.0 } else { 0.0 }),
        ));
        needs_ui |= reactive_set_needs_ui(rt.set_reactive(
            "SEQ",
            &rack_macro_plock_default_field(track, id.index()),
            Value::Number(base_value as f64),
        ));
    }
    needs_ui
}

pub(crate) fn sync_rack_macro_value_field(
    rt: &mut Runtime,
    app: &app::App,
    track: usize,
    id: sequencer::sequencer::RackMacroId,
    display_step: Option<usize>,
) -> bool {
    let (base_value, plock_value) = {
        let racks = app.state.pattern.rack_tracks.lock().unwrap();
        let Some(rack_macro) = racks
            .get(track)
            .and_then(Option::as_ref)
            .and_then(|rack| rack.macros.get(id.index()))
        else {
            return false;
        };
        (
            rack_macro.value,
            display_step
                .and_then(|step| rack_macro.plocks.get(step))
                .and_then(|value| *value),
        )
    };
    let value = app
        .effective_rack_macro_value(track, id, display_step)
        .unwrap_or(base_value);
    let mut needs_ui = reactive_set_needs_ui(rt.set_reactive(
        "SEQ",
        &rack_macro_value_field(track, id.index()),
        Value::Number(value as f64),
    ));
    needs_ui |= reactive_set_needs_ui(rt.set_reactive(
        "SEQ",
        &rack_macro_plock_active_field(track, id.index()),
        Value::Number(if plock_value.is_some() { 1.0 } else { 0.0 }),
    ));
    needs_ui |= reactive_set_needs_ui(rt.set_reactive(
        "SEQ",
        &rack_macro_plock_default_field(track, id.index()),
        Value::Number(base_value as f64),
    ));
    needs_ui
}

fn rack_slot_control_value(
    rack: &sequencer::sequencer::RackTrackSnapshot,
    slot_idx: usize,
    slot: &sequencer::sequencer::RackSlotSnapshot,
    param: sequencer::sequencer::RackSlotParam,
    display_step: Option<usize>,
) -> f32 {
    if let Some(value) = display_step.and_then(|step| slot.param_plocks.get(step, param)) {
        return param.clamp(value);
    }
    rack_macro_mapped_value(rack, display_step, |target| {
        matches!(
            target,
            sequencer::sequencer::RackMacroTarget::SlotParam {
                slot,
                param: target_param,
            } if *slot == slot_idx
                && sequencer::sequencer::RackSlotParam::from_name(target_param) == Some(param)
        )
    })
    .map(|value| param.clamp(value))
    .unwrap_or_else(|| param.clamp(slot.param_value_at_step(param, usize::MAX)))
}

fn set_rack_value_field_updates(
    rt: &mut Runtime,
    updates: impl IntoIterator<Item = (String, Value)>,
) -> bool {
    updates.into_iter().fold(false, |needs_ui, (field, value)| {
        reactive_set_needs_ui(rt.set_reactive("SEQ", &field, value)) || needs_ui
    })
}

fn rack_slot_sample_duration(app: &app::App, slot: &sequencer::sequencer::RackSlotSnapshot) -> f64 {
    let Some((buffer_id, sample_name, _)) = slot.sample_id.as_ref() else {
        return 1.0;
    };
    app.sample_buffer_path_registry
        .get(buffer_id)
        .or_else(|| app.sample_path_registry.get(sample_name))
        .and_then(|path| {
            eseqlisp::audio::sample::get_registered_sample(&path.display().to_string())
        })
        .map(|sample| sample.duration_seconds)
        .unwrap_or(1.0)
}

fn rack_sampler_selection_update(
    app: &app::App,
    track: usize,
    slot_idx: usize,
    slot: &sequencer::sequencer::RackSlotSnapshot,
    param_idx: usize,
    stored_value: f32,
) -> Option<(String, Value)> {
    if slot.instrument_type != sequencer::sequencer::InstrumentType::Sampler {
        return None;
    }
    let marker = match param_idx {
        2 => "start",
        3 => "end",
        _ => return None,
    };
    Some((
        rack_slot_sampler_selection_time_field(track, slot_idx, marker),
        Value::Number(stored_value as f64 * rack_slot_sample_duration(app, slot)),
    ))
}

pub(crate) fn sync_rack_slot_control_value_field(
    rt: &mut Runtime,
    app: &app::App,
    track: usize,
    slot_idx: usize,
    param: sequencer::sequencer::RackSlotParam,
    display_step: Option<usize>,
) -> bool {
    let value = {
        let racks = app.state.pattern.rack_tracks.lock().unwrap();
        let Some(rack) = racks.get(track).and_then(Option::as_ref) else {
            return false;
        };
        let Some(slot) = rack.slots.get(slot_idx) else {
            return false;
        };
        rack_slot_control_value(rack, slot_idx, slot, param, display_step)
    };
    let value = if matches!(
        param,
        sequencer::sequencer::RackSlotParam::Mute | sequencer::sequencer::RackSlotParam::Solo
    ) {
        Value::Bool(value > 0.5)
    } else {
        Value::Number(value as f64)
    };
    reactive_set_needs_ui(rt.set_reactive(
        "SEQ",
        &rack_slot_value_field(track, slot_idx, param),
        value,
    ))
}

pub(crate) fn sync_rack_slot_instrument_param_value_field(
    rt: &mut Runtime,
    app: &app::App,
    track: usize,
    slot_idx: usize,
    param_idx: usize,
    display_step: Option<usize>,
) -> bool {
    let (name, value, selection_update) = {
        let racks = app.state.pattern.rack_tracks.lock().unwrap();
        let Some(rack) = racks.get(track).and_then(Option::as_ref) else {
            return false;
        };
        let Some(slot) = rack.slots.get(slot_idx) else {
            return false;
        };
        let Some(descriptor) = app.rack_slot_instrument_descriptor(slot) else {
            return false;
        };
        let Some(param) = descriptor.params.get(param_idx) else {
            return false;
        };
        let stored =
            rack_slot_param_value(rack, slot_idx, slot, &descriptor, param_idx, display_step);
        (
            param.name.clone(),
            param.stored_to_user(stored),
            rack_sampler_selection_update(app, track, slot_idx, slot, param_idx, stored),
        )
    };
    let mut needs_ui = reactive_set_needs_ui(rt.set_reactive(
        "SEQ",
        &rack_slot_instrument_param_value_field(track, slot_idx, param_idx, &name),
        Value::Number(value as f64),
    ));
    if let Some((field, value)) = selection_update {
        needs_ui |= reactive_set_needs_ui(rt.set_reactive("SEQ", &field, value));
    }
    needs_ui
}

pub(crate) fn sync_rack_slot_effect_param_value_field(
    rt: &mut Runtime,
    app: &app::App,
    track: usize,
    rack_slot: usize,
    effect_slot: usize,
    param_idx: usize,
    display_step: Option<usize>,
) -> bool {
    let (name, value) = {
        let racks = app.state.pattern.rack_tracks.lock().unwrap();
        let Some(rack) = racks.get(track).and_then(Option::as_ref) else {
            return false;
        };
        let Some(slot) = rack.slots.get(rack_slot) else {
            return false;
        };
        let Some(descriptor) = slot.effect_descriptors.get(effect_slot) else {
            return false;
        };
        let Some(snapshot) = slot.effect_slots.get(effect_slot) else {
            return false;
        };
        let Some(param) = descriptor.params.get(param_idx) else {
            return false;
        };
        let value = rack_effect_param_value(
            rack,
            rack_slot,
            effect_slot,
            snapshot,
            descriptor,
            param_idx,
            display_step,
        );
        (param.name.clone(), value)
    };
    reactive_set_needs_ui(rt.set_reactive(
        "SEQ",
        &rack_slot_effect_param_value_field(track, rack_slot, effect_slot, param_idx, &name),
        Value::Number(value as f64),
    ))
}

pub(crate) fn sync_rack_panel_param_value_fields(
    rt: &mut Runtime,
    app: &app::App,
    track: usize,
    display_step: Option<usize>,
) -> bool {
    let mut updates = Vec::new();
    {
        let racks = app.state.pattern.rack_tracks.lock().unwrap();
        let Some(Some(rack)) = racks.get(track) else {
            return false;
        };
        for (slot_idx, slot) in rack.slots.iter().enumerate() {
            for param in sequencer::sequencer::RackSlotParam::ALL {
                let value = rack_slot_control_value(rack, slot_idx, slot, param, display_step);
                let value = if matches!(
                    param,
                    sequencer::sequencer::RackSlotParam::Mute
                        | sequencer::sequencer::RackSlotParam::Solo
                ) {
                    Value::Bool(value > 0.5)
                } else {
                    Value::Number(value as f64)
                };
                updates.push((rack_slot_value_field(track, slot_idx, param), value));
            }

            if let Some(descriptor) = app.rack_slot_instrument_descriptor(slot) {
                for (param_idx, param) in descriptor.params.iter().enumerate() {
                    let value = rack_slot_param_value(
                        rack,
                        slot_idx,
                        slot,
                        &descriptor,
                        param_idx,
                        display_step,
                    );
                    updates.push((
                        rack_slot_instrument_param_value_field(
                            track,
                            slot_idx,
                            param_idx,
                            &param.name,
                        ),
                        Value::Number(param.stored_to_user(value) as f64),
                    ));
                    if let Some(update) =
                        rack_sampler_selection_update(app, track, slot_idx, slot, param_idx, value)
                    {
                        updates.push(update);
                    }
                }
            }

            for (effect_slot, (descriptor, snapshot)) in slot
                .effect_descriptors
                .iter()
                .zip(&slot.effect_slots)
                .enumerate()
            {
                if snapshot.node_id == 0 {
                    continue;
                }
                for (param_idx, param) in descriptor.params.iter().enumerate() {
                    let value = rack_effect_param_value(
                        rack,
                        slot_idx,
                        effect_slot,
                        snapshot,
                        descriptor,
                        param_idx,
                        display_step,
                    );
                    updates.push((
                        rack_slot_effect_param_value_field(
                            track,
                            slot_idx,
                            effect_slot,
                            param_idx,
                            &param.name,
                        ),
                        Value::Number(value as f64),
                    ));
                }
            }
        }
    }
    set_rack_value_field_updates(rt, updates)
}

pub(crate) fn sync_rack_macro_target_value_fields(
    rt: &mut Runtime,
    app: &app::App,
    track: usize,
    id: sequencer::sequencer::RackMacroId,
    display_step: Option<usize>,
) -> bool {
    let mut updates = Vec::new();
    {
        let racks = app.state.pattern.rack_tracks.lock().unwrap();
        let Some(Some(rack)) = racks.get(track) else {
            return false;
        };
        let Some(rack_macro) = rack.macros.get(id.index()) else {
            return false;
        };
        let mut sampler_descriptor = None;
        for mapping in &rack_macro.mappings {
            match &mapping.target {
                sequencer::sequencer::RackMacroTarget::SlotParam { slot, param } => {
                    let Some(param) = sequencer::sequencer::RackSlotParam::from_name(param) else {
                        continue;
                    };
                    let Some(slot_data) = rack.slots.get(*slot) else {
                        continue;
                    };
                    let displayed =
                        rack_slot_control_value(rack, *slot, slot_data, param, display_step);
                    let value = if matches!(
                        param,
                        sequencer::sequencer::RackSlotParam::Mute
                            | sequencer::sequencer::RackSlotParam::Solo
                    ) {
                        Value::Bool(displayed > 0.5)
                    } else {
                        Value::Number(displayed as f64)
                    };
                    updates.push((rack_slot_value_field(track, *slot, param), value));
                }
                sequencer::sequencer::RackMacroTarget::SlotInstrumentParam {
                    slot,
                    param_index,
                    ..
                } => {
                    let Some(slot_data) = rack.slots.get(*slot) else {
                        continue;
                    };
                    let descriptor = if let Some(descriptor) =
                        app.rack_slot_cached_instrument_descriptor(slot_data)
                    {
                        descriptor
                    } else if matches!(
                        slot_data.instrument_type,
                        sequencer::sequencer::InstrumentType::Sampler
                    ) {
                        sampler_descriptor.get_or_insert_with(
                            sequencer::effects::EffectDescriptor::builtin_sampler,
                        )
                    } else {
                        continue;
                    };
                    let Some(param) = descriptor.params.get(*param_index) else {
                        continue;
                    };
                    let stored = rack_slot_param_value(
                        rack,
                        *slot,
                        slot_data,
                        descriptor,
                        *param_index,
                        display_step,
                    );
                    updates.push((
                        rack_slot_instrument_param_value_field(
                            track,
                            *slot,
                            *param_index,
                            &param.name,
                        ),
                        Value::Number(param.stored_to_user(stored) as f64),
                    ));
                    if let Some(update) = rack_sampler_selection_update(
                        app,
                        track,
                        *slot,
                        slot_data,
                        *param_index,
                        stored,
                    ) {
                        updates.push(update);
                    }
                }
                sequencer::sequencer::RackMacroTarget::SlotEffectParam {
                    slot,
                    effect_slot,
                    param_index,
                    ..
                } => {
                    let Some(slot_data) = rack.slots.get(*slot) else {
                        continue;
                    };
                    let Some(descriptor) = slot_data.effect_descriptors.get(*effect_slot) else {
                        continue;
                    };
                    let Some(snapshot) = slot_data.effect_slots.get(*effect_slot) else {
                        continue;
                    };
                    let Some(param) = descriptor.params.get(*param_index) else {
                        continue;
                    };
                    let displayed = rack_effect_param_value(
                        rack,
                        *slot,
                        *effect_slot,
                        snapshot,
                        descriptor,
                        *param_index,
                        display_step,
                    );
                    updates.push((
                        rack_slot_effect_param_value_field(
                            track,
                            *slot,
                            *effect_slot,
                            *param_index,
                            &param.name,
                        ),
                        Value::Number(displayed as f64),
                    ));
                }
            }
        }
    }
    set_rack_value_field_updates(rt, updates)
}

pub(crate) fn sync_instrument_param_value_field_with_neural_selection(
    rt: &mut Runtime,
    app: &app::App,
    track: usize,
    param_idx: usize,
    display_step: Option<usize>,
    selected_neural_neurons: Option<
        &std::collections::BTreeSet<sequencer::lisp_host::SelectedNeuralNeuron>,
    >,
) -> bool {
    if let Some((name, value)) = app
        .graph
        .instrument_descriptors
        .get(track)
        .and_then(|desc| desc.params.get(param_idx))
        .and_then(|pdesc| {
            app.state.pattern.instrument_slots.get(track).map(|slot| {
                let neural_value = selected_neural_neurons.and_then(|selection| {
                    sequencer::lisp_host::selected_neural_instrument_plock_value(
                        &app.state, selection, track, param_idx,
                    )
                });
                let stored = neural_value
                    .or_else(|| display_step.and_then(|step| slot.plocks.get(step, param_idx)))
                    .or_else(|| app.effective_instrument_param_value(track, param_idx))
                    .unwrap_or_else(|| {
                        slot_param_stored_value(slot, pdesc, param_idx, display_step)
                    });
                (pdesc.name.clone(), pdesc.stored_to_user(stored))
            })
        })
    {
        return reactive_set_needs_ui(rt.set_reactive(
            "SEQ",
            &instrument_param_value_field(track, param_idx, &name),
            Value::Number(value as f64),
        ));
    }
    false
}

pub(crate) fn sync_sampler_selection_time_fields(
    rt: &mut Runtime,
    app: &app::App,
    track: usize,
    display_step: Option<usize>,
) -> bool {
    if !app.is_sampler_track(track) {
        return false;
    }
    let sample_duration = app
        .sampler_path_for_track(track)
        .as_ref()
        .and_then(|p| eseqlisp::audio::sample::get_registered_sample(&p.display().to_string()))
        .map(|sample| sample.duration_seconds)
        .unwrap_or(1.0);
    let Some(slot) = app.state.pattern.instrument_slots.get(track) else {
        return false;
    };
    let start_raw = display_step
        .and_then(|step| slot.plocks.get(step, 2))
        .unwrap_or_else(|| slot.defaults.get(2));
    let end_raw = display_step
        .and_then(|step| slot.plocks.get(step, 3))
        .unwrap_or_else(|| slot.defaults.get(3));
    let start = start_raw as f64 * sample_duration;
    let end = end_raw as f64 * sample_duration;
    let mut needs_ui = reactive_set_needs_ui(rt.set_reactive(
        "SEQ",
        &sampler_selection_time_field(track, "start"),
        Value::Number(start),
    ));
    needs_ui |= reactive_set_needs_ui(rt.set_reactive(
        "SEQ",
        &sampler_selection_time_field(track, "end"),
        Value::Number(end),
    ));
    needs_ui
}

pub(crate) fn sync_instrument_base_note_value_field(
    rt: &mut Runtime,
    app: &app::App,
    track: usize,
) -> bool {
    if track < app.tracks.len() {
        let value = f32::from_bits(
            app.state.pattern.instrument_base_note_offsets[track].load(Ordering::Relaxed),
        );
        return reactive_set_needs_ui(rt.set_reactive(
            "SEQ",
            &instrument_base_note_value_field(track),
            Value::Number(value as f64),
        ));
    }
    false
}

pub(crate) fn sync_track_effect_param_value_field(
    rt: &mut Runtime,
    app: &app::App,
    track: usize,
    slot_idx: usize,
    param_idx: usize,
    display_step: Option<usize>,
) -> bool {
    if let Some((name, value)) = app
        .graph
        .effect_descriptors
        .get(track)
        .and_then(|slots| slots.get(slot_idx))
        .and_then(|desc| desc.params.get(param_idx).map(|p| (&desc.name, p)))
        .and_then(|(_, pdesc)| {
            app.state
                .pattern
                .effect_chains
                .get(track)
                .and_then(|chain| chain.get(slot_idx))
                .map(|slot| {
                    let stored = display_step
                        .and_then(|step| slot.plocks.get(step, param_idx))
                        .or_else(|| app.effective_slot_param_value(track, slot_idx, param_idx))
                        .unwrap_or_else(|| {
                            slot_param_stored_value(slot, pdesc, param_idx, display_step)
                        });
                    (pdesc.name.clone(), stored)
                })
        })
    {
        return reactive_set_needs_ui(rt.set_reactive(
            "SEQ",
            &track_effect_param_value_field(track, slot_idx, param_idx, &name),
            Value::Number(value as f64),
        ));
    }
    false
}

pub(crate) fn sync_track_effect_param_value_field_with_neural_selection(
    rt: &mut Runtime,
    app: &app::App,
    track: usize,
    slot_idx: usize,
    param_idx: usize,
    display_step: Option<usize>,
    selected_neural_neurons: Option<
        &std::collections::BTreeSet<sequencer::lisp_host::SelectedNeuralNeuron>,
    >,
) -> bool {
    if let Some((name, value)) = app
        .graph
        .effect_descriptors
        .get(track)
        .and_then(|slots| slots.get(slot_idx))
        .and_then(|desc| desc.params.get(param_idx).map(|p| (&desc.name, p)))
        .and_then(|(_, pdesc)| {
            app.state
                .pattern
                .effect_chains
                .get(track)
                .and_then(|chain| chain.get(slot_idx))
                .map(|slot| {
                    let neural_value = selected_neural_neurons.and_then(|selection| {
                        sequencer::lisp_host::selected_neural_effect_plock_value(
                            &app.state, selection, track, slot_idx, param_idx,
                        )
                    });
                    let stored = neural_value
                        .or_else(|| display_step.and_then(|step| slot.plocks.get(step, param_idx)))
                        .or_else(|| app.effective_slot_param_value(track, slot_idx, param_idx))
                        .unwrap_or_else(|| {
                            slot_param_stored_value(slot, pdesc, param_idx, display_step)
                        });
                    (pdesc.name.clone(), stored)
                })
        })
    {
        return reactive_set_needs_ui(rt.set_reactive(
            "SEQ",
            &track_effect_param_value_field(track, slot_idx, param_idx, &name),
            Value::Number(value as f64),
        ));
    }
    false
}

pub(crate) fn sync_midi_fx_param_value_field(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    track: usize,
    slot_idx: usize,
    param_idx: usize,
    display_step: Option<usize>,
) -> bool {
    let chain = state.pattern.track_params[track].midi_fx_chain();
    if let Some((name, value)) = chain
        .get(slot_idx)
        .and_then(|fx_name| sequencer::lisp_host::load_midi_fx_descriptor(fx_name))
        .and_then(|desc| desc.params.get(param_idx).cloned())
        .and_then(|pdesc| {
            state
                .pattern
                .midi_fx_slots
                .get(track)
                .and_then(|slots| slots.get(slot_idx))
                .map(|slot| {
                    let stored = slot_param_stored_value(slot, &pdesc, param_idx, display_step);
                    (pdesc.name, stored)
                })
        })
    {
        return reactive_set_needs_ui(rt.set_reactive(
            "SEQ",
            &midi_fx_param_value_field(track, slot_idx, param_idx, &name),
            Value::Number(value as f64),
        ));
    }
    false
}

pub(crate) fn sync_bus_effect_param_value_field(
    rt: &mut Runtime,
    app: &app::App,
    bus_idx: usize,
    slot_idx: usize,
    param_idx: usize,
) -> bool {
    if let Some((name, value)) = app.buses.get(bus_idx).and_then(|bus| {
        bus.effect_descriptors
            .get(slot_idx)
            .and_then(|desc| desc.params.get(param_idx))
            .and_then(|pdesc| {
                bus.effect_slots.get(slot_idx).map(|slot| {
                    (
                        pdesc.name.clone(),
                        slot.defaults
                            .get(param_idx)
                            .copied()
                            .unwrap_or(pdesc.default),
                    )
                })
            })
    }) {
        return reactive_set_needs_ui(rt.set_reactive(
            "SEQ",
            &bus_effect_param_value_field(bus_idx, slot_idx, param_idx, &name),
            Value::Number(value as f64),
        ));
    }
    false
}

pub(crate) fn sync_fx_param_binding_fields(
    rt: &mut Runtime,
    app: &app::App,
    state: &Arc<SequencerState>,
    track: usize,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
) -> bool {
    sync_fx_param_binding_fields_with_neural_selection(rt, app, state, track, selected_steps, None)
}

pub(crate) fn sync_fx_param_binding_fields_with_neural_selection(
    rt: &mut Runtime,
    app: &app::App,
    state: &Arc<SequencerState>,
    track: usize,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
    selected_neural_neurons: Option<
        &std::collections::BTreeSet<sequencer::lisp_host::SelectedNeuralNeuron>,
    >,
) -> bool {
    let mut needs_ui = false;
    if track < app.tracks.len() {
        let selected_step = selected_plock_step(selected_steps);
        let display_step = displayed_plock_step(state, track, selected_step);
        needs_ui |= sync_rack_macro_value_fields(rt, app, track, display_step);
        needs_ui |= sync_rack_panel_param_value_fields(rt, app, track, display_step);
        needs_ui |= sync_instrument_base_note_value_field(rt, app, track);
        needs_ui |= sync_sampler_selection_time_fields(rt, app, track, display_step);
        if let Some(desc) = app.graph.instrument_descriptors.get(track) {
            for (param_idx, pdesc) in desc.params.iter().enumerate() {
                if param_supports_value_binding(pdesc) {
                    needs_ui |= sync_instrument_param_value_field_with_neural_selection(
                        rt,
                        app,
                        track,
                        param_idx,
                        display_step,
                        selected_neural_neurons,
                    );
                }
            }
            for tensor_idx in 0..desc.tensor_params.len() {
                needs_ui |=
                    sync_instrument_tensor_value_field(rt, app, track, tensor_idx, display_step);
            }
        }
        if let Some(slots) = app.graph.effect_descriptors.get(track) {
            for (slot_idx, desc) in slots.iter().enumerate() {
                for (param_idx, pdesc) in desc.params.iter().enumerate() {
                    if param_supports_value_binding(pdesc) {
                        needs_ui |= sync_track_effect_param_value_field_with_neural_selection(
                            rt,
                            app,
                            track,
                            slot_idx,
                            param_idx,
                            display_step,
                            selected_neural_neurons,
                        );
                    }
                }
            }
        }
        for (slot_idx, name) in state.pattern.track_params[track]
            .midi_fx_chain()
            .iter()
            .enumerate()
        {
            if let Some(desc) = sequencer::lisp_host::load_midi_fx_descriptor(name) {
                for (param_idx, pdesc) in desc.params.iter().enumerate() {
                    if param_supports_value_binding(pdesc) {
                        needs_ui |= sync_midi_fx_param_value_field(
                            rt,
                            state,
                            track,
                            slot_idx,
                            param_idx,
                            display_step,
                        );
                    }
                }
            }
        }
    }

    for (bus_idx, bus) in app.buses.iter().enumerate() {
        for (slot_idx, desc) in bus.effect_descriptors.iter().enumerate() {
            for (param_idx, pdesc) in desc.params.iter().enumerate() {
                if param_supports_value_binding(pdesc) {
                    needs_ui |=
                        sync_bus_effect_param_value_field(rt, app, bus_idx, slot_idx, param_idx);
                }
            }
        }
    }
    needs_ui
}

pub(crate) fn sync_track_playhead_field_delta(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    app: &app::App,
    previous: &mut Vec<u32>,
) -> bool {
    let track_count = app.tracks.len();
    let mut current = Vec::with_capacity(track_count);
    let mut effects_dirty = false;
    let mut snapshot_changed = previous.len() != track_count;

    for t in 0..track_count {
        let playhead = state.transport.track_playheads[t].load(Ordering::Relaxed);
        let active_step = track_active_playhead_step(state, t);
        let active_row = active_step / PAGE_SIZE;
        let active_col = active_step % PAGE_SIZE;
        if let Some(prev_playhead) = previous.get(t).copied() {
            if prev_playhead != playhead {
                snapshot_changed = true;
                let num_steps = state.pattern.track_params[t]
                    .get_num_steps()
                    .max(1)
                    .min(MAX_STEPS);
                let prev_active_step = (prev_playhead as usize).min(num_steps.saturating_sub(1));
                let prev_active_row = prev_active_step / PAGE_SIZE;
                if prev_active_row != active_row {
                    effects_dirty |= rt
                        .set_reactive(
                            "SEQ",
                            &track_playhead_page_field(t),
                            Value::Number(active_row as f64),
                        )
                        .effects_dirty;
                    effects_dirty |= rt
                        .set_reactive(
                            "SEQ",
                            &track_playhead_row_field(t, prev_active_row),
                            Value::Number(-1.0),
                        )
                        .effects_dirty;
                }
                if prev_active_step != active_step {
                    effects_dirty |= rt
                        .set_reactive(
                            "SEQ",
                            &track_playhead_active_field(t, prev_active_step),
                            Value::Bool(false),
                        )
                        .effects_dirty;
                    effects_dirty |= rt
                        .set_reactive(
                            "SEQ",
                            &track_playhead_active_field(t, active_step),
                            Value::Bool(true),
                        )
                        .effects_dirty;
                    effects_dirty |= rt
                        .set_reactive(
                            "SEQ",
                            &track_playhead_row_field(t, active_row),
                            Value::Number(active_col as f64),
                        )
                        .effects_dirty;
                }
            }
        } else {
            effects_dirty |= rt
                .set_reactive(
                    "SEQ",
                    &track_playhead_page_field(t),
                    Value::Number(active_row as f64),
                )
                .effects_dirty;
            effects_dirty |= rt
                .set_reactive(
                    "SEQ",
                    &track_playhead_active_field(t, active_step),
                    Value::Bool(true),
                )
                .effects_dirty;
            effects_dirty |= rt
                .set_reactive(
                    "SEQ",
                    &track_playhead_row_field(t, active_row),
                    Value::Number(active_col as f64),
                )
                .effects_dirty;
        }
        current.push(playhead);
    }

    if snapshot_changed {
        *previous = current;
    }

    effects_dirty
}

pub(crate) fn sync_all_track_sequencer_state(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    app: &app::App,
    current_track_idx: usize,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
) {
    sync_all_track_sequencer_state_inner(rt, state, app, current_track_idx, selected_steps, None);
}

pub(crate) fn sync_all_track_sequencer_state_profiled(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    app: &app::App,
    current_track_idx: usize,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
) -> AllTrackSequencerSyncProfile {
    let mut profile = AllTrackSequencerSyncProfile::default();
    sync_all_track_sequencer_state_inner(
        rt,
        state,
        app,
        current_track_idx,
        selected_steps,
        Some(&mut profile),
    );
    profile
}

fn sync_all_track_sequencer_state_inner(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    app: &app::App,
    current_track_idx: usize,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
    mut profile: Option<&mut AllTrackSequencerSyncProfile>,
) {
    let total_started = profile.as_ref().map(|_| Instant::now());
    let started = profile.as_ref().map(|_| Instant::now());
    rt.set_reactive(
        "SEQ",
        "track-steps",
        build_all_track_steps_value(state, app),
    );
    if let Some(profile) = profile.as_deref_mut() {
        profile.track_steps = started.expect("profile timer").elapsed();
    }

    let started = profile.as_ref().map(|_| Instant::now());
    rt.set_reactive(
        "SEQ",
        "track-num-steps",
        build_all_track_num_steps_value(state, app),
    );
    if let Some(profile) = profile.as_deref_mut() {
        profile.track_num_steps = started.expect("profile timer").elapsed();
    }

    let started = profile.as_ref().map(|_| Instant::now());
    rt.set_reactive(
        "SEQ",
        "track-timebases",
        build_all_track_timebase_labels_value(state, app, current_track_idx, selected_steps),
    );
    if let Some(profile) = profile.as_deref_mut() {
        profile.track_timebases = started.expect("profile timer").elapsed();
    }

    let started = profile.as_ref().map(|_| Instant::now());
    rt.set_reactive(
        "SEQ",
        "track-duration-spans",
        build_all_track_duration_spans_value(state, app),
    );
    if let Some(profile) = profile.as_deref_mut() {
        profile.track_duration_spans = started.expect("profile timer").elapsed();
    }

    let started = profile.as_ref().map(|_| Instant::now());
    let plock_masks: Vec<[u64; MAX_STEPS / 64]> = (0..app.tracks.len())
        .map(|track| track_step_plock_mask(state, track, &app.graph.effect_descriptors))
        .collect();
    rt.set_reactive(
        "SEQ",
        "track-step-has-plocks",
        build_all_track_step_has_plocks_from_masks(&plock_masks),
    );
    rt.set_reactive(
        "SEQ",
        "track-step-plock-kinds",
        build_all_track_step_plock_kinds(state, app),
    );
    rt.set_reactive(
        "SEQ",
        "track-step-variant-r",
        build_all_track_step_variant_color_channel(state, app, 0),
    );
    rt.set_reactive(
        "SEQ",
        "track-step-variant-g",
        build_all_track_step_variant_color_channel(state, app, 1),
    );
    rt.set_reactive(
        "SEQ",
        "track-step-variant-b",
        build_all_track_step_variant_color_channel(state, app, 2),
    );
    if let Some(profile) = profile.as_deref_mut() {
        profile.track_step_has_plocks = started.expect("profile timer").elapsed();
    }

    let started = profile.as_ref().map(|_| Instant::now());
    rt.set_reactive(
        "SEQ",
        "track-playheads",
        build_all_track_playheads_value(state, app),
    );
    if let Some(profile) = profile.as_deref_mut() {
        profile.track_playheads = started.expect("profile timer").elapsed();
    }

    let started = profile.as_ref().map(|_| Instant::now());
    rt.set_reactive(
        "SEQ",
        "track-velocities",
        build_all_track_param_lists_value(state, app, StepParam::Velocity),
    );
    if let Some(profile) = profile.as_deref_mut() {
        profile.track_velocities = started.expect("profile timer").elapsed();
    }

    let started = profile.as_ref().map(|_| Instant::now());
    rt.set_reactive(
        "SEQ",
        "track-durations",
        build_all_track_param_lists_value(state, app, StepParam::Duration),
    );
    if let Some(profile) = profile.as_deref_mut() {
        profile.track_durations = started.expect("profile timer").elapsed();
    }

    let started = profile.as_ref().map(|_| Instant::now());
    rt.set_reactive(
        "SEQ",
        "track-auxas",
        build_all_track_param_lists_value(state, app, StepParam::AuxA),
    );
    if let Some(profile) = profile.as_deref_mut() {
        profile.track_auxas = started.expect("profile timer").elapsed();
    }

    let started = profile.as_ref().map(|_| Instant::now());
    rt.set_reactive(
        "SEQ",
        "track-transposes",
        build_all_track_param_lists_value(state, app, StepParam::Transpose),
    );
    rt.set_reactive("SEQ", "track-drum-racks", build_track_drum_racks_value(app));
    rt.set_reactive(
        "SEQ",
        "track-drum-sounds",
        build_all_track_drum_sounds_value(app),
    );
    sync_all_rack_slot_selection_binding_fields(rt, app);
    sync_all_drum_lane_step_binding_fields(rt, state, app);
    if let Some(profile) = profile.as_deref_mut() {
        profile.track_transposes = started.expect("profile timer").elapsed();
    }

    let started = profile.as_ref().map(|_| Instant::now());
    rt.set_reactive(
        "SEQ",
        "track-pans",
        build_all_track_param_lists_value(state, app, StepParam::Pan),
    );
    if let Some(profile) = profile.as_deref_mut() {
        profile.track_pans = started.expect("profile timer").elapsed();
    }

    let started = profile.as_ref().map(|_| Instant::now());
    rt.set_reactive(
        "SEQ",
        "track-syncs",
        build_all_track_param_lists_value(state, app, StepParam::Sync),
    );
    if let Some(profile) = profile.as_deref_mut() {
        profile.track_syncs = started.expect("profile timer").elapsed();
    }

    let started = profile.as_ref().map(|_| Instant::now());
    rt.set_reactive(
        "SEQ",
        "track-delays",
        build_all_track_param_lists_value(state, app, StepParam::Delay),
    );
    if let Some(profile) = profile.as_deref_mut() {
        profile.track_delays = started.expect("profile timer").elapsed();
    }
    rt.set_reactive(
        "SEQ",
        "track-process-lanes",
        build_all_track_process_lanes_value(state, app.tracks.len()),
    );

    if let Some(profile) = profile.as_deref_mut() {
        profile.step_bindings = sync_all_track_step_binding_fields_profiled(
            rt,
            state,
            app,
            current_track_idx,
            selected_steps,
            &plock_masks,
        );
    } else {
        sync_all_track_step_binding_fields(
            rt,
            state,
            app,
            current_track_idx,
            selected_steps,
            &plock_masks,
        );
    }

    let started = profile.as_ref().map(|_| Instant::now());
    sync_all_track_playhead_fields(rt, state, app);
    if let Some(profile) = profile.as_deref_mut() {
        profile.playhead_fields = started.expect("profile timer").elapsed();
        profile.elapsed = total_started.expect("profile timer").elapsed();
    }
}

/// Build a Lisp Value::List of floats for a given step param on a given track.
pub(crate) fn build_param_list(
    state: &Arc<SequencerState>,
    track: usize,
    param: StepParam,
) -> Value {
    let items: Vec<Rc<RefCell<Value>>> = (0..MAX_STEPS)
        .map(|s| {
            let val = state.pattern.step_data[track].get(s, param);
            Rc::new(RefCell::new(Value::Number(val as f64)))
        })
        .collect();
    Value::List(items)
}

pub(crate) fn fx_step_param_value_field(param: StepParam) -> Option<&'static str> {
    match param {
        StepParam::Velocity => Some("fx-step-value-velocity"),
        StepParam::Duration => Some("fx-step-value-duration"),
        StepParam::Transpose => Some("fx-step-value-transpose"),
        _ => None,
    }
}

pub(crate) fn fx_step_cursor_from_runtime(rt: &Runtime) -> usize {
    match rt.global_value("cursor-step") {
        Some(Value::Number(step)) if step >= 0.0 => step as usize,
        _ => 0,
    }
}

/// Refresh the fixed-size step-parameter strip without rerunning its Lisp
/// effect. Every consumer is a retained numeric binding, so cursor and
/// selection changes stay on the targeted widget path.
pub(crate) fn sync_fx_step_cursor_binding_fields(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    track: usize,
    cursor_step: usize,
    selected_step: Option<usize>,
    selected_count: usize,
) -> bool {
    if track >= state.active_track_count() {
        return false;
    }
    let num_steps = state.pattern.track_params[track]
        .get_num_steps()
        .max(1)
        .min(MAX_STEPS);
    let cursor_step = cursor_step.min(num_steps.saturating_sub(1));
    let parameter_step = selected_step
        .unwrap_or(cursor_step)
        .min(num_steps.saturating_sub(1));
    let mut dirty = rt
        .set_reactive(
            "SEQ",
            "fx-step-cursor-number",
            Value::Number((cursor_step + 1) as f64),
        )
        .effects_dirty;
    dirty |= rt
        .set_reactive(
            "SEQ",
            "fx-step-selection-count",
            Value::Number(selected_count as f64),
        )
        .effects_dirty;
    dirty |= rt
        .set_reactive(
            "SEQ",
            "fx-step-parameter-step",
            Value::Number(parameter_step as f64),
        )
        .effects_dirty;
    for param in [StepParam::Velocity, StepParam::Duration, StepParam::Transpose] {
        let field = fx_step_param_value_field(param)
            .expect("step parameter strip field should exist");
        dirty |= rt
            .set_reactive(
                "SEQ",
                field,
                Value::Number(state.pattern.step_data[track].get(parameter_step, param) as f64),
            )
            .effects_dirty;
    }
    dirty
}

pub(crate) fn sync_step_param_lists(rt: &mut Runtime, state: &Arc<SequencerState>, track: usize) {
    rt.set_reactive(
        "SEQ",
        "velocities",
        build_param_list(state, track, StepParam::Velocity),
    );
    rt.set_reactive(
        "SEQ",
        "durations",
        build_param_list(state, track, StepParam::Duration),
    );
    rt.set_reactive(
        "SEQ",
        "transposes",
        build_param_list(state, track, StepParam::Transpose),
    );
    rt.set_reactive(
        "SEQ",
        "auxas",
        build_param_list(state, track, StepParam::AuxA),
    );
    rt.set_reactive(
        "SEQ",
        "pans",
        build_param_list(state, track, StepParam::Pan),
    );
    rt.set_reactive(
        "SEQ",
        "syncs",
        build_param_list(state, track, StepParam::Sync),
    );
    rt.set_reactive(
        "SEQ",
        "delays",
        build_param_list(state, track, StepParam::Delay),
    );
    rt.set_reactive(
        "SEQ",
        "track-velocities",
        build_all_active_track_param_lists_value(state, StepParam::Velocity),
    );
    rt.set_reactive(
        "SEQ",
        "track-durations",
        build_all_active_track_param_lists_value(state, StepParam::Duration),
    );
    rt.set_reactive(
        "SEQ",
        "track-transposes",
        build_all_active_track_param_lists_value(state, StepParam::Transpose),
    );
    rt.set_reactive(
        "SEQ",
        "track-auxas",
        build_all_active_track_param_lists_value(state, StepParam::AuxA),
    );
    rt.set_reactive(
        "SEQ",
        "track-pans",
        build_all_active_track_param_lists_value(state, StepParam::Pan),
    );
    rt.set_reactive(
        "SEQ",
        "track-syncs",
        build_all_active_track_param_lists_value(state, StepParam::Sync),
    );
    rt.set_reactive(
        "SEQ",
        "track-delays",
        build_all_active_track_param_lists_value(state, StepParam::Delay),
    );
    sync_process_chain_state(rt, state, state.active_track_count(), track);
}

pub(crate) fn build_accumulator_names(app: &app::App) -> Vec<String> {
    let mut names = BUILTIN_ACCUMULATOR_NAMES
        .iter()
        .map(|name| (*name).to_string())
        .collect::<Vec<_>>();
    if let Some(runtime) = app.editor.scratch_runtime.as_ref() {
        names.extend(runtime.accumulator_names());
    }
    names
}

pub(crate) fn build_accumulator_options(app: &app::App) -> Value {
    let items = build_accumulator_names(app)
        .into_iter()
        .map(|name| Rc::new(RefCell::new(Value::String(name))))
        .collect();
    Value::List(items)
}

pub(crate) fn build_accum_mode_options() -> Value {
    let items = ACCUM_MODE_LABELS
        .iter()
        .map(|label| Rc::new(RefCell::new(Value::String((*label).to_string()))))
        .collect();
    Value::List(items)
}

pub(crate) fn build_fts_options() -> Value {
    let items = FTS_SCALE_NAMES
        .iter()
        .map(|scale| Rc::new(RefCell::new(Value::String((*scale).to_string()))))
        .collect();
    Value::List(items)
}

pub(crate) fn mute_group_label(group: u8) -> String {
    match group.min(8) {
        0 => "Off".to_string(),
        group => group.to_string(),
    }
}

pub(crate) fn build_mute_group_options() -> Value {
    let items = std::iter::once("Off".to_string())
        .chain((1..=8).map(|group| group.to_string()))
        .map(|label| Rc::new(RefCell::new(Value::String(label))))
        .collect();
    Value::List(items)
}

pub(crate) fn builtin_accumulator_default_limit(idx: usize) -> f32 {
    match idx {
        1 => 48.0,
        2 => 1.0,
        _ => 0.0,
    }
}

pub(crate) fn accum_mode_label(mode: u32) -> &'static str {
    ACCUM_MODE_LABELS
        .get(mode as usize)
        .copied()
        .unwrap_or(ACCUM_MODE_LABELS[0])
}

pub(crate) fn selected_accumulator_name(app: &app::App, track: usize) -> String {
    let tp = &app.state.pattern.track_params[track];
    if let Some(name) = tp.script_accumulator_name() {
        return name;
    }
    build_accumulator_names(app)
        .get(tp.get_accumulator_idx())
        .cloned()
        .unwrap_or_else(|| "Off".to_string())
}

/// Build a Lisp Value::List of bools for record-armed state per track.
pub(crate) fn build_record_armed_value(armed: &[bool]) -> Value {
    let items: Vec<Rc<RefCell<Value>>> = armed
        .iter()
        .map(|a| Rc::new(RefCell::new(Value::Bool(*a))))
        .collect();
    Value::List(items)
}

/// Build a Lisp Value::List of track name strings.
pub(crate) fn build_track_names(names: &[String]) -> Value {
    let items: Vec<Rc<RefCell<Value>>> = names
        .iter()
        .map(|name| Rc::new(RefCell::new(Value::String(name.clone()))))
        .collect();
    Value::List(items)
}

pub(crate) fn build_track_colors(app: &app::App) -> Value {
    let items: Vec<Rc<RefCell<Value>>> = (0..app.tracks.len())
        .map(|track| {
            let color = app
                .track_colors
                .get(track)
                .copied()
                .unwrap_or_else(|| sequencer::track_color::TrackColor::palette_color(track))
                .clamped();
            Rc::new(RefCell::new(Value::List(vec![
                Rc::new(RefCell::new(Value::Number(color.r as f64))),
                Rc::new(RefCell::new(Value::Number(color.g as f64))),
                Rc::new(RefCell::new(Value::Number(color.b as f64))),
            ])))
        })
        .collect();
    Value::List(items)
}

pub(crate) fn build_track_collapsed_from_slice(collapsed: &[bool], track_count: usize) -> Value {
    Value::List(
        (0..track_count)
            .map(|track| {
                Rc::new(RefCell::new(Value::Bool(
                    collapsed.get(track).copied().unwrap_or(false),
                )))
            })
            .collect(),
    )
}

pub(crate) fn build_track_collapsed(app: &app::App) -> Value {
    build_track_collapsed_from_slice(&app.track_collapsed, app.tracks.len())
}

pub(crate) fn build_track_ids(app: &app::App) -> Value {
    let items: Vec<Rc<RefCell<Value>>> = app
        .graph
        .track_node_ids
        .iter()
        .map(|ids| Rc::new(RefCell::new(Value::Number(ids.pan_id as f64))))
        .collect();
    Value::List(items)
}

pub(crate) fn build_track_instrument_types(app: &app::App) -> Value {
    let items: Vec<Rc<RefCell<Value>>> = app
        .graph
        .track_instrument_types
        .iter()
        .map(|instrument_type| {
            let label = match instrument_type {
                sequencer::sequencer::InstrumentType::Sampler => "sampler",
                sequencer::sequencer::InstrumentType::Custom => "custom",
                sequencer::sequencer::InstrumentType::Modulator => "modulator",
                sequencer::sequencer::InstrumentType::Rack => "rack",
            };
            Rc::new(RefCell::new(Value::String(label.to_string())))
        })
        .collect();
    Value::List(items)
}

pub(crate) fn build_track_mod_output_available(app: &app::App) -> Value {
    let items: Vec<Rc<RefCell<Value>>> = (0..app.graph.track_instrument_types.len())
        .map(|track| {
            Rc::new(RefCell::new(Value::Bool(
                app.graph.track_exposes_mod_output(track),
            )))
        })
        .collect();
    Value::List(items)
}

pub(crate) fn build_track_instrument_run_modes(app: &app::App) -> Value {
    let items: Vec<Rc<RefCell<Value>>> = app
        .graph
        .track_instrument_run_modes
        .iter()
        .map(|run_mode| {
            let label = match run_mode {
                sequencer::sequencer::CustomInstrumentRunMode::Instrument => "instrument",
                sequencer::sequencer::CustomInstrumentRunMode::FreePatch => "free_patch",
            };
            Rc::new(RefCell::new(Value::String(label.to_string())))
        })
        .collect();
    Value::List(items)
}

pub(crate) fn sync_track_name_state(
    rt: &mut Runtime,
    track_names: &mut Vec<String>,
    app: &app::App,
) {
    rt.set_reactive("SEQ", "track-ids", build_track_ids(app));
    rt.set_reactive(
        "SEQ",
        "track-instrument-types",
        build_track_instrument_types(app),
    );
    rt.set_reactive("SEQ", "track-drum-racks", build_track_drum_racks_value(app));
    rt.set_reactive(
        "SEQ",
        "track-drum-sounds",
        build_all_track_drum_sounds_value(app),
    );
    sync_all_rack_slot_selection_binding_fields(rt, app);
    rt.set_reactive(
        "SEQ",
        "track-mod-output-available",
        build_track_mod_output_available(app),
    );
    rt.set_reactive(
        "SEQ",
        "track-instrument-run-modes",
        build_track_instrument_run_modes(app),
    );
    if *track_names != app.tracks {
        *track_names = app.tracks.clone();
    }
    rt.set_reactive("SEQ", "num-tracks", Value::Number(track_names.len() as f64));
    rt.set_reactive("SEQ", "track-names", build_track_names(track_names));
    rt.set_reactive("SEQ", "track-colors", build_track_colors(app));
    rt.set_reactive("SEQ", "track-collapsed", build_track_collapsed(app));
}

/// Build a Lisp Value::List of per-track volumes (0.0–1.0).
pub(crate) fn build_track_volumes(state: &Arc<SequencerState>) -> Value {
    let count = state.active_track_count();
    let items: Vec<Rc<RefCell<Value>>> = (0..count)
        .map(|t| {
            let vol = state.pattern.track_params[t].get_volume();
            Rc::new(RefCell::new(Value::Number(vol as f64)))
        })
        .collect();
    Value::List(items)
}

pub(crate) fn build_track_pans(state: &Arc<SequencerState>) -> Value {
    let count = state.active_track_count();
    let items: Vec<Rc<RefCell<Value>>> = (0..count)
        .map(|t| {
            Rc::new(RefCell::new(Value::Number(
                state.pattern.track_params[t].get_pan() as f64,
            )))
        })
        .collect();
    Value::List(items)
}

pub(crate) fn track_volume_field(track: usize) -> String {
    format!("track-{track}-volume")
}

pub(crate) fn track_pan_field(track: usize) -> String {
    format!("track-{track}-pan")
}

pub(crate) fn sync_track_volume_binding_field(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    track: usize,
) {
    if let Some(tp) = state.pattern.track_params.get(track) {
        rt.set_reactive(
            "SEQ",
            &track_volume_field(track),
            Value::Number(tp.get_volume() as f64),
        );
    }
}

pub(crate) fn sync_track_pan_binding_field(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    track: usize,
) {
    if let Some(tp) = state.pattern.track_params.get(track) {
        rt.set_reactive(
            "SEQ",
            &track_pan_field(track),
            Value::Number(tp.get_pan() as f64),
        );
    }
}

pub(crate) fn sync_track_volume_pan_binding_fields(rt: &mut Runtime, state: &Arc<SequencerState>) {
    for track in 0..state.active_track_count() {
        sync_track_volume_binding_field(rt, state, track);
        sync_track_pan_binding_field(rt, state, track);
    }
}

pub(crate) fn build_track_outputs(app: &app::App, state: &Arc<SequencerState>) -> Value {
    let count = state.active_track_count();
    let items: Vec<Rc<RefCell<Value>>> = (0..count)
        .map(|t| {
            Rc::new(RefCell::new(build_track_output_label(
                app,
                &state.pattern.track_params[t],
            )))
        })
        .collect();
    Value::List(items)
}

pub(crate) fn build_all_track_bus_sends(app: &app::App, state: &Arc<SequencerState>) -> Value {
    let count = state.active_track_count();
    let items: Vec<Rc<RefCell<Value>>> = (0..count)
        .map(|t| {
            Rc::new(RefCell::new(build_track_bus_sends(
                app,
                &state.pattern.track_params[t],
            )))
        })
        .collect();
    Value::List(items)
}

pub(crate) fn track_bus_send_field(track: usize, bus_idx: usize) -> String {
    format!("track-{track}-bus-{bus_idx}-send")
}

pub(crate) fn current_track_bus_send_field(bus_idx: usize) -> String {
    format!("tp-bus-{bus_idx}-send")
}

pub(crate) fn track_bus_send_amount(
    app: &app::App,
    state: &Arc<SequencerState>,
    track: usize,
    bus_idx: usize,
) -> Option<f32> {
    let bus = app.buses.get(bus_idx)?;
    if bus.id == sequencer::sequencer::BusId::MIX {
        return None;
    }
    let tp = state.pattern.track_params.get(track)?;
    Some(
        tp.sends()
            .iter()
            .find(|send| send.destination == bus.id)
            .map(|send| send.amount)
            .unwrap_or(0.0),
    )
}

pub(crate) fn sync_track_bus_send_binding_field(
    rt: &mut Runtime,
    app: &app::App,
    state: &Arc<SequencerState>,
    track: usize,
    bus_idx: usize,
) {
    if let Some(amount) = track_bus_send_amount(app, state, track, bus_idx) {
        rt.set_reactive(
            "SEQ",
            &track_bus_send_field(track, bus_idx),
            Value::Number(amount as f64),
        );
    }
}

pub(crate) fn sync_track_bus_send_binding_fields(
    rt: &mut Runtime,
    app: &app::App,
    state: &Arc<SequencerState>,
) {
    for track in 0..state.active_track_count() {
        for (bus_idx, bus) in app.buses.iter().enumerate() {
            if bus.id != sequencer::sequencer::BusId::MIX {
                sync_track_bus_send_binding_field(rt, app, state, track, bus_idx);
            }
        }
    }
}

pub(crate) fn sync_current_track_bus_send_binding_field(
    rt: &mut Runtime,
    app: &app::App,
    state: &Arc<SequencerState>,
    track: usize,
    bus_idx: usize,
) {
    if let Some(amount) = track_bus_send_amount(app, state, track, bus_idx) {
        rt.set_reactive(
            "SEQ",
            &current_track_bus_send_field(bus_idx),
            Value::Number(amount as f64),
        );
    }
}

pub(crate) fn sync_current_track_bus_send_binding_fields(
    rt: &mut Runtime,
    app: &app::App,
    state: &Arc<SequencerState>,
    track: usize,
) {
    for (bus_idx, bus) in app.buses.iter().enumerate() {
        if bus.id != sequencer::sequencer::BusId::MIX {
            sync_current_track_bus_send_binding_field(rt, app, state, track, bus_idx);
        }
    }
}

pub(crate) fn build_mod_routes(state: &Arc<SequencerState>) -> Value {
    let routes = state.current_mod_connections();
    Value::List(
        routes
            .into_iter()
            .map(|connection| {
                let mut map = std::collections::HashMap::new();
                map.insert(
                    "source".to_string(),
                    Rc::new(RefCell::new(Value::Number(connection.source_track as f64))),
                );
                map.insert(
                    "dest-kind".to_string(),
                    Rc::new(RefCell::new(mod_destination_kind_value(
                        connection.destination,
                    ))),
                );
                map.insert(
                    "dest".to_string(),
                    Rc::new(RefCell::new(mod_destination_id_value(
                        connection.destination,
                    ))),
                );
                map.insert(
                    "input".to_string(),
                    Rc::new(RefCell::new(Value::Number(connection.dest_input as f64))),
                );
                Rc::new(RefCell::new(Value::Map(map)))
            })
            .collect(),
    )
}

pub(crate) fn build_track_mutes(state: &Arc<SequencerState>) -> Value {
    let count = state.active_track_count();
    let items: Vec<Rc<RefCell<Value>>> = (0..count)
        .map(|t| {
            Rc::new(RefCell::new(Value::Bool(
                state.pattern.track_params[t].is_muted(),
            )))
        })
        .collect();
    Value::List(items)
}

pub(crate) fn build_track_solos(state: &Arc<SequencerState>) -> Value {
    let count = state.active_track_count();
    let items: Vec<Rc<RefCell<Value>>> = (0..count)
        .map(|t| {
            Rc::new(RefCell::new(Value::Bool(
                state.pattern.track_params[t].is_solo(),
            )))
        })
        .collect();
    Value::List(items)
}

pub(crate) fn build_track_muted_by_solo(state: &Arc<SequencerState>) -> Value {
    let count = state.active_track_count();
    let has_solo = any_track_solo(state);
    let items: Vec<Rc<RefCell<Value>>> = (0..count)
        .map(|t| {
            Rc::new(RefCell::new(Value::Bool(track_muted_by_solo(
                state, t, has_solo,
            ))))
        })
        .collect();
    Value::List(items)
}

fn track_effectively_muted(state: &Arc<SequencerState>, track: usize, has_solo: bool) -> bool {
    let params = &state.pattern.track_params[track];
    params.is_muted() || (has_solo && !params.is_solo())
}

fn any_track_solo(state: &Arc<SequencerState>) -> bool {
    (0..state.active_track_count()).any(|t| state.pattern.track_params[t].is_solo())
}

fn track_muted_by_solo(state: &Arc<SequencerState>, track: usize, has_solo: bool) -> bool {
    has_solo && !state.pattern.track_params[track].is_solo()
}

/// Per-track "effectively muted" (explicit mute OR muted by another track's
/// solo) as 0/1 numbers, for widget bindings via `bind-seq-nth`. Lets row
/// `:muted` props update without rerunning the row's subtree.
pub(crate) fn build_track_muted_effective(state: &Arc<SequencerState>) -> Value {
    let count = state.active_track_count();
    let has_solo = any_track_solo(state);
    let items: Vec<Rc<RefCell<Value>>> = (0..count)
        .map(|t| {
            let muted = track_effectively_muted(state, t, has_solo);
            Rc::new(RefCell::new(Value::Number(if muted { 1.0 } else { 0.0 })))
        })
        .collect();
    Value::List(items)
}

/// Per-track step-cell color channel with the mute dim baked in, matching the
/// Lisp seqv-track-color-r/g/b formulas. Published as flat per-channel lists
/// so step-cell shader props can use `bind-seq-nth` instead of reading
/// SEQ.track-mutes/track-colors in the row subtree.
pub(crate) fn build_track_color_channel_effective(
    app: &app::App,
    state: &Arc<SequencerState>,
    channel: usize,
) -> Value {
    let count = state.active_track_count();
    let has_solo = any_track_solo(state);
    let items: Vec<Rc<RefCell<Value>>> = (0..count)
        .map(|track| {
            let muted = track_effectively_muted(state, track, has_solo);
            let value = track_color_channel_effective_value(app, track, channel, muted);
            Rc::new(RefCell::new(Value::Number(value)))
        })
        .collect();
    Value::List(items)
}

fn track_color_channel_effective_value(
    app: &app::App,
    track: usize,
    channel: usize,
    muted: bool,
) -> f64 {
    let color = app
        .track_colors
        .get(track)
        .copied()
        .unwrap_or_else(|| sequencer::track_color::TrackColor::palette_color(track))
        .clamped();
    let (raw, dim_base) = match channel {
        0 => (color.r as f64, 0.10),
        1 => (color.g as f64, 0.10),
        _ => (color.b as f64, 0.11),
    };
    if muted {
        raw * 0.34 + dim_base * 0.66
    } else {
        raw
    }
}

fn track_color_channel_effective_field(channel: usize) -> &'static str {
    match channel {
        0 => "track-color-r-effective",
        1 => "track-color-g-effective",
        _ => "track-color-b-effective",
    }
}

pub(crate) fn sync_track_mute_visual_binding_fields(
    rt: &mut Runtime,
    app: &app::App,
    state: &Arc<SequencerState>,
    tracks: impl IntoIterator<Item = usize>,
    sync_muted_by_solo: bool,
) -> bool {
    let count = state.active_track_count();
    let has_solo = any_track_solo(state);
    let mut effects_dirty = false;

    for track in tracks {
        if track >= count {
            continue;
        }
        if sync_muted_by_solo {
            effects_dirty |= rt
                .set_reactive_list_index(
                    "SEQ",
                    "track-muted-by-solo",
                    track,
                    Value::Bool(track_muted_by_solo(state, track, has_solo)),
                )
                .effects_dirty;
        }

        let muted = track_effectively_muted(state, track, has_solo);
        effects_dirty |= rt
            .set_reactive_list_index(
                "SEQ",
                "track-muted-effective",
                track,
                Value::Number(if muted { 1.0 } else { 0.0 }),
            )
            .effects_dirty;

        for channel in 0..3 {
            effects_dirty |= rt
                .set_reactive_list_index(
                    "SEQ",
                    track_color_channel_effective_field(channel),
                    track,
                    Value::Number(track_color_channel_effective_value(
                        app, track, channel, muted,
                    )),
                )
                .effects_dirty;
        }
    }

    effects_dirty
}

pub(crate) fn sync_track_mixer_state(
    rt: &mut Runtime,
    app: &app::App,
    state: &Arc<SequencerState>,
) {
    rt.set_reactive("SEQ", "track-colors", build_track_colors(app));
    rt.set_reactive("SEQ", "track-collapsed", build_track_collapsed(app));
    rt.set_reactive(
        "SEQ",
        "track-pattern-cells",
        build_track_pattern_cells_value(state, app.tracks.len()),
    );
    sync_track_pattern_cell_state_fields(rt, state, app.tracks.len());
    rt.set_reactive(
        "SEQ",
        "track-instrument-types",
        build_track_instrument_types(app),
    );
    rt.set_reactive("SEQ", "track-drum-racks", build_track_drum_racks_value(app));
    rt.set_reactive(
        "SEQ",
        "track-drum-sounds",
        build_all_track_drum_sounds_value(app),
    );
    sync_all_rack_slot_selection_binding_fields(rt, app);
    sync_all_drum_lane_step_binding_fields(rt, state, app);
    rt.set_reactive(
        "SEQ",
        "track-mod-output-available",
        build_track_mod_output_available(app),
    );
    rt.set_reactive(
        "SEQ",
        "track-instrument-run-modes",
        build_track_instrument_run_modes(app),
    );
    rt.set_reactive("SEQ", "track-volumes", build_track_volumes(state));
    rt.set_reactive("SEQ", "track-mixer-pans", build_track_pans(state));
    sync_track_volume_pan_binding_fields(rt, state);
    rt.set_reactive("SEQ", "track-outputs", build_track_outputs(app, state));
    rt.set_reactive(
        "SEQ",
        "track-bus-sends",
        build_all_track_bus_sends(app, state),
    );
    sync_track_bus_send_binding_fields(rt, app, state);
    rt.set_reactive("SEQ", "mod-routes", build_mod_routes(state));
    rt.set_reactive("SEQ", "track-mutes", build_track_mutes(state));
    rt.set_reactive("SEQ", "track-solos", build_track_solos(state));
    rt.set_reactive(
        "SEQ",
        "track-muted-by-solo",
        build_track_muted_by_solo(state),
    );
    rt.set_reactive(
        "SEQ",
        "track-muted-effective",
        build_track_muted_effective(state),
    );
    rt.set_reactive(
        "SEQ",
        "track-color-r-effective",
        build_track_color_channel_effective(app, state, 0),
    );
    rt.set_reactive(
        "SEQ",
        "track-color-g-effective",
        build_track_color_channel_effective(app, state, 1),
    );
    rt.set_reactive(
        "SEQ",
        "track-color-b-effective",
        build_track_color_channel_effective(app, state, 2),
    );
}

pub(crate) fn sync_bus_mixer_control_state(rt: &mut Runtime, app: &app::App) {
    let names: Vec<String> = app.buses.iter().map(|bus| bus.name.clone()).collect();
    let volumes: Vec<Rc<RefCell<Value>>> = app
        .buses
        .iter()
        .map(|bus| Rc::new(RefCell::new(Value::Number(bus.volume as f64))))
        .collect();
    let mutes: Vec<Rc<RefCell<Value>>> = app
        .buses
        .iter()
        .map(|bus| Rc::new(RefCell::new(Value::Bool(bus.mute))))
        .collect();
    let solos: Vec<Rc<RefCell<Value>>> = app
        .buses
        .iter()
        .map(|bus| Rc::new(RefCell::new(Value::Bool(bus.solo))))
        .collect();
    let ids: Vec<Rc<RefCell<Value>>> = app
        .buses
        .iter()
        .map(|bus| Rc::new(RefCell::new(Value::Number(bus.id.0 as f64))))
        .collect();
    rt.set_reactive("SEQ", "bus-ids", Value::List(ids));
    rt.set_reactive("SEQ", "bus-names", build_track_names(&names));
    rt.set_reactive("SEQ", "bus-volumes", Value::List(volumes));
    rt.set_reactive("SEQ", "bus-mutes", Value::List(mutes));
    rt.set_reactive("SEQ", "bus-solos", Value::List(solos));
}

pub(crate) fn sync_bus_mixer_state(rt: &mut Runtime, app: &app::App) {
    sync_bus_mixer_control_state(rt, app);
    rt.set_reactive("SEQ", "bus-effects", build_bus_effects_value(app));
    rt.set_reactive("SEQ", "bus-steps", build_bus_steps_value(app));
    rt.set_reactive(
        "SEQ",
        "bus-velocities",
        build_bus_param_lists(app, "velocity"),
    );
    rt.set_reactive(
        "SEQ",
        "bus-durations",
        build_bus_param_lists(app, "duration"),
    );
    rt.set_reactive("SEQ", "bus-syncs", build_bus_param_lists(app, "sync"));
    rt.set_reactive("SEQ", "bus-num-steps", build_bus_num_steps_value(app));
    rt.set_reactive("SEQ", "bus-timebases", build_bus_timebase_value(app));
    rt.set_reactive("SEQ", "bus-swings", build_bus_swing_value(app));
    rt.set_reactive(
        "SEQ",
        "bus-swing-resolutions",
        build_bus_swing_resolution_value(app),
    );
    rt.set_reactive("SEQ", "bus-step-has-plocks", build_bus_step_has_plocks(app));
    rt.set_reactive("SEQ", "bus-playheads", build_bus_playheads_value(app));
}

pub(crate) fn sync_track_mixer_empty_state(rt: &mut Runtime) {
    rt.set_reactive("SEQ", "track-volumes", Value::List(vec![]));
    rt.set_reactive("SEQ", "track-mixer-pans", Value::List(vec![]));
    rt.set_reactive("SEQ", "track-outputs", Value::List(vec![]));
    rt.set_reactive("SEQ", "track-colors", Value::List(vec![]));
    rt.set_reactive("SEQ", "track-collapsed", Value::List(vec![]));
    rt.set_reactive("SEQ", "track-pattern-cells", Value::List(vec![]));
    rt.set_reactive("SEQ", "track-instrument-types", Value::List(vec![]));
    rt.set_reactive("SEQ", "track-drum-racks", Value::List(vec![]));
    rt.set_reactive("SEQ", "track-drum-sounds", Value::List(vec![]));
    rt.set_reactive("SEQ", "track-mod-output-available", Value::List(vec![]));
    rt.set_reactive("SEQ", "track-bus-sends", Value::List(vec![]));
    rt.set_reactive("SEQ", "track-mutes", Value::List(vec![]));
    rt.set_reactive("SEQ", "track-solos", Value::List(vec![]));
    rt.set_reactive("SEQ", "track-muted-by-solo", Value::List(vec![]));
    rt.set_reactive("SEQ", "track-muted-effective", Value::List(vec![]));
    rt.set_reactive("SEQ", "track-color-r-effective", Value::List(vec![]));
    rt.set_reactive("SEQ", "track-color-g-effective", Value::List(vec![]));
    rt.set_reactive("SEQ", "track-color-b-effective", Value::List(vec![]));
    rt.set_reactive("SEQ", "bus-names", Value::List(vec![]));
    rt.set_reactive("SEQ", "bus-volumes", Value::List(vec![]));
    rt.set_reactive("SEQ", "bus-mutes", Value::List(vec![]));
    rt.set_reactive("SEQ", "bus-solos", Value::List(vec![]));
    rt.set_reactive("SEQ", "bus-steps", Value::List(vec![]));
    rt.set_reactive("SEQ", "bus-velocities", Value::List(vec![]));
    rt.set_reactive("SEQ", "bus-durations", Value::List(vec![]));
    rt.set_reactive("SEQ", "bus-syncs", Value::List(vec![]));
    rt.set_reactive("SEQ", "bus-num-steps", Value::List(vec![]));
    rt.set_reactive("SEQ", "bus-timebases", Value::List(vec![]));
    rt.set_reactive("SEQ", "bus-swings", Value::List(vec![]));
    rt.set_reactive("SEQ", "bus-swing-resolutions", Value::List(vec![]));
    rt.set_reactive("SEQ", "bus-step-has-plocks", Value::List(vec![]));
    rt.set_reactive("SEQ", "bus-playheads", Value::List(vec![]));
}

pub(crate) fn build_bus_playheads_value(app: &app::App) -> Value {
    Value::List(
        bus_playhead_snapshot(app)
            .into_iter()
            .map(|step| Rc::new(RefCell::new(Value::Number(step as f64))))
            .collect(),
    )
}

pub(crate) fn bus_playhead_snapshot(app: &app::App) -> Vec<usize> {
    let playheads = app.graph.bus_gate_playheads.lock().unwrap();
    app.buses
        .iter()
        .map(|bus| {
            playheads
                .iter()
                .find(|(id, _)| *id == bus.id)
                .map(|(_, step)| *step)
                .unwrap_or(0)
        })
        .collect()
}

pub(crate) fn build_bus_steps_value(app: &app::App) -> Value {
    Value::List(
        app.buses
            .iter()
            .map(|bus| {
                Rc::new(RefCell::new(Value::List(
                    bus.gate_sequence
                        .steps
                        .iter()
                        .map(|active| Rc::new(RefCell::new(Value::Bool(*active))))
                        .collect(),
                )))
            })
            .collect(),
    )
}

pub(crate) fn build_bus_param_lists(app: &app::App, param: &str) -> Value {
    Value::List(
        app.buses
            .iter()
            .map(|bus| {
                let values = match param {
                    "duration" => &bus.gate_sequence.durations,
                    "sync" => &bus.gate_sequence.syncs,
                    _ => &bus.gate_sequence.velocities,
                };
                Rc::new(RefCell::new(Value::List(
                    values
                        .iter()
                        .map(|value| Rc::new(RefCell::new(Value::Number(*value as f64))))
                        .collect(),
                )))
            })
            .collect(),
    )
}

pub(crate) fn build_bus_num_steps_value(app: &app::App) -> Value {
    Value::List(
        app.buses
            .iter()
            .map(|bus| {
                Rc::new(RefCell::new(Value::Number(
                    bus.gate_sequence.num_steps as f64,
                )))
            })
            .collect(),
    )
}

pub(crate) fn build_bus_timebase_value(app: &app::App) -> Value {
    Value::List(
        app.buses
            .iter()
            .map(|bus| {
                Rc::new(RefCell::new(Value::String(
                    bus.gate_sequence.timebase.label().to_string(),
                )))
            })
            .collect(),
    )
}

pub(crate) fn build_bus_swing_value(app: &app::App) -> Value {
    Value::List(
        app.buses
            .iter()
            .map(|bus| Rc::new(RefCell::new(Value::Number(bus.gate_sequence.swing as f64))))
            .collect(),
    )
}

pub(crate) fn build_bus_swing_resolution_value(app: &app::App) -> Value {
    Value::List(
        app.buses
            .iter()
            .map(|bus| {
                Rc::new(RefCell::new(Value::String(
                    bus.gate_sequence.swing_resolution.label().to_string(),
                )))
            })
            .collect(),
    )
}

pub(crate) fn build_bus_step_has_plocks(app: &app::App) -> Value {
    Value::List(
        app.buses
            .iter()
            .map(|bus| {
                Rc::new(RefCell::new(Value::List(
                    (0..MAX_STEPS)
                        .map(|step| {
                            let effect_has_plock = bus.effect_slots.iter().any(|slot| {
                                slot.plocks
                                    .get(step)
                                    .map(|params| params.iter().any(Option::is_some))
                                    .unwrap_or(false)
                            });
                            Rc::new(RefCell::new(Value::Bool(
                                bus.gate_sequence.has_step_plock(step) || effect_has_plock,
                            )))
                        })
                        .collect(),
                )))
            })
            .collect(),
    )
}

pub(crate) fn push_panner_bool(
    lg_raw: *mut sequencer::audiograph::LiveGraph,
    pan_id: i32,
    param_idx: u64,
    value: bool,
) {
    if pan_id < 0 {
        return;
    }
    unsafe {
        sequencer::audiograph::params_push_wrapper(
            lg_raw,
            sequencer::audiograph::ParamMsg {
                idx: param_idx,
                logical_id: pan_id as u64,
                fvalue: if value { 1.0 } else { 0.0 },
            },
        );
    }
}

pub(crate) fn push_solo_mutes(
    lg_raw: *mut sequencer::audiograph::LiveGraph,
    state: &Arc<SequencerState>,
    pan_ids: &[i32],
) {
    let count = state.active_track_count();
    let has_solo = (0..count).any(|track| state.pattern.track_params[track].is_solo());
    for track in 0..count {
        let muted_by_solo = has_solo && !state.pattern.track_params[track].is_solo();
        if let Some(&pan_id) = pan_ids.get(track) {
            push_panner_bool(
                lg_raw,
                pan_id,
                sequencer::effects::stereo_panner::STEREO_PANNER_PARAM_MUTED_BY_SOLO,
                muted_by_solo,
            );
        }
    }
}

fn read_panner_peak_levels(lg: sequencer::audiograph::LiveGraphPtr, node_ids: &[i32]) -> Vec<f64> {
    node_ids
        .iter()
        .map(|&node_id| read_panner_peak_level(lg, node_id))
        .collect()
}

fn read_panner_peak_level(lg: sequencer::audiograph::LiveGraphPtr, node_id: i32) -> f64 {
    const PANNER_STATE_LEN: usize = sequencer::effects::stereo_panner::STEREO_PANNER_STATE_SIZE;
    const PANNER_STATE_BYTES: usize = PANNER_STATE_LEN * std::mem::size_of::<f32>();
    if node_id < 0 {
        return 0.0;
    }
    let mut state_size = 0usize;
    let mut state = [0.0_f32; PANNER_STATE_LEN];
    let copied = unsafe {
        sequencer::audiograph::get_node_state_into(
            lg.0,
            node_id,
            state.as_mut_ptr().cast(),
            PANNER_STATE_BYTES,
            &mut state_size as *mut usize,
        )
    };
    if !copied || state_size < PANNER_STATE_BYTES {
        return 0.0;
    }
    let peak = state[sequencer::effects::stereo_panner::STATE_PEAK_L]
        .max(state[sequencer::effects::stereo_panner::STATE_PEAK_R]);
    meter_display_level(peak)
}

pub(crate) fn read_track_peak_levels(
    lg: sequencer::audiograph::LiveGraphPtr,
    pan_ids: &[i32],
) -> Vec<f64> {
    read_panner_peak_levels(lg, pan_ids)
}

pub(crate) fn rack_slot_peak_field(track: usize, slot_idx: usize) -> String {
    format!("rack-slot-peak-{track}-{slot_idx}")
}

/// Per-track, per-slot peak levels read from each rack slot's panner node.
/// Tracks without a rack yield an empty inner vec.
pub(crate) fn read_rack_slot_peak_levels(
    lg: sequencer::audiograph::LiveGraphPtr,
    app: &app::App,
) -> Vec<Vec<f64>> {
    app.graph
        .track_node_ids
        .iter()
        .map(|track_nodes| {
            track_nodes
                .rack_slots
                .iter()
                .map(|nodes| read_panner_peak_level(lg, nodes.slot_pan_id))
                .collect()
        })
        .collect()
}

pub(crate) fn sync_rack_slot_peak_field_delta(
    rt: &mut Runtime,
    previous: &[Vec<f64>],
    levels: &[Vec<f64>],
) -> bool {
    let mut effects_dirty = false;
    for (track, slots) in levels.iter().enumerate() {
        let prev_slots = previous.get(track);
        for (slot_idx, &level) in slots.iter().enumerate() {
            let changed = prev_slots
                .and_then(|prev| prev.get(slot_idx))
                .is_none_or(|&old_level| old_level != level);
            if changed {
                effects_dirty |= rt
                    .set_reactive(
                        "SEQ",
                        &rack_slot_peak_field(track, slot_idx),
                        Value::Number(level),
                    )
                    .effects_dirty;
            }
        }
        // Zero out slots that disappeared so stale meters don't stick.
        if let Some(prev) = prev_slots {
            for slot_idx in slots.len()..prev.len() {
                effects_dirty |= rt
                    .set_reactive(
                        "SEQ",
                        &rack_slot_peak_field(track, slot_idx),
                        Value::Number(0.0),
                    )
                    .effects_dirty;
            }
        }
    }
    effects_dirty
}

pub(crate) fn read_bus_peak_levels(
    lg: sequencer::audiograph::LiveGraphPtr,
    bus_nodes: &[app::BusNodeIds],
) -> Vec<f64> {
    bus_nodes
        .iter()
        .map(|bus| {
            read_panner_peak_level(lg, bus.merge_id)
                .max(read_panner_peak_level(lg, bus.gate_id))
                .max(read_panner_peak_level(lg, bus.volume_id))
        })
        .collect()
}

pub(crate) fn modulator_phase_field(track: usize) -> String {
    format!("modulator-phase-{track}")
}

pub(crate) fn modulator_level_field(track: usize) -> String {
    format!("modulator-level-{track}")
}

pub(crate) fn read_modulator_display_values(
    lg: sequencer::audiograph::LiveGraphPtr,
    app: &app::App,
) -> (Vec<f64>, Vec<f64>) {
    let mut phases = Vec::with_capacity(app.graph.track_node_ids.len());
    let mut levels = Vec::with_capacity(app.graph.track_node_ids.len());
    for (nodes, instrument_type) in app
        .graph
        .track_node_ids
        .iter()
        .zip(app.graph.track_instrument_types.iter())
    {
        if *instrument_type == sequencer::sequencer::InstrumentType::Modulator {
            let (phase, level) = read_modulator_display_value(lg, nodes.mod_env_id);
            phases.push(phase);
            levels.push(level);
        } else {
            phases.push(0.0);
            levels.push(0.0);
        }
    }
    (phases, levels)
}

fn read_modulator_display_value(
    lg: sequencer::audiograph::LiveGraphPtr,
    node_id: i32,
) -> (f64, f64) {
    const STATE_LEN: usize = sequencer::track_modulator::MODULATOR_ENVELOPE_STATE_SIZE;
    const STATE_BYTES: usize = STATE_LEN * std::mem::size_of::<f32>();
    if node_id < 0 {
        return (0.0, 0.0);
    }
    let mut state_size = 0usize;
    let mut state = [0.0_f32; STATE_LEN];
    let copied = unsafe {
        sequencer::audiograph::get_node_state_into(
            lg.0,
            node_id,
            state.as_mut_ptr().cast(),
            STATE_BYTES,
            &mut state_size as *mut usize,
        )
    };
    if !copied || state_size < STATE_BYTES {
        return (0.0, 0.0);
    }
    decode_modulator_display_state(&state)
}

fn decode_modulator_display_state(state: &[f32]) -> (f64, f64) {
    let phase = state
        .get(sequencer::track_modulator::STATE_DISPLAY_PHASE)
        .copied()
        .unwrap_or(0.0);
    let level = state
        .get(sequencer::track_modulator::STATE_VALUE)
        .copied()
        .unwrap_or(0.0);
    (
        quantize_modulator_unit_value(phase),
        quantize_modulator_unit_value(level),
    )
}

fn quantize_modulator_unit_value(value: f32) -> f64 {
    ((value.clamp(0.0, 1.0) * 128.0).round() / 128.0) as f64
}

pub(crate) fn build_track_peaks_value(levels: &[f64]) -> Value {
    let items: Vec<Rc<RefCell<Value>>> = levels
        .iter()
        .map(|&level| Rc::new(RefCell::new(Value::Number(level))))
        .collect();
    Value::List(items)
}

pub(crate) fn sync_modulator_phase_fields(rt: &mut Runtime, phases: &[f64]) -> bool {
    let mut effects_dirty = false;
    for (idx, &phase) in phases.iter().enumerate() {
        effects_dirty |= rt
            .set_reactive("SEQ", &modulator_phase_field(idx), Value::Number(phase))
            .effects_dirty;
    }
    effects_dirty
}

pub(crate) fn sync_modulator_level_fields(rt: &mut Runtime, levels: &[f64]) -> bool {
    let mut effects_dirty = false;
    for (idx, &level) in levels.iter().enumerate() {
        effects_dirty |= rt
            .set_reactive("SEQ", &modulator_level_field(idx), Value::Number(level))
            .effects_dirty;
    }
    effects_dirty
}

pub(crate) fn sync_modulator_phase_field_delta(
    rt: &mut Runtime,
    previous: &[f64],
    phases: &[f64],
) -> bool {
    let mut effects_dirty = false;
    if previous.len() != phases.len() {
        effects_dirty |= sync_modulator_phase_fields(rt, phases);
        for idx in phases.len()..previous.len() {
            effects_dirty |= rt
                .set_reactive("SEQ", &modulator_phase_field(idx), Value::Number(0.0))
                .effects_dirty;
        }
        return effects_dirty;
    }

    for (idx, (&old_phase, &phase)) in previous.iter().zip(phases.iter()).enumerate() {
        if old_phase != phase {
            effects_dirty |= rt
                .set_reactive("SEQ", &modulator_phase_field(idx), Value::Number(phase))
                .effects_dirty;
        }
    }
    effects_dirty
}

pub(crate) fn sync_modulator_level_field_delta(
    rt: &mut Runtime,
    previous: &[f64],
    levels: &[f64],
) -> bool {
    let mut effects_dirty = false;
    if previous.len() != levels.len() {
        effects_dirty |= sync_modulator_level_fields(rt, levels);
        for idx in levels.len()..previous.len() {
            effects_dirty |= rt
                .set_reactive("SEQ", &modulator_level_field(idx), Value::Number(0.0))
                .effects_dirty;
        }
        return effects_dirty;
    }

    for (idx, (&old_level, &level)) in previous.iter().zip(levels.iter()).enumerate() {
        if old_level != level {
            effects_dirty |= rt
                .set_reactive("SEQ", &modulator_level_field(idx), Value::Number(level))
                .effects_dirty;
        }
    }
    effects_dirty
}

pub(crate) fn sync_track_peak_fields(rt: &mut Runtime, levels: &[f64]) -> bool {
    let mut effects_dirty = false;
    for (idx, &level) in levels.iter().enumerate() {
        effects_dirty |= rt
            .set_reactive("SEQ", &format!("track-peak-{idx}"), Value::Number(level))
            .effects_dirty;
    }
    effects_dirty
}

pub(crate) fn sync_neural_visualization_fields(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
) -> bool {
    let mut effects_dirty = rt
        .set_reactive(
            "SEQ",
            "neural-energy-matrix",
            build_neural_energy_matrix_value(state),
        )
        .effects_dirty;
    effects_dirty |= rt
        .set_reactive(
            "SEQ",
            "neural-trigger-matrix",
            build_neural_trigger_matrix_value(state),
        )
        .effects_dirty;
    effects_dirty |= rt
        .set_reactive(
            "SEQ",
            "neural-dampening-matrix",
            build_neural_dampening_matrix_value(state),
        )
        .effects_dirty;
    effects_dirty |= rt
        .set_reactive(
            "SEQ",
            "graph-visualizations",
            build_graph_visualizations_value(state),
        )
        .effects_dirty;
    effects_dirty |= rt
        .set_reactive(
            "SEQ",
            "track-events",
            build_track_output_events_value(state),
        )
        .effects_dirty;
    effects_dirty |= rt
        .set_reactive(
            "SEQ",
            "track-event-current-beat",
            build_track_output_current_beat_value(state),
        )
        .effects_dirty;
    effects_dirty
}

pub(crate) fn sync_bus_peak_fields(rt: &mut Runtime, levels: &[f64]) -> bool {
    let mut effects_dirty = false;
    for (idx, &level) in levels.iter().enumerate() {
        effects_dirty |= rt
            .set_reactive("SEQ", &format!("bus-peak-{idx}"), Value::Number(level))
            .effects_dirty;
    }
    effects_dirty
}

pub(crate) fn sync_track_peak_field_delta(
    rt: &mut Runtime,
    previous: &[f64],
    levels: &[f64],
) -> bool {
    let mut effects_dirty = false;
    if previous.len() != levels.len() {
        effects_dirty |= sync_track_peak_fields(rt, levels);
        for idx in levels.len()..previous.len() {
            effects_dirty |= rt
                .set_reactive("SEQ", &format!("track-peak-{idx}"), Value::Number(0.0))
                .effects_dirty;
        }
        return effects_dirty;
    }

    for (idx, (&old_level, &level)) in previous.iter().zip(levels.iter()).enumerate() {
        if old_level != level {
            effects_dirty |= rt
                .set_reactive("SEQ", &format!("track-peak-{idx}"), Value::Number(level))
                .effects_dirty;
        }
    }
    effects_dirty
}

pub(crate) fn sync_bus_peak_field_delta(
    rt: &mut Runtime,
    previous: &[f64],
    levels: &[f64],
) -> bool {
    let mut effects_dirty = false;
    if previous.len() != levels.len() {
        effects_dirty |= sync_bus_peak_fields(rt, levels);
        for idx in levels.len()..previous.len() {
            effects_dirty |= rt
                .set_reactive("SEQ", &format!("bus-peak-{idx}"), Value::Number(0.0))
                .effects_dirty;
        }
        return effects_dirty;
    }

    for (idx, (&old_level, &level)) in previous.iter().zip(levels.iter()).enumerate() {
        if old_level != level {
            effects_dirty |= rt
                .set_reactive("SEQ", &format!("bus-peak-{idx}"), Value::Number(level))
                .effects_dirty;
        }
    }
    effects_dirty
}

pub(crate) fn sync_playhead_fields(rt: &mut Runtime, playhead: usize, num_steps: usize) -> bool {
    let clamped_steps = num_steps.max(1).min(MAX_STEPS);
    let active_step = playhead.min(clamped_steps.saturating_sub(1));
    let mut effects_dirty = rt
        .set_reactive(
            "SEQ",
            "playhead-page",
            Value::Number((active_step / PAGE_SIZE) as f64),
        )
        .effects_dirty;
    effects_dirty |= rt
        .set_reactive("SEQ", "playhead", Value::Number(active_step as f64))
        .effects_dirty;
    for idx in 0..MAX_STEPS {
        effects_dirty |= rt
            .set_reactive(
                "SEQ",
                &format!("playhead-active-{idx}"),
                Value::Bool(idx == active_step && idx < clamped_steps),
            )
            .effects_dirty;
    }
    effects_dirty
}

pub(crate) fn sync_playhead_field_delta(
    rt: &mut Runtime,
    prev_playhead: usize,
    playhead: usize,
    num_steps: usize,
) -> bool {
    let clamped_steps = num_steps.max(1).min(MAX_STEPS);
    let prev_active = prev_playhead.min(clamped_steps.saturating_sub(1));
    let active_step = playhead.min(clamped_steps.saturating_sub(1));
    let mut effects_dirty = rt
        .set_reactive(
            "SEQ",
            "playhead-page",
            Value::Number((active_step / PAGE_SIZE) as f64),
        )
        .effects_dirty;
    effects_dirty |= rt
        .set_reactive("SEQ", "playhead", Value::Number(active_step as f64))
        .effects_dirty;
    if prev_active != active_step {
        effects_dirty |= rt
            .set_reactive(
                "SEQ",
                &format!("playhead-active-{prev_active}"),
                Value::Bool(false),
            )
            .effects_dirty;
        effects_dirty |= rt
            .set_reactive(
                "SEQ",
                &format!("playhead-active-{active_step}"),
                Value::Bool(true),
            )
            .effects_dirty;
    }
    effects_dirty
}

pub(crate) fn sync_track_topology_state(
    rt: &mut Runtime,
    app: &app::App,
    state: &Arc<SequencerState>,
    track_names: &mut Vec<String>,
    current_track_idx: usize,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
    piano_roll_selection: &Arc<Mutex<HashSet<u64>>>,
    accumulator_names: &Arc<Mutex<Vec<String>>>,
    record_armed: &Arc<Mutex<Vec<bool>>>,
    track_peak_levels: &[f64],
) {
    sync_macro_state(rt, app);
    sync_track_name_state(rt, track_names, app);
    sync_bus_mixer_state(rt, app);
    sync_pattern_state(rt, state);
    set_current_track_reactive(rt, app.tracks.len(), current_track_idx);
    rt.set_reactive(
        "SEQ",
        "record-armed",
        build_record_armed_value(&record_armed.lock().unwrap()),
    );
    let (selected_step, selected_step_count) = {
        let selected = selected_steps.lock().unwrap();
        (selected.iter().copied().min(), selected.len())
    };
    rt.set_reactive(
        "SEQ",
        "fx-step-selection-count",
        Value::Number(selected_step_count as f64),
    );
    rt.set_reactive("SEQ", "fx-step-cursor-number", Value::Number(1.0));
    rt.set_reactive("SEQ", "fx-step-parameter-step", Value::Number(0.0));

    if app.tracks.is_empty() {
        sync_playhead_fields(rt, 0, 1);
        rt.set_reactive("SEQ", "steps", Value::List(vec![]));
        rt.set_reactive("SEQ", "piano-roll-items", Value::List(vec![]));
        rt.set_reactive("SEQ", "piano-roll-selection", Value::List(vec![]));
        rt.set_reactive("SEQ", "velocities", Value::List(vec![]));
        rt.set_reactive("SEQ", "durations", Value::List(vec![]));
        rt.set_reactive("SEQ", "transposes", Value::List(vec![]));
        rt.set_reactive("SEQ", "auxas", Value::List(vec![]));
        rt.set_reactive("SEQ", "pans", Value::List(vec![]));
        rt.set_reactive("SEQ", "syncs", Value::List(vec![]));
        rt.set_reactive("SEQ", "delays", Value::List(vec![]));
        sync_track_mixer_state(rt, app, state);
        sync_bus_mixer_state(rt, app);
        rt.set_reactive("SEQ", "effects", Value::List(vec![]));
        rt.set_reactive("SEQ", "midi-effects", Value::List(vec![]));
        rt.set_reactive("SEQ", "instrument-panel", Value::List(vec![]));
        rt.set_reactive("SEQ", "step-has-plocks", Value::List(vec![]));
        rt.set_reactive("SEQ", "step-plock-kinds", Value::List(vec![]));
        rt.set_reactive("SEQ", "step-variant-r", Value::List(vec![]));
        rt.set_reactive("SEQ", "step-variant-g", Value::List(vec![]));
        rt.set_reactive("SEQ", "step-variant-b", Value::List(vec![]));
        rt.set_reactive("SEQ", "track-steps", Value::List(vec![]));
        rt.set_reactive("SEQ", "track-num-steps", Value::List(vec![]));
        rt.set_reactive("SEQ", "track-timebases", Value::List(vec![]));
        rt.set_reactive("SEQ", "track-duration-spans", Value::List(vec![]));
        rt.set_reactive("SEQ", "track-playheads", Value::List(vec![]));
        rt.set_reactive("SEQ", "track-step-has-plocks", Value::List(vec![]));
        rt.set_reactive("SEQ", "track-step-plock-kinds", Value::List(vec![]));
        rt.set_reactive("SEQ", "track-step-variant-r", Value::List(vec![]));
        rt.set_reactive("SEQ", "track-step-variant-g", Value::List(vec![]));
        rt.set_reactive("SEQ", "track-step-variant-b", Value::List(vec![]));
        rt.set_reactive("SEQ", "track-velocities", Value::List(vec![]));
        rt.set_reactive("SEQ", "track-durations", Value::List(vec![]));
        rt.set_reactive("SEQ", "track-auxas", Value::List(vec![]));
        rt.set_reactive("SEQ", "track-transposes", Value::List(vec![]));
        rt.set_reactive("SEQ", "track-drum-racks", Value::List(vec![]));
        rt.set_reactive("SEQ", "track-drum-sounds", Value::List(vec![]));
        rt.set_reactive("SEQ", "track-pans", Value::List(vec![]));
        rt.set_reactive("SEQ", "track-syncs", Value::List(vec![]));
        rt.set_reactive("SEQ", "track-delays", Value::List(vec![]));
        rt.set_reactive("SEQ", "track-process-lanes", Value::List(vec![]));
        rt.set_reactive("SEQ", "process-lanes", Value::List(vec![]));
        rt.set_reactive("SEQ", "process-slots", Value::List(vec![]));
        rt.set_reactive("SEQ", "track-process-slots", Value::List(vec![]));
        rt.set_reactive("SEQ", "process-library", Value::List(vec![]));
        rt.set_reactive("SEQ", "track-ids", Value::List(vec![]));
        rt.set_reactive("SEQ", "track-plocks", Value::List(vec![]));
        rt.set_reactive("SEQ", "track-plock-variants", Value::List(vec![]));
        for param in [StepParam::Velocity, StepParam::Duration, StepParam::Transpose] {
            rt.set_reactive(
                "SEQ",
                fx_step_param_value_field(param)
                    .expect("step parameter strip field should exist"),
                Value::Number(0.0),
            );
        }
        return;
    }

    sync_all_track_sequencer_state(rt, state, app, current_track_idx, selected_steps);
    let cursor_step = fx_step_cursor_from_runtime(rt);
    sync_fx_step_cursor_binding_fields(
        rt,
        state,
        current_track_idx,
        cursor_step,
        selected_step,
        selected_step_count,
    );

    sync_playhead_fields(
        rt,
        state.transport.track_playheads[current_track_idx].load(Ordering::Relaxed) as usize,
        state.pattern.track_params[current_track_idx].get_num_steps(),
    );
    rt.set_reactive("SEQ", "steps", build_steps_value(state, current_track_idx));
    sync_piano_roll_state(rt, state, current_track_idx, piano_roll_selection);
    sync_step_param_lists(rt, state, current_track_idx);
    sync_track_mixer_state(rt, app, state);
    sync_bus_mixer_state(rt, app);
    sync_track_peak_fields(rt, track_peak_levels);
    rt.set_reactive(
        "SEQ",
        "effects",
        build_effects_value(
            state,
            current_track_idx,
            &app.graph.effect_descriptors,
            selected_steps,
        ),
    );
    rt.set_reactive(
        "SEQ",
        "midi-effects",
        build_midi_effects_value(state, current_track_idx, selected_steps),
    );
    rt.set_reactive(
        "SEQ",
        "instrument-panel",
        build_instrument_panel_value(app, current_track_idx, selected_steps),
    );
    rt.set_reactive(
        "SEQ",
        "fx-step-display-step",
        displayed_plock_step(state, current_track_idx, selected_plock_step(selected_steps))
            .map(|step| Value::Number(step as f64))
            .unwrap_or(Value::Number(-1.0)),
    );
    sync_fx_param_binding_fields(rt, app, state, current_track_idx, selected_steps);
    *accumulator_names.lock().unwrap() = build_accumulator_names(app);
    sync_track_params(rt, app, state, current_track_idx, selected_steps);
    rt.set_reactive(
        "SEQ",
        "step-has-plocks",
        build_step_has_plocks(state, current_track_idx, &app.graph.effect_descriptors),
    );
    rt.set_reactive(
        "SEQ",
        "step-plock-kinds",
        build_step_plock_kinds(state, current_track_idx),
    );
    rt.set_reactive(
        "SEQ",
        "step-variant-r",
        build_step_variant_color_channel(state, current_track_idx, 0),
    );
    rt.set_reactive(
        "SEQ",
        "step-variant-g",
        build_step_variant_color_channel(state, current_track_idx, 1),
    );
    rt.set_reactive(
        "SEQ",
        "step-variant-b",
        build_step_variant_color_channel(state, current_track_idx, 2),
    );
    sync_sidebar_browser(rt, app, current_track_idx);
}

pub(crate) fn sync_pattern_state(rt: &mut Runtime, state: &Arc<SequencerState>) {
    rt.set_reactive(
        "SEQ",
        "current-pattern",
        Value::Number(state.current_scene_index() as f64),
    );
    rt.set_reactive(
        "SEQ",
        "num-patterns",
        Value::Number(state.scene_count() as f64),
    );
    rt.set_reactive(
        "SEQ",
        "track-pattern-cells",
        build_track_pattern_cells_value(state, state.active_track_count()),
    );
    sync_track_pattern_cell_state_fields(rt, state, state.active_track_count());
    rt.set_reactive("SEQ", "neural-networks", build_neural_networks_value(state));
    rt.set_reactive(
        "SEQ",
        "neural-energy-matrix",
        build_neural_energy_matrix_value(state),
    );
    rt.set_reactive(
        "SEQ",
        "neural-trigger-matrix",
        build_neural_trigger_matrix_value(state),
    );
    rt.set_reactive(
        "SEQ",
        "neural-dampening-matrix",
        build_neural_dampening_matrix_value(state),
    );
    rt.set_reactive(
        "SEQ",
        "graph-visualizations",
        build_graph_visualizations_value(state),
    );
    rt.set_reactive(
        "SEQ",
        "track-events",
        build_track_output_events_value(state),
    );
    rt.set_reactive(
        "SEQ",
        "track-event-current-beat",
        build_track_output_current_beat_value(state),
    );
}

pub(crate) fn build_neural_networks_value(state: &Arc<SequencerState>) -> Value {
    Value::List(
        state
            .current_neural_networks()
            .iter()
            .map(neural_network_value)
            .map(|network| Rc::new(RefCell::new(network)))
            .collect(),
    )
}

pub(crate) fn build_neural_dampening_matrix_value(state: &Arc<SequencerState>) -> Value {
    let snapshot = state.neural_visualization();
    let size = snapshot.num_neurons.min(sequencer::neural::NUM_NEURONS);
    Value::List(
        (0..size)
            .map(|row| {
                Rc::new(RefCell::new(Value::List(
                    (0..size)
                        .map(|col| {
                            Rc::new(RefCell::new(Value::Number(neural_dampening_display_value(
                                snapshot.dampening[row][col],
                            ))))
                        })
                        .collect(),
                )))
            })
            .collect(),
    )
}

pub(crate) fn build_neural_energy_matrix_value(state: &Arc<SequencerState>) -> Value {
    let snapshot = state.neural_visualization();
    let size = snapshot.num_neurons.min(sequencer::neural::NUM_NEURONS);
    neural_column_matrix_value(
        (0..size).map(|idx| neural_energy_display_value(snapshot.energy[idx])),
    )
}

pub(crate) fn build_neural_trigger_matrix_value(state: &Arc<SequencerState>) -> Value {
    let snapshot = state.neural_visualization();
    let size = snapshot.num_neurons.min(sequencer::neural::NUM_NEURONS);
    neural_column_matrix_value(
        (0..size).map(|idx| neural_trigger_display_value(snapshot.trigger_activity[idx])),
    )
}

pub(crate) fn build_graph_visualizations_value(state: &Arc<SequencerState>) -> Value {
    Value::List(
        state
            .graph_visualizations()
            .iter()
            .map(|snapshot| Rc::new(RefCell::new(graph_visualization_value(snapshot))))
            .collect(),
    )
}

pub(crate) fn build_track_output_events_value(state: &Arc<SequencerState>) -> Value {
    Value::List(
        state
            .track_output_events()
            .into_iter()
            .map(|event| Rc::new(RefCell::new(track_output_event_value(event))))
            .collect(),
    )
}

pub(crate) fn build_track_output_current_beat_value(state: &Arc<SequencerState>) -> Value {
    Value::Number(state.track_output_current_beat())
}

pub(crate) fn build_active_notes_value(notes: &[u8]) -> Value {
    Value::List(
        notes
            .iter()
            .map(|note| Rc::new(RefCell::new(Value::Number(*note as f64))))
            .collect(),
    )
}

fn track_output_event_value(event: sequencer::sequencer::TrackOutputEvent) -> Value {
    map_value([
        ("node", Value::Nil),
        ("track", Value::Number(event.track as f64)),
        ("sample", Value::Number(event.sample_time as f64)),
        ("beat", Value::Number(event.beat)),
        ("transpose", Value::Number(event.transpose as f64)),
        ("velocity", Value::Number(event.velocity as f64)),
    ])
}

fn graph_visualization_value(snapshot: &sequencer::graph::GraphVisualizationSnapshot) -> Value {
    let mut map = HashMap::new();
    map.insert(
        "id".to_string(),
        value_cell(Value::Number(snapshot.id as f64)),
    );
    map.insert(
        "name".to_string(),
        value_cell(Value::String(snapshot.name.clone())),
    );
    map.insert(
        "active".to_string(),
        value_cell(Value::Bool(snapshot.active)),
    );
    map.insert(
        "current-beat".to_string(),
        value_cell(Value::Number(snapshot.current_beat)),
    );
    map.insert(
        "num-nodes".to_string(),
        value_cell(Value::Number(snapshot.num_nodes as f64)),
    );
    map.insert(
        "energy-matrix".to_string(),
        value_cell(neural_column_matrix_value(
            snapshot
                .energy
                .iter()
                .take(snapshot.num_nodes)
                .map(|value| graph_energy_display_value(*value)),
        )),
    );
    map.insert(
        "trigger-matrix".to_string(),
        value_cell(neural_column_matrix_value(
            snapshot
                .trigger_activity
                .iter()
                .take(snapshot.num_nodes)
                .map(|value| neural_trigger_display_value(*value)),
        )),
    );
    map.insert(
        "weight-matrix".to_string(),
        value_cell(graph_dense_edge_matrix_value(snapshot, |edge| {
            graph_weight_display_value(edge.weight)
        })),
    );
    map.insert(
        "dampening-matrix".to_string(),
        value_cell(graph_dense_edge_matrix_value(snapshot, |edge| {
            neural_dampening_display_value(edge.dampening as f32)
        })),
    );
    map.insert(
        "delay-matrix".to_string(),
        value_cell(graph_dense_edge_matrix_value(snapshot, |edge| {
            edge.delay_steps as f64
        })),
    );
    map.insert(
        "edges".to_string(),
        value_cell(Value::List(
            snapshot
                .edges
                .iter()
                .map(|edge| Rc::new(RefCell::new(graph_edge_value(*edge))))
                .collect(),
        )),
    );
    map.insert(
        "node-events".to_string(),
        value_cell(Value::List(
            snapshot
                .node_events
                .iter()
                .take(snapshot.num_nodes)
                .map(|event| Rc::new(RefCell::new(graph_optional_event_value(*event))))
                .collect(),
        )),
    );
    map.insert(
        "events".to_string(),
        value_cell(Value::List(
            snapshot
                .node_events
                .iter()
                .take(snapshot.num_nodes)
                .flatten()
                .copied()
                .map(|event| Rc::new(RefCell::new(graph_event_value(event))))
                .collect(),
        )),
    );
    map.insert(
        "event-history".to_string(),
        value_cell(Value::List(
            snapshot
                .event_history
                .iter()
                .copied()
                .map(|event| Rc::new(RefCell::new(graph_raw_event_value(event))))
                .collect(),
        )),
    );
    Value::Map(map)
}

fn value_cell(value: Value) -> Rc<RefCell<Value>> {
    Rc::new(RefCell::new(value))
}

fn graph_dense_edge_matrix_value(
    snapshot: &sequencer::graph::GraphVisualizationSnapshot,
    value: impl Fn(sequencer::graph::GraphVisualizationEdge) -> f64,
) -> Value {
    let mut matrix = vec![vec![0.0; snapshot.num_nodes]; snapshot.num_nodes];
    for edge in &snapshot.edges {
        if edge.from < snapshot.num_nodes && edge.to < snapshot.num_nodes {
            matrix[edge.from][edge.to] = value(*edge);
        }
    }
    Value::List(
        matrix
            .into_iter()
            .map(|row| {
                Rc::new(RefCell::new(Value::List(
                    row.into_iter()
                        .map(|cell| Rc::new(RefCell::new(Value::Number(cell))))
                        .collect(),
                )))
            })
            .collect(),
    )
}

fn graph_edge_value(edge: sequencer::graph::GraphVisualizationEdge) -> Value {
    let mut map = HashMap::new();
    map.insert(
        "from".to_string(),
        value_cell(Value::Number(edge.from as f64)),
    );
    map.insert("to".to_string(), value_cell(Value::Number(edge.to as f64)));
    map.insert(
        "weight".to_string(),
        value_cell(Value::Number(graph_weight_display_value(edge.weight))),
    );
    map.insert(
        "dampening".to_string(),
        value_cell(Value::Number(neural_dampening_display_value(
            edge.dampening as f32,
        ))),
    );
    map.insert(
        "delay".to_string(),
        value_cell(Value::Number(edge.delay_steps as f64)),
    );
    map.insert(
        "distribution".to_string(),
        value_cell(Value::String(match edge.distribution {
            sequencer::graph::EdgeDistribution::BroadcastWeighted => {
                "broadcast-weighted".to_string()
            }
            sequencer::graph::EdgeDistribution::WeightedChoice => "weighted-choice".to_string(),
        })),
    );
    Value::Map(map)
}

fn graph_optional_event_value(event: Option<sequencer::graph::GraphVisualizationEvent>) -> Value {
    event.map(graph_event_value).unwrap_or(Value::Nil)
}

fn graph_event_value(event: sequencer::graph::GraphVisualizationEvent) -> Value {
    let mut map = HashMap::new();
    map.insert(
        "node".to_string(),
        value_cell(Value::Number(event.node_index as f64)),
    );
    map.insert(
        "track".to_string(),
        value_cell(
            event
                .track
                .map(|track| Value::Number(track as f64))
                .unwrap_or(Value::Nil),
        ),
    );
    map.insert(
        "sample".to_string(),
        value_cell(Value::Number(event.sample_time as f64)),
    );
    map.insert("beat".to_string(), value_cell(Value::Number(event.beat)));
    map.insert(
        "transpose".to_string(),
        value_cell(Value::Number(graph_weight_display_value(
            event.transpose as f64,
        ))),
    );
    map.insert(
        "velocity".to_string(),
        value_cell(Value::Number(neural_trigger_display_value(event.velocity))),
    );
    Value::Map(map)
}

fn graph_raw_event_value(event: sequencer::graph::GraphVisualizationEvent) -> Value {
    let mut map = HashMap::new();
    map.insert(
        "node".to_string(),
        value_cell(Value::Number(event.node_index as f64)),
    );
    map.insert(
        "track".to_string(),
        value_cell(
            event
                .track
                .map(|track| Value::Number(track as f64))
                .unwrap_or(Value::Nil),
        ),
    );
    map.insert(
        "sample".to_string(),
        value_cell(Value::Number(event.sample_time as f64)),
    );
    map.insert("beat".to_string(), value_cell(Value::Number(event.beat)));
    map.insert(
        "transpose".to_string(),
        value_cell(Value::Number(event.transpose as f64)),
    );
    map.insert(
        "velocity".to_string(),
        value_cell(Value::Number(event.velocity as f64)),
    );
    Value::Map(map)
}

fn neural_column_matrix_value(values: impl Iterator<Item = f64>) -> Value {
    Value::List(
        values
            .map(|value| {
                Rc::new(RefCell::new(Value::List(vec![Rc::new(RefCell::new(
                    Value::Number(value),
                ))])))
            })
            .collect(),
    )
}

fn neural_energy_display_value(value: f32) -> f64 {
    let value = value.clamp(0.0, 4.0) as f64;
    (value * 100.0).round() / 100.0
}

fn graph_energy_display_value(value: f64) -> f64 {
    let value = value.clamp(0.0, 4.0);
    (value * 100.0).round() / 100.0
}

fn graph_weight_display_value(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

fn neural_trigger_display_value(value: f32) -> f64 {
    value.clamp(0.0, 1.0) as f64
}

fn neural_dampening_display_value(value: f32) -> f64 {
    let value = value.clamp(0.0, 1.0) as f64;
    (value * 100.0).round() / 100.0
}

fn neural_network_value(network: &sequencer::neural::ProjectNeuralNetwork) -> Value {
    let mut map = HashMap::new();
    map.insert(
        "id".to_string(),
        Rc::new(RefCell::new(Value::Number(network.id as f64))),
    );
    map.insert(
        "name".to_string(),
        Rc::new(RefCell::new(Value::String(network.name.clone()))),
    );
    map.insert(
        "enabled".to_string(),
        Rc::new(RefCell::new(Value::Bool(network.enabled))),
    );
    map.insert(
        "num-neurons".to_string(),
        Rc::new(RefCell::new(Value::Number(network.num_neurons as f64))),
    );
    map.insert(
        "reset-bars".to_string(),
        Rc::new(RefCell::new(Value::Number(
            network.reset_interval_bars as f64,
        ))),
    );
    map.insert(
        "energy-decay".to_string(),
        Rc::new(RefCell::new(Value::Number(network.energy_decay as f64))),
    );
    map.insert(
        "max-poly".to_string(),
        Rc::new(RefCell::new(Value::Number(network.max_poly as f64))),
    );
    map.insert(
        "max-poly-selection".to_string(),
        Rc::new(RefCell::new(Value::String(
            network.max_poly_selection.as_str().to_string(),
        ))),
    );
    map.insert(
        "weights".to_string(),
        Rc::new(RefCell::new(Value::List(
            network
                .weights
                .iter()
                .map(|row| {
                    Rc::new(RefCell::new(Value::List(
                        row.iter()
                            .map(|value| Rc::new(RefCell::new(Value::Number(*value as f64))))
                            .collect(),
                    )))
                })
                .collect(),
        ))),
    );
    map.insert(
        "neurons".to_string(),
        Rc::new(RefCell::new(Value::List(
            network
                .neurons
                .iter()
                .enumerate()
                .map(|(idx, neuron)| Rc::new(RefCell::new(neural_neuron_value(idx, neuron))))
                .collect(),
        ))),
    );
    Value::Map(map)
}

fn neural_neuron_value(idx: usize, neuron: &sequencer::neural::ProjectNeuron) -> Value {
    let mut map = HashMap::new();
    map.insert(
        "index".to_string(),
        Rc::new(RefCell::new(Value::Number(idx as f64))),
    );
    map.insert(
        "route".to_string(),
        Rc::new(RefCell::new(
            neuron
                .route
                .map(|route| Value::Number(route as f64))
                .unwrap_or(Value::Nil),
        )),
    );
    map.insert(
        "resolution".to_string(),
        Rc::new(RefCell::new(Value::Keyword(
            neuron.resolution_timebase().label().to_string(),
        ))),
    );
    map.insert(
        "delay".to_string(),
        Rc::new(RefCell::new(Value::Number(neuron.delay_steps as f64))),
    );
    map.insert(
        "threshold".to_string(),
        Rc::new(RefCell::new(Value::Number(neuron.threshold as f64))),
    );
    map.insert(
        "transpose".to_string(),
        Rc::new(RefCell::new(Value::Number(neuron.transpose as f64))),
    );
    map.insert(
        "quantize".to_string(),
        Rc::new(RefCell::new(
            neuron
                .quantize_timebase()
                .map(|timebase| Value::Keyword(timebase.label().to_string()))
                .unwrap_or(Value::Nil),
        )),
    );
    map.insert(
        "dampening".to_string(),
        Rc::new(RefCell::new(Value::Number(neuron.dampening_amount as f64))),
    );
    map.insert(
        "dampening-recovery".to_string(),
        Rc::new(RefCell::new(Value::Number(
            neuron.dampening_recovery as f64,
        ))),
    );
    Value::Map(map)
}

pub(crate) fn build_sync_labels() -> Value {
    let items: Vec<Rc<RefCell<Value>>> = SYNC_RESOLUTIONS
        .iter()
        .map(|(_, label)| {
            let mut compact = label.replace(' ', "");
            compact.truncate(4);
            Rc::new(RefCell::new(Value::String(compact)))
        })
        .collect();
    Value::List(items)
}

/// Build a Lisp Value::List of effect slot maps for a track.
/// Each slot is a map: {:name "Filter" :params ({:name "cutoff" :value 1000 :min 20 :max 20000} ...)}
pub(crate) fn build_effects_value(
    state: &Arc<SequencerState>,
    track: usize,
    descriptors: &[Vec<sequencer::effects::EffectDescriptor>],
    selected: &Arc<Mutex<HashSet<usize>>>,
) -> Value {
    use sequencer::effects::{ParamKind, SyncDivision};
    use std::collections::HashMap;
    let Some(track_descs) = descriptors.get(track) else {
        return Value::List(vec![]);
    };
    let chain = &state.pattern.effect_chains[track];
    let sel = selected.lock().unwrap();
    // If steps are selected, show p-lock value from first selected step
    let plock_step = sel.iter().copied().min();

    struct UiModMetadata {
        source_param_idx: Option<usize>,
        depth_param_idx: usize,
        source_slot: f32,
        source_value_field: Option<String>,
        depth_value: f32,
        depth_value_field: Option<String>,
        depth_min: f32,
        depth_max: f32,
        depth_unit: Option<String>,
    }

    fn is_generated_host_mod_param(name: &str) -> bool {
        name.starts_with("__host_mod__")
    }

    fn is_hidden_dgen_mod_param(name: &str) -> bool {
        name.starts_with("__dgen_mod_active__")
    }

    fn is_source_param(node_param_idx: u32) -> bool {
        sequencer::voice_modulator::is_source_param(node_param_idx)
    }

    fn rename_source_param(name: &str) -> String {
        sequencer::voice_modulator::source_param_display_name(name)
    }

    fn insert_mod_metadata(
        pmap: &mut HashMap<String, Rc<RefCell<Value>>>,
        targets: &[UiModMetadata],
    ) {
        pmap.insert(
            "modulatable".to_string(),
            Rc::new(RefCell::new(Value::Bool(true))),
        );
        let target_values = targets
            .iter()
            .map(|meta| {
                let mut target = HashMap::new();
                if let Some(source_param_idx) = meta.source_param_idx {
                    target.insert(
                        "source-idx".to_string(),
                        Rc::new(RefCell::new(Value::Number(source_param_idx as f64))),
                    );
                }
                target.insert(
                    "depth-idx".to_string(),
                    Rc::new(RefCell::new(Value::Number(meta.depth_param_idx as f64))),
                );
                target.insert(
                    "source-slot".to_string(),
                    Rc::new(RefCell::new(Value::Number(meta.source_slot as f64))),
                );
                if let Some(field) = &meta.source_value_field {
                    target.insert(
                        "source-value-field".to_string(),
                        Rc::new(RefCell::new(Value::String(field.clone()))),
                    );
                }
                target.insert(
                    "depth".to_string(),
                    Rc::new(RefCell::new(Value::Number(meta.depth_value as f64))),
                );
                if let Some(field) = &meta.depth_value_field {
                    target.insert(
                        "depth-value-field".to_string(),
                        Rc::new(RefCell::new(Value::String(field.clone()))),
                    );
                }
                target.insert(
                    "depth-min".to_string(),
                    Rc::new(RefCell::new(Value::Number(meta.depth_min as f64))),
                );
                target.insert(
                    "depth-max".to_string(),
                    Rc::new(RefCell::new(Value::Number(meta.depth_max as f64))),
                );
                if let Some(unit) = &meta.depth_unit {
                    target.insert(
                        "depth-unit".to_string(),
                        Rc::new(RefCell::new(Value::String(unit.clone()))),
                    );
                }
                Rc::new(RefCell::new(Value::Map(target)))
            })
            .collect();
        pmap.insert(
            "mod-targets".to_string(),
            Rc::new(RefCell::new(Value::List(target_values))),
        );
    }

    let slots: Vec<Rc<RefCell<Value>>> = track_descs
        .iter()
        .enumerate()
        .map(|(slot_idx, desc)| {
            let mut slot_map: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();

            slot_map.insert(
                "name".to_string(),
                Rc::new(RefCell::new(Value::String(desc.name.clone()))),
            );

            slot_map.insert(
                "slot-idx".to_string(),
                Rc::new(RefCell::new(Value::Number(slot_idx as f64))),
            );
            slot_map.insert(
                "track-idx".to_string(),
                Rc::new(RefCell::new(Value::Number(track as f64))),
            );
            slot_map.insert(
                "builtin".to_string(),
                Rc::new(RefCell::new(Value::Bool(
                    sequencer::effects::EffectDescriptor::builtin_insert(&desc.name).is_some()
                        || sequencer::effects::conv_reverb::is_dgen_builtin(&desc.name),
                ))),
            );

            let slot = chain.get(slot_idx);

            // Convolution Reverb: surface the current IR's display name for the
            // panel label (keyed by the live node id).
            if sequencer::effects::conv_reverb::is_dgen_builtin(&desc.name) {
                let node_id = slot
                    .map(|s| s.node_id.load(Ordering::Relaxed) as i32)
                    .unwrap_or(0);
                let ir_name = sequencer::effects::conv_reverb::ir_name_for(node_id)
                    .unwrap_or_else(|| "No IR".to_string());
                slot_map.insert(
                    "ir-name".to_string(),
                    Rc::new(RefCell::new(Value::String(ir_name))),
                );
            }
            let mut modulation_targets: HashMap<usize, Vec<UiModMetadata>> = HashMap::new();
            for target in desc
                .instrument_modulation_targets
                .iter()
                .filter_map(|target| {
                    let depth_desc = desc.params.get(target.depth_param_idx)?;
                    let source_default = if let Some(source_param_idx) = target.source_param_idx {
                        if let Some(slot) = slot {
                            if source_param_idx < slot.num_params.load(Ordering::Relaxed) as usize {
                                slot.defaults.get(source_param_idx)
                            } else {
                                desc.params.get(source_param_idx)?.default
                            }
                        } else {
                            desc.params.get(source_param_idx)?.default
                        }
                    } else {
                        target.modulator_slot as f32
                    };
                    let depth_default = if let Some(slot) = slot {
                        if target.depth_param_idx < slot.num_params.load(Ordering::Relaxed) as usize
                        {
                            slot.defaults.get(target.depth_param_idx)
                        } else {
                            depth_desc.default
                        }
                    } else {
                        depth_desc.default
                    };
                    let source_current = target
                        .source_param_idx
                        .and_then(|source_param_idx| {
                            plock_step.and_then(|step| {
                                slot.and_then(|slot| slot.plocks.get(step, source_param_idx))
                            })
                        })
                        .unwrap_or(source_default);
                    let depth_current = plock_step
                        .and_then(|step| {
                            slot.and_then(|slot| slot.plocks.get(step, target.depth_param_idx))
                        })
                        .unwrap_or(depth_default);
                    Some((
                        target.base_param_idx,
                        UiModMetadata {
                            source_param_idx: target.source_param_idx,
                            depth_param_idx: target.depth_param_idx,
                            source_slot: target
                                .source_param_idx
                                .and_then(|source_param_idx| {
                                    desc.params.get(source_param_idx).map(|source_desc| {
                                        source_desc.stored_to_user(source_current)
                                    })
                                })
                                .unwrap_or(source_current),
                            source_value_field: target.source_param_idx.map(|source_param_idx| {
                                let source_desc = &desc.params[source_param_idx];
                                track_effect_param_value_field(
                                    track,
                                    slot_idx,
                                    source_param_idx,
                                    &source_desc.name,
                                )
                            }),
                            depth_value: depth_desc.stored_to_user(depth_current),
                            depth_value_field: Some(track_effect_param_value_field(
                                track,
                                slot_idx,
                                target.depth_param_idx,
                                &depth_desc.name,
                            )),
                            depth_min: target.depth_min,
                            depth_max: target.depth_max,
                            depth_unit: target.depth_unit.clone(),
                        },
                    ))
                })
            {
                modulation_targets
                    .entry(target.0)
                    .or_default()
                    .push(target.1);
            }

            let modulation_routing_params = modulation_routing_param_indices(desc);

            let params: Vec<Rc<RefCell<Value>>> = desc
                .params
                .iter()
                .enumerate()
                .filter_map(|(param_idx, pdesc)| {
                    if (is_source_param(pdesc.node_param_idx)
                        && !matches!(
                            pdesc.host_control,
                            Some(sequencer::effects::HostControl::FxSidechain { .. })
                        ))
                        || modulation_routing_params.contains(&param_idx)
                        || is_generated_host_mod_param(&pdesc.name)
                        || is_hidden_dgen_mod_param(&pdesc.name)
                    {
                        return None;
                    }
                    let delay_synced = if desc.name == "Delay" {
                        chain
                            .get(slot_idx)
                            .map(|s| s.defaults.get(1) > 0.5)
                            .unwrap_or(false)
                    } else {
                        false
                    };
                    let default_val = chain
                        .get(slot_idx)
                        .map(|s| {
                            if param_idx < s.num_params.load(Ordering::Relaxed) as usize {
                                s.defaults.get(param_idx)
                            } else {
                                pdesc.default
                            }
                        })
                        .unwrap_or(pdesc.default);
                    // Show p-lock value if steps are selected, fall back to default
                    let current_val = plock_step
                        .and_then(|step| chain.get(slot_idx)?.plocks.get(step, param_idx))
                        .unwrap_or(default_val);

                    let mut pmap: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
                    pmap.insert(
                        "name".to_string(),
                        Rc::new(RefCell::new(Value::String(pdesc.name.clone()))),
                    );
                    pmap.insert(
                        "idx".to_string(),
                        Rc::new(RefCell::new(Value::Number(param_idx as f64))),
                    );
                    pmap.insert(
                        "value".to_string(),
                        Rc::new(RefCell::new(Value::Number(current_val as f64))),
                    );
                    pmap.insert(
                        "min".to_string(),
                        Rc::new(RefCell::new(Value::Number(pdesc.min as f64))),
                    );
                    pmap.insert(
                        "max".to_string(),
                        Rc::new(RefCell::new(Value::Number(pdesc.max as f64))),
                    );
                    match &pdesc.kind {
                        ParamKind::Boolean => {
                            pmap.insert(
                                "boolean".to_string(),
                                Rc::new(RefCell::new(Value::Bool(true))),
                            );
                            if param_supports_value_binding(pdesc) {
                                insert_string_prop(
                                    &mut pmap,
                                    "value-field",
                                    track_effect_param_value_field(
                                        track,
                                        slot_idx,
                                        param_idx,
                                        &pdesc.name,
                                    ),
                                );
                            }
                        }
                        ParamKind::Enum { labels } => {
                            let selected = labels
                                .get(current_val.round() as usize)
                                .cloned()
                                .unwrap_or_default();
                            let option_values = labels
                                .iter()
                                .cloned()
                                .map(|label| Rc::new(RefCell::new(Value::String(label))))
                                .collect();
                            pmap.insert(
                                "text-value".to_string(),
                                Rc::new(RefCell::new(Value::String(selected))),
                            );
                            pmap.insert(
                                "options".to_string(),
                                Rc::new(RefCell::new(Value::List(option_values))),
                            );
                            if param_supports_value_binding(pdesc) {
                                insert_string_prop(
                                    &mut pmap,
                                    "value-field",
                                    track_effect_param_value_field(
                                        track,
                                        slot_idx,
                                        param_idx,
                                        &pdesc.name,
                                    ),
                                );
                            }
                        }
                        ParamKind::Continuous { .. } => {
                            if desc.name == "Delay" && param_idx == 2 && delay_synced {
                                let labels: Vec<String> = SyncDivision::ALL
                                    .iter()
                                    .map(|d| d.label().to_string())
                                    .collect();
                                let selected_idx = (current_val.round() as usize)
                                    .min(labels.len().saturating_sub(1));
                                let selected =
                                    labels.get(selected_idx).cloned().unwrap_or_default();
                                let option_values = labels
                                    .into_iter()
                                    .map(|label| Rc::new(RefCell::new(Value::String(label))))
                                    .collect();
                                pmap.insert(
                                    "text-value".to_string(),
                                    Rc::new(RefCell::new(Value::String(selected))),
                                );
                                pmap.insert(
                                    "options".to_string(),
                                    Rc::new(RefCell::new(Value::List(option_values))),
                                );
                                pmap.insert(
                                    "min".to_string(),
                                    Rc::new(RefCell::new(Value::Number(0.0))),
                                );
                                pmap.insert(
                                    "max".to_string(),
                                    Rc::new(RefCell::new(Value::Number(
                                        (SyncDivision::ALL.len() - 1) as f64,
                                    ))),
                                );
                            }
                            if param_supports_value_binding(pdesc) {
                                insert_string_prop(
                                    &mut pmap,
                                    "value-field",
                                    track_effect_param_value_field(
                                        track,
                                        slot_idx,
                                        param_idx,
                                        &pdesc.name,
                                    ),
                                );
                            }
                        }
                    }
                    if let Some(targets) = modulation_targets.get(&param_idx) {
                        insert_mod_metadata(&mut pmap, targets);
                    }
                    insert_param_ui_metadata(&mut pmap, pdesc.ui_metadata.as_ref());
                    Some(Rc::new(RefCell::new(Value::Map(pmap))))
                })
                .collect();

            let source_actual =
                selected_voice_mod_source_indices_for_optional_slot(desc, slot, plock_step);
            let mut source_sections: Vec<Rc<RefCell<Value>>> = Vec::new();
            let mut source_names: Vec<Rc<RefCell<Value>>> = Vec::new();
            for slot_number in 1..=sequencer::voice_modulator::SLOT_COUNT {
                let section_name =
                    sequencer::voice_modulator::modulator_slot_label(slot_number, "");
                let mut section_params: Vec<Rc<RefCell<Value>>> = Vec::new();
                let mut source_param: Option<Rc<RefCell<Value>>> = None;
                for &param_idx in &source_actual {
                    let Some(pdesc) = desc.params.get(param_idx) else {
                        continue;
                    };
                    if sequencer::voice_modulator::slot_from_param_name(&pdesc.name)
                        != Some(slot_number)
                    {
                        continue;
                    }
                    let default_val = slot
                        .map(|slot| {
                            if param_idx < slot.num_params.load(Ordering::Relaxed) as usize {
                                slot.defaults.get(param_idx)
                            } else {
                                pdesc.default
                            }
                        })
                        .unwrap_or(pdesc.default);
                    let current_val = plock_step
                        .and_then(|step| slot.and_then(|slot| slot.plocks.get(step, param_idx)))
                        .unwrap_or(default_val);
                    let mut pmap: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
                    pmap.insert(
                        "name".to_string(),
                        Rc::new(RefCell::new(Value::String(rename_source_param(
                            &pdesc.name,
                        )))),
                    );
                    pmap.insert(
                        "idx".to_string(),
                        Rc::new(RefCell::new(Value::Number(param_idx as f64))),
                    );
                    pmap.insert(
                        "value".to_string(),
                        Rc::new(RefCell::new(Value::Number(
                            pdesc.stored_to_user(current_val) as f64,
                        ))),
                    );
                    pmap.insert(
                        "min".to_string(),
                        Rc::new(RefCell::new(Value::Number(
                            pdesc.stored_to_user(pdesc.min) as f64
                        ))),
                    );
                    pmap.insert(
                        "max".to_string(),
                        Rc::new(RefCell::new(Value::Number(
                            pdesc.stored_to_user(pdesc.max) as f64
                        ))),
                    );
                    match &pdesc.kind {
                        ParamKind::Boolean => {
                            pmap.insert(
                                "boolean".to_string(),
                                Rc::new(RefCell::new(Value::Bool(true))),
                            );
                        }
                        ParamKind::Enum { labels } => {
                            let selected = labels
                                .get(current_val.round() as usize)
                                .cloned()
                                .unwrap_or_default();
                            let option_values = labels
                                .iter()
                                .cloned()
                                .map(|label| Rc::new(RefCell::new(Value::String(label))))
                                .collect();
                            pmap.insert(
                                "text-value".to_string(),
                                Rc::new(RefCell::new(Value::String(selected))),
                            );
                            pmap.insert(
                                "options".to_string(),
                                Rc::new(RefCell::new(Value::List(option_values))),
                            );
                        }
                        ParamKind::Continuous { .. } => {}
                    }
                    insert_string_prop(
                        &mut pmap,
                        "value-field",
                        track_effect_param_value_field(track, slot_idx, param_idx, &pdesc.name),
                    );
                    let param_value = Rc::new(RefCell::new(Value::Map(pmap)));
                    if sequencer::voice_modulator::source_type_name_from_param_name(&pdesc.name)
                        == Some("source")
                    {
                        source_param = Some(param_value);
                    } else {
                        section_params.push(param_value);
                    }
                }
                source_names.push(Rc::new(RefCell::new(Value::String(section_name.clone()))));
                let mut section_map: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
                section_map.insert(
                    "name".to_string(),
                    Rc::new(RefCell::new(Value::String(section_name))),
                );
                section_map.insert(
                    "slot".to_string(),
                    Rc::new(RefCell::new(Value::Number(slot_number as f64))),
                );
                if let Some(source_param) = source_param {
                    section_map.insert("source-param".to_string(), source_param);
                }
                section_map.insert(
                    "params".to_string(),
                    Rc::new(RefCell::new(Value::List(section_params))),
                );
                source_sections.push(Rc::new(RefCell::new(Value::Map(section_map))));
            }

            slot_map.insert(
                "params".to_string(),
                Rc::new(RefCell::new(Value::List(params))),
            );
            slot_map.insert(
                "modulators".to_string(),
                Rc::new(RefCell::new(Value::List(
                    desc.instrument_modulators
                        .iter()
                        .map(|modulator| {
                            let mut map: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
                            map.insert(
                                "slot".to_string(),
                                Rc::new(RefCell::new(Value::Number(modulator.slot as f64))),
                            );
                            map.insert(
                                "label".to_string(),
                                Rc::new(RefCell::new(Value::String(modulator.label.clone()))),
                            );
                            Rc::new(RefCell::new(Value::Map(map)))
                        })
                        .collect(),
                ))),
            );
            slot_map.insert(
                "source-names".to_string(),
                Rc::new(RefCell::new(Value::List(source_names))),
            );
            slot_map.insert(
                "sources".to_string(),
                Rc::new(RefCell::new(Value::List(source_sections))),
            );

            Rc::new(RefCell::new(Value::Map(slot_map)))
        })
        .collect();

    Value::List(slots)
}

pub(crate) fn build_bus_effects_value(app: &app::App) -> Value {
    build_bus_effects_value_for_selection(app, None)
}

pub(crate) fn build_bus_effects_value_for_selection(
    app: &app::App,
    selected: Option<&Arc<Mutex<HashSet<usize>>>>,
) -> Value {
    use sequencer::effects::{ParamKind, SyncDivision};
    use std::collections::HashMap;

    let plock_step = selected.and_then(|selected| selected.lock().unwrap().iter().copied().min());

    struct UiModMetadata {
        source_param_idx: Option<usize>,
        depth_param_idx: usize,
        source_slot: f32,
        source_value_field: Option<String>,
        depth_value: f32,
        depth_value_field: Option<String>,
        depth_min: f32,
        depth_max: f32,
        depth_unit: Option<String>,
    }

    fn is_generated_host_mod_param(name: &str) -> bool {
        name.starts_with("__host_mod__")
    }

    fn is_hidden_dgen_mod_param(name: &str) -> bool {
        name.starts_with("__dgen_mod_active__")
    }

    fn is_source_param(node_param_idx: u32) -> bool {
        sequencer::voice_modulator::is_source_param(node_param_idx)
    }

    fn rename_source_param(name: &str) -> String {
        sequencer::voice_modulator::source_param_display_name(name)
    }

    fn bus_slot_param_stored_value(
        slot: Option<&sequencer::effects::EffectSlotSnapshot>,
        desc: &sequencer::effects::EffectDescriptor,
        param_idx: usize,
        plock_step: Option<usize>,
    ) -> f32 {
        let Some(pdesc) = desc.params.get(param_idx) else {
            return 0.0;
        };
        slot.and_then(|slot| {
            plock_step
                .and_then(|step| {
                    slot.plocks
                        .get(step)
                        .and_then(|step_plocks| step_plocks.get(param_idx))
                        .copied()
                        .flatten()
                })
                .or_else(|| {
                    if param_idx < slot.num_params as usize {
                        slot.defaults.get(param_idx).copied()
                    } else {
                        None
                    }
                })
        })
        .unwrap_or(pdesc.default)
    }

    fn selected_bus_voice_mod_source_indices(
        desc: &sequencer::effects::EffectDescriptor,
        slot: Option<&sequencer::effects::EffectSlotSnapshot>,
        plock_step: Option<usize>,
    ) -> Vec<usize> {
        sequencer::voice_modulator::selected_source_param_indices(&desc.params, |idx, param| {
            slot.map(|_| bus_slot_param_stored_value(slot, desc, idx, plock_step))
                .unwrap_or(param.default)
        })
    }

    fn insert_mod_metadata(
        pmap: &mut HashMap<String, Rc<RefCell<Value>>>,
        targets: &[UiModMetadata],
    ) {
        pmap.insert(
            "modulatable".to_string(),
            Rc::new(RefCell::new(Value::Bool(true))),
        );
        let target_values = targets
            .iter()
            .map(|meta| {
                let mut target = HashMap::new();
                if let Some(source_param_idx) = meta.source_param_idx {
                    target.insert(
                        "source-idx".to_string(),
                        Rc::new(RefCell::new(Value::Number(source_param_idx as f64))),
                    );
                }
                target.insert(
                    "depth-idx".to_string(),
                    Rc::new(RefCell::new(Value::Number(meta.depth_param_idx as f64))),
                );
                target.insert(
                    "source-slot".to_string(),
                    Rc::new(RefCell::new(Value::Number(meta.source_slot as f64))),
                );
                if let Some(field) = &meta.source_value_field {
                    target.insert(
                        "source-value-field".to_string(),
                        Rc::new(RefCell::new(Value::String(field.clone()))),
                    );
                }
                target.insert(
                    "depth".to_string(),
                    Rc::new(RefCell::new(Value::Number(meta.depth_value as f64))),
                );
                if let Some(field) = &meta.depth_value_field {
                    target.insert(
                        "depth-value-field".to_string(),
                        Rc::new(RefCell::new(Value::String(field.clone()))),
                    );
                }
                target.insert(
                    "depth-min".to_string(),
                    Rc::new(RefCell::new(Value::Number(meta.depth_min as f64))),
                );
                target.insert(
                    "depth-max".to_string(),
                    Rc::new(RefCell::new(Value::Number(meta.depth_max as f64))),
                );
                if let Some(unit) = &meta.depth_unit {
                    target.insert(
                        "depth-unit".to_string(),
                        Rc::new(RefCell::new(Value::String(unit.clone()))),
                    );
                }
                Rc::new(RefCell::new(Value::Map(target)))
            })
            .collect();
        pmap.insert(
            "mod-targets".to_string(),
            Rc::new(RefCell::new(Value::List(target_values))),
        );
    }

    let buses: Vec<Rc<RefCell<Value>>> = app
        .buses
        .iter()
        .enumerate()
        .map(|(bus_idx, bus)| {
            let slots: Vec<Rc<RefCell<Value>>> = bus
                .effect_descriptors
                .iter()
                .enumerate()
                .map(|(slot_idx, desc)| {
                    let mut slot_map: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
                    slot_map.insert(
                        "name".to_string(),
                        Rc::new(RefCell::new(Value::String(desc.name.clone()))),
                    );
                    slot_map.insert(
                        "slot-idx".to_string(),
                        Rc::new(RefCell::new(Value::Number(slot_idx as f64))),
                    );
                    slot_map.insert(
                        "bus-idx".to_string(),
                        Rc::new(RefCell::new(Value::Number(bus_idx as f64))),
                    );
                    slot_map.insert(
                        "bus-fx".to_string(),
                        Rc::new(RefCell::new(Value::Bool(true))),
                    );
                    slot_map.insert(
                        "builtin".to_string(),
                        Rc::new(RefCell::new(Value::Bool(
                            sequencer::effects::EffectDescriptor::builtin_insert(&desc.name)
                                .is_some()
                                || sequencer::effects::conv_reverb::is_dgen_builtin(&desc.name),
                        ))),
                    );
                    // Convolution Reverb: surface the current IR name for the label.
                    if sequencer::effects::conv_reverb::is_dgen_builtin(&desc.name) {
                        let node_id = bus
                            .effect_slots
                            .get(slot_idx)
                            .map(|s| s.node_id as i32)
                            .unwrap_or(0);
                        let ir_name = sequencer::effects::conv_reverb::ir_name_for(node_id)
                            .unwrap_or_else(|| "No IR".to_string());
                        slot_map.insert(
                            "ir-name".to_string(),
                            Rc::new(RefCell::new(Value::String(ir_name))),
                        );
                    }

                    let slot = bus.effect_slots.get(slot_idx);
                    let mut modulation_targets: HashMap<usize, Vec<UiModMetadata>> = HashMap::new();
                    for target in desc
                        .instrument_modulation_targets
                        .iter()
                        .filter_map(|target| {
                            let depth_desc = desc.params.get(target.depth_param_idx)?;
                            let source_current =
                                if let Some(source_param_idx) = target.source_param_idx {
                                    bus_slot_param_stored_value(
                                        slot,
                                        desc,
                                        source_param_idx,
                                        plock_step,
                                    )
                                } else {
                                    target.modulator_slot as f32
                                };
                            let depth_current = bus_slot_param_stored_value(
                                slot,
                                desc,
                                target.depth_param_idx,
                                plock_step,
                            );
                            Some((
                                target.base_param_idx,
                                UiModMetadata {
                                    source_param_idx: target.source_param_idx,
                                    depth_param_idx: target.depth_param_idx,
                                    source_slot: target
                                        .source_param_idx
                                        .and_then(|source_param_idx| {
                                            desc.params.get(source_param_idx).map(|source_desc| {
                                                source_desc.stored_to_user(source_current)
                                            })
                                        })
                                        .unwrap_or(source_current),
                                    source_value_field: target.source_param_idx.map(
                                        |source_param_idx| {
                                            let source_desc = &desc.params[source_param_idx];
                                            bus_effect_param_value_field(
                                                bus_idx,
                                                slot_idx,
                                                source_param_idx,
                                                &source_desc.name,
                                            )
                                        },
                                    ),
                                    depth_value: depth_desc.stored_to_user(depth_current),
                                    depth_value_field: Some(bus_effect_param_value_field(
                                        bus_idx,
                                        slot_idx,
                                        target.depth_param_idx,
                                        &depth_desc.name,
                                    )),
                                    depth_min: target.depth_min,
                                    depth_max: target.depth_max,
                                    depth_unit: target.depth_unit.clone(),
                                },
                            ))
                        })
                    {
                        modulation_targets
                            .entry(target.0)
                            .or_default()
                            .push(target.1);
                    }

                    let modulation_routing_params = modulation_routing_param_indices(desc);

                    let params: Vec<Rc<RefCell<Value>>> = desc
                        .params
                        .iter()
                        .enumerate()
                        .filter_map(|(param_idx, pdesc)| {
                            if (is_source_param(pdesc.node_param_idx)
                                && !matches!(
                                    pdesc.host_control,
                                    Some(sequencer::effects::HostControl::FxSidechain { .. })
                                ))
                                || modulation_routing_params.contains(&param_idx)
                                || is_generated_host_mod_param(&pdesc.name)
                                || is_hidden_dgen_mod_param(&pdesc.name)
                            {
                                return None;
                            }
                            let current_val =
                                bus_slot_param_stored_value(slot, desc, param_idx, plock_step);
                            let delay_synced = if desc.name == "Delay" {
                                slot.map(|slot| slot.defaults.get(1).copied().unwrap_or(0.0) > 0.5)
                                    .unwrap_or(false)
                            } else {
                                false
                            };
                            let mut pmap: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
                            pmap.insert(
                                "name".to_string(),
                                Rc::new(RefCell::new(Value::String(pdesc.name.clone()))),
                            );
                            pmap.insert(
                                "idx".to_string(),
                                Rc::new(RefCell::new(Value::Number(param_idx as f64))),
                            );
                            pmap.insert(
                                "value".to_string(),
                                Rc::new(RefCell::new(Value::Number(current_val as f64))),
                            );
                            pmap.insert(
                                "min".to_string(),
                                Rc::new(RefCell::new(Value::Number(pdesc.min as f64))),
                            );
                            pmap.insert(
                                "max".to_string(),
                                Rc::new(RefCell::new(Value::Number(pdesc.max as f64))),
                            );
                            match &pdesc.kind {
                                ParamKind::Boolean => {
                                    pmap.insert(
                                        "boolean".to_string(),
                                        Rc::new(RefCell::new(Value::Bool(true))),
                                    );
                                    if param_supports_value_binding(pdesc) {
                                        insert_string_prop(
                                            &mut pmap,
                                            "value-field",
                                            bus_effect_param_value_field(
                                                bus_idx,
                                                slot_idx,
                                                param_idx,
                                                &pdesc.name,
                                            ),
                                        );
                                    }
                                }
                                ParamKind::Enum { labels } => {
                                    let selected = labels
                                        .get(current_val.round() as usize)
                                        .cloned()
                                        .unwrap_or_default();
                                    let option_values = labels
                                        .iter()
                                        .cloned()
                                        .map(|label| Rc::new(RefCell::new(Value::String(label))))
                                        .collect();
                                    pmap.insert(
                                        "text-value".to_string(),
                                        Rc::new(RefCell::new(Value::String(selected))),
                                    );
                                    pmap.insert(
                                        "options".to_string(),
                                        Rc::new(RefCell::new(Value::List(option_values))),
                                    );
                                    if param_supports_value_binding(pdesc) {
                                        insert_string_prop(
                                            &mut pmap,
                                            "value-field",
                                            bus_effect_param_value_field(
                                                bus_idx,
                                                slot_idx,
                                                param_idx,
                                                &pdesc.name,
                                            ),
                                        );
                                    }
                                }
                                ParamKind::Continuous { .. } => {
                                    if desc.name == "Delay" && param_idx == 2 && delay_synced {
                                        let labels: Vec<String> = SyncDivision::ALL
                                            .iter()
                                            .map(|d| d.label().to_string())
                                            .collect();
                                        let selected_idx = (current_val.round() as usize)
                                            .min(labels.len().saturating_sub(1));
                                        let selected =
                                            labels.get(selected_idx).cloned().unwrap_or_default();
                                        let option_values = labels
                                            .into_iter()
                                            .map(|label| {
                                                Rc::new(RefCell::new(Value::String(label)))
                                            })
                                            .collect();
                                        pmap.insert(
                                            "text-value".to_string(),
                                            Rc::new(RefCell::new(Value::String(selected))),
                                        );
                                        pmap.insert(
                                            "options".to_string(),
                                            Rc::new(RefCell::new(Value::List(option_values))),
                                        );
                                        pmap.insert(
                                            "min".to_string(),
                                            Rc::new(RefCell::new(Value::Number(0.0))),
                                        );
                                        pmap.insert(
                                            "max".to_string(),
                                            Rc::new(RefCell::new(Value::Number(
                                                (SyncDivision::ALL.len() - 1) as f64,
                                            ))),
                                        );
                                    }
                                    if param_supports_value_binding(pdesc) {
                                        insert_string_prop(
                                            &mut pmap,
                                            "value-field",
                                            bus_effect_param_value_field(
                                                bus_idx,
                                                slot_idx,
                                                param_idx,
                                                &pdesc.name,
                                            ),
                                        );
                                    }
                                }
                            }
                            if let Some(targets) = modulation_targets.get(&param_idx) {
                                insert_mod_metadata(&mut pmap, targets);
                            }
                            insert_param_ui_metadata(&mut pmap, pdesc.ui_metadata.as_ref());
                            Some(Rc::new(RefCell::new(Value::Map(pmap))))
                        })
                        .collect();

                    let source_actual =
                        selected_bus_voice_mod_source_indices(desc, slot, plock_step);
                    let mut source_sections: Vec<Rc<RefCell<Value>>> = Vec::new();
                    let mut source_names: Vec<Rc<RefCell<Value>>> = Vec::new();
                    for slot_number in 1..=sequencer::voice_modulator::SLOT_COUNT {
                        let section_name =
                            sequencer::voice_modulator::modulator_slot_label(slot_number, "");
                        let mut section_params: Vec<Rc<RefCell<Value>>> = Vec::new();
                        let mut source_param: Option<Rc<RefCell<Value>>> = None;
                        for &param_idx in &source_actual {
                            let Some(pdesc) = desc.params.get(param_idx) else {
                                continue;
                            };
                            if sequencer::voice_modulator::slot_from_param_name(&pdesc.name)
                                != Some(slot_number)
                            {
                                continue;
                            }
                            let current_val =
                                bus_slot_param_stored_value(slot, desc, param_idx, plock_step);
                            let mut pmap: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
                            pmap.insert(
                                "name".to_string(),
                                Rc::new(RefCell::new(Value::String(rename_source_param(
                                    &pdesc.name,
                                )))),
                            );
                            pmap.insert(
                                "idx".to_string(),
                                Rc::new(RefCell::new(Value::Number(param_idx as f64))),
                            );
                            pmap.insert(
                                "value".to_string(),
                                Rc::new(RefCell::new(Value::Number(
                                    pdesc.stored_to_user(current_val) as f64,
                                ))),
                            );
                            pmap.insert(
                                "min".to_string(),
                                Rc::new(RefCell::new(Value::Number(
                                    pdesc.stored_to_user(pdesc.min) as f64,
                                ))),
                            );
                            pmap.insert(
                                "max".to_string(),
                                Rc::new(RefCell::new(Value::Number(
                                    pdesc.stored_to_user(pdesc.max) as f64,
                                ))),
                            );
                            match &pdesc.kind {
                                ParamKind::Boolean => {
                                    pmap.insert(
                                        "boolean".to_string(),
                                        Rc::new(RefCell::new(Value::Bool(true))),
                                    );
                                }
                                ParamKind::Enum { labels } => {
                                    let selected = labels
                                        .get(current_val.round() as usize)
                                        .cloned()
                                        .unwrap_or_default();
                                    let option_values = labels
                                        .iter()
                                        .filter(|label| {
                                            !(is_source_param(pdesc.node_param_idx)
                                                && label.as_str() == "env")
                                        })
                                        .cloned()
                                        .map(|label| Rc::new(RefCell::new(Value::String(label))))
                                        .collect();
                                    pmap.insert(
                                        "text-value".to_string(),
                                        Rc::new(RefCell::new(Value::String(selected))),
                                    );
                                    pmap.insert(
                                        "options".to_string(),
                                        Rc::new(RefCell::new(Value::List(option_values))),
                                    );
                                }
                                ParamKind::Continuous { .. } => {}
                            }
                            insert_string_prop(
                                &mut pmap,
                                "value-field",
                                bus_effect_param_value_field(
                                    bus_idx,
                                    slot_idx,
                                    param_idx,
                                    &pdesc.name,
                                ),
                            );
                            let param_value = Rc::new(RefCell::new(Value::Map(pmap)));
                            if sequencer::voice_modulator::source_type_name_from_param_name(
                                &pdesc.name,
                            ) == Some("source")
                            {
                                source_param = Some(param_value);
                            } else {
                                section_params.push(param_value);
                            }
                        }
                        source_names
                            .push(Rc::new(RefCell::new(Value::String(section_name.clone()))));
                        let mut section_map: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
                        section_map.insert(
                            "name".to_string(),
                            Rc::new(RefCell::new(Value::String(section_name))),
                        );
                        section_map.insert(
                            "slot".to_string(),
                            Rc::new(RefCell::new(Value::Number(slot_number as f64))),
                        );
                        if let Some(source_param) = source_param {
                            section_map.insert("source-param".to_string(), source_param);
                        }
                        section_map.insert(
                            "params".to_string(),
                            Rc::new(RefCell::new(Value::List(section_params))),
                        );
                        source_sections.push(Rc::new(RefCell::new(Value::Map(section_map))));
                    }

                    slot_map.insert(
                        "params".to_string(),
                        Rc::new(RefCell::new(Value::List(params))),
                    );
                    slot_map.insert(
                        "modulators".to_string(),
                        Rc::new(RefCell::new(Value::List(
                            desc.instrument_modulators
                                .iter()
                                .map(|modulator| {
                                    let mut map: HashMap<String, Rc<RefCell<Value>>> =
                                        HashMap::new();
                                    map.insert(
                                        "slot".to_string(),
                                        Rc::new(RefCell::new(Value::Number(modulator.slot as f64))),
                                    );
                                    map.insert(
                                        "label".to_string(),
                                        Rc::new(RefCell::new(Value::String(
                                            modulator.label.clone(),
                                        ))),
                                    );
                                    Rc::new(RefCell::new(Value::Map(map)))
                                })
                                .collect(),
                        ))),
                    );
                    slot_map.insert(
                        "source-names".to_string(),
                        Rc::new(RefCell::new(Value::List(source_names))),
                    );
                    slot_map.insert(
                        "sources".to_string(),
                        Rc::new(RefCell::new(Value::List(source_sections))),
                    );
                    Rc::new(RefCell::new(Value::Map(slot_map)))
                })
                .collect();
            Rc::new(RefCell::new(Value::List(slots)))
        })
        .collect();

    Value::List(buses)
}

pub(crate) fn build_midi_effects_value(
    state: &Arc<SequencerState>,
    track: usize,
    selected: &Arc<Mutex<HashSet<usize>>>,
) -> Value {
    use sequencer::effects::{EffectDescriptor, ParamKind};
    use std::collections::HashMap;

    let descriptors = sequencer::lisp_host::load_midi_fx_descriptors();
    let descriptor_for = |name: &str| -> Option<EffectDescriptor> {
        descriptors
            .iter()
            .find(|desc| desc.name.eq_ignore_ascii_case(name))
            .cloned()
    };
    let Some(track_params) = state.pattern.track_params.get(track) else {
        return Value::List(vec![]);
    };
    let chain = track_params.midi_fx_chain();
    let sel = selected.lock().unwrap();
    let plock_step = sel.iter().copied().min();

    let slots: Vec<Rc<RefCell<Value>>> = chain
        .iter()
        .enumerate()
        .filter_map(|(slot_idx, name)| {
            let desc = descriptor_for(name)?;
            let slot = state
                .pattern
                .midi_fx_slots
                .get(track)
                .and_then(|slots| slots.get(slot_idx));
            let mut slot_map: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
            slot_map.insert(
                "name".to_string(),
                Rc::new(RefCell::new(Value::String(desc.name.clone()))),
            );
            slot_map.insert(
                "slot-idx".to_string(),
                Rc::new(RefCell::new(Value::Number(slot_idx as f64))),
            );
            slot_map.insert(
                "midi-fx".to_string(),
                Rc::new(RefCell::new(Value::Bool(true))),
            );

            let params: Vec<Rc<RefCell<Value>>> = desc
                .params
                .iter()
                .enumerate()
                .map(|(param_idx, pdesc)| {
                    let default_val = slot
                        .map(|s| {
                            if param_idx < s.num_params.load(Ordering::Relaxed) as usize {
                                s.defaults.get(param_idx)
                            } else {
                                pdesc.default
                            }
                        })
                        .unwrap_or(pdesc.default);
                    let current_val = plock_step
                        .and_then(|step| slot.and_then(|s| s.plocks.get(step, param_idx)))
                        .unwrap_or(default_val);
                    let mut pmap: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
                    pmap.insert(
                        "name".to_string(),
                        Rc::new(RefCell::new(Value::String(pdesc.name.clone()))),
                    );
                    pmap.insert(
                        "idx".to_string(),
                        Rc::new(RefCell::new(Value::Number(param_idx as f64))),
                    );
                    pmap.insert(
                        "value".to_string(),
                        Rc::new(RefCell::new(Value::Number(current_val as f64))),
                    );
                    pmap.insert(
                        "min".to_string(),
                        Rc::new(RefCell::new(Value::Number(pdesc.min as f64))),
                    );
                    pmap.insert(
                        "max".to_string(),
                        Rc::new(RefCell::new(Value::Number(pdesc.max as f64))),
                    );
                    match &pdesc.kind {
                        ParamKind::Boolean => {
                            pmap.insert(
                                "boolean".to_string(),
                                Rc::new(RefCell::new(Value::Bool(true))),
                            );
                            if param_supports_value_binding(pdesc) {
                                insert_string_prop(
                                    &mut pmap,
                                    "value-field",
                                    midi_fx_param_value_field(
                                        track,
                                        slot_idx,
                                        param_idx,
                                        &pdesc.name,
                                    ),
                                );
                            }
                        }
                        ParamKind::Enum { labels } => {
                            let selected = labels
                                .get(current_val.round() as usize)
                                .cloned()
                                .unwrap_or_default();
                            let option_values = labels
                                .iter()
                                .cloned()
                                .map(|label| Rc::new(RefCell::new(Value::String(label))))
                                .collect();
                            pmap.insert(
                                "text-value".to_string(),
                                Rc::new(RefCell::new(Value::String(selected))),
                            );
                            pmap.insert(
                                "options".to_string(),
                                Rc::new(RefCell::new(Value::List(option_values))),
                            );
                            if param_supports_value_binding(pdesc) {
                                insert_string_prop(
                                    &mut pmap,
                                    "value-field",
                                    midi_fx_param_value_field(
                                        track,
                                        slot_idx,
                                        param_idx,
                                        &pdesc.name,
                                    ),
                                );
                            }
                        }
                        ParamKind::Continuous { .. } => {
                            if param_supports_value_binding(pdesc) {
                                insert_string_prop(
                                    &mut pmap,
                                    "value-field",
                                    midi_fx_param_value_field(
                                        track,
                                        slot_idx,
                                        param_idx,
                                        &pdesc.name,
                                    ),
                                );
                            }
                        }
                    }
                    Rc::new(RefCell::new(Value::Map(pmap)))
                })
                .collect();
            slot_map.insert(
                "params".to_string(),
                Rc::new(RefCell::new(Value::List(params))),
            );
            Some(Rc::new(RefCell::new(Value::Map(slot_map))))
        })
        .collect();

    Value::List(slots)
}

pub(crate) fn build_sampler_panel_value(
    app: &app::App,
    track: usize,
    selected: &Arc<Mutex<HashSet<usize>>>,
) -> Value {
    use std::collections::HashMap;

    fn is_mod_param(name: &str) -> bool {
        name.starts_with("mod ")
    }

    fn is_generated_host_mod_param(name: &str) -> bool {
        name.starts_with("__host_mod__")
    }

    fn is_hidden_dgen_mod_param(name: &str) -> bool {
        name.starts_with("__dgen_mod_active__")
    }

    fn is_source_param(node_param_idx: u32) -> bool {
        sequencer::voice_modulator::is_source_param(node_param_idx)
    }

    fn rename_source_param(name: &str) -> String {
        sequencer::voice_modulator::source_param_display_name(name)
    }

    app.publish_sampler_analysis_runtime(track);

    let sel = selected.lock().unwrap();
    let plock_step = sel.iter().copied().min();
    let slot = &app.state.pattern.instrument_slots[track];
    let desc = app
        .graph
        .instrument_descriptors
        .get(track)
        .cloned()
        .unwrap_or_else(sequencer::effects::EffectDescriptor::builtin_sampler);

    // Look up the pre-registered SampleBuffer and pass its Value map directly
    // to the Lisp side, so the waveform widget can use it without re-loading.
    let sampler_path = app.sampler_path_for_track(track);
    let registered_sample = sampler_path.as_ref().and_then(|p| {
        let key = p.display().to_string();
        eseqlisp::audio::sample::get_registered_sample(&key).or_else(|| {
            match eseqlisp::audio::sample::SampleBuffer::load_wav(p) {
                Ok(sample) => {
                    sample.register();
                    eseqlisp::audio::sample::get_registered_sample(&key)
                }
                Err(e) => {
                    eprintln!("waveform: failed to register sample {}: {e}", p.display());
                    None
                }
            }
        })
    });
    let buffer_value = registered_sample.as_ref().map(|s| s.to_value());
    let sample_duration = registered_sample
        .as_ref()
        .map(|s| s.duration_seconds)
        .unwrap_or(1.0);

    struct UiModMetadata {
        source_param_idx: Option<usize>,
        depth_param_idx: usize,
        source_slot: f32,
        source_value_field: Option<String>,
        depth_value: f32,
        depth_value_field: Option<String>,
        depth_min: f32,
        depth_max: f32,
        depth_unit: Option<String>,
    }

    fn insert_mod_metadata(
        pmap: &mut HashMap<String, Rc<RefCell<Value>>>,
        targets: &[UiModMetadata],
    ) {
        pmap.insert(
            "modulatable".to_string(),
            Rc::new(RefCell::new(Value::Bool(true))),
        );
        let target_values = targets
            .iter()
            .map(|meta| {
                let mut target = HashMap::new();
                if let Some(source_param_idx) = meta.source_param_idx {
                    target.insert(
                        "source-idx".to_string(),
                        Rc::new(RefCell::new(Value::Number(source_param_idx as f64))),
                    );
                }
                target.insert(
                    "depth-idx".to_string(),
                    Rc::new(RefCell::new(Value::Number(meta.depth_param_idx as f64))),
                );
                target.insert(
                    "source-slot".to_string(),
                    Rc::new(RefCell::new(Value::Number(meta.source_slot as f64))),
                );
                if let Some(field) = &meta.source_value_field {
                    target.insert(
                        "source-value-field".to_string(),
                        Rc::new(RefCell::new(Value::String(field.clone()))),
                    );
                }
                target.insert(
                    "depth".to_string(),
                    Rc::new(RefCell::new(Value::Number(meta.depth_value as f64))),
                );
                if let Some(field) = &meta.depth_value_field {
                    target.insert(
                        "depth-value-field".to_string(),
                        Rc::new(RefCell::new(Value::String(field.clone()))),
                    );
                }
                target.insert(
                    "depth-min".to_string(),
                    Rc::new(RefCell::new(Value::Number(meta.depth_min as f64))),
                );
                target.insert(
                    "depth-max".to_string(),
                    Rc::new(RefCell::new(Value::Number(meta.depth_max as f64))),
                );
                if let Some(unit) = &meta.depth_unit {
                    target.insert(
                        "depth-unit".to_string(),
                        Rc::new(RefCell::new(Value::String(unit.clone()))),
                    );
                }
                Rc::new(RefCell::new(Value::Map(target)))
            })
            .collect();
        pmap.insert(
            "mod-targets".to_string(),
            Rc::new(RefCell::new(Value::List(target_values))),
        );
    }

    let mut modulation_targets: HashMap<usize, Vec<UiModMetadata>> = HashMap::new();
    for target in desc
        .instrument_modulation_targets
        .iter()
        .filter_map(|target| {
            let depth_desc = desc.params.get(target.depth_param_idx)?;
            let source_default = if let Some(source_param_idx) = target.source_param_idx {
                if source_param_idx < slot.num_params.load(Ordering::Relaxed) as usize {
                    slot.defaults.get(source_param_idx)
                } else {
                    desc.params.get(source_param_idx)?.default
                }
            } else {
                target.modulator_slot as f32
            };
            let depth_default =
                if target.depth_param_idx < slot.num_params.load(Ordering::Relaxed) as usize {
                    slot.defaults.get(target.depth_param_idx)
                } else {
                    depth_desc.default
                };
            let source_current = target
                .source_param_idx
                .and_then(|source_param_idx| {
                    plock_step.and_then(|step| slot.plocks.get(step, source_param_idx))
                })
                .unwrap_or(source_default);
            let depth_current = plock_step
                .and_then(|step| slot.plocks.get(step, target.depth_param_idx))
                .unwrap_or(depth_default);
            let (depth_min, depth_max) = sampler_modulation_depth_display_range(depth_desc, target);
            Some((
                target.base_param_idx,
                UiModMetadata {
                    source_param_idx: target.source_param_idx,
                    depth_param_idx: target.depth_param_idx,
                    source_slot: target
                        .source_param_idx
                        .and_then(|source_param_idx| {
                            desc.params
                                .get(source_param_idx)
                                .map(|source_desc| source_desc.stored_to_user(source_current))
                        })
                        .unwrap_or(source_current),
                    source_value_field: target.source_param_idx.map(|source_param_idx| {
                        let source_desc = &desc.params[source_param_idx];
                        instrument_param_value_field(track, source_param_idx, &source_desc.name)
                    }),
                    depth_value: depth_desc.stored_to_user(depth_current),
                    depth_value_field: Some(instrument_param_value_field(
                        track,
                        target.depth_param_idx,
                        &depth_desc.name,
                    )),
                    depth_min,
                    depth_max,
                    depth_unit: target.depth_unit.clone(),
                },
            ))
        })
    {
        modulation_targets
            .entry(target.0)
            .or_default()
            .push(target.1);
    }

    let mut synth_params: Vec<Rc<RefCell<Value>>> = Vec::new();
    let mut mod_params: Vec<Rc<RefCell<Value>>> = Vec::new();
    let mut source_params_by_slot: HashMap<usize, Vec<Rc<RefCell<Value>>>> = HashMap::new();
    let mut source_type_param_by_slot: HashMap<usize, Rc<RefCell<Value>>> = HashMap::new();
    let visible_source_indices: std::collections::HashSet<usize> =
        selected_voice_mod_source_indices(&desc, slot, plock_step)
            .into_iter()
            .collect();
    let base_note = f32::from_bits(
        app.state.pattern.instrument_base_note_offsets[track].load(Ordering::Relaxed),
    );
    {
        let mut pmap: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
        pmap.insert(
            "name".to_string(),
            Rc::new(RefCell::new(Value::String("base".to_string()))),
        );
        pmap.insert(
            "control".to_string(),
            Rc::new(RefCell::new(Value::String("base-note".to_string()))),
        );
        pmap.insert(
            "value".to_string(),
            Rc::new(RefCell::new(Value::Number(base_note as f64))),
        );
        pmap.insert(
            "min".to_string(),
            Rc::new(RefCell::new(Value::Number(-48.0))),
        );
        pmap.insert(
            "max".to_string(),
            Rc::new(RefCell::new(Value::Number(48.0))),
        );
        insert_string_prop(
            &mut pmap,
            "value-field",
            instrument_base_note_value_field(track),
        );
        synth_params.push(Rc::new(RefCell::new(Value::Map(pmap))));
    }
    for (param_idx, pdesc) in desc.params.iter().enumerate() {
        let default_val = if param_idx < slot.num_params.load(Ordering::Relaxed) as usize {
            slot.defaults.get(param_idx)
        } else {
            pdesc.default
        };
        let current_val = plock_step
            .and_then(|step| slot.plocks.get(step, param_idx))
            .unwrap_or(default_val);
        let mut pmap: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
        pmap.insert(
            "name".to_string(),
            Rc::new(RefCell::new(Value::String(pdesc.name.clone()))),
        );
        pmap.insert(
            "idx".to_string(),
            Rc::new(RefCell::new(Value::Number(param_idx as f64))),
        );
        pmap.insert(
            "control".to_string(),
            Rc::new(RefCell::new(Value::String("param".to_string()))),
        );
        pmap.insert(
            "value".to_string(),
            Rc::new(RefCell::new(Value::Number(
                pdesc.stored_to_user(current_val) as f64,
            ))),
        );
        pmap.insert(
            "min".to_string(),
            Rc::new(RefCell::new(Value::Number(
                pdesc.stored_to_user(pdesc.min) as f64
            ))),
        );
        pmap.insert(
            "max".to_string(),
            Rc::new(RefCell::new(Value::Number(
                pdesc.stored_to_user(pdesc.max) as f64
            ))),
        );
        match &pdesc.kind {
            sequencer::effects::ParamKind::Boolean => {
                pmap.insert(
                    "boolean".to_string(),
                    Rc::new(RefCell::new(Value::Bool(true))),
                );
                if param_supports_value_binding(pdesc) {
                    insert_string_prop(
                        &mut pmap,
                        "value-field",
                        instrument_param_value_field(track, param_idx, &pdesc.name),
                    );
                }
            }
            sequencer::effects::ParamKind::Enum { labels } => {
                let selected = labels
                    .get(current_val.round() as usize)
                    .cloned()
                    .unwrap_or_default();
                let option_values = labels
                    .iter()
                    .cloned()
                    .map(|label| Rc::new(RefCell::new(Value::String(label))))
                    .collect();
                pmap.insert(
                    "text-value".to_string(),
                    Rc::new(RefCell::new(Value::String(selected))),
                );
                pmap.insert(
                    "options".to_string(),
                    Rc::new(RefCell::new(Value::List(option_values))),
                );
                if param_supports_value_binding(pdesc) {
                    insert_string_prop(
                        &mut pmap,
                        "value-field",
                        instrument_param_value_field(track, param_idx, &pdesc.name),
                    );
                }
            }
            sequencer::effects::ParamKind::Continuous { .. } => {
                if param_supports_value_binding(pdesc) {
                    insert_string_prop(
                        &mut pmap,
                        "value-field",
                        instrument_param_value_field(track, param_idx, &pdesc.name),
                    );
                }
            }
        }
        if is_generated_host_mod_param(&pdesc.name) || is_hidden_dgen_mod_param(&pdesc.name) {
            continue;
        }
        if is_source_param(pdesc.node_param_idx) {
            if let Some(Value::String(name)) = pmap.get("name").map(|v| v.borrow().clone()) {
                pmap.insert(
                    "name".to_string(),
                    Rc::new(RefCell::new(Value::String(rename_source_param(&name)))),
                );
            }
            if let Some(slot_number) = sequencer::voice_modulator::slot_from_param_name(&pdesc.name)
            {
                let param_value = Rc::new(RefCell::new(Value::Map(pmap)));
                if sequencer::voice_modulator::source_type_name_from_param_name(&pdesc.name)
                    == Some("source")
                {
                    source_type_param_by_slot.insert(slot_number, param_value);
                } else if visible_source_indices.contains(&param_idx) {
                    source_params_by_slot
                        .entry(slot_number)
                        .or_default()
                        .push(param_value);
                }
            }
        } else if is_mod_param(&pdesc.name) {
            if let Some(Value::String(name)) = pmap.get("name").map(|v| v.borrow().clone()) {
                pmap.insert(
                    "name".to_string(),
                    Rc::new(RefCell::new(Value::String(
                        name.strip_prefix("mod ").unwrap_or(&name).to_string(),
                    ))),
                );
            }
            mod_params.push(Rc::new(RefCell::new(Value::Map(pmap))));
        } else {
            if let Some(targets) = modulation_targets.get(&param_idx) {
                insert_mod_metadata(&mut pmap, targets);
            }
            synth_params.push(Rc::new(RefCell::new(Value::Map(pmap))));
        }
    }

    let mut source_sections: Vec<Rc<RefCell<Value>>> = Vec::new();
    let mut source_names: Vec<Rc<RefCell<Value>>> = Vec::new();
    for slot_number in 1..=sequencer::voice_modulator::SLOT_COUNT {
        let section_name = sequencer::voice_modulator::modulator_slot_label(slot_number, "");
        let params = source_params_by_slot
            .remove(&slot_number)
            .unwrap_or_default();
        let source_param = source_type_param_by_slot.remove(&slot_number);
        source_names.push(Rc::new(RefCell::new(Value::String(section_name.clone()))));
        let mut section_map: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
        section_map.insert(
            "name".to_string(),
            Rc::new(RefCell::new(Value::String(section_name))),
        );
        section_map.insert(
            "slot".to_string(),
            Rc::new(RefCell::new(Value::Number(slot_number as f64))),
        );
        if let Some(source_param) = source_param {
            section_map.insert("source-param".to_string(), source_param);
        }
        section_map.insert(
            "params".to_string(),
            Rc::new(RefCell::new(Value::List(params))),
        );
        source_sections.push(Rc::new(RefCell::new(Value::Map(section_map))));
    }

    let mut panel_map: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
    panel_map.insert(
        "type".to_string(),
        Rc::new(RefCell::new(Value::String("sampler".to_string()))),
    );
    panel_map.insert(
        "track".to_string(),
        Rc::new(RefCell::new(Value::Number(track as f64))),
    );
    if let Some(buf_val) = buffer_value {
        panel_map.insert("buffer".to_string(), Rc::new(RefCell::new(buf_val)));
    }
    let buffer_id = app.graph.track_buffer_ids.get(track).copied().unwrap_or(-1);
    let analysis_entry = app.sample_analysis.cache().get(buffer_id);
    let mut analysis_status = "none".to_string();
    let mut analysis_message = String::new();
    let mut onset_values: Vec<Rc<RefCell<Value>>> = Vec::new();
    if let Some(entry) = analysis_entry {
        match entry.as_ref() {
            sequencer::analysis::AnalysisEntry::Pending => {
                analysis_status = "pending".to_string();
                analysis_message = "Analyzing...".to_string();
            }
            sequencer::analysis::AnalysisEntry::Ready(result) => {
                analysis_status = "ready".to_string();
                analysis_message = format!("{:.1} BPM", result.bpm);
                panel_map.insert(
                    "analysis-bpm".to_string(),
                    Rc::new(RefCell::new(Value::Number(result.bpm as f64))),
                );
                panel_map.insert(
                    "analysis-confidence".to_string(),
                    Rc::new(RefCell::new(Value::Number(result.bpm_confidence as f64))),
                );
                if let Some(frame) = result.downbeat_frame {
                    let seconds = frame as f64 / app.graph.sample_rate.max(1) as f64;
                    panel_map.insert(
                        "downbeat-time".to_string(),
                        Rc::new(RefCell::new(Value::Number(seconds))),
                    );
                }
                onset_values = result
                    .onsets_frames
                    .iter()
                    .map(|frame| {
                        Rc::new(RefCell::new(Value::Number(
                            *frame as f64 / app.graph.sample_rate.max(1) as f64,
                        )))
                    })
                    .collect();
            }
            sequencer::analysis::AnalysisEntry::Failed(error) => {
                analysis_status = "failed".to_string();
                analysis_message = error.clone();
            }
        }
    }
    panel_map.insert(
        "analysis-status".to_string(),
        Rc::new(RefCell::new(Value::String(analysis_status))),
    );
    panel_map.insert(
        "analysis-message".to_string(),
        Rc::new(RefCell::new(Value::String(analysis_message))),
    );
    panel_map.insert(
        "onsets".to_string(),
        Rc::new(RefCell::new(Value::List(onset_values))),
    );
    panel_map.insert(
        "params".to_string(),
        Rc::new(RefCell::new(Value::List(synth_params.clone()))),
    );
    panel_map.insert(
        "synth".to_string(),
        Rc::new(RefCell::new(Value::List(synth_params))),
    );
    panel_map.insert(
        "mod".to_string(),
        Rc::new(RefCell::new(Value::List(mod_params))),
    );
    panel_map.insert(
        "modulators".to_string(),
        Rc::new(RefCell::new(Value::List(
            desc.instrument_modulators
                .iter()
                .map(|modulator| {
                    let mut map: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
                    map.insert(
                        "slot".to_string(),
                        Rc::new(RefCell::new(Value::Number(modulator.slot as f64))),
                    );
                    map.insert(
                        "label".to_string(),
                        Rc::new(RefCell::new(Value::String(modulator.label.clone()))),
                    );
                    Rc::new(RefCell::new(Value::Map(map)))
                })
                .collect(),
        ))),
    );
    panel_map.insert(
        "source-names".to_string(),
        Rc::new(RefCell::new(Value::List(source_names))),
    );
    panel_map.insert(
        "sources".to_string(),
        Rc::new(RefCell::new(Value::List(source_sections))),
    );
    // Start/end as seconds for the waveform selection overlay.
    // Raw stored values are 0.0-1.0 normalized; multiply by duration.
    let start_raw = plock_step
        .and_then(|step| slot.plocks.get(step, 2))
        .unwrap_or_else(|| slot.defaults.get(2));
    let end_raw = plock_step
        .and_then(|step| slot.plocks.get(step, 3))
        .unwrap_or_else(|| slot.defaults.get(3));
    panel_map.insert(
        "start-time".to_string(),
        Rc::new(RefCell::new(Value::Number(
            (start_raw as f64) * sample_duration,
        ))),
    );
    insert_string_prop(
        &mut panel_map,
        "start-time-field",
        sampler_selection_time_field(track, "start"),
    );
    panel_map.insert(
        "end-time".to_string(),
        Rc::new(RefCell::new(Value::Number(
            (end_raw as f64) * sample_duration,
        ))),
    );
    insert_string_prop(
        &mut panel_map,
        "end-time-field",
        sampler_selection_time_field(track, "end"),
    );
    panel_map.insert(
        "duration".to_string(),
        Rc::new(RefCell::new(Value::Number(sample_duration))),
    );

    Value::List(vec![Rc::new(RefCell::new(Value::Map(panel_map)))])
}

pub(crate) fn build_instrument_panel_value(
    app: &app::App,
    track: usize,
    selected: &Arc<Mutex<HashSet<usize>>>,
) -> Value {
    use std::collections::HashMap;

    if app.graph.track_instrument_types.get(track)
        == Some(&sequencer::sequencer::InstrumentType::Rack)
    {
        return build_rack_panel_value(app, track, selected);
    }
    if app.is_sampler_track(track) {
        return build_sampler_panel_value(app, track, selected);
    }
    let Some(desc) = app.graph.instrument_descriptors.get(track) else {
        return Value::List(vec![]);
    };
    if desc.params.is_empty() && desc.tensor_params.is_empty() {
        return Value::List(vec![]);
    }

    let sel = selected.lock().unwrap();
    let plock_step = sel.iter().copied().min();
    let slot = &app.state.pattern.instrument_slots[track];
    let base_note_default = f32::from_bits(
        app.state.pattern.instrument_base_note_offsets[track].load(Ordering::Relaxed),
    );
    let base_note_current = base_note_default;

    struct UiModMetadata {
        source_param_idx: Option<usize>,
        depth_param_idx: usize,
        source_slot: f32,
        source_value_field: Option<String>,
        depth_value: f32,
        depth_value_field: Option<String>,
        depth_min: f32,
        depth_max: f32,
        depth_unit: Option<String>,
    }

    fn push_param(
        out: &mut Vec<Rc<RefCell<Value>>>,
        name: String,
        control: &str,
        idx: Option<usize>,
        value: f32,
        min: f32,
        max: f32,
        options: Option<&Vec<String>>,
        value_field: Option<String>,
        mod_targets: Option<&Vec<UiModMetadata>>,
        ui_metadata: Option<&sequencer::effects::ParamUiMetadata>,
        key_locks: &[(u8, f32)],
    ) {
        let is_boolean_name = name == "enabled" || name == "sync";
        let mut pmap: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
        pmap.insert(
            "name".to_string(),
            Rc::new(RefCell::new(Value::String(name))),
        );
        pmap.insert(
            "control".to_string(),
            Rc::new(RefCell::new(Value::String(control.to_string()))),
        );
        if let Some(idx) = idx {
            pmap.insert(
                "idx".to_string(),
                Rc::new(RefCell::new(Value::Number(idx as f64))),
            );
        }
        pmap.insert(
            "value".to_string(),
            Rc::new(RefCell::new(Value::Number(value as f64))),
        );
        pmap.insert(
            "min".to_string(),
            Rc::new(RefCell::new(Value::Number(min as f64))),
        );
        pmap.insert(
            "max".to_string(),
            Rc::new(RefCell::new(Value::Number(max as f64))),
        );
        if let Some(labels) = options {
            let selected = labels
                .get(value.round() as usize)
                .cloned()
                .unwrap_or_default();
            let option_values = labels
                .iter()
                .cloned()
                .map(|label| Rc::new(RefCell::new(Value::String(label))))
                .collect();
            pmap.insert(
                "text-value".to_string(),
                Rc::new(RefCell::new(Value::String(selected))),
            );
            pmap.insert(
                "options".to_string(),
                Rc::new(RefCell::new(Value::List(option_values))),
            );
        }
        if options.is_none() && is_boolean_name {
            pmap.insert(
                "boolean".to_string(),
                Rc::new(RefCell::new(Value::Bool(true))),
            );
        }
        if let Some(value_field) = value_field {
            insert_string_prop(&mut pmap, "value-field", value_field);
        }
        if !key_locks.is_empty() {
            let rows = key_locks
                .iter()
                .map(|(note, value)| {
                    let mut row = HashMap::new();
                    row.insert(
                        "note".to_string(),
                        Rc::new(RefCell::new(Value::Number(*note as f64))),
                    );
                    row.insert(
                        "value".to_string(),
                        Rc::new(RefCell::new(Value::Number(*value as f64))),
                    );
                    Rc::new(RefCell::new(Value::Map(row)))
                })
                .collect();
            pmap.insert(
                "key-locks".to_string(),
                Rc::new(RefCell::new(Value::List(rows))),
            );
        }
        if let Some(targets) = mod_targets {
            pmap.insert(
                "modulatable".to_string(),
                Rc::new(RefCell::new(Value::Bool(true))),
            );
            let target_values = targets
                .iter()
                .map(|meta| {
                    let mut target = HashMap::new();
                    if let Some(source_param_idx) = meta.source_param_idx {
                        target.insert(
                            "source-idx".to_string(),
                            Rc::new(RefCell::new(Value::Number(source_param_idx as f64))),
                        );
                    }
                    target.insert(
                        "depth-idx".to_string(),
                        Rc::new(RefCell::new(Value::Number(meta.depth_param_idx as f64))),
                    );
                    target.insert(
                        "source-slot".to_string(),
                        Rc::new(RefCell::new(Value::Number(meta.source_slot as f64))),
                    );
                    if let Some(field) = &meta.source_value_field {
                        target.insert(
                            "source-value-field".to_string(),
                            Rc::new(RefCell::new(Value::String(field.clone()))),
                        );
                    }
                    target.insert(
                        "depth".to_string(),
                        Rc::new(RefCell::new(Value::Number(meta.depth_value as f64))),
                    );
                    if let Some(field) = &meta.depth_value_field {
                        target.insert(
                            "depth-value-field".to_string(),
                            Rc::new(RefCell::new(Value::String(field.clone()))),
                        );
                    }
                    target.insert(
                        "depth-min".to_string(),
                        Rc::new(RefCell::new(Value::Number(meta.depth_min as f64))),
                    );
                    target.insert(
                        "depth-max".to_string(),
                        Rc::new(RefCell::new(Value::Number(meta.depth_max as f64))),
                    );
                    if let Some(unit) = &meta.depth_unit {
                        target.insert(
                            "depth-unit".to_string(),
                            Rc::new(RefCell::new(Value::String(unit.clone()))),
                        );
                    }
                    Rc::new(RefCell::new(Value::Map(target)))
                })
                .collect();
            pmap.insert(
                "mod-targets".to_string(),
                Rc::new(RefCell::new(Value::List(target_values))),
            );
        }
        insert_param_ui_metadata(&mut pmap, ui_metadata);
        out.push(Rc::new(RefCell::new(Value::Map(pmap))));
    }

    fn is_mod_param(name: &str) -> bool {
        name.starts_with("mod ")
    }

    fn is_generated_host_mod_param(name: &str) -> bool {
        name.starts_with("__host_mod__")
    }

    fn is_hidden_dgen_mod_param(name: &str) -> bool {
        name.starts_with("__dgen_mod_active__")
    }

    fn is_source_param(node_param_idx: u32) -> bool {
        sequencer::voice_modulator::is_source_param(node_param_idx)
    }

    fn rename_source_param(name: &str) -> String {
        sequencer::voice_modulator::source_param_display_name(name)
    }

    let source_actual = selected_voice_mod_source_indices(desc, slot, plock_step);
    let slot_num_params = slot.num_params.load(Ordering::Relaxed) as usize;
    let mut key_locks_by_param = vec![Vec::<(u8, f32)>::new(); desc.params.len()];
    for note in 0..sequencer::effects::MAX_MIDI_NOTES {
        let note = note as u8;
        if !slot.key_locks.note_has_any_lock(note, slot_num_params) {
            continue;
        }
        for (param_idx, pdesc) in desc.params.iter().enumerate().take(slot_num_params) {
            let Some(value) = slot.key_locks.get(note, param_idx) else {
                continue;
            };
            if slot.key_locks.get_id(note, param_idx) != slot.param_node_id(param_idx) {
                continue;
            }
            if let Some(rows) = key_locks_by_param.get_mut(param_idx) {
                rows.push((note, pdesc.stored_to_user(value)));
            }
        }
    }
    let key_lock_assignments = app
        .state
        .reconcile_key_lock_variant_registry_for_track(track);
    let key_lock_note_variants = key_lock_assignments
        .iter()
        .enumerate()
        .filter_map(|(note, assignment)| {
            let assignment = assignment.as_ref()?;
            let mut map = HashMap::new();
            map.insert(
                "note".to_string(),
                Rc::new(RefCell::new(Value::Number(note as f64))),
            );
            map.insert(
                "label".to_string(),
                Rc::new(RefCell::new(Value::String(assignment.label.clone()))),
            );
            map.insert(
                "count".to_string(),
                Rc::new(RefCell::new(Value::Number(assignment.param_count as f64))),
            );
            map.insert(
                "color-r".to_string(),
                Rc::new(RefCell::new(Value::Number(assignment.color[0] as f64))),
            );
            map.insert(
                "color-g".to_string(),
                Rc::new(RefCell::new(Value::Number(assignment.color[1] as f64))),
            );
            map.insert(
                "color-b".to_string(),
                Rc::new(RefCell::new(Value::Number(assignment.color[2] as f64))),
            );
            Some(Rc::new(RefCell::new(Value::Map(map))))
        })
        .collect::<Vec<_>>();
    let mut key_lock_variant_items = Vec::new();
    let mut def_map = HashMap::new();
    def_map.insert(
        "kind".to_string(),
        Rc::new(RefCell::new(Value::String("def".to_string()))),
    );
    def_map.insert(
        "label".to_string(),
        Rc::new(RefCell::new(Value::String("def".to_string()))),
    );
    def_map.insert(
        "display".to_string(),
        Rc::new(RefCell::new(Value::String("base".to_string()))),
    );
    def_map.insert(
        "count".to_string(),
        Rc::new(RefCell::new(Value::Number(0.0))),
    );
    def_map.insert(
        "color-r".to_string(),
        Rc::new(RefCell::new(Value::Number(0.545_098_07))),
    );
    def_map.insert(
        "color-g".to_string(),
        Rc::new(RefCell::new(Value::Number(0.545_098_07))),
    );
    def_map.insert(
        "color-b".to_string(),
        Rc::new(RefCell::new(Value::Number(0.588_235_3))),
    );
    key_lock_variant_items.push(Rc::new(RefCell::new(Value::Map(def_map))));
    for entry in app.state.key_lock_variant_registry_snapshot(track).entries {
        let mut map = HashMap::new();
        map.insert(
            "kind".to_string(),
            Rc::new(RefCell::new(Value::String("variant".to_string()))),
        );
        map.insert(
            "label".to_string(),
            Rc::new(RefCell::new(Value::String(entry.label.clone()))),
        );
        map.insert(
            "display".to_string(),
            Rc::new(RefCell::new(Value::String(
                entry.name.clone().unwrap_or_else(|| entry.label.clone()),
            ))),
        );
        map.insert(
            "count".to_string(),
            Rc::new(RefCell::new(Value::Number(entry.key.param_count() as f64))),
        );
        map.insert(
            "color-r".to_string(),
            Rc::new(RefCell::new(Value::Number(entry.color[0] as f64))),
        );
        map.insert(
            "color-g".to_string(),
            Rc::new(RefCell::new(Value::Number(entry.color[1] as f64))),
        );
        map.insert(
            "color-b".to_string(),
            Rc::new(RefCell::new(Value::Number(entry.color[2] as f64))),
        );
        key_lock_variant_items.push(Rc::new(RefCell::new(Value::Map(map))));
    }

    let mut synth_params: Vec<Rc<RefCell<Value>>> = Vec::new();
    let mut mod_params: Vec<Rc<RefCell<Value>>> = Vec::new();
    let mut modulation_targets: HashMap<usize, Vec<UiModMetadata>> = HashMap::new();
    for target in desc
        .instrument_modulation_targets
        .iter()
        .filter_map(|target| {
            let depth_desc = desc.params.get(target.depth_param_idx)?;
            let source_default = if let Some(source_param_idx) = target.source_param_idx {
                if source_param_idx < slot.num_params.load(Ordering::Relaxed) as usize {
                    slot.defaults.get(source_param_idx)
                } else {
                    desc.params.get(source_param_idx)?.default
                }
            } else {
                target.modulator_slot as f32
            };
            let depth_default =
                if target.depth_param_idx < slot.num_params.load(Ordering::Relaxed) as usize {
                    slot.defaults.get(target.depth_param_idx)
                } else {
                    depth_desc.default
                };
            let source_current = target
                .source_param_idx
                .and_then(|source_param_idx| {
                    plock_step.and_then(|step| slot.plocks.get(step, source_param_idx))
                })
                .unwrap_or(source_default);
            let depth_current = plock_step
                .and_then(|step| slot.plocks.get(step, target.depth_param_idx))
                .unwrap_or(depth_default);
            let (depth_min, depth_max) = instrument_modulation_depth_display_range(target);
            Some((
                target.base_param_idx,
                UiModMetadata {
                    source_param_idx: target.source_param_idx,
                    depth_param_idx: target.depth_param_idx,
                    source_slot: target
                        .source_param_idx
                        .and_then(|source_param_idx| {
                            desc.params
                                .get(source_param_idx)
                                .map(|source_desc| source_desc.stored_to_user(source_current))
                        })
                        .unwrap_or(source_current),
                    source_value_field: target.source_param_idx.map(|source_param_idx| {
                        let source_desc = &desc.params[source_param_idx];
                        instrument_param_value_field(track, source_param_idx, &source_desc.name)
                    }),
                    depth_value: depth_desc.stored_to_user(depth_current),
                    depth_value_field: Some(instrument_param_value_field(
                        track,
                        target.depth_param_idx,
                        &depth_desc.name,
                    )),
                    depth_min,
                    depth_max,
                    depth_unit: target.depth_unit.clone(),
                },
            ))
        })
    {
        modulation_targets
            .entry(target.0)
            .or_default()
            .push(target.1);
    }
    push_param(
        &mut synth_params,
        "base_note".to_string(),
        "base-note",
        None,
        base_note_current,
        -48.0,
        48.0,
        None,
        Some(instrument_base_note_value_field(track)),
        None,
        None,
        &[],
    );

    for (param_idx, pdesc) in desc.params.iter().enumerate() {
        let default_val = if param_idx < slot.num_params.load(Ordering::Relaxed) as usize {
            slot.defaults.get(param_idx)
        } else {
            pdesc.default
        };
        let current_val = plock_step
            .and_then(|step| slot.plocks.get(step, param_idx))
            .unwrap_or(default_val);
        let options = match &pdesc.kind {
            sequencer::effects::ParamKind::Enum { labels } => Some(labels),
            _ => None,
        };
        if is_source_param(pdesc.node_param_idx)
            || is_generated_host_mod_param(&pdesc.name)
            || is_hidden_dgen_mod_param(&pdesc.name)
        {
            continue;
        }
        if is_mod_param(&pdesc.name) {
            let mod_name = pdesc
                .name
                .strip_prefix("mod ")
                .unwrap_or(&pdesc.name)
                .to_string();
            push_param(
                &mut mod_params,
                mod_name,
                "param",
                Some(param_idx),
                pdesc.stored_to_user(current_val),
                pdesc.stored_to_user(pdesc.min),
                pdesc.stored_to_user(pdesc.max),
                options,
                Some(instrument_param_value_field(track, param_idx, &pdesc.name)),
                None,
                None,
                key_locks_by_param
                    .get(param_idx)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]),
            );
        } else {
            push_param(
                &mut synth_params,
                pdesc.name.clone(),
                "param",
                Some(param_idx),
                pdesc.stored_to_user(current_val),
                pdesc.stored_to_user(pdesc.min),
                pdesc.stored_to_user(pdesc.max),
                options,
                Some(instrument_param_value_field(track, param_idx, &pdesc.name)),
                modulation_targets.get(&param_idx),
                pdesc.ui_metadata.as_ref(),
                key_locks_by_param
                    .get(param_idx)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]),
            );
        }
    }

    let mut source_sections: Vec<Rc<RefCell<Value>>> = Vec::new();
    let mut source_names: Vec<Rc<RefCell<Value>>> = Vec::new();
    for slot_number in 1..=sequencer::voice_modulator::SLOT_COUNT {
        let section_name = sequencer::voice_modulator::modulator_slot_label(slot_number, "");
        let mut params: Vec<Rc<RefCell<Value>>> = Vec::new();
        let mut source_param: Option<Rc<RefCell<Value>>> = None;
        for &param_idx in &source_actual {
            let Some(pdesc) = desc.params.get(param_idx) else {
                continue;
            };
            if sequencer::voice_modulator::slot_from_param_name(&pdesc.name) != Some(slot_number) {
                continue;
            }
            let default_val = if param_idx < slot.num_params.load(Ordering::Relaxed) as usize {
                slot.defaults.get(param_idx)
            } else {
                pdesc.default
            };
            let current_val = plock_step
                .and_then(|step| slot.plocks.get(step, param_idx))
                .unwrap_or(default_val);
            let options = match &pdesc.kind {
                sequencer::effects::ParamKind::Enum { labels } => Some(labels),
                _ => None,
            };
            push_param(
                &mut params,
                rename_source_param(&pdesc.name),
                "param",
                Some(param_idx),
                pdesc.stored_to_user(current_val),
                pdesc.stored_to_user(pdesc.min),
                pdesc.stored_to_user(pdesc.max),
                options,
                Some(instrument_param_value_field(track, param_idx, &pdesc.name)),
                None,
                None,
                key_locks_by_param
                    .get(param_idx)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]),
            );
            if sequencer::voice_modulator::source_type_name_from_param_name(&pdesc.name)
                == Some("source")
            {
                source_param = params.pop();
            }
        }
        source_names.push(Rc::new(RefCell::new(Value::String(section_name.clone()))));
        let mut section_map: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
        section_map.insert(
            "name".to_string(),
            Rc::new(RefCell::new(Value::String(section_name))),
        );
        section_map.insert(
            "slot".to_string(),
            Rc::new(RefCell::new(Value::Number(slot_number as f64))),
        );
        if let Some(source_param) = source_param {
            section_map.insert("source-param".to_string(), source_param);
        }
        section_map.insert(
            "params".to_string(),
            Rc::new(RefCell::new(Value::List(params))),
        );
        source_sections.push(Rc::new(RefCell::new(Value::Map(section_map))));
    }

    let mut tensor_params: Vec<Rc<RefCell<Value>>> = Vec::new();
    for (tensor_idx, tensor_desc) in desc.tensor_params.iter().enumerate() {
        let values = slot
            .tensor_params
            .resolved_values(plock_step, tensor_idx)
            .unwrap_or_else(|| tensor_desc.default.clone());
        let value_list = values
            .into_iter()
            .map(|value| Rc::new(RefCell::new(Value::Number(value as f64))))
            .collect();
        let mut tensor_map: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
        tensor_map.insert(
            "idx".to_string(),
            Rc::new(RefCell::new(Value::Number(tensor_idx as f64))),
        );
        tensor_map.insert(
            "name".to_string(),
            Rc::new(RefCell::new(Value::String(tensor_desc.name.clone()))),
        );
        tensor_map.insert(
            "rows".to_string(),
            Rc::new(RefCell::new(Value::Number(tensor_desc.rows() as f64))),
        );
        tensor_map.insert(
            "cols".to_string(),
            Rc::new(RefCell::new(Value::Number(tensor_desc.cols() as f64))),
        );
        tensor_map.insert(
            "min".to_string(),
            Rc::new(RefCell::new(Value::Number(tensor_desc.min as f64))),
        );
        tensor_map.insert(
            "max".to_string(),
            Rc::new(RefCell::new(Value::Number(tensor_desc.max as f64))),
        );
        tensor_map.insert(
            "value-field".to_string(),
            Rc::new(RefCell::new(Value::String(instrument_tensor_value_field(
                track,
                tensor_idx,
                &tensor_desc.name,
            )))),
        );
        tensor_map.insert(
            "value".to_string(),
            Rc::new(RefCell::new(Value::List(value_list))),
        );
        tensor_params.push(Rc::new(RefCell::new(Value::Map(tensor_map))));
    }

    let mut panel_map: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
    let instrument_type = app
        .graph
        .track_instrument_types
        .get(track)
        .copied()
        .unwrap_or(sequencer::sequencer::InstrumentType::Custom);
    let instrument_name = current_custom_instrument_name(app, track).unwrap_or_else(|| {
        if instrument_type == sequencer::sequencer::InstrumentType::Modulator {
            "Modulator".to_string()
        } else {
            "Instrument".to_string()
        }
    });
    let instrument_type_name = match instrument_type {
        sequencer::sequencer::InstrumentType::Sampler => "sampler",
        sequencer::sequencer::InstrumentType::Custom => "custom",
        sequencer::sequencer::InstrumentType::Modulator => "modulator",
        sequencer::sequencer::InstrumentType::Rack => "rack",
    };
    panel_map.insert(
        "type".to_string(),
        Rc::new(RefCell::new(Value::String(
            instrument_type_name.to_string(),
        ))),
    );
    panel_map.insert(
        "track".to_string(),
        Rc::new(RefCell::new(Value::Number(track as f64))),
    );
    panel_map.insert(
        "phase-field".to_string(),
        Rc::new(RefCell::new(Value::String(modulator_phase_field(track)))),
    );
    panel_map.insert(
        "level-field".to_string(),
        Rc::new(RefCell::new(Value::String(modulator_level_field(track)))),
    );
    panel_map.insert(
        "name".to_string(),
        Rc::new(RefCell::new(Value::String(instrument_name.clone()))),
    );
    panel_map.insert(
        "display-name".to_string(),
        Rc::new(RefCell::new(Value::String(instrument_display_name(
            &instrument_name,
        )))),
    );
    panel_map.insert(
        "synth".to_string(),
        Rc::new(RefCell::new(Value::List(synth_params))),
    );
    panel_map.insert(
        "mod".to_string(),
        Rc::new(RefCell::new(Value::List(mod_params))),
    );
    panel_map.insert(
        "tensors".to_string(),
        Rc::new(RefCell::new(Value::List(tensor_params))),
    );
    panel_map.insert(
        "modulators".to_string(),
        Rc::new(RefCell::new(Value::List(
            desc.instrument_modulators
                .iter()
                .map(|modulator| {
                    let mut map: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
                    map.insert(
                        "slot".to_string(),
                        Rc::new(RefCell::new(Value::Number(modulator.slot as f64))),
                    );
                    map.insert(
                        "label".to_string(),
                        Rc::new(RefCell::new(Value::String(modulator.label.clone()))),
                    );
                    Rc::new(RefCell::new(Value::Map(map)))
                })
                .collect(),
        ))),
    );
    panel_map.insert(
        "source-names".to_string(),
        Rc::new(RefCell::new(Value::List(source_names))),
    );
    panel_map.insert(
        "sources".to_string(),
        Rc::new(RefCell::new(Value::List(source_sections))),
    );
    panel_map.insert(
        "key-lock-note-variants".to_string(),
        Rc::new(RefCell::new(Value::List(key_lock_note_variants))),
    );
    panel_map.insert(
        "key-lock-variants".to_string(),
        Rc::new(RefCell::new(Value::List(key_lock_variant_items))),
    );

    Value::List(vec![Rc::new(RefCell::new(Value::Map(panel_map)))])
}

fn rack_slot_type_name(slot: &sequencer::sequencer::RackSlotSnapshot) -> &'static str {
    match slot.instrument_type {
        sequencer::sequencer::InstrumentType::Sampler => "sampler",
        sequencer::sequencer::InstrumentType::Custom => "custom",
        sequencer::sequencer::InstrumentType::Modulator => "modulator",
        sequencer::sequencer::InstrumentType::Rack => "rack",
    }
}

fn rack_slot_raw_name(
    app: &app::App,
    slot_idx: usize,
    slot: &sequencer::sequencer::RackSlotSnapshot,
) -> String {
    match slot.instrument_type {
        sequencer::sequencer::InstrumentType::Sampler => slot
            .sample_id
            .as_ref()
            .map(|(_, name, _)| name.clone())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| format!("Sampler {}", slot_idx + 1)),
        sequencer::sequencer::InstrumentType::Custom
        | sequencer::sequencer::InstrumentType::Modulator => slot
            .track_sound_state
            .engine_id
            .and_then(|engine_id| app.editor.engine_registry.get(engine_id))
            .map(|engine| engine.name.clone())
            .or_else(|| slot.track_sound_state.loaded_preset.clone())
            .unwrap_or_else(|| format!("Instrument {}", slot_idx + 1)),
        sequencer::sequencer::InstrumentType::Rack => format!("Unsupported {}", slot_idx + 1),
    }
}

fn drum_rack_pad_label(pad_note: i32) -> String {
    let name = match pad_note.rem_euclid(12) {
        0 => "C",
        1 => "C#",
        2 => "D",
        3 => "D#",
        4 => "E",
        5 => "F",
        6 => "F#",
        7 => "G",
        8 => "G#",
        9 => "A",
        10 => "A#",
        _ => "B",
    };
    format!("{name}{}", 4 + pad_note.div_euclid(12))
}

fn drum_rack_pad_bank_label(bank_start: i32) -> String {
    let bank_end = (bank_start + sequencer::sequencer::DRUM_RACK_PAD_COUNT as i32 - 1)
        .min(sequencer::sequencer::DRUM_RACK_LAST_PAD_NOTE);
    format!(
        "{} - {}",
        drum_rack_pad_label(bank_start),
        drum_rack_pad_label(bank_end)
    )
}

fn drum_rack_uses_pad_notes(app: &app::App, track: usize) -> bool {
    app.state
        .pattern
        .rack_tracks
        .lock()
        .unwrap()
        .get(track)
        .and_then(Option::as_ref)
        .is_some_and(|rack| rack.routing == sequencer::sequencer::RackRouting::ByPitch)
}

pub(crate) fn build_track_drum_racks_value(app: &app::App) -> Value {
    Value::List(
        (0..app.tracks.len())
            .map(|track| value_cell(Value::Bool(drum_rack_uses_pad_notes(app, track))))
            .collect(),
    )
}

/// The sequencer treats a drum-rack transpose as the pad note that routes to
/// a slot. Keep this display mapping in one place so the pad grid, expanded
/// sequencer, compact sequencer, and step inspector name the same sound.
pub(crate) struct DrumRackSoundOption {
    pub(crate) pad_note: i32,
    slot_idx: usize,
    name: String,
    pad_label: String,
    label: String,
    short_label: String,
}

fn drum_rack_sound_short_label(name: &str) -> String {
    let letters = name
        .chars()
        .filter(|character| character.is_alphabetic())
        .take(3)
        .collect::<String>();
    if letters.is_empty() {
        name.chars()
            .filter(|character| !character.is_whitespace())
            .take(3)
            .collect()
    } else {
        letters
    }
}

pub(crate) fn drum_rack_sound_options(app: &app::App, track: usize) -> Vec<DrumRackSoundOption> {
    let rack = app
        .state
        .pattern
        .rack_tracks
        .lock()
        .unwrap()
        .get(track)
        .cloned()
        .flatten();
    let Some(rack) = rack else {
        return Vec::new();
    };
    if rack.routing != sequencer::sequencer::RackRouting::ByPitch {
        return Vec::new();
    }
    let mut options = rack
        .slots
        .iter()
        .enumerate()
        .filter_map(|(slot_idx, slot)| {
            let pad_note = slot.pad_note?;
            let name = instrument_display_name(&rack_slot_raw_name(app, slot_idx, slot));
            let pad_label = drum_rack_pad_label(pad_note);
            Some(DrumRackSoundOption {
                pad_note,
                slot_idx,
                label: format!("{pad_label} · {name}"),
                short_label: drum_rack_sound_short_label(&name),
                name,
                pad_label,
            })
        })
        .collect::<Vec<_>>();
    options.sort_by_key(|sound| sound.pad_note);
    options
}

fn drum_rack_sound_value(track: usize, option: DrumRackSoundOption) -> Rc<RefCell<Value>> {
    let mut value = HashMap::new();
    value.insert(
        "transpose".to_string(),
        value_cell(Value::Number(option.pad_note as f64)),
    );
    value.insert(
        "slot-idx".to_string(),
        value_cell(Value::Number(option.slot_idx as f64)),
    );
    insert_string_prop(
        &mut value,
        "gain-field",
        rack_slot_value_field(
            track,
            option.slot_idx,
            sequencer::sequencer::RackSlotParam::Gain,
        ),
    );
    insert_string_prop(
        &mut value,
        "mute-field",
        rack_slot_value_field(
            track,
            option.slot_idx,
            sequencer::sequencer::RackSlotParam::Mute,
        ),
    );
    insert_string_prop(
        &mut value,
        "solo-field",
        rack_slot_value_field(
            track,
            option.slot_idx,
            sequencer::sequencer::RackSlotParam::Solo,
        ),
    );
    insert_string_prop(
        &mut value,
        "peak-field",
        rack_slot_peak_field(track, option.slot_idx),
    );
    insert_string_prop(
        &mut value,
        "selected-field",
        rack_slot_selected_field(track, option.slot_idx),
    );
    insert_string_prop(&mut value, "name", option.name);
    insert_string_prop(&mut value, "pad-label", option.pad_label);
    insert_string_prop(&mut value, "label", option.label);
    insert_string_prop(&mut value, "short-label", option.short_label);
    Rc::new(RefCell::new(Value::Map(value)))
}

pub(crate) fn build_all_track_drum_sounds_value(app: &app::App) -> Value {
    Value::List(
        (0..app.tracks.len())
            .map(|track| {
                let sounds = drum_rack_sound_options(app, track)
                    .into_iter()
                    .map(|option| drum_rack_sound_value(track, option))
                    .collect();
                value_cell(Value::List(sounds))
            })
            .collect(),
    )
}

/// Publishes the rack's global slot selection for drum-lane widgets without
/// rebuilding the sequencer tree. The selected identity for a drum rack is a
/// pad note, so resolve it through the rack snapshot before lighting a slot.
pub(crate) fn sync_all_rack_slot_selection_binding_fields(
    rt: &mut Runtime,
    app: &app::App,
) -> bool {
    let racks = app.state.pattern.rack_tracks.lock().unwrap();
    let mut dirty = false;
    for (track, rack) in racks.iter().enumerate() {
        let Some(rack) = rack.as_ref() else {
            continue;
        };
        let selected_slot = app.selected_rack_slot_index_for_rack(track, rack);
        for slot_idx in 0..rack.slots.len() {
            dirty |= rt
                .set_reactive(
                    "SEQ",
                    &rack_slot_selected_field(track, slot_idx),
                    Value::Bool(Some(slot_idx) == selected_slot),
                )
                .effects_dirty;
        }
    }
    dirty
}

pub(crate) fn drum_lane_step_active(
    state: &Arc<SequencerState>,
    track: usize,
    pad_note: i32,
    step: usize,
) -> bool {
    if track >= state.pattern.patterns.len()
        || step >= MAX_STEPS
        || !state.pattern.patterns[track].is_active(step)
    {
        return false;
    }
    let count = state.pattern.chord_data[track].count(step);
    if count == 0 {
        return state.pattern.step_data[track]
            .get(step, StepParam::Transpose)
            .round() as i32
            == pad_note;
    }
    (0..count)
        .any(|voice| state.pattern.chord_data[track].get(step, voice).round() as i32 == pad_note)
}

pub(crate) fn drum_lane_step_duration_covered(
    state: &Arc<SequencerState>,
    track: usize,
    pad_note: i32,
    target_step: usize,
) -> bool {
    if track >= state.pattern.patterns.len() || target_step >= MAX_STEPS {
        return false;
    }
    let num_steps = state.pattern.track_params[track]
        .get_num_steps()
        .min(MAX_STEPS);
    target_step < num_steps
        && (0..=target_step).any(|source_step| {
            state
                .drum_lane_step_duration(track, source_step, pad_note)
                .is_some_and(|duration| duration > (target_step - source_step) as f32)
        })
}

pub(crate) fn sync_drum_lane_step_binding_fields(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    app: &app::App,
    track: usize,
    only_step: Option<usize>,
) -> bool {
    if track >= app.tracks.len() {
        return false;
    }
    let sounds = drum_rack_sound_options(app, track);
    if sounds.is_empty() {
        return false;
    }
    let mut registered_fields = match rt.global_value("SEQ") {
        Some(Value::Map(fields)) => fields.keys().cloned().collect::<HashSet<_>>(),
        _ => HashSet::new(),
    };
    let steps = only_step.map_or(0..MAX_STEPS, |step| {
        step..step.saturating_add(1).min(MAX_STEPS)
    });
    let mut dirty = false;
    for sound in sounds {
        for step in steps.clone() {
            let selected_field = drum_lane_step_selected_field(track, sound.pad_note, step);
            if registered_fields.insert(selected_field.clone()) {
                dirty |= rt
                    .set_reactive("SEQ", &selected_field, Value::Bool(false))
                    .effects_dirty;
            }
            let duration_field = drum_lane_step_duration_field(track, sound.pad_note, step);
            dirty |= rt
                .set_reactive(
                    "SEQ",
                    &duration_field,
                    Value::Bool(drum_lane_step_duration_covered(
                        state,
                        track,
                        sound.pad_note,
                        step,
                    )),
                )
                .effects_dirty;
            dirty |= rt
                .set_reactive(
                    "SEQ",
                    &drum_lane_step_active_field(track, sound.pad_note, step),
                    Value::Bool(drum_lane_step_active(state, track, sound.pad_note, step)),
                )
                .effects_dirty;
        }
    }
    dirty
}

pub(crate) fn sync_drum_lane_step_binding_fields_for_steps(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    app: &app::App,
    track: usize,
    steps: &[usize],
) -> bool {
    if track >= app.tracks.len() || steps.is_empty() {
        return false;
    }
    let sounds = drum_rack_sound_options(app, track);
    if sounds.is_empty() {
        return false;
    }
    let mut registered_fields = match rt.global_value("SEQ") {
        Some(Value::Map(fields)) => fields.keys().cloned().collect::<HashSet<_>>(),
        _ => HashSet::new(),
    };
    let mut dirty = false;
    for sound in sounds {
        for &step in steps.iter().filter(|step| **step < MAX_STEPS) {
            let selected_field = drum_lane_step_selected_field(track, sound.pad_note, step);
            if registered_fields.insert(selected_field.clone()) {
                dirty |= rt
                    .set_reactive("SEQ", &selected_field, Value::Bool(false))
                    .effects_dirty;
            }
            dirty |= rt
                .set_reactive(
                    "SEQ",
                    &drum_lane_step_duration_field(track, sound.pad_note, step),
                    Value::Bool(drum_lane_step_duration_covered(
                        state,
                        track,
                        sound.pad_note,
                        step,
                    )),
                )
                .effects_dirty;
            dirty |= rt
                .set_reactive(
                    "SEQ",
                    &drum_lane_step_active_field(track, sound.pad_note, step),
                    Value::Bool(drum_lane_step_active(state, track, sound.pad_note, step)),
                )
                .effects_dirty;
        }
    }
    dirty
}

pub(crate) fn sync_all_drum_lane_step_binding_fields(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    app: &app::App,
) -> bool {
    let mut dirty = false;
    for track in 0..app.tracks.len() {
        dirty |= sync_drum_lane_step_binding_fields(rt, state, app, track, None);
    }
    dirty
}

fn drum_rack_pad_bank_value(bank_start: i32, selected_bank_start: i32) -> Rc<RefCell<Value>> {
    let mut bank_map: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
    bank_map.insert(
        "bank-start".to_string(),
        value_cell(Value::Number(bank_start as f64)),
    );
    bank_map.insert(
        "selected".to_string(),
        value_cell(Value::Bool(bank_start == selected_bank_start)),
    );
    insert_string_prop(&mut bank_map, "label", drum_rack_pad_bank_label(bank_start));
    Rc::new(RefCell::new(Value::Map(bank_map)))
}

fn rack_pad_value(
    app: &app::App,
    track: usize,
    rack: &sequencer::sequencer::RackTrackSnapshot,
    pad_note: i32,
    selected_pad_note: i32,
) -> Rc<RefCell<Value>> {
    let slot_idx = rack
        .slots
        .iter()
        .position(|slot| slot.pad_note == Some(pad_note));
    let mut pad_map: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
    pad_map.insert("track".to_string(), value_cell(Value::Number(track as f64)));
    pad_map.insert(
        "pad-note".to_string(),
        value_cell(Value::Number(pad_note as f64)),
    );
    pad_map.insert(
        "selected".to_string(),
        value_cell(Value::Bool(pad_note == selected_pad_note)),
    );
    insert_string_prop(&mut pad_map, "label", drum_rack_pad_label(pad_note));
    if let Some(slot_idx) = slot_idx {
        let slot = &rack.slots[slot_idx];
        pad_map.insert(
            "slot".to_string(),
            value_cell(Value::Number(slot_idx as f64)),
        );
        pad_map.insert(
            "idx".to_string(),
            value_cell(Value::Number(slot_idx as f64)),
        );
        pad_map.insert("occupied".to_string(), value_cell(Value::Bool(true)));
        insert_string_prop(&mut pad_map, "type", rack_slot_type_name(slot));
        let raw_name = rack_slot_raw_name(app, slot_idx, slot);
        insert_string_prop(&mut pad_map, "name", raw_name.clone());
        insert_string_prop(
            &mut pad_map,
            "display-name",
            instrument_display_name(&raw_name),
        );
        pad_map.insert("mute".to_string(), value_cell(Value::Bool(slot.mute)));
        pad_map.insert("solo".to_string(), value_cell(Value::Bool(slot.solo)));
        pad_map.insert(
            "choke-group".to_string(),
            value_cell(Value::Number(slot.choke_group.unwrap_or(0) as f64)),
        );
    } else {
        pad_map.insert("occupied".to_string(), value_cell(Value::Bool(false)));
        pad_map.insert("slot".to_string(), value_cell(Value::Number(-1.0)));
        pad_map.insert("idx".to_string(), value_cell(Value::Number(-1.0)));
        insert_string_prop(&mut pad_map, "type", "empty");
        insert_string_prop(&mut pad_map, "name", "");
        insert_string_prop(&mut pad_map, "display-name", "");
        pad_map.insert("mute".to_string(), value_cell(Value::Bool(false)));
        pad_map.insert("solo".to_string(), value_cell(Value::Bool(false)));
        pad_map.insert("choke-group".to_string(), value_cell(Value::Number(0.0)));
    }
    Rc::new(RefCell::new(Value::Map(pad_map)))
}

fn insert_rack_param_target(
    pmap: &mut HashMap<String, Rc<RefCell<Value>>>,
    track: usize,
    slot_idx: usize,
) {
    pmap.insert(
        "rack-track".to_string(),
        value_cell(Value::Number(track as f64)),
    );
    pmap.insert(
        "rack-slot".to_string(),
        value_cell(Value::Number(slot_idx as f64)),
    );
}

struct RackUiModMetadata {
    source_param_idx: Option<usize>,
    depth_param_idx: usize,
    source_slot: f32,
    source_value_field: Option<String>,
    depth_value: f32,
    depth_value_field: String,
    depth_min: f32,
    depth_max: f32,
    depth_unit: Option<String>,
}

fn insert_rack_mod_metadata(
    pmap: &mut HashMap<String, Rc<RefCell<Value>>>,
    targets: &[RackUiModMetadata],
) {
    pmap.insert("modulatable".to_string(), value_cell(Value::Bool(true)));
    let target_values = targets
        .iter()
        .map(|meta| {
            let mut target = HashMap::new();
            if let Some(source_param_idx) = meta.source_param_idx {
                target.insert(
                    "source-idx".to_string(),
                    value_cell(Value::Number(source_param_idx as f64)),
                );
            }
            target.insert(
                "depth-idx".to_string(),
                value_cell(Value::Number(meta.depth_param_idx as f64)),
            );
            target.insert(
                "source-slot".to_string(),
                value_cell(Value::Number(meta.source_slot as f64)),
            );
            if let Some(field) = &meta.source_value_field {
                insert_string_prop(&mut target, "source-value-field", field);
            }
            target.insert(
                "depth".to_string(),
                value_cell(Value::Number(meta.depth_value as f64)),
            );
            insert_string_prop(&mut target, "depth-value-field", &meta.depth_value_field);
            target.insert(
                "depth-min".to_string(),
                value_cell(Value::Number(meta.depth_min as f64)),
            );
            target.insert(
                "depth-max".to_string(),
                value_cell(Value::Number(meta.depth_max as f64)),
            );
            if let Some(unit) = &meta.depth_unit {
                insert_string_prop(&mut target, "depth-unit", unit);
            }
            value_cell(Value::Map(target))
        })
        .collect();
    pmap.insert(
        "mod-targets".to_string(),
        value_cell(Value::List(target_values)),
    );
}

fn rack_slot_param_map(
    track: usize,
    slot_idx: usize,
    name: String,
    control: &str,
    idx: Option<usize>,
    value_field: String,
    value: f32,
    min: f32,
    max: f32,
    options: Option<&Vec<String>>,
    mod_targets: Option<&Vec<RackUiModMetadata>>,
    ui_metadata: Option<&sequencer::effects::ParamUiMetadata>,
) -> Rc<RefCell<Value>> {
    let mut pmap: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
    insert_string_prop(&mut pmap, "name", name.clone());
    insert_string_prop(&mut pmap, "control", control);
    if let Some(idx) = idx {
        pmap.insert("idx".to_string(), value_cell(Value::Number(idx as f64)));
    }
    insert_string_prop(&mut pmap, "value-field", value_field);
    pmap.insert("value".to_string(), value_cell(Value::Number(value as f64)));
    pmap.insert("min".to_string(), value_cell(Value::Number(min as f64)));
    pmap.insert("max".to_string(), value_cell(Value::Number(max as f64)));
    if let Some(labels) = options {
        let selected = labels
            .get(value.round() as usize)
            .cloned()
            .unwrap_or_default();
        insert_string_prop(&mut pmap, "text-value", selected);
        pmap.insert(
            "options".to_string(),
            value_cell(Value::List(
                labels
                    .iter()
                    .cloned()
                    .map(|label| value_cell(Value::String(label)))
                    .collect(),
            )),
        );
    } else if name == "enabled" || name == "sync" {
        pmap.insert("boolean".to_string(), value_cell(Value::Bool(true)));
    }
    if let Some(targets) = mod_targets {
        insert_rack_mod_metadata(&mut pmap, targets);
    }
    insert_param_ui_metadata(&mut pmap, ui_metadata);
    insert_rack_param_target(&mut pmap, track, slot_idx);
    Rc::new(RefCell::new(Value::Map(pmap)))
}

fn rack_slot_param_value(
    rack: &sequencer::sequencer::RackTrackSnapshot,
    slot_idx: usize,
    slot: &sequencer::sequencer::RackSlotSnapshot,
    desc: &sequencer::effects::EffectDescriptor,
    param_idx: usize,
    selected_step: Option<usize>,
) -> f32 {
    if let Some(step) = selected_step {
        if let Some(value) = slot
            .instrument_slot
            .plocks
            .get(step)
            .and_then(|step_plocks| step_plocks.get(param_idx))
            .copied()
            .flatten()
        {
            return value;
        }
    }
    if let Some(value) = rack_macro_mapped_value(rack, selected_step, |target| {
        matches!(
            target,
            sequencer::sequencer::RackMacroTarget::SlotInstrumentParam {
                slot,
                param_index,
                ..
            } if *slot == slot_idx && *param_index == param_idx
        )
    }) {
        return value;
    }
    slot.instrument_slot
        .defaults
        .get(param_idx)
        .copied()
        .unwrap_or_else(|| {
            desc.params
                .get(param_idx)
                .map(|param| param.default)
                .unwrap_or_default()
        })
}

fn rack_macro_mapped_value(
    rack: &sequencer::sequencer::RackTrackSnapshot,
    selected_step: Option<usize>,
    target_matches: impl Fn(&sequencer::sequencer::RackMacroTarget) -> bool,
) -> Option<f32> {
    rack.macros.iter().find_map(|rack_macro| {
        rack_macro.mappings.iter().find_map(|mapping| {
            if !target_matches(&mapping.target) {
                return None;
            }
            let macro_value = selected_step
                .map(|step| rack_macro.value_at(step))
                .unwrap_or(rack_macro.value);
            let curved = match mapping.curve {
                sequencer::sequencer::RackMacroCurve::Linear => macro_value,
                sequencer::sequencer::RackMacroCurve::Exp => macro_value * macro_value,
                sequencer::sequencer::RackMacroCurve::Log => macro_value.sqrt(),
            };
            Some(mapping.range_min + (mapping.range_max - mapping.range_min) * curved)
        })
    })
}

fn selected_rack_slot_voice_mod_source_indices(
    rack: &sequencer::sequencer::RackTrackSnapshot,
    slot_idx: usize,
    desc: &sequencer::effects::EffectDescriptor,
    slot: &sequencer::sequencer::RackSlotSnapshot,
    selected_step: Option<usize>,
) -> Vec<usize> {
    sequencer::voice_modulator::selected_source_param_indices(&desc.params, |idx, _| {
        rack_slot_param_value(rack, slot_idx, slot, desc, idx, selected_step)
    })
}

fn build_selected_rack_slot_instrument_value(
    app: &app::App,
    rack: &sequencer::sequencer::RackTrackSnapshot,
    track: usize,
    slot_idx: usize,
    slot: &sequencer::sequencer::RackSlotSnapshot,
    selected_step: Option<usize>,
) -> Option<Rc<RefCell<Value>>> {
    let desc = app.rack_slot_instrument_descriptor(slot)?;
    let raw_name = rack_slot_raw_name(app, slot_idx, slot);
    let mut synth_params: Vec<Rc<RefCell<Value>>> = Vec::new();
    let mut mod_params: Vec<Rc<RefCell<Value>>> = Vec::new();
    let mut modulation_targets: HashMap<usize, Vec<RackUiModMetadata>> = HashMap::new();
    let use_sampler_depth_units =
        slot.instrument_type == sequencer::sequencer::InstrumentType::Sampler;

    for target in desc
        .instrument_modulation_targets
        .iter()
        .filter_map(|target| {
            let depth_desc = desc.params.get(target.depth_param_idx)?;
            let source_current = target
                .source_param_idx
                .map(|source_param_idx| {
                    rack_slot_param_value(
                        rack,
                        slot_idx,
                        slot,
                        &desc,
                        source_param_idx,
                        selected_step,
                    )
                })
                .unwrap_or(target.modulator_slot as f32);
            let depth_current = rack_slot_param_value(
                rack,
                slot_idx,
                slot,
                &desc,
                target.depth_param_idx,
                selected_step,
            );
            let (depth_min, depth_max) = if use_sampler_depth_units {
                sampler_modulation_depth_display_range(depth_desc, target)
            } else {
                instrument_modulation_depth_display_range(target)
            };
            Some((
                target.base_param_idx,
                RackUiModMetadata {
                    source_param_idx: target.source_param_idx,
                    depth_param_idx: target.depth_param_idx,
                    source_slot: target
                        .source_param_idx
                        .and_then(|source_param_idx| {
                            desc.params
                                .get(source_param_idx)
                                .map(|source_desc| source_desc.stored_to_user(source_current))
                        })
                        .unwrap_or(source_current),
                    source_value_field: target.source_param_idx.map(|source_param_idx| {
                        let source_desc = &desc.params[source_param_idx];
                        rack_slot_instrument_param_value_field(
                            track,
                            slot_idx,
                            source_param_idx,
                            &source_desc.name,
                        )
                    }),
                    depth_value: depth_desc.stored_to_user(depth_current),
                    depth_value_field: rack_slot_instrument_param_value_field(
                        track,
                        slot_idx,
                        target.depth_param_idx,
                        &depth_desc.name,
                    ),
                    depth_min,
                    depth_max,
                    depth_unit: target.depth_unit.clone(),
                },
            ))
        })
    {
        modulation_targets
            .entry(target.0)
            .or_default()
            .push(target.1);
    }

    synth_params.push(rack_slot_param_map(
        track,
        slot_idx,
        "base_note".to_string(),
        "base-note",
        None,
        rack_slot_value_field(
            track,
            slot_idx,
            sequencer::sequencer::RackSlotParam::BaseNote,
        ),
        slot.param_value_at_step(
            sequencer::sequencer::RackSlotParam::BaseNote,
            selected_step.unwrap_or(usize::MAX),
        ),
        -48.0,
        48.0,
        None,
        None,
        None,
    ));

    for (param_idx, pdesc) in desc.params.iter().enumerate() {
        if sequencer::voice_modulator::is_source_param(pdesc.node_param_idx)
            || pdesc.name.starts_with("__host_mod__")
            || pdesc.name.starts_with("__dgen_mod_active__")
        {
            continue;
        }
        let current = rack_slot_param_value(rack, slot_idx, slot, &desc, param_idx, selected_step);
        let options = match &pdesc.kind {
            sequencer::effects::ParamKind::Enum { labels } => Some(labels),
            _ => None,
        };
        if pdesc.name.starts_with("mod ") {
            mod_params.push(rack_slot_param_map(
                track,
                slot_idx,
                pdesc
                    .name
                    .strip_prefix("mod ")
                    .unwrap_or(&pdesc.name)
                    .to_string(),
                "param",
                Some(param_idx),
                rack_slot_instrument_param_value_field(track, slot_idx, param_idx, &pdesc.name),
                pdesc.stored_to_user(current),
                pdesc.stored_to_user(pdesc.min),
                pdesc.stored_to_user(pdesc.max),
                options,
                None,
                None,
            ));
        } else {
            synth_params.push(rack_slot_param_map(
                track,
                slot_idx,
                pdesc.name.clone(),
                "param",
                Some(param_idx),
                rack_slot_instrument_param_value_field(track, slot_idx, param_idx, &pdesc.name),
                pdesc.stored_to_user(current),
                pdesc.stored_to_user(pdesc.min),
                pdesc.stored_to_user(pdesc.max),
                options,
                modulation_targets.get(&param_idx),
                pdesc.ui_metadata.as_ref(),
            ));
        }
    }

    let source_actual =
        selected_rack_slot_voice_mod_source_indices(rack, slot_idx, &desc, slot, selected_step);
    let mut source_sections: Vec<Rc<RefCell<Value>>> = Vec::new();
    let mut source_names: Vec<Rc<RefCell<Value>>> = Vec::new();
    for slot_number in 1..=sequencer::voice_modulator::SLOT_COUNT {
        let section_name = sequencer::voice_modulator::modulator_slot_label(slot_number, "");
        let mut params: Vec<Rc<RefCell<Value>>> = Vec::new();
        let mut source_param: Option<Rc<RefCell<Value>>> = None;
        for &param_idx in &source_actual {
            let Some(pdesc) = desc.params.get(param_idx) else {
                continue;
            };
            if sequencer::voice_modulator::slot_from_param_name(&pdesc.name) != Some(slot_number) {
                continue;
            }
            let current =
                rack_slot_param_value(rack, slot_idx, slot, &desc, param_idx, selected_step);
            let options = match &pdesc.kind {
                sequencer::effects::ParamKind::Enum { labels } => Some(labels),
                _ => None,
            };
            let param = rack_slot_param_map(
                track,
                slot_idx,
                sequencer::voice_modulator::source_param_display_name(&pdesc.name),
                "param",
                Some(param_idx),
                rack_slot_instrument_param_value_field(track, slot_idx, param_idx, &pdesc.name),
                pdesc.stored_to_user(current),
                pdesc.stored_to_user(pdesc.min),
                pdesc.stored_to_user(pdesc.max),
                options,
                None,
                None,
            );
            if sequencer::voice_modulator::source_type_name_from_param_name(&pdesc.name)
                == Some("source")
            {
                source_param = Some(param);
            } else {
                params.push(param);
            }
        }
        source_names.push(value_cell(Value::String(section_name.clone())));
        let mut section_map: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
        insert_string_prop(&mut section_map, "name", section_name);
        section_map.insert(
            "slot".to_string(),
            value_cell(Value::Number(slot_number as f64)),
        );
        if let Some(source_param) = source_param {
            section_map.insert("source-param".to_string(), source_param);
        }
        section_map.insert("params".to_string(), value_cell(Value::List(params)));
        source_sections.push(value_cell(Value::Map(section_map)));
    }

    let mut panel_map: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
    insert_string_prop(&mut panel_map, "type", rack_slot_type_name(slot));
    panel_map.insert("track".to_string(), value_cell(Value::Number(track as f64)));
    panel_map.insert(
        "rack-track".to_string(),
        value_cell(Value::Number(track as f64)),
    );
    panel_map.insert(
        "rack-slot".to_string(),
        value_cell(Value::Number(slot_idx as f64)),
    );
    insert_string_prop(&mut panel_map, "name", raw_name.clone());
    insert_string_prop(
        &mut panel_map,
        "display-name",
        instrument_display_name(&raw_name),
    );
    panel_map.insert(
        "synth".to_string(),
        value_cell(Value::List(synth_params.clone())),
    );
    panel_map.insert("params".to_string(), value_cell(Value::List(synth_params)));
    panel_map.insert("mod".to_string(), value_cell(Value::List(mod_params)));
    panel_map.insert(
        "modulators".to_string(),
        value_cell(Value::List(
            desc.instrument_modulators
                .iter()
                .map(|modulator| {
                    let mut map: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
                    map.insert(
                        "slot".to_string(),
                        value_cell(Value::Number(modulator.slot as f64)),
                    );
                    insert_string_prop(&mut map, "label", modulator.label.clone());
                    Rc::new(RefCell::new(Value::Map(map)))
                })
                .collect(),
        )),
    );
    panel_map.insert(
        "source-names".to_string(),
        value_cell(Value::List(source_names)),
    );
    panel_map.insert(
        "sources".to_string(),
        value_cell(Value::List(source_sections)),
    );
    panel_map.insert(
        "phase-field".to_string(),
        value_cell(Value::String(modulator_phase_field(track))),
    );
    panel_map.insert(
        "level-field".to_string(),
        value_cell(Value::String(modulator_level_field(track))),
    );

    if slot.instrument_type == sequencer::sequencer::InstrumentType::Sampler {
        let (buffer_id, sample_name, _) = slot
            .sample_id
            .clone()
            .unwrap_or_else(|| (-1, raw_name.clone(), app.graph.sample_rate.max(1)));
        let sampler_path = app
            .sample_buffer_path_registry
            .get(&buffer_id)
            .cloned()
            .or_else(|| app.sample_path_registry.get(&sample_name).cloned());
        let registered_sample = sampler_path.as_ref().and_then(|path| {
            let key = path.display().to_string();
            eseqlisp::audio::sample::get_registered_sample(&key).or_else(|| {
                match eseqlisp::audio::sample::SampleBuffer::load_wav(path) {
                    Ok(sample) => {
                        sample.register();
                        eseqlisp::audio::sample::get_registered_sample(&key)
                    }
                    Err(error) => {
                        eprintln!(
                            "rack waveform: failed to register sample {}: {error}",
                            path.display()
                        );
                        None
                    }
                }
            })
        });
        let sample_duration = registered_sample
            .as_ref()
            .map(|sample| sample.duration_seconds)
            .unwrap_or(1.0);
        if let Some(buffer_value) = registered_sample.as_ref().map(|sample| sample.to_value()) {
            panel_map.insert("buffer".to_string(), value_cell(buffer_value));
        }
        let start_raw = rack_slot_param_value(rack, slot_idx, slot, &desc, 2, selected_step);
        let end_raw = rack_slot_param_value(rack, slot_idx, slot, &desc, 3, selected_step);
        panel_map.insert(
            "start-time".to_string(),
            value_cell(Value::Number(start_raw as f64 * sample_duration)),
        );
        insert_string_prop(
            &mut panel_map,
            "start-time-field",
            rack_slot_sampler_selection_time_field(track, slot_idx, "start"),
        );
        panel_map.insert(
            "end-time".to_string(),
            value_cell(Value::Number(end_raw as f64 * sample_duration)),
        );
        insert_string_prop(
            &mut panel_map,
            "end-time-field",
            rack_slot_sampler_selection_time_field(track, slot_idx, "end"),
        );
        panel_map.insert(
            "duration".to_string(),
            value_cell(Value::Number(sample_duration)),
        );
    }

    Some(Rc::new(RefCell::new(Value::Map(panel_map))))
}

fn rack_effect_param_value(
    rack: &sequencer::sequencer::RackTrackSnapshot,
    rack_slot: usize,
    effect_slot: usize,
    snapshot: &sequencer::effects::EffectSlotSnapshot,
    descriptor: &sequencer::effects::EffectDescriptor,
    param_idx: usize,
    selected_step: Option<usize>,
) -> f32 {
    let fallback = descriptor.params[param_idx].default;
    if let Some(step) = selected_step {
        if let Some(value) = snapshot
            .plocks
            .get(step)
            .and_then(|step_plocks| step_plocks.get(param_idx))
            .copied()
            .flatten()
        {
            return value;
        }
    }
    rack_macro_mapped_value(rack, selected_step, |target| {
        matches!(
            target,
            sequencer::sequencer::RackMacroTarget::SlotEffectParam {
                slot,
                effect_slot: target_effect_slot,
                param_index,
                ..
            } if *slot == rack_slot
                && *target_effect_slot == effect_slot
                && *param_index == param_idx
        )
    })
    .unwrap_or_else(|| {
        snapshot
            .defaults
            .get(param_idx)
            .copied()
            .unwrap_or(fallback)
    })
}

fn build_rack_slot_effect_value(
    rack: &sequencer::sequencer::RackTrackSnapshot,
    track: usize,
    rack_slot: usize,
    effect_slot: usize,
    descriptor: &sequencer::effects::EffectDescriptor,
    snapshot: &sequencer::effects::EffectSlotSnapshot,
    selected_step: Option<usize>,
) -> Rc<RefCell<Value>> {
    let mut modulation_targets: HashMap<usize, Vec<RackUiModMetadata>> = HashMap::new();
    for target in descriptor
        .instrument_modulation_targets
        .iter()
        .filter_map(|target| {
            let depth_desc = descriptor.params.get(target.depth_param_idx)?;
            let source_current = target
                .source_param_idx
                .map(|source_idx| {
                    rack_effect_param_value(
                        rack,
                        rack_slot,
                        effect_slot,
                        snapshot,
                        descriptor,
                        source_idx,
                        selected_step,
                    )
                })
                .unwrap_or(target.modulator_slot as f32);
            let depth_current = rack_effect_param_value(
                rack,
                rack_slot,
                effect_slot,
                snapshot,
                descriptor,
                target.depth_param_idx,
                selected_step,
            );
            Some((
                target.base_param_idx,
                RackUiModMetadata {
                    source_param_idx: target.source_param_idx,
                    depth_param_idx: target.depth_param_idx,
                    source_slot: target
                        .source_param_idx
                        .and_then(|source_idx| {
                            descriptor
                                .params
                                .get(source_idx)
                                .map(|source| source.stored_to_user(source_current))
                        })
                        .unwrap_or(source_current),
                    source_value_field: target.source_param_idx.map(|source_idx| {
                        let source = &descriptor.params[source_idx];
                        rack_slot_effect_param_value_field(
                            track,
                            rack_slot,
                            effect_slot,
                            source_idx,
                            &source.name,
                        )
                    }),
                    depth_value: depth_desc.stored_to_user(depth_current),
                    depth_value_field: rack_slot_effect_param_value_field(
                        track,
                        rack_slot,
                        effect_slot,
                        target.depth_param_idx,
                        &depth_desc.name,
                    ),
                    depth_min: target.depth_min,
                    depth_max: target.depth_max,
                    depth_unit: target.depth_unit.clone(),
                },
            ))
        })
    {
        modulation_targets
            .entry(target.0)
            .or_default()
            .push(target.1);
    }

    let routing_params = modulation_routing_param_indices(descriptor);
    let params = descriptor
        .params
        .iter()
        .enumerate()
        .filter_map(|(param_idx, param)| {
            if param.node_param_idx == u32::MAX
                || (sequencer::voice_modulator::is_source_param(param.node_param_idx)
                    && !matches!(
                        param.host_control,
                        Some(sequencer::effects::HostControl::FxSidechain { .. })
                    ))
                || routing_params.contains(&param_idx)
                || param.name.starts_with("__host_mod__")
                || param.name.starts_with("__dgen_mod_active__")
            {
                return None;
            }
            let current = rack_effect_param_value(
                rack,
                rack_slot,
                effect_slot,
                snapshot,
                descriptor,
                param_idx,
                selected_step,
            );
            let options = match &param.kind {
                sequencer::effects::ParamKind::Enum { labels } => Some(labels),
                _ => None,
            };
            let value = rack_slot_param_map(
                track,
                rack_slot,
                param.name.clone(),
                "param",
                Some(param_idx),
                rack_slot_effect_param_value_field(
                    track,
                    rack_slot,
                    effect_slot,
                    param_idx,
                    &param.name,
                ),
                current,
                param.min,
                param.max,
                options,
                modulation_targets.get(&param_idx),
                param.ui_metadata.as_ref(),
            );
            if matches!(param.kind, sequencer::effects::ParamKind::Boolean) {
                if let Value::Map(map) = &mut *value.borrow_mut() {
                    map.insert("boolean".to_string(), value_cell(Value::Bool(true)));
                }
            }
            Some(value)
        })
        .collect::<Vec<_>>();

    let source_actual =
        sequencer::voice_modulator::selected_source_param_indices(&descriptor.params, |idx, _| {
            rack_effect_param_value(
                rack,
                rack_slot,
                effect_slot,
                snapshot,
                descriptor,
                idx,
                selected_step,
            )
        });
    let mut source_sections = Vec::new();
    let mut source_names = Vec::new();
    for slot_number in 1..=sequencer::voice_modulator::SLOT_COUNT {
        let section_name = sequencer::voice_modulator::modulator_slot_label(slot_number, "");
        let mut section_params = Vec::new();
        let mut source_param = None;
        for &param_idx in &source_actual {
            let Some(param) = descriptor.params.get(param_idx) else {
                continue;
            };
            if sequencer::voice_modulator::slot_from_param_name(&param.name) != Some(slot_number) {
                continue;
            }
            let current = rack_effect_param_value(
                rack,
                rack_slot,
                effect_slot,
                snapshot,
                descriptor,
                param_idx,
                selected_step,
            );
            let options = match &param.kind {
                sequencer::effects::ParamKind::Enum { labels } => Some(labels),
                _ => None,
            };
            let value = rack_slot_param_map(
                track,
                rack_slot,
                sequencer::voice_modulator::source_param_display_name(&param.name),
                "param",
                Some(param_idx),
                rack_slot_effect_param_value_field(
                    track,
                    rack_slot,
                    effect_slot,
                    param_idx,
                    &param.name,
                ),
                param.stored_to_user(current),
                param.stored_to_user(param.min),
                param.stored_to_user(param.max),
                options,
                None,
                None,
            );
            if sequencer::voice_modulator::source_type_name_from_param_name(&param.name)
                == Some("source")
            {
                source_param = Some(value);
            } else {
                section_params.push(value);
            }
        }
        source_names.push(value_cell(Value::String(section_name.clone())));
        let mut section = HashMap::new();
        insert_string_prop(&mut section, "name", section_name);
        section.insert(
            "slot".to_string(),
            value_cell(Value::Number(slot_number as f64)),
        );
        if let Some(source_param) = source_param {
            section.insert("source-param".to_string(), source_param);
        }
        section.insert(
            "params".to_string(),
            value_cell(Value::List(section_params)),
        );
        source_sections.push(value_cell(Value::Map(section)));
    }

    let mut effect = HashMap::new();
    effect.insert(
        "slot-idx".to_string(),
        value_cell(Value::Number(effect_slot as f64)),
    );
    insert_string_prop(&mut effect, "name", descriptor.name.clone());
    effect.insert(
        "track-idx".to_string(),
        value_cell(Value::Number(track as f64)),
    );
    effect.insert(
        "rack-slot".to_string(),
        value_cell(Value::Number(rack_slot as f64)),
    );
    effect.insert("rack-fx".to_string(), value_cell(Value::Bool(true)));
    effect.insert(
        "builtin".to_string(),
        value_cell(Value::Bool(
            sequencer::effects::EffectDescriptor::builtin_insert(&descriptor.name).is_some()
                || sequencer::effects::conv_reverb::is_dgen_builtin(&descriptor.name),
        )),
    );
    effect.insert("params".to_string(), value_cell(Value::List(params)));
    effect.insert(
        "modulators".to_string(),
        value_cell(Value::List(
            descriptor
                .instrument_modulators
                .iter()
                .map(|modulator| {
                    let mut map = HashMap::new();
                    map.insert(
                        "slot".to_string(),
                        value_cell(Value::Number(modulator.slot as f64)),
                    );
                    insert_string_prop(&mut map, "label", modulator.label.clone());
                    value_cell(Value::Map(map))
                })
                .collect(),
        )),
    );
    effect.insert(
        "source-names".to_string(),
        value_cell(Value::List(source_names)),
    );
    effect.insert(
        "sources".to_string(),
        value_cell(Value::List(source_sections)),
    );
    value_cell(Value::Map(effect))
}

fn rack_macro_mapping_display_metadata(
    app: &app::App,
    rack: &sequencer::sequencer::RackTrackSnapshot,
    mapping: &sequencer::sequencer::RackMacroMapping,
) -> (String, String, f32, f32, f32, f32, f32, u8, String) {
    let (slot_idx, descriptor, param_idx) = match &mapping.target {
        sequencer::sequencer::RackMacroTarget::SlotInstrumentParam {
            slot, param_index, ..
        } => (
            *slot,
            rack.slots
                .get(*slot)
                .and_then(|slot| app.rack_slot_instrument_descriptor(slot)),
            *param_index,
        ),
        sequencer::sequencer::RackMacroTarget::SlotEffectParam {
            slot,
            effect_slot,
            param_index,
            ..
        } => (
            *slot,
            rack.slots
                .get(*slot)
                .and_then(|slot| slot.effect_descriptors.get(*effect_slot))
                .cloned(),
            *param_index,
        ),
        sequencer::sequencer::RackMacroTarget::SlotParam { slot, param } => {
            return (
                format!("Layer {}", slot + 1),
                param.clone(),
                mapping.range_min,
                mapping.range_max,
                mapping.range_min,
                mapping.range_max,
                1.0,
                2,
                String::new(),
            );
        }
    };
    let Some(descriptor) = descriptor else {
        return (
            format!("Layer {}", slot_idx + 1),
            match &mapping.target {
                sequencer::sequencer::RackMacroTarget::SlotInstrumentParam { param, .. }
                | sequencer::sequencer::RackMacroTarget::SlotEffectParam { param, .. } => {
                    param.clone()
                }
                sequencer::sequencer::RackMacroTarget::SlotParam { param, .. } => param.clone(),
            },
            mapping.range_min,
            mapping.range_max,
            mapping.range_min,
            mapping.range_max,
            1.0,
            2,
            String::new(),
        );
    };
    let Some(param) = descriptor.params.get(param_idx) else {
        return (
            format!("Layer {} · {}", slot_idx + 1, descriptor.name),
            "missing parameter".to_string(),
            mapping.range_min,
            mapping.range_max,
            mapping.range_min,
            mapping.range_max,
            1.0,
            2,
            String::new(),
        );
    };
    let scale = if param.is_percent() { 100.0 } else { 1.0 };
    let (decimals, unit) = match &param.kind {
        sequencer::effects::ParamKind::Boolean | sequencer::effects::ParamKind::Enum { .. } => {
            (0, String::new())
        }
        sequencer::effects::ParamKind::Continuous { unit } => (
            if unit.as_deref() == Some("%") { 1 } else { 2 },
            unit.clone().unwrap_or_default(),
        ),
    };
    (
        format!("Layer {} · {}", slot_idx + 1, descriptor.name),
        param.name.clone(),
        param.stored_to_user(mapping.range_min),
        param.stored_to_user(mapping.range_max),
        param.stored_to_user(param.min),
        param.stored_to_user(param.max),
        scale,
        decimals,
        unit,
    )
}

fn build_rack_panel_value(
    app: &app::App,
    track: usize,
    selected: &Arc<Mutex<HashSet<usize>>>,
) -> Value {
    let rack = app
        .state
        .pattern
        .rack_tracks
        .lock()
        .unwrap()
        .get(track)
        .cloned()
        .flatten();
    let Some(mut rack) = rack else {
        return Value::List(vec![]);
    };
    for rack_macro in &mut rack.macros {
        if let Some(value) = app.effective_rack_macro_value(track, rack_macro.id, None) {
            rack_macro.value = value;
        }
    }

    let routing_name = match rack.routing {
        sequencer::sequencer::RackRouting::Broadcast => "broadcast",
        sequencer::sequencer::RackRouting::ByPitch => "by-pitch",
    };
    let selected_pad_note = app.rack_selected_pad_note(track);
    let pad_bank_start = app.rack_pad_bank_start(track);
    let selected_slot = app.selected_rack_slot_index_for_rack(track, &rack);
    let selected_step = selected_plock_step(selected);
    let slots: Vec<Rc<RefCell<Value>>> = rack
        .slots
        .iter()
        .enumerate()
        .map(|(slot_idx, slot)| {
            let slot_type = rack_slot_type_name(slot);
            let raw_name = rack_slot_raw_name(app, slot_idx, slot);
            let mut slot_map: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
            slot_map.insert(
                "idx".to_string(),
                value_cell(Value::Number(slot_idx as f64)),
            );
            slot_map.insert("track".to_string(), value_cell(Value::Number(track as f64)));
            insert_string_prop(&mut slot_map, "type", slot_type);
            insert_string_prop(&mut slot_map, "name", raw_name.clone());
            insert_string_prop(
                &mut slot_map,
                "display-name",
                instrument_display_name(&raw_name),
            );
            if let Some(pad_note) = slot.pad_note {
                slot_map.insert(
                    "pad-note".to_string(),
                    value_cell(Value::Number(pad_note as f64)),
                );
                insert_string_prop(&mut slot_map, "pad-label", drum_rack_pad_label(pad_note));
            }
            slot_map.insert(
                "choke-group".to_string(),
                value_cell(Value::Number(slot.choke_group.unwrap_or(0) as f64)),
            );
            slot_map.insert(
                "base-note".to_string(),
                value_cell(Value::Number(slot.param_value_at_step(
                    sequencer::sequencer::RackSlotParam::BaseNote,
                    selected_step.unwrap_or(usize::MAX),
                ) as f64)),
            );
            insert_string_prop(
                &mut slot_map,
                "base-note-field",
                rack_slot_value_field(
                    track,
                    slot_idx,
                    sequencer::sequencer::RackSlotParam::BaseNote,
                ),
            );
            slot_map.insert(
                "base-note-min".to_string(),
                value_cell(Value::Number(-48.0)),
            );
            slot_map.insert("base-note-max".to_string(), value_cell(Value::Number(48.0)));
            slot_map.insert(
                "gain".to_string(),
                value_cell(Value::Number(slot.param_value_at_step(
                    sequencer::sequencer::RackSlotParam::Gain,
                    selected_step.unwrap_or(usize::MAX),
                ) as f64)),
            );
            insert_string_prop(
                &mut slot_map,
                "gain-field",
                rack_slot_value_field(track, slot_idx, sequencer::sequencer::RackSlotParam::Gain),
            );
            slot_map.insert("gain-min".to_string(), value_cell(Value::Number(0.0)));
            slot_map.insert("gain-max".to_string(), value_cell(Value::Number(2.0)));
            slot_map.insert(
                "pan".to_string(),
                value_cell(Value::Number(slot.param_value_at_step(
                    sequencer::sequencer::RackSlotParam::Pan,
                    selected_step.unwrap_or(usize::MAX),
                ) as f64)),
            );
            insert_string_prop(
                &mut slot_map,
                "pan-field",
                rack_slot_value_field(track, slot_idx, sequencer::sequencer::RackSlotParam::Pan),
            );
            slot_map.insert("pan-min".to_string(), value_cell(Value::Number(-1.0)));
            slot_map.insert("pan-max".to_string(), value_cell(Value::Number(1.0)));
            slot_map.insert(
                "mute".to_string(),
                value_cell(Value::Bool(
                    slot.param_value_at_step(
                        sequencer::sequencer::RackSlotParam::Mute,
                        selected_step.unwrap_or(usize::MAX),
                    ) > 0.5,
                )),
            );
            insert_string_prop(
                &mut slot_map,
                "mute-field",
                rack_slot_value_field(track, slot_idx, sequencer::sequencer::RackSlotParam::Mute),
            );
            slot_map.insert(
                "solo".to_string(),
                value_cell(Value::Bool(
                    slot.param_value_at_step(
                        sequencer::sequencer::RackSlotParam::Solo,
                        selected_step.unwrap_or(usize::MAX),
                    ) > 0.5,
                )),
            );
            insert_string_prop(
                &mut slot_map,
                "solo-field",
                rack_slot_value_field(track, slot_idx, sequencer::sequencer::RackSlotParam::Solo),
            );
            slot_map.insert(
                "max-polyphony".to_string(),
                value_cell(Value::Number(slot.param_value_at_step(
                    sequencer::sequencer::RackSlotParam::MaxPolyphony,
                    selected_step.unwrap_or(usize::MAX),
                ) as f64)),
            );
            insert_string_prop(
                &mut slot_map,
                "max-polyphony-field",
                rack_slot_value_field(
                    track,
                    slot_idx,
                    sequencer::sequencer::RackSlotParam::MaxPolyphony,
                ),
            );
            slot_map.insert(
                "max-polyphony-min".to_string(),
                value_cell(Value::Number(1.0)),
            );
            slot_map.insert(
                "max-polyphony-max".to_string(),
                value_cell(Value::Number(64.0)),
            );
            slot_map.insert(
                "selected".to_string(),
                value_cell(Value::Bool(Some(slot_idx) == selected_slot)),
            );
            let effects = slot
                .effect_descriptors
                .iter()
                .zip(&slot.effect_slots)
                .enumerate()
                .filter_map(|(effect_idx, (descriptor, snapshot))| {
                    (snapshot.node_id != 0).then(|| {
                        build_rack_slot_effect_value(
                            &rack,
                            track,
                            slot_idx,
                            effect_idx,
                            descriptor,
                            snapshot,
                            selected_step,
                        )
                    })
                });
            let effects = effects.collect::<Vec<_>>();
            slot_map.insert(
                "effect-count".to_string(),
                value_cell(Value::Number(effects.len() as f64)),
            );
            slot_map.insert(
                "processing-cost".to_string(),
                // A stable, inspectable work-unit estimate: one unit per
                // available voice plus one per post-voice slot effect.
                value_cell(Value::Number((slot.max_polyphony + effects.len()) as f64)),
            );
            slot_map.insert("effects".to_string(), value_cell(Value::List(effects)));
            Rc::new(RefCell::new(Value::Map(slot_map)))
        })
        .collect();
    let mut pads: Vec<Rc<RefCell<Value>>> =
        Vec::with_capacity(sequencer::sequencer::DRUM_RACK_PAD_COUNT);
    for row in (0..4).rev() {
        for col in 0..4 {
            let pad_note = pad_bank_start + row * 4 + col;
            pads.push(rack_pad_value(
                app,
                track,
                &rack,
                pad_note,
                selected_pad_note,
            ));
        }
    }
    let mut bank_starts = Vec::new();
    let mut bank_start = sequencer::sequencer::DRUM_RACK_FIRST_PAD_NOTE;
    loop {
        bank_starts.push(bank_start);
        if bank_start >= sequencer::sequencer::DRUM_RACK_LAST_PAD_BANK_START {
            break;
        }
        let next = bank_start + sequencer::sequencer::DRUM_RACK_PAD_BANK_STRIDE;
        bank_start = next.min(sequencer::sequencer::DRUM_RACK_LAST_PAD_BANK_START);
    }
    let pad_banks: Vec<Rc<RefCell<Value>>> = bank_starts
        .into_iter()
        .rev()
        .map(|bank_start| drum_rack_pad_bank_value(bank_start, pad_bank_start))
        .collect();
    let selected_instrument = selected_slot.and_then(|slot_idx| {
        rack.slots.get(slot_idx).and_then(|slot| {
            build_selected_rack_slot_instrument_value(
                app,
                &rack,
                track,
                slot_idx,
                slot,
                selected_step,
            )
        })
    });

    let mut panel_map: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
    insert_string_prop(&mut panel_map, "type", "rack");
    panel_map.insert("track".to_string(), value_cell(Value::Number(track as f64)));
    panel_map.insert(
        "selected-slot".to_string(),
        value_cell(Value::Number(
            selected_slot
                .map(|slot_idx| slot_idx as f64)
                .unwrap_or(-1.0),
        )),
    );
    panel_map.insert(
        "selected-pad-note".to_string(),
        value_cell(Value::Number(selected_pad_note as f64)),
    );
    panel_map.insert(
        "pad-bank-start".to_string(),
        value_cell(Value::Number(pad_bank_start as f64)),
    );
    insert_string_prop(
        &mut panel_map,
        "pad-bank-label",
        drum_rack_pad_bank_label(pad_bank_start),
    );
    insert_string_prop(
        &mut panel_map,
        "name",
        app.tracks.get(track).cloned().unwrap_or_default(),
    );
    insert_string_prop(
        &mut panel_map,
        "display-name",
        app.tracks
            .get(track)
            .map(|name| instrument_display_name(name))
            .unwrap_or_else(|| "Rack".to_string()),
    );
    insert_string_prop(&mut panel_map, "routing", routing_name);
    let macros = rack
        .macros
        .iter()
        .map(|rack_macro| {
            let mut map = HashMap::new();
            map.insert(
                "id".to_string(),
                value_cell(Value::Number(rack_macro.id.index() as f64)),
            );
            insert_string_prop(&mut map, "key", &rack_macro.id.stable_key());
            insert_string_prop(&mut map, "name", &rack_macro.name);
            insert_string_prop(&mut map, "scope", "rack");
            map.insert(
                "value".to_string(),
                value_cell(Value::Number(
                    app.effective_rack_macro_value(track, rack_macro.id, selected_step)
                        .unwrap_or(rack_macro.value) as f64,
                )),
            );
            insert_string_prop(
                &mut map,
                "value-field",
                rack_macro_value_field(track, rack_macro.id.index()),
            );
            insert_string_prop(
                &mut map,
                "plock-active-field",
                rack_macro_plock_active_field(track, rack_macro.id.index()),
            );
            insert_string_prop(
                &mut map,
                "plock-default-field",
                rack_macro_plock_default_field(track, rack_macro.id.index()),
            );
            map.insert(
                "mapping-count".to_string(),
                value_cell(Value::Number(rack_macro.mappings.len() as f64)),
            );
            let mappings = rack_macro
                .mappings
                .iter()
                .enumerate()
                .map(|(mapping_idx, mapping)| {
                    let (
                        path_label,
                        param_label,
                        display_min,
                        display_max,
                        domain_min,
                        domain_max,
                        display_scale,
                        display_decimals,
                        display_unit,
                    ) = rack_macro_mapping_display_metadata(app, &rack, mapping);
                    let mut target = HashMap::new();
                    target.insert(
                        "mapping-idx".to_string(),
                        value_cell(Value::Number(mapping_idx as f64)),
                    );
                    target.insert(
                        "min".to_string(),
                        value_cell(Value::Number(mapping.range_min as f64)),
                    );
                    target.insert(
                        "max".to_string(),
                        value_cell(Value::Number(mapping.range_max as f64)),
                    );
                    insert_string_prop(&mut target, "path-label", path_label);
                    insert_string_prop(&mut target, "param-label", param_label);
                    target.insert(
                        "display-min".to_string(),
                        value_cell(Value::Number(display_min as f64)),
                    );
                    target.insert(
                        "display-max".to_string(),
                        value_cell(Value::Number(display_max as f64)),
                    );
                    target.insert(
                        "domain-min".to_string(),
                        value_cell(Value::Number(domain_min as f64)),
                    );
                    target.insert(
                        "domain-max".to_string(),
                        value_cell(Value::Number(domain_max as f64)),
                    );
                    target.insert(
                        "display-scale".to_string(),
                        value_cell(Value::Number(display_scale as f64)),
                    );
                    target.insert(
                        "display-decimals".to_string(),
                        value_cell(Value::Number(display_decimals as f64)),
                    );
                    insert_string_prop(&mut target, "display-unit", display_unit);
                    insert_string_prop(
                        &mut target,
                        "curve",
                        match mapping.curve {
                            sequencer::sequencer::RackMacroCurve::Linear => "linear",
                            sequencer::sequencer::RackMacroCurve::Exp => "exp",
                            sequencer::sequencer::RackMacroCurve::Log => "log",
                        },
                    );
                    target.insert("suspended".to_string(), value_cell(Value::Bool(false)));
                    match &mapping.target {
                        sequencer::sequencer::RackMacroTarget::SlotParam { slot, param } => {
                            insert_string_prop(&mut target, "kind", "rack-slot");
                            target.insert(
                                "rack-slot".to_string(),
                                value_cell(Value::Number(*slot as f64)),
                            );
                            insert_string_prop(&mut target, "param", param);
                        }
                        sequencer::sequencer::RackMacroTarget::SlotInstrumentParam {
                            slot,
                            param,
                            param_index,
                        } => {
                            insert_string_prop(&mut target, "kind", "rack-slot-instrument");
                            target.insert(
                                "rack-slot".to_string(),
                                value_cell(Value::Number(*slot as f64)),
                            );
                            target.insert(
                                "param-idx".to_string(),
                                value_cell(Value::Number(*param_index as f64)),
                            );
                            insert_string_prop(&mut target, "param", param);
                        }
                        sequencer::sequencer::RackMacroTarget::SlotEffectParam {
                            slot,
                            effect_slot,
                            param,
                            param_index,
                        } => {
                            insert_string_prop(&mut target, "kind", "rack-slot-effect");
                            target.insert(
                                "rack-slot".to_string(),
                                value_cell(Value::Number(*slot as f64)),
                            );
                            target.insert(
                                "effect-slot".to_string(),
                                value_cell(Value::Number(*effect_slot as f64)),
                            );
                            target.insert(
                                "param-idx".to_string(),
                                value_cell(Value::Number(*param_index as f64)),
                            );
                            insert_string_prop(&mut target, "param", param);
                        }
                    }
                    Rc::new(RefCell::new(Value::Map(target)))
                })
                .collect();
            map.insert("mappings".to_string(), value_cell(Value::List(mappings)));
            Rc::new(RefCell::new(Value::Map(map)))
        })
        .collect();
    panel_map.insert("macros".to_string(), value_cell(Value::List(macros)));
    panel_map.insert(
        "slots".to_string(),
        Rc::new(RefCell::new(Value::List(slots))),
    );
    panel_map.insert(
        "processing-cost".to_string(),
        value_cell(Value::Number(
            rack.slots
                .iter()
                .map(|slot| {
                    slot.max_polyphony
                        + slot
                            .effect_slots
                            .iter()
                            .filter(|effect| effect.node_id != 0)
                            .count()
                })
                .sum::<usize>() as f64,
        )),
    );
    panel_map.insert("pads".to_string(), Rc::new(RefCell::new(Value::List(pads))));
    panel_map.insert(
        "pad-banks".to_string(),
        Rc::new(RefCell::new(Value::List(pad_banks))),
    );
    if let Some(selected_instrument) = selected_instrument {
        panel_map.insert("selected-instrument".to_string(), selected_instrument);
    }
    Value::List(vec![Rc::new(RefCell::new(Value::Map(panel_map)))])
}

/// Build a Lisp Value::List of bools indicating which steps are selected.
pub(crate) fn build_selection_value(selected: &Arc<Mutex<HashSet<usize>>>) -> Value {
    let set = selected.lock().unwrap();
    build_selection_value_from_set(&set)
}

/// Build a Lisp Value::List of bools from an already-held selection snapshot.
pub(crate) fn build_selection_value_from_set(set: &HashSet<usize>) -> Value {
    let items: Vec<Rc<RefCell<Value>>> = (0..MAX_STEPS)
        .map(|s| Rc::new(RefCell::new(Value::Bool(set.contains(&s)))))
        .collect();
    Value::List(items)
}

/// Build list of available effect names from the effects/ directory.
pub(crate) fn build_available_effects() -> Value {
    let names = sequencer::lisp_host::list_saved_effects();
    let items: Vec<Rc<RefCell<Value>>> = names
        .into_iter()
        .map(|n| Rc::new(RefCell::new(Value::String(n))))
        .collect();
    Value::List(items)
}

pub(crate) fn build_available_builtin_effects() -> Value {
    let mut items: Vec<Rc<RefCell<Value>>> =
        sequencer::effects::EffectDescriptor::builtin_insert_names()
            .iter()
            .map(|name| Rc::new(RefCell::new(Value::String((*name).to_string()))))
            .collect();
    // dgenlisp-backed builtins (added through the builtin path, DSP is dgenlisp)
    items.push(Rc::new(RefCell::new(Value::String(
        sequencer::effects::conv_reverb::NAME.to_string(),
    ))));
    Value::List(items)
}

pub(crate) fn build_available_midi_effects() -> Value {
    let mut names: Vec<String> = sequencer::lisp_host::load_midi_fx_descriptors()
        .into_iter()
        .map(|desc| desc.name)
        .collect();
    names.sort();
    let items: Vec<Rc<RefCell<Value>>> = names
        .into_iter()
        .map(|name| Rc::new(RefCell::new(Value::String(name))))
        .collect();
    Value::List(items)
}

pub(crate) fn midi_fx_option_index(fx_name: &str, param_idx: usize, label: &str) -> Option<usize> {
    sequencer::lisp_host::load_midi_fx_descriptor(fx_name)
        .and_then(|desc| desc.params.get(param_idx).cloned())
        .and_then(|param| match param.kind {
            sequencer::effects::ParamKind::Enum { labels } => {
                labels.iter().position(|item| item == label)
            }
            _ => None,
        })
}

const METER_FLOOR_DBFS: f32 = -60.0;

pub(crate) fn master_meter_level(peak: f32) -> f64 {
    if peak <= 0.0 || !peak.is_finite() {
        0.0
    } else {
        let db = 20.0 * peak.log10();
        ((db - METER_FLOOR_DBFS) / -METER_FLOOR_DBFS).clamp(0.0, 1.0) as f64
    }
}

pub(crate) fn quantize_meter_level(level: f64) -> f64 {
    ((level.clamp(0.0, 1.0) * METER_LEVEL_STEPS).round()) / METER_LEVEL_STEPS
}

pub(crate) fn meter_display_level(peak: f32) -> f64 {
    quantize_meter_level(master_meter_level(peak))
}

pub(crate) fn sync_project_state(rt: &mut Runtime, app: &app::App) {
    rt.set_reactive(
        "SEQ",
        "current-project-name",
        Value::String(app.current_project_name.clone().unwrap_or_default()),
    );
    rt.set_reactive("SEQ", "sound-presets", build_sound_presets_value());
}

pub(crate) fn build_sound_presets_value() -> Value {
    let sounds = sequencer::project::list_sound_presets().unwrap_or_default();
    list_value(sounds.into_iter().filter_map(|path| {
        let preset = sequencer::project::load_sound_preset(&path).ok()?;
        let label = if preset.metadata.name.trim().is_empty() {
            path.file_stem()?.to_str()?.to_string()
        } else {
            preset.metadata.name
        };
        Some(map_value([
            ("kind", Value::String("sound".to_string())),
            ("label", Value::String(label.clone())),
            ("name", Value::String(label)),
            ("path", Value::String(path.to_string_lossy().to_string())),
            ("author", Value::String(preset.metadata.author)),
            (
                "tags",
                list_value(preset.metadata.tags.into_iter().map(Value::String)),
            ),
        ]))
    }))
}

pub(crate) const PROJECT_SCRATCH_BUFFER_NAME: &str = "*scratch*";

fn project_scratch_source_path() -> PathBuf {
    sequencer::paths::project_scratch_source_path()
}

pub(crate) fn clear_project_script_tabs(editor: &mut Editor) -> Result<(), String> {
    editor
        .runtime_mut()
        .eval_str("(seq-clear-project-script-tabs)")
        .map_err(|error| format!("Failed to clear project script tabs: {error:?}"))?;
    editor.refresh_runtime_side_effects();
    Ok(())
}

fn project_script_load_path(line: &str) -> Option<String> {
    let tokens = Parser::new(line.to_string()).parse().ok()?;
    let expressions = ASTParser::new(tokens).parse().ok()?;
    let [Expression::List(items)] = expressions.as_slice() else {
        return None;
    };
    match items.as_slice() {
        [Expression::Symbol(load), Expression::String(path)] if load == "load" => {
            Some(path.clone())
        }
        _ => None,
    }
}

fn canonical_project_script_path(path: &str) -> PathBuf {
    let path = Path::new(path);
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        sequencer::paths::workspace_root().join(path)
    };
    std::fs::canonicalize(&absolute).unwrap_or(absolute)
}

pub(crate) fn remove_project_script_from_scratch(editor: &mut Editor, source_path: &str) -> bool {
    let target = canonical_project_script_path(source_path);
    let Some(buffer) = editor
        .buffers
        .iter_mut()
        .find(|buffer| buffer.name == PROJECT_SCRATCH_BUFFER_NAME)
    else {
        return false;
    };

    let mut removed = false;
    let mut previous_blank = true;
    let mut kept = Vec::with_capacity(buffer.lines.len());
    for line in &buffer.lines {
        let matches_target = project_script_load_path(line)
            .is_some_and(|path| canonical_project_script_path(&path) == target);
        if matches_target {
            removed = true;
            continue;
        }
        let blank = line.trim().is_empty();
        if blank && previous_blank {
            continue;
        }
        kept.push(line.clone());
        previous_blank = blank;
    }
    while kept.last().is_some_and(|line| line.trim().is_empty()) {
        kept.pop();
    }
    if removed {
        buffer.set_text(&kept.join("\n"));
        editor.mark_needs_redraw();
    }
    removed
}

pub(crate) fn push_project_scratch_to_named_buffer(editor: &mut Editor, app: &app::App) {
    let scratch_text = app.editor.scratch_buffer.clone();
    let scratch_cursor = app.editor.scratch_cursor;

    let id = editor.upsert_scratch_buffer(PROJECT_SCRATCH_BUFFER_NAME, &scratch_text);
    let scratch_path = project_scratch_source_path();
    if let Some(buffer) = editor.buffers.iter_mut().find(|buffer| buffer.id == id) {
        buffer.path = Some(scratch_path);
    }

    if editor.active_buffer().name == PROJECT_SCRATCH_BUFFER_NAME {
        let buffer = editor.active_buffer_mut();
        let row = scratch_cursor.0.min(buffer.lines.len().saturating_sub(1));
        let col = scratch_cursor.1.min(buffer.lines[row].len());
        buffer.cursor = (row, col);
    }
}

pub(crate) fn evaluate_project_scratch_on_ui_runtime(
    editor: &mut Editor,
    app: &app::App,
) -> Result<(), String> {
    let scratch_text = app.editor.scratch_buffer.clone();
    if scratch_text.trim().is_empty() {
        return Ok(());
    }

    let overlays = editor.snapshot_file_backed_sources();
    let report = editor.runtime_mut().eval_source_transactional(
        Some(project_scratch_source_path()),
        &scratch_text,
        overlays,
    );
    let result = if report.success {
        Ok(())
    } else {
        Err(report.failure_message())
    };
    editor.process_lisp_reload_report(report);
    if let Some(status) = editor.runtime_mut().take_status_message() {
        editor.show_transient_message(status);
    }
    result
}

pub(crate) fn pull_named_scratch_buffer_into_project(editor: &Editor, app: &mut app::App) {
    let Some(buffer) = editor
        .buffers
        .iter()
        .find(|buffer| buffer.name == PROJECT_SCRATCH_BUFFER_NAME)
    else {
        return;
    };

    let text = buffer.text();
    let cursor = buffer.cursor;
    if app.editor.scratch_buffer != text || app.editor.scratch_cursor != cursor {
        app.editor.scratch_buffer = text.clone();
        app.editor.scratch_cursor = cursor;
        app.state.set_scratch_source(text);
        app.editor.scratch_runtime = None;
    }
}

pub(crate) fn current_custom_instrument_name(app: &app::App, track: usize) -> Option<String> {
    if app.tracks.is_empty() || app.is_sampler_track(track) {
        None
    } else if let Some(Some(engine_id)) = app.graph.track_engine_ids.get(track) {
        app.editor
            .engine_registry
            .get(*engine_id)
            .map(|engine| engine.name.clone())
    } else {
        app.tracks.get(track).cloned()
    }
}

pub(crate) fn sync_sidebar_browser(rt: &mut Runtime, app: &app::App, track: usize) {
    rt.set_reactive(
        "SEQ",
        "project-instrument-engines",
        build_string_list(&project_instrument_engine_names(app)),
    );
    if app.graph.track_instrument_types.get(track)
        == Some(&sequencer::sequencer::InstrumentType::Sampler)
    {
        let selected_sample = app
            .sampler_path_for_track(track)
            .map(|path| path.to_string_lossy().to_string())
            .unwrap_or_default();
        rt.set_reactive("SEQ", "sidebar-kind", Value::String("sampler".to_string()));
        rt.set_reactive(
            "SEQ",
            "sidebar-instrument-name",
            Value::String(String::new()),
        );
        rt.set_reactive(
            "SEQ",
            "sidebar-instrument-display-name",
            Value::String(String::new()),
        );
        rt.set_reactive("SEQ", "sidebar-loaded-preset", Value::String(String::new()));
        rt.set_reactive("SEQ", "sidebar-track-index", Value::Number(track as f64));
        rt.set_reactive(
            "SEQ",
            "sidebar-selected-sample",
            Value::String(selected_sample),
        );
        rt.set_reactive("SEQ", "sidebar-presets", Value::List(vec![]));
        rt.set_reactive("SEQ", "sidebar-preset-tree", Value::List(vec![]));
        return;
    }

    let is_rack = app.graph.track_instrument_types.get(track)
        == Some(&sequencer::sequencer::InstrumentType::Rack);
    let instrument_name = if is_rack {
        app.tracks.get(track).cloned().unwrap_or_default()
    } else {
        current_custom_instrument_name(app, track).unwrap_or_default()
    };
    let loaded_preset = app
        .state
        .pattern
        .track_sound_state
        .lock()
        .unwrap()
        .get(track)
        .and_then(|meta| meta.loaded_preset.clone())
        .unwrap_or_default();
    let preset_items = visible_preset_items_for_track(app, track);

    rt.set_reactive(
        "SEQ",
        "sidebar-kind",
        Value::String("instrument".to_string()),
    );
    rt.set_reactive(
        "SEQ",
        "sidebar-instrument-name",
        Value::String(instrument_name.clone()),
    );
    rt.set_reactive(
        "SEQ",
        "sidebar-instrument-display-name",
        Value::String(instrument_display_name(&instrument_name)),
    );
    rt.set_reactive(
        "SEQ",
        "sidebar-loaded-preset",
        Value::String(loaded_preset.clone()),
    );
    rt.set_reactive("SEQ", "sidebar-track-index", Value::Number(track as f64));
    rt.set_reactive(
        "SEQ",
        "sidebar-selected-sample",
        Value::String(String::new()),
    );
    rt.set_reactive("SEQ", "sidebar-presets", build_string_list(&preset_items));
    rt.set_reactive(
        "SEQ",
        "sidebar-preset-tree",
        build_flat_tree_items(&preset_items),
    );
}

pub(crate) fn load_instrument_preset_into_track(
    app: &mut app::App,
    track: usize,
    preset_name: &str,
) -> Result<(), String> {
    let instrument_name = current_custom_instrument_name(app, track)
        .ok_or_else(|| "Current track is not a custom instrument".to_string())?;
    let presets = sequencer::lisp_host::load_instrument_presets(&instrument_name)
        .map_err(|e| e.to_string())?;
    let preset = presets
        .into_iter()
        .find(|preset| preset.name == preset_name)
        .ok_or_else(|| format!("Preset '{preset_name}' not found"))?;
    let desc = app
        .graph
        .instrument_descriptors
        .get(track)
        .cloned()
        .ok_or_else(|| "Instrument descriptor unavailable".to_string())?;

    let engine_id = app.graph.track_engine_ids.get(track).and_then(|id| *id);
    let preset_label = preset.name.clone();
    sequencer::app::edit::apply_recorded_instrument_values_mutation(
        app,
        track,
        format!("Load preset '{preset_label}'"),
        move |app| {
            let slot = &app.state.pattern.instrument_slots[track];
            for (param_idx, param) in desc.params.iter().enumerate() {
                let value = preset
                    .params
                    .get(&param.name)
                    .copied()
                    .unwrap_or(param.default);
                let clamped = param.clamp(value);
                slot.defaults.set(param_idx, clamped);
                app.send_instrument_param(track, param_idx, clamped);
            }
            sequencer::effects::restore_key_locks_by_param_name(
                slot,
                &desc,
                &preset.key_locks,
            );
            app.state.pattern.instrument_base_note_offsets[track]
                .store(preset.base_note_offset.to_bits(), Ordering::Relaxed);
            app.state.schedule_mod_resync();
            if let Some(meta) = app
                .state
                .pattern
                .track_sound_state
                .lock()
                .unwrap()
                .get_mut(track)
            {
                meta.engine_id = engine_id;
                meta.loaded_preset = Some(preset.name.clone());
                meta.dirty = false;
            }
            Ok(())
        },
    )
    .map(|_| ())
    .map_err(|error| format!("{error:?}"))
}

/// Extract the :path string from a host-command payload dict.
pub(crate) fn extract_path_from_payload(payload: &Value) -> Option<String> {
    extract_string_from_payload(payload, "path")
}

pub(crate) fn extract_string_from_payload(payload: &Value, key: &str) -> Option<String> {
    if let Value::Map(map) = payload {
        if let Some(cell) = map.get(key) {
            if let Value::String(s) | Value::Keyword(s) | Value::Symbol(s) = &*cell.borrow() {
                return Some(s.clone());
            }
        }
    }
    None
}

pub(crate) fn extract_usize_from_payload(payload: &Value, key: &str) -> Option<usize> {
    if let Value::Map(map) = payload {
        if let Some(cell) = map.get(key) {
            if let Value::Number(n) = &*cell.borrow() {
                return (*n >= 0.0).then_some(*n as usize);
            }
        }
    }
    None
}

pub(crate) fn extract_i32_from_payload(payload: &Value, key: &str) -> Option<i32> {
    if let Value::Map(map) = payload {
        if let Some(cell) = map.get(key) {
            if let Value::Number(n) = &*cell.borrow() {
                return Some(*n as i32);
            }
        }
    }
    None
}

pub(crate) fn extract_f32_from_payload(payload: &Value, key: &str) -> Option<f32> {
    if let Value::Map(map) = payload {
        if let Some(cell) = map.get(key) {
            if let Value::Number(n) = &*cell.borrow() {
                return Some(*n as f32);
            }
        }
    }
    None
}

pub(crate) fn extract_bool_from_payload(payload: &Value, key: &str) -> bool {
    if let Value::Map(map) = payload {
        if let Some(cell) = map.get(key) {
            return matches!(&*cell.borrow(), Value::Bool(true));
        }
    }
    false
}

/// Push individual tp-* reactive fields for the current track.
pub(crate) fn sync_track_params(
    rt: &mut Runtime,
    app: &app::App,
    state: &Arc<SequencerState>,
    track: usize,
    selected: &Arc<Mutex<HashSet<usize>>>,
) {
    let tp = &state.pattern.track_params[track];
    rt.set_reactive("SEQ", "tp-attack", Value::Number(tp.get_attack_ms() as f64));
    rt.set_reactive(
        "SEQ",
        "tp-release",
        Value::Number(tp.get_release_ms() as f64),
    );
    rt.set_reactive("SEQ", "tp-send", Value::Number(tp.get_send() as f64));
    rt.set_reactive("SEQ", "tp-output", build_track_output_label(app, tp));
    rt.set_reactive(
        "SEQ",
        "track-output-options",
        build_track_output_options(app),
    );
    rt.set_reactive("SEQ", "tp-bus-sends", build_track_bus_sends(app, tp));
    sync_current_track_bus_send_binding_fields(rt, app, state, track);
    rt.set_reactive(
        "SEQ",
        "tp-num-steps",
        Value::Number(tp.get_num_steps() as f64),
    );
    rt.set_reactive("SEQ", "tp-gate", Value::Bool(tp.is_gate_on()));
    // For a Rack track, playback polyphony is governed per-slot
    // (RackSlotSnapshot::max_polyphony, read by fire_rack_slot_note /
    // fire_live_keyboard_rack_note) — the track-level TrackParams poly/voices
    // fields below are never consulted for Sampler/Custom rack slots. Surface
    // the *selected slot's* values here (and which slot they'd be writing to)
    // so this panel's poly/voices controls can be routed to the right place
    // instead of silently editing a value playback ignores.
    let rack_slot_poly = (app.graph.track_instrument_types.get(track)
        == Some(&sequencer::sequencer::InstrumentType::Rack))
    .then(|| {
        let rack = app
            .state
            .pattern
            .rack_tracks
            .lock()
            .unwrap()
            .get(track)
            .cloned()
            .flatten()?;
        let selected_slot = app.selected_rack_slot_index_for_rack(track, &rack)?;
        let max_polyphony = rack.slots.get(selected_slot)?.max_polyphony;
        Some((selected_slot, max_polyphony))
    })
    .flatten();
    rt.set_reactive("SEQ", "tp-is-rack", Value::Bool(rack_slot_poly.is_some()));
    rt.set_reactive(
        "SEQ",
        "tp-rack-slot-idx",
        Value::Number(rack_slot_poly.map(|(slot_idx, _)| slot_idx).unwrap_or(0) as f64),
    );
    let (tp_poly, max_polyphony) = match rack_slot_poly {
        Some((_, max_polyphony)) => (max_polyphony > 1, max_polyphony),
        // Non-rack tracks: `is_polyphonic` is its own independently-toggled
        // flag, distinct from the voice-count value — don't derive it from
        // max_polyphony or the toggle button's state gets stomped every
        // render.
        None => (tp.is_polyphonic(), tp.get_max_polyphony()),
    };
    rt.set_reactive("SEQ", "tp-poly", Value::Bool(tp_poly));
    rt.set_reactive(
        "SEQ",
        "tp-max-polyphony",
        Value::Number(max_polyphony as f64),
    );
    let _ = sync_track_selection_param_binding_fields(rt, state, track, selected);
    rt.set_reactive(
        "SEQ",
        "tp-fts",
        Value::String(
            FTS_SCALE_NAMES
                .get(tp.get_fts_scale())
                .copied()
                .unwrap_or("Off")
                .to_string(),
        ),
    );
    rt.set_reactive(
        "SEQ",
        "tp-mute-group",
        Value::String(mute_group_label(tp.get_mute_group())),
    );
    rt.set_reactive(
        "SEQ",
        "tp-accumulator",
        Value::String(selected_accumulator_name(app, track)),
    );
    rt.set_reactive(
        "SEQ",
        "tp-accum-limit",
        Value::Number(tp.get_accum_limit() as f64),
    );
    rt.set_reactive(
        "SEQ",
        "tp-accum-mode",
        Value::String(accum_mode_label(tp.get_accum_mode()).to_string()),
    );
    rt.set_reactive("SEQ", "accumulator-options", build_accumulator_options(app));
    rt.set_reactive("SEQ", "fts-options", build_fts_options());
    rt.set_reactive("SEQ", "mute-group-options", build_mute_group_options());
    rt.set_reactive("SEQ", "accum-mode-options", build_accum_mode_options());
    rt.set_reactive(
        "SEQ",
        "track-plocks",
        build_track_plocks_value(app, state, track, selected),
    );
    rt.set_reactive(
        "SEQ",
        "track-plock-variants",
        build_track_plock_variants_value(state, track, selected),
    );
}

/// Refreshes only track-parameter fields whose displayed value follows the
/// selected step's p-lock. Selection changes should use this instead of
/// rebuilding every track parameter and option list.
pub(crate) fn sync_track_selection_param_binding_fields(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    track: usize,
    selected: &Arc<Mutex<HashSet<usize>>>,
) -> bool {
    let tp = &state.pattern.track_params[track];
    let selected_step = selected_plock_step(selected);
    let display_step = displayed_plock_step(state, track, selected_step);
    let swing = display_step
        .and_then(|step| state.pattern.swing_plocks[track].get(step))
        .unwrap_or_else(|| tp.get_swing());
    let timebase = display_step
        .and_then(|step| state.pattern.timebase_plocks[track].get(step))
        .unwrap_or_else(|| tp.get_timebase());
    let swing_resolution = display_step
        .and_then(|step| state.pattern.swing_resolution_plocks[track].get(step))
        .unwrap_or_else(|| tp.get_swing_resolution());

    let mut dirty = rt
        .set_reactive("SEQ", "tp-swing", Value::Number(swing as f64))
        .effects_dirty;
    dirty |= rt
        .set_reactive(
            "SEQ",
            "tp-timebase",
            Value::String(timebase.label().to_string()),
        )
        .effects_dirty;
    dirty |= rt
        .set_reactive(
            "SEQ",
            "tp-swing-resolution",
            Value::String(swing_resolution.label().to_string()),
        )
        .effects_dirty;
    dirty
}

pub(crate) fn sync_track_params_with_neural_selection(
    rt: &mut Runtime,
    app: &app::App,
    state: &Arc<SequencerState>,
    track: usize,
    selected: &Arc<Mutex<HashSet<usize>>>,
    selected_neural_neurons: Option<
        &std::collections::BTreeSet<sequencer::lisp_host::SelectedNeuralNeuron>,
    >,
) {
    sync_track_params(rt, app, state, track, selected);
    rt.set_reactive(
        "SEQ",
        "track-plocks",
        build_track_plocks_value_with_neural_selection(
            app,
            state,
            track,
            selected,
            selected_neural_neurons,
        ),
    );
    rt.set_reactive(
        "SEQ",
        "track-plock-variants",
        build_track_plock_variants_value(state, track, selected),
    );
}

fn plock_entry(
    step: usize,
    target: &str,
    group: &str,
    name: &str,
    value: f32,
    default: f32,
    min: f32,
    max: f32,
    slot_idx: Option<usize>,
    param_idx: Option<usize>,
    options: Option<Vec<String>>,
) -> Rc<RefCell<Value>> {
    plock_entry_with_label(
        &format!("S{}", step + 1),
        step,
        target,
        group,
        name,
        value,
        default,
        min,
        max,
        slot_idx,
        param_idx,
        options,
        None,
        None,
        None,
    )
}

fn rack_effect_plock_entry(
    step: usize,
    rack_slot: usize,
    effect_slot: usize,
    effect_name: &str,
    param_name: &str,
    value: f32,
    default: f32,
    min: f32,
    max: f32,
    param_idx: usize,
    options: Option<Vec<String>>,
) -> Rc<RefCell<Value>> {
    let entry = plock_entry(
        step,
        "rack-effect",
        effect_name,
        param_name,
        value,
        default,
        min,
        max,
        Some(effect_slot),
        Some(param_idx),
        options,
    );
    if let Value::Map(map) = &mut *entry.borrow_mut() {
        map.insert(
            "rack-slot".to_string(),
            Rc::new(RefCell::new(Value::Number(rack_slot as f64))),
        );
    }
    entry
}

fn plock_entry_with_label(
    label: &str,
    step: usize,
    target: &str,
    group: &str,
    name: &str,
    value: f32,
    default: f32,
    min: f32,
    max: f32,
    slot_idx: Option<usize>,
    param_idx: Option<usize>,
    options: Option<Vec<String>>,
    source: Option<&str>,
    target_track: Option<usize>,
    network_id: Option<u64>,
) -> Rc<RefCell<Value>> {
    use std::collections::HashMap;

    let mut map: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
    map.insert(
        "label".to_string(),
        Rc::new(RefCell::new(Value::String(label.to_string()))),
    );
    map.insert(
        "step".to_string(),
        Rc::new(RefCell::new(Value::Number((step + 1) as f64))),
    );
    map.insert(
        "step-idx".to_string(),
        Rc::new(RefCell::new(Value::Number(step as f64))),
    );
    map.insert(
        "target".to_string(),
        Rc::new(RefCell::new(Value::String(target.to_string()))),
    );
    map.insert(
        "group".to_string(),
        Rc::new(RefCell::new(Value::String(group.to_string()))),
    );
    map.insert(
        "name".to_string(),
        Rc::new(RefCell::new(Value::String(name.to_string()))),
    );
    map.insert(
        "value".to_string(),
        Rc::new(RefCell::new(Value::Number(value as f64))),
    );
    map.insert(
        "default".to_string(),
        Rc::new(RefCell::new(Value::Number(default as f64))),
    );
    map.insert(
        "min".to_string(),
        Rc::new(RefCell::new(Value::Number(min as f64))),
    );
    map.insert(
        "max".to_string(),
        Rc::new(RefCell::new(Value::Number(max as f64))),
    );
    if let Some(slot_idx) = slot_idx {
        map.insert(
            "slot-idx".to_string(),
            Rc::new(RefCell::new(Value::Number(slot_idx as f64))),
        );
    }
    if let Some(param_idx) = param_idx {
        map.insert(
            "param-idx".to_string(),
            Rc::new(RefCell::new(Value::Number(param_idx as f64))),
        );
    }
    if let Some(source) = source {
        map.insert(
            "source".to_string(),
            Rc::new(RefCell::new(Value::String(source.to_string()))),
        );
        if source == "neuron" {
            map.insert(
                "neuron-idx".to_string(),
                Rc::new(RefCell::new(Value::Number(step as f64))),
            );
        }
    }
    if let Some(target_track) = target_track {
        map.insert(
            "target-track".to_string(),
            Rc::new(RefCell::new(Value::Number(target_track as f64))),
        );
    }
    if let Some(network_id) = network_id {
        map.insert(
            "network-id".to_string(),
            Rc::new(RefCell::new(Value::Number(network_id as f64))),
        );
    }
    if let Some(options) = options {
        let selected = options
            .get(value.round().max(0.0) as usize)
            .cloned()
            .unwrap_or_default();
        let default_text = options
            .get(default.round().max(0.0) as usize)
            .cloned()
            .unwrap_or_default();
        map.insert(
            "text-value".to_string(),
            Rc::new(RefCell::new(Value::String(selected))),
        );
        map.insert(
            "default-text".to_string(),
            Rc::new(RefCell::new(Value::String(default_text))),
        );
        map.insert(
            "options".to_string(),
            Rc::new(RefCell::new(Value::List(
                options
                    .into_iter()
                    .map(|label| Rc::new(RefCell::new(Value::String(label))))
                    .collect(),
            ))),
        );
    } else {
        map.insert(
            "text-value".to_string(),
            Rc::new(RefCell::new(Value::String(format!("{value:.2}")))),
        );
        map.insert(
            "default-text".to_string(),
            Rc::new(RefCell::new(Value::String(format!("{default:.2}")))),
        );
    }
    map.insert(
        "domain".to_string(),
        Rc::new(RefCell::new(Value::String(
            plock_entry_domain(target).to_string(),
        ))),
    );
    Rc::new(RefCell::new(Value::Map(map)))
}

fn plock_entry_domain(target: &str) -> &'static str {
    match target {
        "instrument"
        | "instrument-tensor"
        | "rack-macro"
        | "rack-slot-param"
        | "rack-slot-instrument"
        | "rack-slot-instrument-tensor" => "inst",
        "effect" | "effect-tensor" | "bus-effect" | "rack-effect" => "fx",
        "neural-instrument" | "neural-effect" => "neural",
        _ => "seq",
    }
}

fn plock_param_options(kind: &sequencer::effects::ParamKind) -> Option<Vec<String>> {
    match kind {
        sequencer::effects::ParamKind::Enum { labels } => Some(labels.clone()),
        sequencer::effects::ParamKind::Boolean => Some(vec!["off".to_string(), "on".to_string()]),
        sequencer::effects::ParamKind::Continuous { .. } => None,
    }
}

fn preview_plock_entry(
    label: &str,
    target: &str,
    group: &str,
    name: &str,
    value: f32,
    default: f32,
    min: f32,
    max: f32,
    slot_idx: Option<usize>,
    param_idx: Option<usize>,
    options: Option<Vec<String>>,
) -> Rc<RefCell<Value>> {
    let entry = plock_entry_with_label(
        label,
        0,
        target,
        group,
        name,
        value,
        default,
        min,
        max,
        slot_idx,
        param_idx,
        options,
        Some("preview"),
        None,
        None,
    );
    {
        let mut value = entry.borrow_mut();
        if let Value::Map(map) = &mut *value {
            map.insert("preview".to_string(), value_cell(Value::Bool(true)));
            map.insert(
                "preview-label".to_string(),
                value_cell(Value::String(label.to_string())),
            );
        }
    }
    entry
}

fn tensor_cell_label(desc: &sequencer::effects::TensorParamDescriptor, cell_idx: usize) -> String {
    let rows = desc.rows();
    let cols = desc.cols();
    if rows > 1 && cols > 0 {
        let row = cell_idx / cols;
        let col = cell_idx % cols;
        format!("{} {}:{}", desc.name, row + 1, col + 1)
    } else {
        format!("{} {}", desc.name, cell_idx + 1)
    }
}

fn live_tensor_default_cell(
    slot: &sequencer::effects::EffectSlotState,
    desc: &sequencer::effects::TensorParamDescriptor,
    tensor_idx: usize,
    cell_idx: usize,
) -> f32 {
    slot.tensor_params
        .default_values(tensor_idx)
        .and_then(|values| values.get(cell_idx).copied())
        .or_else(|| desc.default.get(cell_idx).copied())
        .unwrap_or_default()
}

fn snapshot_tensor_default_cell(
    slot: &sequencer::effects::EffectSlotSnapshot,
    desc: &sequencer::effects::TensorParamDescriptor,
    tensor_idx: usize,
    cell_idx: usize,
) -> f32 {
    slot.tensor_default_values(tensor_idx)
        .and_then(|values| values.get(cell_idx).copied())
        .or_else(|| desc.default.get(cell_idx).copied())
        .unwrap_or_default()
}

fn rack_slot_param_by_index(index: usize) -> Option<sequencer::sequencer::RackSlotParam> {
    sequencer::sequencer::RackSlotParam::ALL
        .iter()
        .copied()
        .find(|param| param.index() == index)
}

fn rack_slot_param_bounds(param: sequencer::sequencer::RackSlotParam) -> (f32, f32) {
    match param {
        sequencer::sequencer::RackSlotParam::BaseNote => (-48.0, 48.0),
        sequencer::sequencer::RackSlotParam::Gain => (0.0, 2.0),
        sequencer::sequencer::RackSlotParam::Pan => (-1.0, 1.0),
        sequencer::sequencer::RackSlotParam::MaxPolyphony => (
            1.0,
            sequencer::sequencer::RackSlotParam::MaxPolyphony.clamp(f32::MAX),
        ),
        sequencer::sequencer::RackSlotParam::Mute | sequencer::sequencer::RackSlotParam::Solo => {
            (0.0, 1.0)
        }
    }
}

fn rack_slot_param_options(param: sequencer::sequencer::RackSlotParam) -> Option<Vec<String>> {
    match param {
        sequencer::sequencer::RackSlotParam::Mute | sequencer::sequencer::RackSlotParam::Solo => {
            Some(vec!["off".to_string(), "on".to_string()])
        }
        _ => None,
    }
}

fn build_track_plock_preview_row_for_variant_entry(
    app: &app::App,
    state: &Arc<SequencerState>,
    track: usize,
    label: &str,
    entry: &sequencer::plock_variants::PlockVariantEntry,
) -> Option<Rc<RefCell<Value>>> {
    let stored_value = f32::from_bits(entry.value_bits);
    match entry.domain {
        sequencer::plock_variants::PlockVariantDomain::TrackTimebase => {
            let default = state.pattern.track_params.get(track)?.get_timebase() as u32 as f32;
            Some(preview_plock_entry(
                label,
                "timebase",
                "track",
                "timebase",
                stored_value,
                default,
                0.0,
                (Timebase::ALL.len() - 1) as f32,
                None,
                None,
                Some(
                    Timebase::LABELS
                        .iter()
                        .map(|label| label.to_string())
                        .collect(),
                ),
            ))
        }
        sequencer::plock_variants::PlockVariantDomain::TrackSwing => {
            let default = state.pattern.track_params.get(track)?.get_swing();
            Some(preview_plock_entry(
                label,
                "swing",
                "track",
                "swing",
                stored_value,
                default,
                50.0,
                75.0,
                None,
                None,
                None,
            ))
        }
        sequencer::plock_variants::PlockVariantDomain::TrackSwingResolution => {
            let default = state
                .pattern
                .track_params
                .get(track)?
                .get_swing_resolution() as u32 as f32;
            Some(preview_plock_entry(
                label,
                "swing-resolution",
                "track",
                "swing res",
                stored_value,
                default,
                0.0,
                (SwingResolution::ALL.len() - 1) as f32,
                None,
                None,
                Some(
                    SwingResolution::LABELS
                        .iter()
                        .map(|label| label.to_string())
                        .collect(),
                ),
            ))
        }
        sequencer::plock_variants::PlockVariantDomain::MidiEffect => {
            let effect_name = state
                .pattern
                .track_params
                .get(track)?
                .midi_fx_chain()
                .get(entry.slot)?
                .clone();
            let desc = sequencer::lisp_host::load_midi_fx_descriptor(&effect_name)?;
            let param = desc.params.get(entry.param)?;
            let slot = state.pattern.midi_fx_slots.get(track)?.get(entry.slot)?;
            Some(preview_plock_entry(
                label,
                "midi-fx",
                &desc.name,
                &param.name,
                stored_value,
                slot.defaults.get(entry.param),
                param.min,
                param.max,
                Some(entry.slot),
                Some(entry.param),
                plock_param_options(&param.kind),
            ))
        }
        sequencer::plock_variants::PlockVariantDomain::MidiEffectTensor => {
            let cell_idx = entry.cell?;
            let effect_name = state
                .pattern
                .track_params
                .get(track)?
                .midi_fx_chain()
                .get(entry.slot)?
                .clone();
            let desc = sequencer::lisp_host::load_midi_fx_descriptor(&effect_name)?;
            let tensor = desc.tensor_params.get(entry.param)?;
            let slot = state.pattern.midi_fx_slots.get(track)?.get(entry.slot)?;
            Some(preview_plock_entry(
                label,
                "midi-fx-tensor",
                &desc.name,
                &tensor_cell_label(tensor, cell_idx),
                stored_value,
                live_tensor_default_cell(slot, tensor, entry.param, cell_idx),
                tensor.min,
                tensor.max,
                Some(entry.slot),
                Some(entry.param),
                None,
            ))
        }
        sequencer::plock_variants::PlockVariantDomain::Instrument => {
            let desc = app.graph.instrument_descriptors.get(track)?;
            let param = desc.params.get(entry.param)?;
            let slot = state.pattern.instrument_slots.get(track)?;
            Some(preview_plock_entry(
                label,
                "instrument",
                "inst",
                &param.name,
                param.stored_to_user(stored_value),
                param.stored_to_user(slot.defaults.get(entry.param)),
                param.stored_to_user(param.min),
                param.stored_to_user(param.max),
                None,
                Some(entry.param),
                plock_param_options(&param.kind),
            ))
        }
        sequencer::plock_variants::PlockVariantDomain::InstrumentTensor => {
            let cell_idx = entry.cell?;
            let desc = app.graph.instrument_descriptors.get(track)?;
            let tensor = desc.tensor_params.get(entry.param)?;
            let slot = state.pattern.instrument_slots.get(track)?;
            Some(preview_plock_entry(
                label,
                "instrument-tensor",
                "inst",
                &tensor_cell_label(tensor, cell_idx),
                stored_value,
                live_tensor_default_cell(slot, tensor, entry.param, cell_idx),
                tensor.min,
                tensor.max,
                None,
                Some(entry.param),
                None,
            ))
        }
        sequencer::plock_variants::PlockVariantDomain::Effect => {
            let desc = app
                .graph
                .effect_descriptors
                .get(track)
                .and_then(|descs| descs.get(entry.slot))?;
            let param = desc.params.get(entry.param)?;
            let slot = state
                .pattern
                .effect_chains
                .get(track)
                .and_then(|chain| chain.get(entry.slot))?;
            Some(preview_plock_entry(
                label,
                "effect",
                &desc.name,
                &param.name,
                stored_value,
                slot.defaults.get(entry.param),
                param.min,
                param.max,
                Some(entry.slot),
                Some(entry.param),
                plock_param_options(&param.kind),
            ))
        }
        sequencer::plock_variants::PlockVariantDomain::EffectTensor => {
            let cell_idx = entry.cell?;
            let desc = app
                .graph
                .effect_descriptors
                .get(track)
                .and_then(|descs| descs.get(entry.slot))?;
            let tensor = desc.tensor_params.get(entry.param)?;
            let slot = state
                .pattern
                .effect_chains
                .get(track)
                .and_then(|chain| chain.get(entry.slot))?;
            Some(preview_plock_entry(
                label,
                "effect-tensor",
                &desc.name,
                &tensor_cell_label(tensor, cell_idx),
                stored_value,
                live_tensor_default_cell(slot, tensor, entry.param, cell_idx),
                tensor.min,
                tensor.max,
                Some(entry.slot),
                Some(entry.param),
                None,
            ))
        }
        sequencer::plock_variants::PlockVariantDomain::RackMacro => {
            let rack = state
                .pattern
                .rack_tracks
                .lock()
                .unwrap()
                .get(track)
                .cloned()
                .flatten()?;
            let rack_macro = rack.macros.get(entry.param)?;
            Some(preview_plock_entry(
                label,
                "rack-macro",
                "rack",
                &rack_macro.name,
                stored_value,
                rack_macro.value,
                0.0,
                1.0,
                None,
                Some(entry.param),
                None,
            ))
        }
        sequencer::plock_variants::PlockVariantDomain::RackSlotParam => {
            let param = rack_slot_param_by_index(entry.param)?;
            let rack = state
                .pattern
                .rack_tracks
                .lock()
                .unwrap()
                .get(track)
                .cloned()
                .flatten()?;
            let slot = rack.slots.get(entry.slot)?;
            let (min, max) = rack_slot_param_bounds(param);
            Some(preview_plock_entry(
                label,
                "rack-slot-param",
                "rack",
                param.name(),
                param.clamp(stored_value),
                slot.param_default(param),
                min,
                max,
                Some(entry.slot),
                Some(entry.param),
                rack_slot_param_options(param),
            ))
        }
        sequencer::plock_variants::PlockVariantDomain::RackSlotInstrument => {
            let rack = state
                .pattern
                .rack_tracks
                .lock()
                .unwrap()
                .get(track)
                .cloned()
                .flatten()?;
            let slot = rack.slots.get(entry.slot)?;
            let desc = app.rack_slot_instrument_descriptor(slot)?;
            let param = desc.params.get(entry.param)?;
            Some(preview_plock_entry(
                label,
                "rack-slot-instrument",
                "rack",
                &param.name,
                param.stored_to_user(stored_value),
                param.stored_to_user(
                    slot.instrument_slot
                        .defaults
                        .get(entry.param)
                        .copied()
                        .unwrap_or(param.default),
                ),
                param.stored_to_user(param.min),
                param.stored_to_user(param.max),
                Some(entry.slot),
                Some(entry.param),
                plock_param_options(&param.kind),
            ))
        }
        sequencer::plock_variants::PlockVariantDomain::RackSlotInstrumentTensor => {
            let cell_idx = entry.cell?;
            let rack = state
                .pattern
                .rack_tracks
                .lock()
                .unwrap()
                .get(track)
                .cloned()
                .flatten()?;
            let slot = rack.slots.get(entry.slot)?;
            let desc = app.rack_slot_instrument_descriptor(slot)?;
            let tensor = desc.tensor_params.get(entry.param)?;
            Some(preview_plock_entry(
                label,
                "rack-slot-instrument-tensor",
                "rack",
                &tensor_cell_label(tensor, cell_idx),
                stored_value,
                snapshot_tensor_default_cell(&slot.instrument_slot, tensor, entry.param, cell_idx),
                tensor.min,
                tensor.max,
                Some(entry.slot),
                Some(entry.param),
                None,
            ))
        }
        sequencer::plock_variants::PlockVariantDomain::InstrumentKeyLock => None,
    }
}

pub(crate) fn build_track_plocks_value_for_variant_label(
    app: &app::App,
    state: &Arc<SequencerState>,
    track: usize,
    label: &str,
) -> Value {
    if label == "def" {
        return Value::List(vec![]);
    }
    let Some(assignment) = state
        .plock_variant_registry_snapshot(track)
        .assignment_for_label(label)
    else {
        return Value::List(vec![]);
    };
    let items = assignment
        .key
        .entries
        .iter()
        .filter_map(|entry| {
            build_track_plock_preview_row_for_variant_entry(
                app,
                state,
                track,
                &assignment.label,
                entry,
            )
        })
        .collect();
    Value::List(items)
}

pub(crate) fn build_track_plocks_value_with_neural_selection(
    app: &app::App,
    state: &Arc<SequencerState>,
    track: usize,
    selected: &Arc<Mutex<HashSet<usize>>>,
    selected_neural_neurons: Option<
        &std::collections::BTreeSet<sequencer::lisp_host::SelectedNeuralNeuron>,
    >,
) -> Value {
    let Some(selection) = selected_neural_neurons else {
        return build_track_plocks_value(app, state, track, selected);
    };
    let current_pattern = state.current_scene_index();
    if !selection
        .iter()
        .any(|selected| selected.pattern_idx == current_pattern)
    {
        return build_track_plocks_value(app, state, track, selected);
    }
    build_selected_neural_plocks_value(app, state, selection)
}

fn build_selected_neural_plocks_value(
    app: &app::App,
    state: &Arc<SequencerState>,
    selection: &std::collections::BTreeSet<sequencer::lisp_host::SelectedNeuralNeuron>,
) -> Value {
    use sequencer::effects::ParamKind;

    let current_pattern = state.current_scene_index();
    let networks = state.current_neural_networks();
    let mut items = Vec::new();

    for selected in selection
        .iter()
        .filter(|selected| selected.pattern_idx == current_pattern)
    {
        let Some(network) = networks
            .iter()
            .find(|network| network.id == selected.network_id)
        else {
            continue;
        };
        let Some(neuron) = network.neurons.get(selected.neuron_idx) else {
            continue;
        };
        let label = format!("N{}", selected.neuron_idx + 1);

        for override_param in &neuron.output_overrides.instrument {
            let target_track = override_param.target_track;
            let Some(desc) = app.graph.instrument_descriptors.get(target_track) else {
                continue;
            };
            let Some(param) = desc.params.get(override_param.param_index) else {
                continue;
            };
            let Some(slot) = state.pattern.instrument_slots.get(target_track) else {
                continue;
            };
            if slot.param_node_id(override_param.param_index) != Some(override_param.param_id) {
                continue;
            }
            let options = match &param.kind {
                ParamKind::Enum { labels } => Some(labels.clone()),
                ParamKind::Boolean => Some(vec!["off".to_string(), "on".to_string()]),
                ParamKind::Continuous { .. } => None,
            };
            items.push(plock_entry_with_label(
                &label,
                selected.neuron_idx,
                "neural-instrument",
                &format!("T{} inst", target_track + 1),
                &param.name,
                param.stored_to_user(override_param.value),
                param.stored_to_user(param.default),
                param.stored_to_user(param.min),
                param.stored_to_user(param.max),
                None,
                Some(override_param.param_index),
                options,
                Some("neuron"),
                Some(target_track),
                Some(network.id),
            ));
        }

        for override_param in &neuron.output_overrides.effects {
            let target_track = override_param.target_track;
            let Some(desc) = app
                .graph
                .effect_descriptors
                .get(target_track)
                .and_then(|descs| descs.get(override_param.slot_index))
            else {
                continue;
            };
            let Some(param) = desc.params.get(override_param.param_index) else {
                continue;
            };
            let Some(slot) = state
                .pattern
                .effect_chains
                .get(target_track)
                .and_then(|chain| chain.get(override_param.slot_index))
            else {
                continue;
            };
            if slot.param_node_id(override_param.param_index) != Some(override_param.param_id) {
                continue;
            }
            let options = match &param.kind {
                ParamKind::Enum { labels } => Some(labels.clone()),
                ParamKind::Boolean => Some(vec!["off".to_string(), "on".to_string()]),
                ParamKind::Continuous { .. } => None,
            };
            items.push(plock_entry_with_label(
                &label,
                selected.neuron_idx,
                "neural-effect",
                &format!("T{} {}", target_track + 1, desc.name),
                &param.name,
                override_param.value,
                param.default,
                param.min,
                param.max,
                Some(override_param.slot_index),
                Some(override_param.param_index),
                options,
                Some("neuron"),
                Some(target_track),
                Some(network.id),
            ));
        }
    }

    Value::List(items)
}

pub(crate) fn build_track_plocks_value(
    app: &app::App,
    state: &Arc<SequencerState>,
    track: usize,
    selected: &Arc<Mutex<HashSet<usize>>>,
) -> Value {
    use sequencer::effects::ParamKind;

    let Some(step) = selected_plock_step(selected) else {
        return Value::List(vec![]);
    };
    let mut items = Vec::new();

    let tp = &state.pattern.track_params[track];
    if let Some(timebase) = state.pattern.timebase_plocks[track].get(step) {
        items.push(plock_entry(
            step,
            "timebase",
            "track",
            "timebase",
            timebase as u32 as f32,
            tp.get_timebase() as u32 as f32,
            0.0,
            (Timebase::ALL.len() - 1) as f32,
            None,
            None,
            Some(
                Timebase::LABELS
                    .iter()
                    .map(|label| label.to_string())
                    .collect(),
            ),
        ));
    }
    if let Some(swing) = state.pattern.swing_plocks[track].get(step) {
        items.push(plock_entry(
            step,
            "swing",
            "track",
            "swing",
            swing,
            tp.get_swing(),
            50.0,
            75.0,
            None,
            None,
            None,
        ));
    }
    if let Some(resolution) = state.pattern.swing_resolution_plocks[track].get(step) {
        items.push(plock_entry(
            step,
            "swing-resolution",
            "track",
            "swing res",
            resolution as u32 as f32,
            tp.get_swing_resolution() as u32 as f32,
            0.0,
            (SwingResolution::ALL.len() - 1) as f32,
            None,
            None,
            Some(
                SwingResolution::LABELS
                    .iter()
                    .map(|label| label.to_string())
                    .collect(),
            ),
        ));
    }
    for param in StepParam::ALL {
        let value = state.pattern.step_data[track].get(step, param);
        if value.to_bits() == param.default_value().to_bits() {
            continue;
        }
        items.push(plock_entry(
            step,
            "step-param",
            "per step",
            param.short_label(),
            value,
            param.default_value(),
            param.min(),
            param.max(),
            None,
            Some(param.index()),
            None,
        ));
    }

    if let Some(desc) = app.graph.instrument_descriptors.get(track) {
        let slot = &state.pattern.instrument_slots[track];
        for (param_idx, param) in desc.params.iter().enumerate() {
            if let Some(value) = slot.plocks.get(step, param_idx) {
                let options = match &param.kind {
                    ParamKind::Enum { labels } => Some(labels.clone()),
                    ParamKind::Boolean => Some(vec!["off".to_string(), "on".to_string()]),
                    ParamKind::Continuous { .. } => None,
                };
                items.push(plock_entry(
                    step,
                    "instrument",
                    "inst",
                    &param.name,
                    param.stored_to_user(value),
                    param.stored_to_user(slot.defaults.get(param_idx)),
                    param.stored_to_user(param.min),
                    param.stored_to_user(param.max),
                    None,
                    Some(param_idx),
                    options,
                ));
            }
        }
    }

    if let Some(descs) = app.graph.effect_descriptors.get(track) {
        for (slot_idx, desc) in descs.iter().enumerate() {
            let Some(slot) = state.pattern.effect_chains[track].get(slot_idx) else {
                continue;
            };
            for (param_idx, param) in desc.params.iter().enumerate() {
                if let Some(value) = slot.plocks.get(step, param_idx) {
                    let options = match &param.kind {
                        ParamKind::Enum { labels } => Some(labels.clone()),
                        ParamKind::Boolean => Some(vec!["off".to_string(), "on".to_string()]),
                        ParamKind::Continuous { .. } => None,
                    };
                    items.push(plock_entry(
                        step,
                        "effect",
                        &desc.name,
                        &param.name,
                        value,
                        slot.defaults.get(param_idx),
                        param.min,
                        param.max,
                        Some(slot_idx),
                        Some(param_idx),
                        options,
                    ));
                }
            }
        }
    }
    if let Some(Some(rack)) = state.pattern.rack_tracks.lock().unwrap().get(track) {
        for rack_macro in &rack.macros {
            if let Some(value) = rack_macro.plocks.get(step).copied().flatten() {
                let entry = plock_entry(
                    step,
                    "rack-macro",
                    "rack",
                    &rack_macro.name,
                    value,
                    rack_macro.value,
                    0.0,
                    1.0,
                    None,
                    Some(rack_macro.id.index()),
                    None,
                );
                if let Value::Map(map) = &mut *entry.borrow_mut() {
                    map.insert(
                        "value-field".to_string(),
                        value_cell(Value::String(rack_macro_value_field(
                            track,
                            rack_macro.id.index(),
                        ))),
                    );
                }
                items.push(entry);
            }
        }
        for (rack_slot_idx, rack_slot) in rack.slots.iter().enumerate() {
            for (effect_slot_idx, (descriptor, effect_slot)) in rack_slot
                .effect_descriptors
                .iter()
                .zip(&rack_slot.effect_slots)
                .enumerate()
            {
                if effect_slot.node_id == 0 {
                    continue;
                }
                for (param_idx, param) in descriptor.params.iter().enumerate() {
                    let Some(value) = effect_slot
                        .plocks
                        .get(step)
                        .and_then(|row| row.get(param_idx))
                        .copied()
                        .flatten()
                    else {
                        continue;
                    };
                    items.push(rack_effect_plock_entry(
                        step,
                        rack_slot_idx,
                        effect_slot_idx,
                        &descriptor.name,
                        &param.name,
                        value,
                        effect_slot
                            .defaults
                            .get(param_idx)
                            .copied()
                            .unwrap_or(param.default),
                        param.min,
                        param.max,
                        param_idx,
                        plock_param_options(&param.kind),
                    ));
                }
            }
        }
    }

    let midi_chain = tp.midi_fx_chain();
    for (slot_idx, slot) in state.pattern.midi_fx_slots[track].iter().enumerate() {
        let Some(desc) = midi_chain
            .get(slot_idx)
            .and_then(|name| sequencer::lisp_host::load_midi_fx_descriptor(name))
        else {
            continue;
        };
        for (param_idx, param) in desc.params.iter().enumerate() {
            if let Some(value) = slot.plocks.get(step, param_idx) {
                let options = match &param.kind {
                    ParamKind::Enum { labels } => Some(labels.clone()),
                    ParamKind::Boolean => Some(vec!["off".to_string(), "on".to_string()]),
                    ParamKind::Continuous { .. } => None,
                };
                items.push(plock_entry(
                    step,
                    "midi-fx",
                    &desc.name,
                    &param.name,
                    value,
                    slot.defaults.get(param_idx),
                    param.min,
                    param.max,
                    Some(slot_idx),
                    Some(param_idx),
                    options,
                ));
            }
        }
    }

    Value::List(items)
}

pub(crate) fn build_track_plock_variants_value(
    state: &Arc<SequencerState>,
    track: usize,
    selected: &Arc<Mutex<HashSet<usize>>>,
) -> Value {
    build_track_plock_variants_value_with_preview(state, track, selected, None)
}

pub(crate) fn build_track_plock_variants_value_with_preview(
    state: &Arc<SequencerState>,
    track: usize,
    selected: &Arc<Mutex<HashSet<usize>>>,
    preview_label: Option<&str>,
) -> Value {
    let registry = state.plock_variant_registry_snapshot(track);
    let selected_step = selected_plock_step(selected);
    let current_key = selected_step.and_then(|step| {
        sequencer::plock_variants::live_track_variant_key(state.as_ref(), track, step)
    });
    let preview_label = current_key.is_none().then_some(preview_label).flatten();
    let preview_is_def = preview_label.map_or(true, |label| label == "def");

    let mut items = Vec::with_capacity(registry.entries.len() + 1);
    let mut def_map = HashMap::new();
    def_map.insert(
        "kind".to_string(),
        Rc::new(RefCell::new(Value::String("def".to_string()))),
    );
    def_map.insert(
        "label".to_string(),
        Rc::new(RefCell::new(Value::String("def".to_string()))),
    );
    def_map.insert(
        "display".to_string(),
        Rc::new(RefCell::new(Value::String("base".to_string()))),
    );
    def_map.insert(
        "count".to_string(),
        Rc::new(RefCell::new(Value::Number(0.0))),
    );
    def_map.insert(
        "current".to_string(),
        Rc::new(RefCell::new(Value::Bool(
            current_key.is_none() && preview_is_def,
        ))),
    );
    def_map.insert(
        "color-r".to_string(),
        Rc::new(RefCell::new(Value::Number(0.545_098_07))),
    );
    def_map.insert(
        "color-g".to_string(),
        Rc::new(RefCell::new(Value::Number(0.545_098_07))),
    );
    def_map.insert(
        "color-b".to_string(),
        Rc::new(RefCell::new(Value::Number(0.588_235_3))),
    );
    items.push(Rc::new(RefCell::new(Value::Map(def_map))));

    for entry in registry.entries {
        let mut map = HashMap::new();
        map.insert(
            "kind".to_string(),
            Rc::new(RefCell::new(Value::String("variant".to_string()))),
        );
        map.insert(
            "label".to_string(),
            Rc::new(RefCell::new(Value::String(entry.label.clone()))),
        );
        map.insert(
            "display".to_string(),
            Rc::new(RefCell::new(Value::String(
                entry.name.clone().unwrap_or_else(|| entry.label.clone()),
            ))),
        );
        map.insert(
            "count".to_string(),
            Rc::new(RefCell::new(Value::Number(entry.key.param_count() as f64))),
        );
        map.insert(
            "current".to_string(),
            Rc::new(RefCell::new(Value::Bool(
                current_key.as_ref().is_some_and(|key| key == &entry.key)
                    || preview_label.is_some_and(|label| label == entry.label),
            ))),
        );
        map.insert(
            "color-r".to_string(),
            Rc::new(RefCell::new(Value::Number(entry.color[0] as f64))),
        );
        map.insert(
            "color-g".to_string(),
            Rc::new(RefCell::new(Value::Number(entry.color[1] as f64))),
        );
        map.insert(
            "color-b".to_string(),
            Rc::new(RefCell::new(Value::Number(entry.color[2] as f64))),
        );
        items.push(Rc::new(RefCell::new(Value::Map(map))));
    }

    Value::List(items)
}

fn build_track_output_label(app: &app::App, tp: &sequencer::sequencer::TrackParams) -> Value {
    let label = match tp.output() {
        sequencer::sequencer::TrackOutput::Mix => "main".to_string(),
        sequencer::sequencer::TrackOutput::None => "sends only".to_string(),
        sequencer::sequencer::TrackOutput::Bus(id) => app
            .buses
            .iter()
            .find(|bus| bus.id == id)
            .map(|bus| bus.name.clone())
            .unwrap_or_else(|| "main".to_string()),
    };
    Value::String(label)
}

fn build_track_output_options(app: &app::App) -> Value {
    let mut labels = vec![
        Rc::new(RefCell::new(Value::String("main".to_string()))),
        Rc::new(RefCell::new(Value::String("sends only".to_string()))),
    ];
    labels.extend(
        app.buses
            .iter()
            .filter(|bus| bus.id != sequencer::sequencer::BusId::MIX)
            .map(|bus| Rc::new(RefCell::new(Value::String(bus.name.clone())))),
    );
    Value::List(labels)
}

fn build_track_bus_sends(app: &app::App, _tp: &sequencer::sequencer::TrackParams) -> Value {
    use std::collections::HashMap;

    let items = app
        .buses
        .iter()
        .enumerate()
        .filter(|(_, bus)| bus.id != sequencer::sequencer::BusId::MIX)
        .map(|(bus_idx, bus)| {
            let mut map = HashMap::new();
            map.insert(
                "bus-idx".to_string(),
                Rc::new(RefCell::new(Value::Number(bus_idx as f64))),
            );
            map.insert(
                "name".to_string(),
                Rc::new(RefCell::new(Value::String(bus.name.clone()))),
            );
            Rc::new(RefCell::new(Value::Map(map)))
        })
        .collect();
    Value::List(items)
}

/// Build a Lisp Value::Map of track parameters for the current track.
pub(crate) fn build_track_params(state: &Arc<SequencerState>, track: usize) -> Value {
    use std::collections::HashMap;
    let tp = &state.pattern.track_params[track];
    let mut map: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
    map.insert(
        "gate".into(),
        Rc::new(RefCell::new(Value::Bool(tp.is_gate_on()))),
    );
    map.insert(
        "attack".into(),
        Rc::new(RefCell::new(Value::Number(tp.get_attack_ms() as f64))),
    );
    map.insert(
        "release".into(),
        Rc::new(RefCell::new(Value::Number(tp.get_release_ms() as f64))),
    );
    map.insert(
        "swing".into(),
        Rc::new(RefCell::new(Value::Number(tp.get_swing() as f64))),
    );
    map.insert(
        "swing-resolution".into(),
        Rc::new(RefCell::new(Value::String(
            tp.get_swing_resolution().label().to_string(),
        ))),
    );
    map.insert(
        "num-steps".into(),
        Rc::new(RefCell::new(Value::Number(tp.get_num_steps() as f64))),
    );
    map.insert(
        "volume".into(),
        Rc::new(RefCell::new(Value::Number(tp.get_volume() as f64))),
    );
    map.insert(
        "pan".into(),
        Rc::new(RefCell::new(Value::Number(tp.get_pan() as f64))),
    );
    map.insert(
        "mute".into(),
        Rc::new(RefCell::new(Value::Bool(tp.is_muted()))),
    );
    map.insert(
        "solo".into(),
        Rc::new(RefCell::new(Value::Bool(tp.is_solo()))),
    );
    map.insert(
        "timebase".into(),
        Rc::new(RefCell::new(Value::String(
            tp.get_timebase().label().to_string(),
        ))),
    );
    map.insert(
        "send".into(),
        Rc::new(RefCell::new(Value::Number(tp.get_send() as f64))),
    );
    map.insert(
        "poly".into(),
        Rc::new(RefCell::new(Value::Bool(tp.is_polyphonic()))),
    );
    map.insert(
        "max-polyphony".into(),
        Rc::new(RefCell::new(Value::Number(tp.get_max_polyphony() as f64))),
    );
    map.insert(
        "mute-group".into(),
        Rc::new(RefCell::new(Value::Number(tp.get_mute_group() as f64))),
    );
    Value::Map(map)
}

/// Build a Lisp Value::List of bools indicating which steps have any p-locks on the given track.
pub(crate) fn build_step_has_plocks(
    state: &Arc<SequencerState>,
    track: usize,
    descriptors: &[Vec<sequencer::effects::EffectDescriptor>],
) -> Value {
    let mask = track_step_plock_mask(state, track, descriptors);
    build_step_has_plocks_from_mask(&mask)
}

pub(crate) fn build_step_has_plocks_from_mask(mask: &[u64; MAX_STEPS / 64]) -> Value {
    let items: Vec<Rc<RefCell<Value>>> = (0..MAX_STEPS)
        .map(|step| {
            Rc::new(RefCell::new(Value::Bool(
                mask[step / 64] & (1u64 << (step % 64)) != 0,
            )))
        })
        .collect();
    Value::List(items)
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct PlockVariantStepRender {
    pub(crate) kind: u8,
    pub(crate) color: [f32; 3],
}

pub(crate) fn plock_variant_step_render_values(
    state: &Arc<SequencerState>,
    track: usize,
) -> Vec<PlockVariantStepRender> {
    const SEQ_ONLY_COLOR: [f32; 3] = [0.545_098_07, 0.545_098_07, 0.588_235_3];
    let assignments = state.reconcile_plock_variant_registry_for_track(track);
    (0..MAX_STEPS)
        .map(|step| {
            if let Some(assignment) = assignments.get(step).and_then(Clone::clone) {
                PlockVariantStepRender {
                    kind: 2,
                    color: assignment.color,
                }
            } else if sequencer::plock_variants::live_track_has_seq_lock(
                state.as_ref(),
                track,
                step,
            ) {
                PlockVariantStepRender {
                    kind: 1,
                    color: SEQ_ONLY_COLOR,
                }
            } else {
                PlockVariantStepRender {
                    kind: 0,
                    color: [0.0, 0.0, 0.0],
                }
            }
        })
        .collect()
}

pub(crate) fn build_step_plock_kinds(state: &Arc<SequencerState>, track: usize) -> Value {
    Value::List(
        plock_variant_step_render_values(state, track)
            .into_iter()
            .map(|render| Rc::new(RefCell::new(Value::Number(render.kind as f64))))
            .collect(),
    )
}

pub(crate) fn build_step_variant_color_channel(
    state: &Arc<SequencerState>,
    track: usize,
    channel: usize,
) -> Value {
    Value::List(
        plock_variant_step_render_values(state, track)
            .into_iter()
            .map(|render| {
                Rc::new(RefCell::new(Value::Number(
                    render.color.get(channel).copied().unwrap_or(0.0) as f64,
                )))
            })
            .collect(),
    )
}

pub(crate) fn build_all_track_step_plock_kinds(
    state: &Arc<SequencerState>,
    app: &app::App,
) -> Value {
    Value::List(
        (0..app.tracks.len())
            .map(|track| Rc::new(RefCell::new(build_step_plock_kinds(state, track))))
            .collect(),
    )
}

pub(crate) fn build_all_track_step_variant_color_channel(
    state: &Arc<SequencerState>,
    app: &app::App,
    channel: usize,
) -> Value {
    Value::List(
        (0..app.tracks.len())
            .map(|track| {
                Rc::new(RefCell::new(build_step_variant_color_channel(
                    state, track, channel,
                )))
            })
            .collect(),
    )
}

/// One bit per step: whether any effect/instrument/midi-fx/timebase/swing
/// plock exists for that step. Single flat scan per slot instead of the
/// per-(step, slot, param) probing done by track_step_has_plock.
pub(crate) fn track_step_plock_mask(
    state: &Arc<SequencerState>,
    track: usize,
    descriptors: &[Vec<sequencer::effects::EffectDescriptor>],
) -> [u64; MAX_STEPS / 64] {
    let mut mask = [0u64; MAX_STEPS / 64];
    let chain = &state.pattern.effect_chains[track];
    let num_slots = descriptors.get(track).map(|d| d.len()).unwrap_or(0);
    for slot_idx in 0..num_slots {
        if let Some(slot) = chain.get(slot_idx) {
            let np = slot.num_params.load(Ordering::Relaxed) as usize;
            slot.plocks.or_step_plock_mask(&mut mask, np);
        }
    }
    for slot in &state.pattern.midi_fx_slots[track] {
        let np = slot.num_params.load(Ordering::Relaxed) as usize;
        slot.plocks.or_step_plock_mask(&mut mask, np);
    }
    let instrument_slot = &state.pattern.instrument_slots[track];
    let instrument_np = instrument_slot.num_params.load(Ordering::Relaxed) as usize;
    instrument_slot
        .plocks
        .or_step_plock_mask(&mut mask, instrument_np);
    if let Some(Some(rack)) = state.pattern.rack_tracks.lock().unwrap().get(track) {
        for rack_macro in &rack.macros {
            for (step, value) in rack_macro.plocks.iter().enumerate().take(MAX_STEPS) {
                if value.is_some() {
                    mask[step / 64] |= 1u64 << (step % 64);
                }
            }
        }
        for slot in &rack.slots {
            for step in 0..MAX_STEPS {
                if slot.param_plocks.step_has_plock(step) {
                    mask[step / 64] |= 1u64 << (step % 64);
                }
            }
            let num_params = slot.instrument_slot.num_params as usize;
            for step in 0..MAX_STEPS {
                let Some(step_plocks) = slot.instrument_slot.plocks.get(step) else {
                    continue;
                };
                if step_plocks
                    .iter()
                    .take(num_params)
                    .any(|value| value.is_some())
                {
                    mask[step / 64] |= 1u64 << (step % 64);
                }
            }
            for effect in &slot.effect_slots {
                let num_params = effect.num_params as usize;
                for step in 0..MAX_STEPS {
                    if effect
                        .plocks
                        .get(step)
                        .is_some_and(|row| row.iter().take(num_params).any(Option::is_some))
                    {
                        mask[step / 64] |= 1u64 << (step % 64);
                    }
                }
            }
        }
    }
    let timebase_plocks = &state.pattern.timebase_plocks[track];
    let swing_plocks = &state.pattern.swing_plocks[track];
    let swing_resolution_plocks = &state.pattern.swing_resolution_plocks[track];
    for step in 0..MAX_STEPS {
        let word = step / 64;
        let bit = 1u64 << (step % 64);
        if mask[word] & bit != 0 {
            continue;
        }
        if timebase_plocks.has_plock(step)
            || swing_plocks.has_plock(step)
            || swing_resolution_plocks.has_plock(step)
            || sequencer::plock_variants::live_track_has_seq_lock(state.as_ref(), track, step)
            || sequencer::plock_variants::live_track_variant_key(state.as_ref(), track, step)
                .is_some()
        {
            mask[word] |= bit;
        }
    }
    mask
}

pub(crate) fn track_step_has_plock(
    state: &Arc<SequencerState>,
    track: usize,
    descriptors: &[Vec<sequencer::effects::EffectDescriptor>],
    step: usize,
) -> bool {
    let chain = &state.pattern.effect_chains[track];
    let midi_fx_slots = &state.pattern.midi_fx_slots[track];
    let num_slots = descriptors.get(track).map(|d| d.len()).unwrap_or(0);
    let instrument_slot = &state.pattern.instrument_slots[track];
    let instrument_num_params = instrument_slot.num_params.load(Ordering::Relaxed) as usize;
    let timebase_plocks = &state.pattern.timebase_plocks[track];
    let swing_plocks = &state.pattern.swing_plocks[track];
    let swing_resolution_plocks = &state.pattern.swing_resolution_plocks[track];
    let effect_has_plock = (0..num_slots).any(|slot_idx| {
        let Some(slot) = chain.get(slot_idx) else {
            return false;
        };
        let np = slot.num_params.load(Ordering::Relaxed) as usize;
        (0..np).any(|p| slot.plocks.get(step, p).is_some())
    });
    let instrument_has_plock =
        (0..instrument_num_params).any(|p| instrument_slot.plocks.get(step, p).is_some());
    let rack_slot_has_plock = state
        .pattern
        .rack_tracks
        .lock()
        .unwrap()
        .get(track)
        .and_then(|rack| rack.as_ref())
        .is_some_and(|rack| {
            if rack
                .macros
                .iter()
                .any(|rack_macro| rack_macro.plocks.get(step).is_some_and(Option::is_some))
            {
                return true;
            }
            rack.slots.iter().any(|slot| {
                if slot.param_plocks.step_has_plock(step) {
                    return true;
                }
                let num_params = slot.instrument_slot.num_params as usize;
                if slot
                    .instrument_slot
                    .plocks
                    .get(step)
                    .is_some_and(|step_plocks| {
                        step_plocks
                            .iter()
                            .take(num_params)
                            .any(|value| value.is_some())
                    })
                {
                    return true;
                }
                slot.effect_slots.iter().any(|effect| {
                    let num_params = effect.num_params as usize;
                    effect
                        .plocks
                        .get(step)
                        .is_some_and(|row| row.iter().take(num_params).any(Option::is_some))
                })
            })
        });
    let midi_fx_has_plock = midi_fx_slots.iter().any(|slot| {
        let np = slot.num_params.load(Ordering::Relaxed) as usize;
        (0..np).any(|p| slot.plocks.get(step, p).is_some())
    });

    effect_has_plock
        || midi_fx_has_plock
        || instrument_has_plock
        || rack_slot_has_plock
        || timebase_plocks.has_plock(step)
        || swing_plocks.has_plock(step)
        || swing_resolution_plocks.has_plock(step)
        || sequencer::plock_variants::live_track_has_seq_lock(state.as_ref(), track, step)
        || sequencer::plock_variants::live_track_variant_key(state.as_ref(), track, step).is_some()
}

#[cfg(test)]
#[path = "state_values_tests.rs"]
mod tests;

pub(crate) fn auto_follow_enabled(override_until: &Arc<Mutex<Option<Instant>>>) -> bool {
    let guard = override_until.lock().unwrap();
    match *guard {
        Some(until) => Instant::now() >= until,
        None => true,
    }
}

pub(crate) fn poll_pending_compile_status(
    app: &mut app::App,
    editor: &mut Editor,
    state: &Arc<SequencerState>,
    current_track: &Arc<AtomicUsize>,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
    fx_epoch: &Arc<AtomicUsize>,
    ui_epoch: &Arc<AtomicUsize>,
) {
    if let Some(status) = app.poll_pending_compile() {
        let ct = current_track.load(Ordering::Relaxed);
        let rt = editor.runtime_mut();
        rt.set_reactive("SEQ", "compiling", Value::Bool(false));
        rt.set_reactive(
            "SEQ",
            "effects",
            if app.tracks.is_empty() {
                Value::List(vec![])
            } else {
                build_effects_value(&state, ct, &app.graph.effect_descriptors, &selected_steps)
            },
        );
        rt.set_reactive(
            "SEQ",
            "midi-effects",
            if app.tracks.is_empty() {
                Value::List(vec![])
            } else {
                build_midi_effects_value(&state, ct, &selected_steps)
            },
        );
        rt.set_reactive(
            "SEQ",
            "instrument-panel",
            if app.tracks.is_empty() {
                Value::List(vec![])
            } else {
                build_instrument_panel_value(&app, ct, &selected_steps)
            },
        );
        sync_fx_param_binding_fields(rt, app, state, ct, selected_steps);
        rt.set_reactive(
            "SEQ",
            "step-has-plocks",
            if app.tracks.is_empty() {
                Value::List(vec![])
            } else {
                build_step_has_plocks(state, ct, &app.graph.effect_descriptors)
            },
        );
        rt.run_reactive_cycle();
        editor.refresh_runtime_side_effects();
        editor.refresh_visible_layouts_for_buffer_named("*fx*");
        fx_epoch.fetch_add(1, Ordering::Relaxed);
        ui_epoch.fetch_add(1, Ordering::Relaxed);
        editor.handle_host_event(HostEvent::Status(status));
    }
}
