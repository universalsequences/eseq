use super::*;

pub(crate) fn push_panner_bool(
    lg_raw: *mut sequencer::audiograph::LiveGraph,
    node_id: u64,
    param_idx: u64,
    value: bool,
) {
    if node_id == 0 {
        return;
    }
    unsafe {
        sequencer::audiograph::params_push_wrapper(
            lg_raw,
            sequencer::audiograph::ParamMsg {
                idx: param_idx,
                logical_id: node_id,
                fvalue: if value { 1.0 } else { 0.0 },
            },
        );
    }
}

pub(crate) fn push_solo_mutes(
    lg_raw: *mut sequencer::audiograph::LiveGraph,
    app: &app::App,
    state: &Arc<SequencerState>,
) {
    let count = state.active_track_count();
    let audibility = app.solo_audibility();
    for track in 0..count {
        let muted_by_solo = audibility.track_is_muted(&state.pattern.track_params[track]);
        let fader_id = state.runtime.delay_lids[track].load(Ordering::Acquire);
        push_panner_bool(
            lg_raw,
            fader_id,
            sequencer::effects::stereo_panner::STEREO_PANNER_PARAM_MUTED_BY_SOLO,
            muted_by_solo,
        );
    }
}

pub(super) fn read_panner_peak_levels(lg: sequencer::audiograph::LiveGraphPtr, node_ids: &[i32]) -> Vec<f64> {
    node_ids
        .iter()
        .map(|&node_id| read_panner_peak_level(lg, node_id))
        .collect()
}

pub(super) fn read_panner_peak_level(lg: sequencer::audiograph::LiveGraphPtr, node_id: i32) -> f64 {
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
    track_nodes: &[app::TrackNodeIds],
) -> Vec<f64> {
    track_nodes
        .iter()
        .map(|nodes| read_panner_peak_level(lg, nodes.delay_id))
        .collect()
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
    const STATE_LEN: usize = sequencer::effects::peak_meter::PEAK_METER_STATE_SIZE;
    const STATE_BYTES: usize = STATE_LEN * std::mem::size_of::<f32>();
    bus_nodes
        .iter()
        .map(|bus| {
            if bus.meter_id < 0 {
                return 0.0;
            }
            let mut state_size = 0usize;
            let mut state = [0.0_f32; STATE_LEN];
            let copied = unsafe {
                sequencer::audiograph::get_node_state_into(
                    lg.0,
                    bus.meter_id,
                    state.as_mut_ptr().cast(),
                    STATE_BYTES,
                    &mut state_size as *mut usize,
                )
            };
            if !copied || state_size < STATE_BYTES {
                return 0.0;
            }
            let peak = state[sequencer::effects::peak_meter::STATE_PEAK_L]
                .max(state[sequencer::effects::peak_meter::STATE_PEAK_R]);
            meter_display_level(peak)
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

pub(super) fn read_modulator_display_value(
    lg: sequencer::audiograph::LiveGraphPtr,
    node_id: i32,
) -> (f64, f64) {
    const STATE_LEN: usize = sequencer::instruments::track_modulator::MODULATOR_ENVELOPE_STATE_SIZE;
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

pub(super) fn decode_modulator_display_state(state: &[f32]) -> (f64, f64) {
    let phase = state
        .get(sequencer::instruments::track_modulator::STATE_DISPLAY_PHASE)
        .copied()
        .unwrap_or(0.0);
    let level = state
        .get(sequencer::instruments::track_modulator::STATE_VALUE)
        .copied()
        .unwrap_or(0.0);
    (
        quantize_modulator_unit_value(phase),
        quantize_modulator_unit_value(level),
    )
}

pub(super) fn quantize_modulator_unit_value(value: f32) -> f64 {
    ((value.clamp(0.0, 1.0) * 128.0).round() / 128.0) as f64
}

// ── Filter Table effective response values (eseq-dtx.13) ──────────────────
//
// The Filter Table's `frame` / `cutoff` / `resonance` are `@mod true` DSP
// params: the engine resolves `(mod frame)` internally from the per-slot
// effect modulator node's audio-rate outputs. Nothing about that changes here.
// Instead the modulator node publishes its last output value per slot into its
// state tail (`voice_modulator::STATE_DISPLAY_SLOT_VALUE`), and this UI-tick
// poller reads that back over the audiograph watchlist and re-applies the same
// additive contract host-side, so the panel's spectrum curve can be drawn from
// the effective values without touching dgen.
//
// Track and bus chains are covered; rack slots keep the base-value curve
// (they have no modulation command target either).

/// Response values the Filter Table spectrum curve is drawn from, after host
/// modulation. Keyed by the effect's graph node id so track and bus instances
/// share one field namespace.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FilterTableResponse {
    pub node_id: i32,
    pub frame: f64,
    pub cutoff: f64,
    pub resonance: f64,
}

pub(crate) fn filter_table_response_field(node_id: i32, param: &str) -> String {
    format!("filter-table-{param}-{node_id}")
}

/// The three Filter Table controls the spectrum curve is drawn from, in the
/// order the `FilterTableResponse` fields are published.
const FILTER_TABLE_RESPONSE_PARAMS: [&str; 3] = [
    "frame",
    sequencer::effects::filter_table::PARAM_CUTOFF,
    "resonance",
];

/// Quantize a response value so audio-rate jitter below a pixel of curve travel
/// does not dirty the widget every tick. Callers pass an already range-clamped
/// value (see `clamp_to`); the 0..1 grid here is the *quantization* grid, not a
/// second range clamp — `frame` and `resonance` are both 0..1 controls.
fn quantize_unit_response(value: f32) -> f64 {
    ((value * 1024.0).round() / 1024.0) as f64
}

/// Cutoff is displayed on a log frequency axis; quantizing in cents keeps the
/// churn threshold perceptually even across 40..18000 Hz.
fn quantize_cutoff_response(value: f32) -> f64 {
    let hz = value.max(1.0);
    let cents = (hz.log2() * 1200.0).round();
    (2.0_f64).powf(cents as f64 / 1200.0)
}

fn read_effect_modulator_slot_values(
    lg: sequencer::audiograph::LiveGraphPtr,
    modulator_node_id: i32,
) -> [f32; sequencer::instruments::voice_modulator::SLOT_COUNT] {
    use sequencer::instruments::voice_modulator as vm;
    const STATE_LEN: usize = vm::STATE_SIZE;
    const STATE_BYTES: usize = STATE_LEN * std::mem::size_of::<f32>();
    let mut slots = [0.0_f32; vm::SLOT_COUNT];
    if modulator_node_id <= 0 {
        return slots;
    }
    let mut state_size = 0usize;
    let mut state = [0.0_f32; STATE_LEN];
    let copied = unsafe {
        sequencer::audiograph::get_node_state_into(
            lg.0,
            modulator_node_id,
            state.as_mut_ptr().cast(),
            STATE_BYTES,
            &mut state_size as *mut usize,
        )
    };
    if !copied || state_size < STATE_BYTES {
        return slots;
    }
    for (slot, value) in slots.iter_mut().enumerate() {
        *value = state[vm::STATE_DISPLAY_SLOT_VALUE + slot];
    }
    slots
}

/// Depth lanes assigned to `base_param_idx`, indexed by modulator slot, plus
/// whether the destination's `__dgen_mod_active__` flag is on. `(0, false)`
/// when the destination is unmodulated — the caller then skips the engine read
/// entirely and the panel renders exactly as it did before this feature.
fn filter_table_mod_lanes(
    desc: &sequencer::effects::EffectDescriptor,
    base_param_idx: usize,
    value_of: &dyn Fn(usize) -> f32,
) -> Option<[f32; sequencer::instruments::voice_modulator::SLOT_COUNT]> {
    use sequencer::instruments::voice_modulator::SLOT_COUNT;
    let mut depths = [0.0_f32; SLOT_COUNT];
    let mut any = false;
    for target in &desc.instrument_modulation_targets {
        if target.base_param_idx != base_param_idx {
            continue;
        }
        if let Some(active_idx) = target.active_param_idx {
            if value_of(active_idx) <= 0.5 {
                continue;
            }
        }
        let slot = target.modulator_slot;
        if slot == 0 || slot > SLOT_COUNT {
            continue;
        }
        let depth = value_of(target.depth_param_idx);
        if depth == 0.0 || !depth.is_finite() {
            continue;
        }
        depths[slot - 1] += depth;
        any = true;
    }
    any.then_some(depths)
}

/// Pure half of the sampler: apply the additive modulation contract to the
/// three response params for one already-sampled set of modulator slot values.
/// All-zero `slot_values` reproduces the base spectrum exactly, which is what
/// an unmodulated (or hidden) panel publishes.
pub(crate) fn filter_table_response_from_slot_values(
    desc: &sequencer::effects::EffectDescriptor,
    node_id: i32,
    value_of: &dyn Fn(usize) -> f32,
    slot_values: &[f32; sequencer::instruments::voice_modulator::SLOT_COUNT],
) -> FilterTableResponse {
    use sequencer::instruments::voice_modulator as vm;
    let param_idx = |name: &str| desc.params.iter().position(|p| p.name == name);
    let resolved = |name: &str, fallback: f32| -> (Option<usize>, f32) {
        match param_idx(name) {
            Some(idx) => (Some(idx), value_of(idx)),
            None => (None, fallback),
        }
    };
    let [frame_name, cutoff_name, resonance_name] = FILTER_TABLE_RESPONSE_PARAMS;
    let (frame_idx, mut frame) = resolved(frame_name, 0.0);
    let (cutoff_idx, mut cutoff) = resolved(cutoff_name, 1_000.0);
    let (resonance_idx, mut resonance) = resolved(resonance_name, 0.0);

    let apply = |idx: Option<usize>, value: &mut f32| -> bool {
        match idx.and_then(|idx| filter_table_mod_lanes(desc, idx, value_of)) {
            Some(depths) => {
                *value = vm::additive_modulated_value(*value, true, &depths, slot_values);
                true
            }
            None => false,
        }
    };
    let frame_modulated = apply(frame_idx, &mut frame);
    let cutoff_modulated = apply(cutoff_idx, &mut cutoff);
    let resonance_modulated = apply(resonance_idx, &mut resonance);

    // The DSP clips each control to its declared range before using it
    // (`filter_table_dsp.lisp`); the curve has to see the same bounds.
    let clamp_to = |idx: Option<usize>, value: f32, min: f32, max: f32| {
        let (min, max) = idx
            .and_then(|idx| desc.params.get(idx))
            .map(|p| (p.min, p.max))
            .unwrap_or((min, max));
        value.clamp(min, max)
    };
    // Quantization only damps *modulated* churn. An unmodulated destination
    // publishes its base value bit-for-bit, so the curve is identical to what
    // the knob's own value field drew before this feature existed.
    FilterTableResponse {
        node_id,
        frame: {
            let value = clamp_to(frame_idx, frame, 0.0, 1.0);
            if frame_modulated {
                quantize_unit_response(value)
            } else {
                value as f64
            }
        },
        cutoff: {
            let value = clamp_to(cutoff_idx, cutoff, 40.0, 18_000.0);
            if cutoff_modulated {
                quantize_cutoff_response(value)
            } else {
                value as f64
            }
        },
        resonance: {
            let value = clamp_to(resonance_idx, resonance, 0.0, 1.0);
            if resonance_modulated {
                quantize_unit_response(value)
            } else {
                value as f64
            }
        },
    }
}

fn filter_table_response_for_slot(
    lg: sequencer::audiograph::LiveGraphPtr,
    desc: &sequencer::effects::EffectDescriptor,
    node_id: i32,
    modulator_node_id: i32,
    value_of: &dyn Fn(usize) -> f32,
    live: bool,
    wanted: &mut HashSet<i32>,
) -> FilterTableResponse {
    use sequencer::instruments::voice_modulator::SLOT_COUNT;
    // Requirement (a): an unmodulated Filter Table costs nothing new — no
    // watchlist entry, no engine state read, and all-zero slot values make the
    // pure half return exactly the base values the panel drew before. The
    // lane lookup is shared with `filter_table_response_from_slot_values` so
    // the gate and the arithmetic can never disagree about what is modulated.
    let modulated = live
        && modulator_node_id > 0
        && FILTER_TABLE_RESPONSE_PARAMS.iter().any(|name| {
            desc.params
                .iter()
                .position(|p| p.name == *name)
                .and_then(|idx| filter_table_mod_lanes(desc, idx, value_of))
                .is_some()
        });
    let slot_values = if modulated {
        wanted.insert(modulator_node_id);
        read_effect_modulator_slot_values(lg, modulator_node_id)
    } else {
        [0.0_f32; SLOT_COUNT]
    };
    filter_table_response_from_slot_values(desc, node_id, value_of, &slot_values)
}

/// Sample every live Filter Table instance's effective response values.
///
/// `live` is the panel-visibility gate: while the FX panel is hidden the
/// modulator nodes are dropped from the watchlist and base values are reported,
/// which is also what makes the display settle back to the base spectrum when
/// modulation is switched off.
///
/// `selected_step` is the raw step selection, not a resolved p-lock step: the
/// base value has to match what the knob's own value field shows, which means
/// `displayed_plock_step` semantics (selection, else the track's live playhead
/// step while playing) and macro-engine overrides. See
/// `sync_track_effect_param_value_field`, which this deliberately mirrors —
/// otherwise a macro driving cutoff, or a p-lock passing under the playhead,
/// moves the knob but not the curve.
pub(crate) fn read_filter_table_responses(
    lg: sequencer::audiograph::LiveGraphPtr,
    app: &app::App,
    state: &Arc<SequencerState>,
    selected_step: Option<usize>,
    live: bool,
    watched: &mut HashSet<i32>,
) -> Vec<FilterTableResponse> {
    let mut responses = Vec::new();
    let mut wanted: HashSet<i32> = HashSet::new();

    for (track, descs) in app.graph.effect_descriptors.iter().enumerate() {
        let Some(chain) = state.pattern.effect_chains.get(track) else {
            continue;
        };
        if track >= state.pattern.track_params.len()
            || descs
                .iter()
                .all(|desc| desc.name != sequencer::effects::filter_table::NAME)
        {
            continue;
        }
        let display_step = displayed_plock_step(state, track, selected_step);
        for (slot_idx, desc) in descs.iter().enumerate() {
            if desc.name != sequencer::effects::filter_table::NAME {
                continue;
            }
            let Some(slot) = chain.get(slot_idx) else {
                continue;
            };
            let node_id = slot.node_id.load(Ordering::Relaxed) as i32;
            if node_id <= 0 {
                continue;
            }
            let value_of = |idx: usize| -> f32 {
                let Some(pdesc) = desc.params.get(idx) else {
                    return 0.0;
                };
                display_step
                    .and_then(|step| slot.plocks.get(step, idx))
                    .or_else(|| app.effective_slot_param_value(track, slot_idx, idx))
                    .unwrap_or_else(|| slot_param_stored_value(slot, pdesc, idx, display_step))
            };
            responses.push(filter_table_response_for_slot(
                lg,
                desc,
                node_id,
                slot.modulator_node_id.load(Ordering::Relaxed) as i32,
                &value_of,
                live,
                &mut wanted,
            ));
        }
    }

    for bus in &app.buses {
        for (slot_idx, desc) in bus.effect_descriptors.iter().enumerate() {
            if desc.name != sequencer::effects::filter_table::NAME {
                continue;
            }
            let Some(slot) = bus.effect_slots.get(slot_idx) else {
                continue;
            };
            let node_id = slot.node_id as i32;
            if node_id <= 0 {
                continue;
            }
            // Bus knobs read defaults only — no p-locks, no macro engine (see
            // `sync_bus_effect_param_value_field`). Mirror that exactly so the
            // curve and the knob never disagree.
            let value_of = |idx: usize| -> f32 {
                slot.defaults
                    .get(idx)
                    .copied()
                    .unwrap_or_else(|| desc.params.get(idx).map(|p| p.default).unwrap_or(0.0))
            };
            responses.push(filter_table_response_for_slot(
                lg,
                desc,
                node_id,
                slot.modulator_node_id as i32,
                &value_of,
                live,
                &mut wanted,
            ));
        }
    }

    // Watchlist bookkeeping: only modulated, visible Filter Tables cost the
    // audio thread a per-block state snapshot.
    watched.retain(|node_id| {
        if wanted.contains(node_id) {
            return true;
        }
        unsafe {
            sequencer::audiograph::remove_node_from_watchlist(lg.0, *node_id);
        }
        false
    });
    for node_id in wanted {
        if watched.contains(&node_id) {
            continue;
        }
        if unsafe { sequencer::audiograph::add_node_to_watchlist(lg.0, node_id) } {
            watched.insert(node_id);
        }
    }

    responses
}

/// Publish the changed response values. Returns `(effects_dirty, published)`:
/// `published` is zero whenever nothing moved, which is the check that an idle
/// (or unmodulated) panel dirties no widget.
pub(crate) fn sync_filter_table_response_field_delta(
    rt: &mut Runtime,
    previous: &[FilterTableResponse],
    current: &[FilterTableResponse],
) -> (bool, usize) {
    let mut effects_dirty = false;
    let mut published = 0usize;
    for response in current {
        let prev = previous
            .iter()
            .find(|candidate| candidate.node_id == response.node_id);
        let mut publish = |param: &str, value: f64, was: Option<f64>| {
            if was == Some(value) {
                return;
            }
            published += 1;
            effects_dirty |= rt
                .set_reactive(
                    "SEQ",
                    &filter_table_response_field(response.node_id, param),
                    Value::Number(value),
                )
                .effects_dirty;
        };
        publish("frame", response.frame, prev.map(|p| p.frame));
        publish("cutoff", response.cutoff, prev.map(|p| p.cutoff));
        publish("resonance", response.resonance, prev.map(|p| p.resonance));
    }
    (effects_dirty, published)
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct VisualizationLiveness {
    neural: bool,
    graph: bool,
    track_output: bool,
}

pub(crate) fn sync_neural_visualization_fields(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    previous_liveness: &mut VisualizationLiveness,
) -> bool {
    let liveness = VisualizationLiveness {
        neural: state.has_neural_visualization(),
        graph: state.has_graph_visualizations(),
        track_output: state.has_track_output_events(),
    };
    let mut effects_dirty = false;

    if liveness.neural || previous_liveness.neural {
        effects_dirty |= rt
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
    }
    if liveness.graph || previous_liveness.graph {
        effects_dirty |= rt
            .set_reactive(
                "SEQ",
                "graph-visualizations",
                build_graph_visualizations_value(state),
            )
            .effects_dirty;
    }
    if liveness.track_output || previous_liveness.track_output {
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
    }
    *previous_liveness = liveness;
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
