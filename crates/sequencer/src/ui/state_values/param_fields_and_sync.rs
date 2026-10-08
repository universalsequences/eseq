use super::*;

/// The value rack slot `slot_idx`'s strip control `param` shows at
/// `display_step`: the step's p-lock, else a rack macro mapped onto it, else
/// the slot's own value (the host
/// kinds' `device.gain-display`, …).
pub(crate) fn rack_slot_control_value(
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

/// A rack slot strip control's value as its fields show it: a flag for
/// mute and solo, else a number (the
/// host kinds' `device.gain`, …).
pub(crate) fn rack_slot_control_reactive_value(
    param: sequencer::sequencer::RackSlotParam,
    value: f32,
) -> Value {
    match param {
        sequencer::sequencer::RackSlotParam::Mute | sequencer::sequencer::RackSlotParam::Solo => {
            Value::Bool(value > 0.5)
        }
        _ => Value::Number(value as f64),
    }
}

/// A rack slot sampler's sample path: its buffer's, else its sample name's.
/// Shared with the host kinds' rack slot media.
pub(crate) fn rack_slot_sample_path<'a>(
    app: &'a app::App,
    slot: &sequencer::sequencer::RackSlotSnapshot,
) -> Option<&'a PathBuf> {
    let (buffer_id, sample_name, _) = slot.sample_id.as_ref()?;
    app.sample_buffer_path_registry
        .get(buffer_id)
        .or_else(|| app.sample_path_registry.get(sample_name))
}

pub(crate) fn sync_all_track_sequencer_state(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    app: &app::App,
) {
    sync_all_track_sequencer_state_inner(rt, state, app, None);
}

pub(crate) fn sync_all_track_sequencer_state_profiled(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    app: &app::App,
) -> AllTrackSequencerSyncProfile {
    let mut profile = AllTrackSequencerSyncProfile::default();
    sync_all_track_sequencer_state_inner(rt, state, app, Some(&mut profile));
    profile
}

pub(super) fn sync_all_track_sequencer_state_inner(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    app: &app::App,
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
        "track-duration-spans",
        build_all_track_duration_spans_value(state, app),
    );
    if let Some(profile) = profile.as_deref_mut() {
        profile.track_duration_spans = started.expect("profile timer").elapsed();
    }

    let started = profile.as_ref().map(|_| Instant::now());
    let variants: Vec<_> = (0..app.tracks.len())
        .map(|track| plock_variant_step_render_values(state, track)).collect();
    let plock_masks: Vec<[u64; MAX_STEPS / 64]> = (0..app.tracks.len())
        .map(|track| track_step_plock_mask_with_render(state, track,
            &app.graph.effect_descriptors, Some(&variants[track])))
        .collect();
    rt.set_reactive(
        "SEQ",
        "track-step-has-plocks",
        build_all_track_step_has_plocks_from_masks(&plock_masks),
    );
    rt.set_reactive("SEQ", "track-step-plock-kinds",
        list_value(variants.iter().map(|values| build_step_plock_kinds_from_render(values))));
    for (channel, field) in ["track-step-variant-r", "track-step-variant-g", "track-step-variant-b"].iter().enumerate() {
        rt.set_reactive("SEQ", field, list_value(variants.iter()
            .map(|values| build_step_variant_color_channel_from_render(values, channel))));
    }
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
        "track-retrigs",
        build_all_track_param_lists_value(state, app, StepParam::Retrig),
    );
    rt.set_reactive(
        "SEQ",
        "track-retrig-rates",
        build_all_track_param_lists_value(state, app, StepParam::RetrigRate),
    );
    rt.set_reactive(
        "SEQ", "track-process-lane-values",
        build_all_track_process_lane_values(state, app.tracks.len()),
    );
    rt.set_reactive(
        "SEQ",
        "track-process-lanes",
        build_all_track_process_lanes_value(state, app.tracks.len()),
    );

    let started = profile.as_ref().map(|_| Instant::now());
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

/// The step params the *step* inspector strip shows as number pickers, in
/// panel order. Every entry must have an [`fx_step_param_value_field`], and
/// this is the single list the readout sync, the print-latch restore and the
/// empty-project reset all walk.
pub(crate) const STEP_INSPECTOR_PARAMS: [StepParam; 6] = [
    StepParam::Transpose,
    StepParam::Velocity,
    StepParam::Duration,
    StepParam::Pan,
    StepParam::Retrig,
    StepParam::RetrigRate,
];

pub(crate) fn fx_step_param_value_field(param: StepParam) -> Option<&'static str> {
    match param {
        StepParam::Velocity => Some("fx-step-value-velocity"),
        StepParam::Duration => Some("fx-step-value-duration"),
        StepParam::Transpose => Some("fx-step-value-transpose"),
        StepParam::Pan => Some("fx-step-value-pan"),
        StepParam::Retrig => Some("fx-step-value-retrig"),
        StepParam::RetrigRate => Some("fx-step-value-retrig-rate"),
        _ => None,
    }
}

pub(crate) fn fx_step_cursor_from_runtime(rt: &Runtime) -> usize {
    fx_step_cursor_value(rt.global_value(FX_STEP_CURSOR_GLOBAL))
}

/// The Lisp global holding the step panel's cursor (`eseq.vanilla/cursor-step`).
pub(crate) const FX_STEP_CURSOR_GLOBAL: &str = "cursor-step";

/// The step cursor a [`FX_STEP_CURSOR_GLOBAL`] value names (0 when unset).
pub(crate) fn fx_step_cursor_value(value: Option<Value>) -> usize {
    match value {
        Some(Value::Number(step)) if step >= 0.0 => step as usize,
        _ => 0,
    }
}

/// The step panel's cursor step and the step it edits (the first selected
/// step, else the cursor), clipped to a track of `num_steps` steps
/// (`SEQ.fx-step-cursor-number` − 1 and `SEQ.fx-step-parameter-step`;
/// `selection.cursor-step` and `selection.edit-step`).
pub(crate) fn fx_step_cursor(
    num_steps: usize,
    cursor_step: usize,
    selected_step: Option<usize>,
) -> (usize, usize) {
    let last = num_steps.max(1).min(MAX_STEPS) - 1;
    let cursor_step = cursor_step.min(last);
    (cursor_step, selected_step.unwrap_or(cursor_step).min(last))
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
    let (cursor_step, parameter_step) = fx_step_cursor(num_steps, cursor_step, selected_step);
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
    for param in STEP_INSPECTOR_PARAMS {
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

pub(crate) fn sync_current_track_step_param_lists(rt: &mut Runtime, state: &Arc<SequencerState>, track: usize) {
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
        "retrigs",
        build_param_list(state, track, StepParam::Retrig),
    );
    rt.set_reactive(
        "SEQ",
        "retrig-rates",
        build_param_list(state, track, StepParam::RetrigRate),
    );
}

pub(crate) fn sync_step_param_lists(rt: &mut Runtime, state: &Arc<SequencerState>, track: usize) {
    sync_current_track_step_param_lists(rt, state, track);
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
    rt.set_reactive(
        "SEQ",
        "track-retrigs",
        build_all_active_track_param_lists_value(state, StepParam::Retrig),
    );
    rt.set_reactive(
        "SEQ",
        "track-retrig-rates",
        build_all_active_track_param_lists_value(state, StepParam::RetrigRate),
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

#[cfg(test)]
pub(crate) fn build_accumulator_options(app: &app::App) -> Value {
    let items = build_accumulator_names(app)
        .into_iter()
        .map(|name| Rc::new(RefCell::new(Value::String(name))))
        .collect();
    Value::List(items)
}

#[cfg(test)]
pub(crate) fn build_accum_mode_options() -> Value {
    let items = ACCUM_MODE_LABELS
        .iter()
        .map(|label| Rc::new(RefCell::new(Value::String((*label).to_string()))))
        .collect();
    Value::List(items)
}

/// The scale dropdown's shown value: the scale (or imported scale) name,
/// with `*` once degrees are detuned or switched off in the scale editor.
pub(crate) fn fts_scale_label(tp: &sequencer::sequencer::TrackParams) -> String {
    fts_label(tp.get_fts_scale(), &tp.tuning())
}

/// [`fts_scale_label`] of scale `scale_idx` under `tuning`.
pub(crate) fn fts_label(scale_idx: usize, tuning: &sequencer::scale::TrackTuning) -> String {
    let name = sequencer::scale::scale_name(scale_idx, tuning);
    if scale_idx != sequencer::scale::SCALE_OFF && tuning.has_degree_edits() {
        format!("{name}*")
    } else {
        name.to_string()
    }
}

pub(crate) const TUNING_ROOT_NAMES: [&str; 12] =
    ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

/// The scale editor's key mappings, by `TuningMode` (`tuning.mode`,
/// `tuning-mode-options`).
pub(crate) const TUNING_MODE_LABELS: [&str; 2] = ["Snap", "Map"];

/// One degree of a scale as the scale editor shows it (`SEQ.tp-tuning-*`
/// lists, the host kinds' `degree`).
pub(crate) struct TuningDegree {
    /// The scale's own pitch, cents above the root.
    pub(crate) base: f32,
    pub(crate) offset: f32,
    pub(crate) enabled: bool,
    /// The sounding pitch (offset and morph applied), cents above the root.
    pub(crate) pitch: f32,
    pub(crate) label: String,
    /// The nearest simple just ratio, empty when none or the period is not
    /// an octave.
    pub(crate) ratio: String,
}

/// The period (cents) and degrees of scale `scale_idx` under `tuning`; no
/// degrees while the scale is Off.
pub(crate) fn tuning_degrees(
    scale_idx: usize,
    tuning: &sequencer::scale::TrackTuning,
) -> (f32, Vec<TuningDegree>) {
    use sequencer::scale;
    let (base, period) = scale::base_scale(scale_idx, tuning).unwrap_or((&[], 1200.0));
    let base = &base[..base.len().min(scale::MAX_SCALE_DEGREES)];
    let root_cents = f32::from(tuning.root) * 100.0;
    let degrees = (0..base.len())
        .map(|degree| {
            let pitch = scale::degree_pitch(base, degree, tuning);
            let ratio = scale::nearest_just_ratio(pitch, 3.0)
                .filter(|_| (period - 1200.0).abs() < 0.5)
                .map(|(num, den, _)| format!("{num}/{den}"))
                .unwrap_or_default();
            TuningDegree {
                base: base[degree],
                offset: tuning.offsets[degree],
                enabled: tuning.degree_enabled(degree),
                pitch,
                label: scale::pitch_label(root_cents + pitch),
                ratio,
            }
        })
        .collect();
    (period, degrees)
}

/// The scale editor's legacy `SEQ.tp-tuning-*` fields for one track (the
/// host-less test seeds). Degree lists are empty while the scale is Off.
#[cfg(test)]
pub(crate) fn tuning_reactive_fields(
    tp: &sequencer::sequencer::TrackParams,
) -> Vec<(&'static str, Value)> {
    let scale_idx = tp.get_fts_scale();
    let tuning = tp.tuning();
    let (period, degrees) = tuning_degrees(scale_idx, &tuning);
    let list = |item: fn(&TuningDegree) -> Value| list_value(degrees.iter().map(item));
    let cents = |cents: f32| Value::Number(f64::from(cents));
    vec![
        ("tp-tuning-on", Value::Bool(!degrees.is_empty())),
        (
            "tp-tuning-scale",
            Value::String(sequencer::scale::scale_name(scale_idx, &tuning).to_string()),
        ),
        ("tp-tuning-custom", Value::Bool(tuning.custom.is_some())),
        ("tp-tuning-edited", Value::Bool(tuning.has_degree_edits())),
        (
            "tp-tuning-root",
            Value::String(tuning_root_label(&tuning).to_string()),
        ),
        (
            "tp-tuning-morph",
            Value::Number((f64::from(tuning.morph) * 100.0).round()),
        ),
        (
            "tp-tuning-mode",
            Value::String(tuning.mode.label().to_string()),
        ),
        ("tp-tuning-period", cents(period)),
        (
            "tp-tuning-degree-count",
            Value::Number(degrees.len() as f64),
        ),
        (
            "tp-tuning-base",
            list(|degree| Value::Number(f64::from(degree.base))),
        ),
        (
            "tp-tuning-offsets",
            list(|degree| Value::Number(f64::from(degree.offset))),
        ),
        (
            "tp-tuning-enabled",
            list(|degree| Value::Bool(degree.enabled)),
        ),
        (
            "tp-tuning-pitches",
            list(|degree| Value::Number(f64::from(degree.pitch))),
        ),
        (
            "tp-tuning-labels",
            list(|degree| Value::String(degree.label.clone())),
        ),
        (
            "tp-tuning-ratios",
            list(|degree| Value::String(degree.ratio.clone())),
        ),
    ]
}

/// The root's name (`SEQ.tp-tuning-root`, `tuning.root`).
pub(crate) fn tuning_root_label(tuning: &sequencer::scale::TrackTuning) -> &'static str {
    TUNING_ROOT_NAMES[usize::from(tuning.root % 12)]
}

#[cfg(test)]
pub(crate) fn build_tuning_root_options() -> Value {
    Value::List(
        TUNING_ROOT_NAMES
            .iter()
            .map(|name| Rc::new(RefCell::new(Value::String((*name).to_string()))))
            .collect(),
    )
}

#[cfg(test)]
pub(crate) fn build_fts_options() -> Value {
    let items = fts_scale_names()
        .map(|scale| Rc::new(RefCell::new(Value::String(scale.to_string()))))
        .collect();
    Value::List(items)
}

#[cfg(test)]
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

/// The accumulator `tp` runs, by name, among `names` ([`build_accumulator_names`]).
pub(crate) fn selected_accumulator_name_in(
    tp: &sequencer::sequencer::TrackParams,
    names: &[String],
) -> String {
    accumulator_name(
        tp.get_accumulator_idx(),
        tp.script_accumulator_name(),
        names,
    )
}

/// The name of accumulator `idx` among `names`, or `script` (the script
/// accumulator's name) when the track runs one.
pub(crate) fn accumulator_name(idx: usize, script: Option<String>, names: &[String]) -> String {
    script
        .or_else(|| names.get(idx).cloned())
        .unwrap_or_else(|| "Off".to_string())
}

/// The voice-priority labels, by `VoicePriority` index (`SEQ.tp-voice-priority`,
/// `track.voice-priority`, `voice-priority-options`).
pub(crate) const VOICE_PRIORITY_LABELS: [&str; 3] = ["Last", "High", "Low"];

/// The mono-trigger labels, by `MonoTrigger` index (`SEQ.tp-mono-trigger`,
/// `track.mono-trigger`, `mono-trigger-options`).
pub(crate) const MONO_TRIGGER_LABELS: [&str; 2] = ["retrig", "legato"];

pub(crate) fn voice_priority_label(priority: sequencer::sequencer::VoicePriority) -> &'static str {
    VOICE_PRIORITY_LABELS[priority as usize]
}

pub(crate) fn mono_trigger_label(trigger: sequencer::sequencer::MonoTrigger) -> &'static str {
    MONO_TRIGGER_LABELS[trigger as usize]
}
