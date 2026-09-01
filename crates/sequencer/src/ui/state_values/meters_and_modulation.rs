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

// ── Effective (post-modulation) parameter values (eseq-dtx.13, eseq-hpc) ───
//
// Effect params marked `@mod true` are resolved inside the engine: the DSP
// reads `(mod cutoff)` from the per-slot effect modulator node's audio-rate
// outputs. Nothing about that changes here. Instead the modulator node
// publishes its last output value per slot into its state tail
// (`voice_modulator::STATE_DISPLAY_SLOT_VALUE`), and this UI-tick poller reads
// that back over the audiograph watchlist and re-applies the same additive
// contract host-side, so panels can *display* the effective value — the knob's
// live dot and curve visualizers like the Filter Table spectrum — without
// touching dgen.
//
// eseq-dtx.13 shipped this for the Filter Table's three response params;
// eseq-hpc generalizes it to every declared modulation destination of every
// effect, published as a sparse per-node (param idx -> value) snapshot. The
// display is read-only telemetry: nothing here ever writes back into widget or
// interactive state, so dragging a modulated knob still edits the base value.
//
// Track and bus chains are covered; rack slots keep base values (they have no
// modulation command target either).

/// One modulation destination's live display values.
///
/// Two numbers, because the two consumers race differently against this
/// meter-rate sampler:
///
/// * `offset` is what a *knob* draws its dot from. The base moves the instant
///   a knob is dragged, so an absolute value would trail the drag by up to a
///   tick and flash a dot beside a knob nothing is modulating. A displacement
///   rides along with whatever base the widget is already showing, and is
///   exactly `0.0` when nothing modulates the param.
/// * `value` is the absolute effective value, for *curve visualizers*. They
///   bind one reactive field straight into a widget prop rather than computing
///   from two (a computed prop would not be re-evaluated when only the bound
///   value field changes), so they need the sum published for them.
/// * `scale` is the multiplicative form of the same displacement, for
///   destinations whose `ModulationMode` is exponential. `base + offset` only
///   composes with a *moving* base for `Additive` lanes: an `Octaves` lane
///   sampled at 1 kHz with +2 octaves publishes an offset of 3 kHz, and a knob
///   dragged to 8 kHz before the next tick would draw its dot at 11 kHz
///   instead of 32 kHz. `scale` is `2^octaves` for those, exactly `1.0`
///   otherwise, and the widget prefers `base * scale` whenever it is not 1.0.
///   One factor is always enough: `effect_mod_lanes` collapses every lane of
///   one destination into a single mode, because the DSP reads a single
///   `(mod <dest>)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ParamModValue {
    pub param_idx: usize,
    pub offset: f64,
    pub value: f64,
    pub scale: f64,
}

/// One effect instance's live modulation display values, keyed by the effect's
/// graph node id so track and bus instances share one field namespace.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct EffectModValues {
    pub node_id: i32,
    /// One entry per modulation destination the descriptor declares, ascending
    /// by param index.
    pub values: Vec<ParamModValue>,
}

/// Reactive field carrying one param's modulation *offset* — how far the live
/// effective value sits from the param's base, in the base's own units. Sparse
/// by construction: only declared modulation destinations of live effects ever
/// get a field.
pub(crate) fn effect_mod_offset_field(node_id: i32, param_idx: usize) -> String {
    format!("fx-mod-offset-{node_id}-{param_idx}")
}

/// Reactive field carrying one param's absolute effective value, for curve
/// visualizers. Same sparsity as the offset field.
pub(crate) fn effect_mod_value_field(node_id: i32, param_idx: usize) -> String {
    format!("fx-mod-value-{node_id}-{param_idx}")
}

/// Reactive field carrying one param's *multiplicative* displacement, for
/// exponential destinations. Exactly `1.0` for additive ones, so a knob that
/// reads it always has a usable factor.
pub(crate) fn effect_mod_scale_field(node_id: i32, param_idx: usize) -> String {
    format!("fx-mod-scale-{node_id}-{param_idx}")
}

/// Quantize an effective value so audio-rate jitter below a pixel of travel
/// does not dirty the widget every tick. Quantizing in the param's *normalized*
/// space (which is log-spaced for `ParamScaling::Exponential` params like
/// cutoff frequencies) keeps the churn threshold perceptually even across the
/// whole range instead of stair-stepping the bottom of a decade.
fn quantize_effective_value(pdesc: &sequencer::effects::ParamDescriptor, value: f32) -> f64 {
    const STEPS: f32 = 2048.0;
    let clamped = value.clamp(pdesc.min, pdesc.max);
    if !matches!(pdesc.kind, sequencer::effects::ParamKind::Continuous { .. }) {
        return clamped as f64;
    }
    let normalized = (pdesc.normalize(clamped) * STEPS).round() / STEPS;
    pdesc.denormalize(normalized) as f64
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
/// how the destination combines them with its base. `None` when the
/// destination is unmodulated — the caller then skips the engine read entirely
/// and the panel renders exactly as it did before this feature.
fn effect_mod_lanes(
    desc: &sequencer::effects::EffectDescriptor,
    base_param_idx: usize,
    value_of: &dyn Fn(usize) -> f32,
) -> Option<(
    sequencer::effects::ModulationMode,
    [f32; sequencer::instruments::voice_modulator::SLOT_COUNT],
)> {
    use sequencer::instruments::voice_modulator::SLOT_COUNT;
    let mut depths = [0.0_f32; SLOT_COUNT];
    let mut mode = sequencer::effects::ModulationMode::default();
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
        // Two lane shapes reach here. dgen effects and custom instruments bake
        // the slot into the descriptor (`modulator_slot`, with a
        // `__dgen_mod_active__` flag); the sampler's lanes instead carry a
        // `mod <dest> src` param whose *value* picks the slot, with 0 = off.
        let slot = match target.source_param_idx {
            Some(source_idx) => value_of(source_idx).round().max(0.0) as usize,
            None => target.modulator_slot,
        };
        if slot == 0 || slot > SLOT_COUNT {
            continue;
        }
        let depth = value_of(target.depth_param_idx);
        if depth == 0.0 || !depth.is_finite() {
            continue;
        }
        depths[slot - 1] += depth;
        // Every lane of one destination shares its combining mode; the DSP
        // reads a single `(mod <dest>)`.
        mode = target.mod_mode;
        any = true;
    }
    any.then_some((mode, depths))
}

/// Apply one destination's modulation the way its DSP does.
///
/// The slot values are the modulator nodes' unipolar outputs; depth carries the
/// sign and the scale. `Additive` sums in the destination's own units,
/// `Octaves` scales exponentially — the built-in Filter multiplies its cutoff
/// by `2^octaves`, so treating its ±4 octave depth as ±4 Hz on a 20..20000 Hz
/// control would move the display by nothing.
fn modulated_value(
    mode: sequencer::effects::ModulationMode,
    base: f32,
    depths: &[f32; sequencer::instruments::voice_modulator::SLOT_COUNT],
    slot_values: &[f32; sequencer::instruments::voice_modulator::SLOT_COUNT],
) -> f32 {
    use sequencer::instruments::voice_modulator::SLOT_COUNT;
    let amount: f32 = (0..SLOT_COUNT)
        .map(|slot| depths[slot] * slot_values[slot].clamp(0.0, 1.0))
        .sum();
    mode.apply(base, amount)
}

/// The param indices this descriptor declares as modulation destinations,
/// ascending and deduplicated. Empty for effects with no mod matrix, which is
/// what keeps the whole feature free for them.
pub(crate) fn effect_mod_destinations(desc: &sequencer::effects::EffectDescriptor) -> Vec<usize> {
    let mut destinations: Vec<usize> = desc
        .instrument_modulation_targets
        .iter()
        .map(|target| target.base_param_idx)
        .filter(|idx| *idx < desc.params.len())
        .collect();
    destinations.sort_unstable();
    destinations.dedup();
    destinations
}

/// Every declared destination paired with the lanes assigned to it, resolved in
/// one pass. Both consumers read this: the watchlist gate (is *anything*
/// modulated?) and the value arithmetic. Walking the mod matrix once for both
/// is what keeps them from disagreeing — and the matrix is walked once per
/// destination, not twice.
type DestinationLanes = Vec<(
    usize,
    Option<(
        sequencer::effects::ModulationMode,
        [f32; sequencer::instruments::voice_modulator::SLOT_COUNT],
    )>,
)>;

fn destination_lanes(
    desc: &sequencer::effects::EffectDescriptor,
    value_of: &dyn Fn(usize) -> f32,
) -> DestinationLanes {
    effect_mod_destinations(desc)
        .into_iter()
        .map(|param_idx| (param_idx, effect_mod_lanes(desc, param_idx, value_of)))
        .collect()
}

/// Pure half of the sampler: apply the modulation contract to every declared
/// destination for one already-sampled set of modulator slot values, reporting
/// each as an offset from that destination's base. All-zero `slot_values`
/// yields all-zero offsets, which is what an unmodulated (or hidden) panel
/// publishes.
pub(crate) fn effect_mod_values_from_slot_values(
    desc: &sequencer::effects::EffectDescriptor,
    node_id: i32,
    value_of: &dyn Fn(usize) -> f32,
    slot_values: &[f32; sequencer::instruments::voice_modulator::SLOT_COUNT],
) -> EffectModValues {
    effect_mod_values_for_lanes(
        desc,
        node_id,
        value_of,
        slot_values,
        &destination_lanes(desc, value_of),
    )
}

/// As above, with the destinations and their lanes already resolved — the
/// poller resolves them once for the watchlist gate and reuses them here.
fn effect_mod_values_for_lanes(
    desc: &sequencer::effects::EffectDescriptor,
    node_id: i32,
    value_of: &dyn Fn(usize) -> f32,
    slot_values: &[f32; sequencer::instruments::voice_modulator::SLOT_COUNT],
    lanes: &DestinationLanes,
) -> EffectModValues {
    let mut values = Vec::with_capacity(lanes.len());
    for &(param_idx, lanes) in lanes {
        let Some(pdesc) = desc.params.get(param_idx) else {
            continue;
        };
        let base = value_of(param_idx);
        let (value, mode) = match lanes {
            Some((mode, depths)) => {
                let modulated = modulated_value(mode, base, &depths, slot_values);
                let value = if modulated == base {
                    // Modulators resting at zero settle back to the base value
                    // exactly, not to base-rounded-to-the-quantization-grid:
                    // an assigned-but-idle lane must leave a knob's dot hidden
                    // and a curve identical to the unmodulated one.
                    base as f64
                } else {
                    // Quantization damps audio-rate jitter below a pixel of
                    // travel; the DSP clips each control to its declared range
                    // before using it, so the display sees the same bounds.
                    quantize_effective_value(pdesc, modulated)
                };
                (value, Some(mode))
            }
            None => (base as f64, None),
        };
        // The offset is derived from the same sample, so an idle destination
        // is exactly 0.0 rather than a rounding residue. The exponential form
        // is derived from the *published* value rather than recomputed from
        // `2^amount`, so a knob drawing `base * scale` lands on exactly the
        // same number a curve visualizer binds.
        let scale = match mode {
            Some(sequencer::effects::ModulationMode::Octaves) if base != 0.0 => value / base as f64,
            _ => 1.0,
        };
        values.push(ParamModValue {
            param_idx,
            offset: value - base as f64,
            value,
            scale,
        });
    }
    EffectModValues { node_id, values }
}

fn effect_mod_values_for_slot(
    lg: sequencer::audiograph::LiveGraphPtr,
    desc: &sequencer::effects::EffectDescriptor,
    node_id: i32,
    modulator_node_id: i32,
    value_of: &dyn Fn(usize) -> f32,
    live: bool,
    wanted: &mut HashSet<i32>,
) -> EffectModValues {
    use sequencer::instruments::voice_modulator::SLOT_COUNT;
    // An unmodulated effect costs nothing new — no watchlist entry, no engine
    // state read, and all-zero slot values make the pure half return exactly
    // the base values. The lanes are resolved once and shared with the
    // arithmetic below, so the gate and the values can never disagree about
    // what is modulated.
    let lanes = destination_lanes(desc, value_of);
    let modulated = live && modulator_node_id > 0 && lanes.iter().any(|(_, lanes)| lanes.is_some());
    let slot_values = if modulated {
        wanted.insert(modulator_node_id);
        read_effect_modulator_slot_values(lg, modulator_node_id)
    } else {
        [0.0_f32; SLOT_COUNT]
    };
    effect_mod_values_for_lanes(desc, node_id, value_of, &slot_values, &lanes)
}

/// Sample every live effect instance's effective parameter values.
///
/// `live` is the panel-visibility gate: while the FX panel is hidden the
/// modulator nodes are dropped from the watchlist and base values are reported,
/// which is also what makes the display settle back to base when modulation is
/// switched off.
///
/// `selected_step` is the raw step selection, not a resolved p-lock step: the
/// base value has to match what the knob's own value field shows, which means
/// `displayed_plock_step` semantics (selection, else the track's live playhead
/// step while playing) and macro-engine overrides. See
/// `sync_track_effect_param_value_field`, which this deliberately mirrors —
/// otherwise a macro driving cutoff, or a p-lock passing under the playhead,
/// moves the knob but not the overlay.
pub(crate) fn read_mod_display_values(
    lg: sequencer::audiograph::LiveGraphPtr,
    app: &app::App,
    state: &Arc<SequencerState>,
    selected_track: Option<usize>,
    selected_step: Option<usize>,
    live: bool,
    watched: &mut HashSet<i32>,
) -> ModDisplayValues {
    let mut responses = Vec::new();
    let mut wanted: HashSet<i32> = HashSet::new();

    for (track, descs) in app.graph.effect_descriptors.iter().enumerate() {
        let Some(chain) = state.pattern.effect_chains.get(track) else {
            continue;
        };
        if track >= state.pattern.track_params.len() {
            continue;
        }
        let display_step = displayed_plock_step(state, track, selected_step);
        for (slot_idx, desc) in descs.iter().enumerate() {
            if desc.instrument_modulation_targets.is_empty() {
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
                    .and_then(|step| {
                        held_plock_value(state, track, step, slot.plocks.has_any_plock(), |s| {
                            slot.plocks.get(s, idx)
                        })
                    })
                    .or_else(|| app.effective_slot_param_value(track, slot_idx, idx))
                    .unwrap_or_else(|| slot_param_stored_value(slot, pdesc, idx, display_step))
            };
            responses.push(effect_mod_values_for_slot(
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
            if desc.instrument_modulation_targets.is_empty() {
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
            // overlay and the knob never disagree.
            let value_of = |idx: usize| -> f32 {
                slot.defaults
                    .get(idx)
                    .copied()
                    .unwrap_or_else(|| desc.params.get(idx).map(|p| p.default).unwrap_or(0.0))
            };
            responses.push(effect_mod_values_for_slot(
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

    // The FX-tile instrument panel shows one instrument, so only the selected
    // track is sampled — its fields are not track-keyed.
    let instrument = selected_track.and_then(|track| {
        read_instrument_mod_values_for_track(
            lg,
            app,
            state,
            track,
            selected_step,
            live,
            &mut wanted,
        )
    });
    // A rack track's panel shows one slot's instrument, so sample that slot.
    let rack_slot = selected_track.and_then(|track| {
        read_rack_slot_mod_values(lg, app, state, track, selected_step, live, &mut wanted)
    });

    // Watchlist bookkeeping: only modulated, visible effects and instruments
    // cost the audio thread a per-block state snapshot.
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

    ModDisplayValues {
        effects: responses,
        instrument,
        rack_slot,
    }
}

/// One instrument's live modulation offsets (eseq-6mva), in the *display*
/// domain the instrument panel's own value fields use (`stored_to_user`), so a
/// knob's dot and its pointer share one scale. Only the selected track is ever
/// sampled: the FX-tile instrument panel's fields are not track-keyed.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct InstrumentModValues {
    pub track: usize,
    pub values: Vec<ParamModValue>,
}

/// Reactive field carrying one instrument param's modulation offset. Matches
/// the `fx-instrument-param-*` naming: the FX-tile panel shows one instrument
/// at a time, so these are relative to the selected track.
pub(crate) fn fx_instrument_mod_offset_field(param_idx: usize) -> String {
    format!("fx-instrument-mod-offset-{param_idx}")
}

/// Reactive field carrying one instrument param's absolute effective value,
/// for curve visualizers.
pub(crate) fn fx_instrument_mod_value_field(param_idx: usize) -> String {
    format!("fx-instrument-mod-value-{param_idx}")
}

/// Multiplicative displacement for exponential destinations; see
/// `effect_mod_scale_field`.
pub(crate) fn fx_instrument_mod_scale_field(param_idx: usize) -> String {
    format!("fx-instrument-mod-scale-{param_idx}")
}

/// Track-keyed variants, for the sampler panel. It is a separate builder whose
/// param maps address their values as `instrument-param-{track}-*` rather than
/// the FX-tile panel's un-tracked `fx-instrument-param-*`, and it is built for
/// tracks other than the selected one, so it needs fields that name their
/// track. Both namespaces carry the same sample.
pub(crate) fn instrument_mod_offset_field(track: usize, param_idx: usize) -> String {
    format!("inst-mod-offset-{track}-{param_idx}")
}

pub(crate) fn instrument_mod_value_field(track: usize, param_idx: usize) -> String {
    format!("inst-mod-value-{track}-{param_idx}")
}

pub(crate) fn instrument_mod_scale_field(track: usize, param_idx: usize) -> String {
    format!("inst-mod-scale-{track}-{param_idx}")
}

/// One rack slot's live modulation display values (eseq-hpc). A rack track has
/// no instrument of its own — each slot does — and the panel shows one slot at
/// a time, so this names the slot it was sampled for.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RackSlotModValues {
    pub track: usize,
    pub slot_idx: usize,
    pub values: Vec<ParamModValue>,
}

/// Reactive fields for a rack slot's params, keyed by track *and* slot the way
/// the rack panel's own value fields are.
pub(crate) fn rack_slot_mod_offset_field(track: usize, slot_idx: usize, param_idx: usize) -> String {
    format!("rack-mod-offset-{track}-{slot_idx}-{param_idx}")
}

pub(crate) fn rack_slot_mod_value_field(track: usize, slot_idx: usize, param_idx: usize) -> String {
    format!("rack-mod-value-{track}-{slot_idx}-{param_idx}")
}

pub(crate) fn rack_slot_mod_scale_field(track: usize, slot_idx: usize, param_idx: usize) -> String {
    format!("rack-mod-scale-{track}-{slot_idx}-{param_idx}")
}

/// Sample the selected track's selected rack slot, if it declares any
/// modulation destinations.
///
/// Mirrors the instrument half: the audio thread publishes the slot's most
/// recently allocated voice's modulator, and the base values come from the
/// rack panel's own value chain (`rack_slot_param_value`, which resolves
/// p-locks and rack macro mappings) so the dot and the knob pointer agree.
fn read_rack_slot_mod_values(
    lg: sequencer::audiograph::LiveGraphPtr,
    app: &app::App,
    state: &Arc<SequencerState>,
    track: usize,
    selected_step: Option<usize>,
    live: bool,
    wanted: &mut HashSet<i32>,
) -> Option<RackSlotModValues> {
    use sequencer::instruments::voice_modulator::SLOT_COUNT;
    // Borrow the snapshot rather than cloning it: a rack track owns every
    // slot's p-lock grid and effect descriptors (~100 String-bearing params
    // each), which is far too much to copy on a 50 ms meter poll. Nothing under
    // this guard takes the same lock — `selected_rack_slot_index_for_rack` and
    // `rack_slot_cached_instrument_descriptor` read `app`, and the modulator
    // sample goes to the live graph — so holding it for the read is safe.
    let racks = state.pattern.rack_tracks.lock().unwrap();
    let rack = racks.get(track)?.as_ref()?;
    let slot_idx = app.selected_rack_slot_index_for_rack(track, rack)?;
    let slot = rack.slots.get(slot_idx)?;
    // Custom engines hand out a borrow; the built-in sampler's descriptor is
    // constructed on demand, so cache one and borrow that instead of rebuilding
    // ~100 param descriptors on every meter tick. It is derived from constants,
    // so one copy is as good as another.
    static SAMPLER_DESCRIPTOR: std::sync::OnceLock<sequencer::effects::EffectDescriptor> =
        std::sync::OnceLock::new();
    let desc = match slot.instrument_type {
        sequencer::sequencer::InstrumentType::Sampler => Some(
            SAMPLER_DESCRIPTOR
                .get_or_init(sequencer::effects::EffectDescriptor::builtin_sampler),
        ),
        _ => app.rack_slot_cached_instrument_descriptor(slot),
    }?;
    if desc.instrument_modulation_targets.is_empty() {
        return None;
    }
    let value_of = |idx: usize| -> f32 {
        super::rack_panel::rack_slot_param_value(rack, slot_idx, slot, desc, idx, selected_step)
    };
    let lanes = destination_lanes(desc, &value_of);
    if lanes.is_empty() {
        return None;
    }
    // A track or slot that has gone out of range simply has no modulator to
    // read; it must never index-panic the UI thread.
    let modulator_node_id = state
        .transport
        .rack_slot_display_modulator_node_ids
        .get(track)
        .and_then(|slots| slots.get(slot_idx))
        .map(|node_id| node_id.load(Ordering::Relaxed) as i32)
        .unwrap_or(0);
    let modulated = live && modulator_node_id > 0 && lanes.iter().any(|(_, lanes)| lanes.is_some());
    let slot_values = if modulated {
        wanted.insert(modulator_node_id);
        read_effect_modulator_slot_values(lg, modulator_node_id)
    } else {
        [0.0_f32; SLOT_COUNT]
    };
    // Rack slot panels display user-domain values like the instrument panel.
    let stored = effect_mod_values_for_lanes(desc, 0, &value_of, &slot_values, &lanes);
    let values = stored
        .values
        .into_iter()
        .filter_map(|sampled| {
            let pdesc = desc.params.get(sampled.param_idx)?;
            Some(ParamModValue {
                param_idx: sampled.param_idx,
                offset: pdesc.stored_to_user(sampled.offset as f32) as f64,
                value: pdesc.stored_to_user(sampled.value as f32) as f64,
                // `stored_to_user` is linear through the origin, so a ratio of
                // two scaled values is the ratio of the stored ones.
                scale: sampled.scale,
            })
        })
        .collect();
    Some(RackSlotModValues {
        track,
        slot_idx,
        values,
    })
}

/// Publish the changed rack slot values. A slot (or track) change republishes
/// every field, so the panel never shows the previous slot's modulation.
pub(crate) fn sync_rack_slot_mod_offset_field_delta(
    rt: &mut Runtime,
    previous: Option<&RackSlotModValues>,
    current: Option<&RackSlotModValues>,
) -> (bool, usize) {
    let mut effects_dirty = false;
    let mut published = 0usize;
    let Some(current) = current else {
        // Nothing to sample any more (slot deselected, rack removed, panel
        // hidden). The fields the last sample wrote would otherwise keep their
        // final values forever, freezing a dot beside an unmodulated knob, so
        // settle them. Only fires on the transition.
        if let Some(previous) = previous {
            for sampled in &previous.values {
                for (field, value) in [
                    (
                        rack_slot_mod_offset_field(
                            previous.track,
                            previous.slot_idx,
                            sampled.param_idx,
                        ),
                        0.0,
                    ),
                    (
                        rack_slot_mod_value_field(
                            previous.track,
                            previous.slot_idx,
                            sampled.param_idx,
                        ),
                        0.0,
                    ),
                    (
                        rack_slot_mod_scale_field(
                            previous.track,
                            previous.slot_idx,
                            sampled.param_idx,
                        ),
                        1.0,
                    ),
                ] {
                    published += 1;
                    effects_dirty |= rt
                        .set_reactive("SEQ", &field, Value::Number(value))
                        .effects_dirty;
                }
            }
        }
        return (effects_dirty, published);
    };
    let previous = previous.filter(|previous| {
        previous.track == current.track && previous.slot_idx == current.slot_idx
    });
    for sampled in &current.values {
        let was = previous.and_then(|previous| {
            previous
                .values
                .iter()
                .find(|candidate| candidate.param_idx == sampled.param_idx)
                .copied()
        });
        let mut publish = |field: String, value: f64, was: Option<f64>| {
            if was == Some(value) {
                return;
            }
            published += 1;
            effects_dirty |= rt
                .set_reactive("SEQ", &field, Value::Number(value))
                .effects_dirty;
        };
        publish(
            rack_slot_mod_offset_field(current.track, current.slot_idx, sampled.param_idx),
            sampled.offset,
            was.map(|was| was.offset),
        );
        publish(
            rack_slot_mod_value_field(current.track, current.slot_idx, sampled.param_idx),
            sampled.value,
            was.map(|was| was.value),
        );
        publish(
            rack_slot_mod_scale_field(current.track, current.slot_idx, sampled.param_idx),
            sampled.scale,
            was.map(|was| was.scale),
        );
    }
    (effects_dirty, published)
}

/// Everything the UI tick publishes for modulated-value display: per-effect
/// values keyed by graph node, plus the selected track's instrument. They share
/// one watchlist reconciliation, so a single pass owns which modulator nodes
/// the audio thread snapshots.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ModDisplayValues {
    pub effects: Vec<EffectModValues>,
    pub instrument: Option<InstrumentModValues>,
    pub rack_slot: Option<RackSlotModValues>,
}

/// Sample the selected track's instrument, if it declares any modulation
/// destinations.
///
/// A poly instrument has one modulator node *per voice*, so unlike an effect
/// there is no single node to read. The audio thread publishes the modulator of
/// the most recently allocated voice into
/// `transport.display_modulator_node_ids` (see `last_voice_modulator_node`),
/// and the display follows that: the note you played last, exactly like a
/// hardware mod indicator.
fn read_instrument_mod_values_for_track(
    lg: sequencer::audiograph::LiveGraphPtr,
    app: &app::App,
    state: &Arc<SequencerState>,
    track: usize,
    selected_step: Option<usize>,
    live: bool,
    wanted: &mut HashSet<i32>,
) -> Option<InstrumentModValues> {
    use sequencer::instruments::voice_modulator::SLOT_COUNT;
    let desc = app.graph.instrument_descriptors.get(track)?;
    if desc.instrument_modulation_targets.is_empty() {
        return None;
    }
    let slot = app.state.pattern.instrument_slots.get(track)?;
    let display_step = displayed_plock_step(state, track, selected_step);
    // Mirrors `instrument_param_display_value` (minus the neural selection,
    // which is a step-editing overlay rather than a live value) so the dot and
    // the knob pointer always agree about the base.
    let value_of = |idx: usize| -> f32 {
        let Some(pdesc) = desc.params.get(idx) else {
            return 0.0;
        };
        display_step
            .and_then(|step| {
                held_plock_value(&app.state, track, step, slot.plocks.has_any_plock(), |s| {
                    slot.plocks.get(s, idx)
                })
            })
            .or_else(|| app.effective_instrument_param_value(track, idx))
            .unwrap_or_else(|| slot_param_stored_value(slot, pdesc, idx, display_step))
    };

    let lanes = destination_lanes(desc, &value_of);
    if lanes.is_empty() {
        return None;
    }
    // A track that has gone out of range simply has no modulator to read; it
    // must never index-panic the UI thread.
    let modulator_node_id = state
        .transport
        .display_modulator_node_ids
        .get(track)
        .map(|node_id| node_id.load(Ordering::Relaxed) as i32)
        .unwrap_or(0);
    let modulated = live && modulator_node_id > 0 && lanes.iter().any(|(_, lanes)| lanes.is_some());
    let slot_values = if modulated {
        wanted.insert(modulator_node_id);
        read_effect_modulator_slot_values(lg, modulator_node_id)
    } else {
        [0.0_f32; SLOT_COUNT]
    };
    // The shared arithmetic works in the stored domain; the instrument panel
    // displays user-domain values (percent params are stored 0..1 and shown
    // 0..100), so scale the offset on the way out. `stored_to_user` is linear,
    // so scaling a difference is the same as differencing two scaled values,
    // and a zero offset stays exactly zero.
    let stored = effect_mod_values_for_lanes(desc, 0, &value_of, &slot_values, &lanes);
    let values = stored
        .values
        .into_iter()
        .filter_map(|sampled| {
            let pdesc = desc.params.get(sampled.param_idx)?;
            Some(ParamModValue {
                param_idx: sampled.param_idx,
                // `stored_to_user` is linear through the origin, so scaling a
                // displacement is the same as differencing two scaled values,
                // and a zero offset stays exactly zero.
                offset: pdesc.stored_to_user(sampled.offset as f32) as f64,
                value: pdesc.stored_to_user(sampled.value as f32) as f64,
                // Linear through the origin, so the ratio survives the domain
                // change unchanged.
                scale: sampled.scale,
            })
        })
        .collect();
    Some(InstrumentModValues { track, values })
}

/// Publish the changed instrument offsets. Same delta contract as the effect
/// half: zero writes while nothing moves, and a track change republishes every
/// field so the panel never shows the previous instrument's modulation.
pub(crate) fn sync_instrument_mod_offset_field_delta(
    rt: &mut Runtime,
    previous: Option<&InstrumentModValues>,
    current: Option<&InstrumentModValues>,
) -> (bool, usize) {
    let mut effects_dirty = false;
    let mut published = 0usize;
    let Some(current) = current else {
        // Same settling as the rack half: the previous sample's fields would
        // otherwise hold their last values forever once the track stops being
        // sampled, freezing a dot beside an unmodulated knob.
        if let Some(previous) = previous {
            for sampled in &previous.values {
                for (field, value) in [
                    (fx_instrument_mod_offset_field(sampled.param_idx), 0.0),
                    (fx_instrument_mod_value_field(sampled.param_idx), 0.0),
                    (fx_instrument_mod_scale_field(sampled.param_idx), 1.0),
                    (
                        instrument_mod_offset_field(previous.track, sampled.param_idx),
                        0.0,
                    ),
                    (
                        instrument_mod_value_field(previous.track, sampled.param_idx),
                        0.0,
                    ),
                    (
                        instrument_mod_scale_field(previous.track, sampled.param_idx),
                        1.0,
                    ),
                ] {
                    published += 1;
                    effects_dirty |= rt
                        .set_reactive("SEQ", &field, Value::Number(value))
                        .effects_dirty;
                }
            }
        }
        return (effects_dirty, published);
    };
    let previous = previous.filter(|previous| previous.track == current.track);
    for sampled in &current.values {
        let was = previous.and_then(|previous| {
            previous
                .values
                .iter()
                .find(|candidate| candidate.param_idx == sampled.param_idx)
                .copied()
        });
        let mut publish = |field: String, value: f64, was: Option<f64>| {
            if was == Some(value) {
                return;
            }
            published += 1;
            effects_dirty |= rt
                .set_reactive("SEQ", &field, Value::Number(value))
                .effects_dirty;
        };
        publish(
            fx_instrument_mod_offset_field(sampled.param_idx),
            sampled.offset,
            was.map(|was| was.offset),
        );
        publish(
            fx_instrument_mod_value_field(sampled.param_idx),
            sampled.value,
            was.map(|was| was.value),
        );
        publish(
            instrument_mod_offset_field(current.track, sampled.param_idx),
            sampled.offset,
            was.map(|was| was.offset),
        );
        publish(
            fx_instrument_mod_scale_field(sampled.param_idx),
            sampled.scale,
            was.map(|was| was.scale),
        );
        publish(
            instrument_mod_value_field(current.track, sampled.param_idx),
            sampled.value,
            was.map(|was| was.value),
        );
        publish(
            instrument_mod_scale_field(current.track, sampled.param_idx),
            sampled.scale,
            was.map(|was| was.scale),
        );
    }
    (effects_dirty, published)
}

/// Publish the changed offsets. Returns `(effects_dirty, published)`:
/// `published` is zero whenever nothing moved, which is the check that an idle
/// (or unmodulated) panel dirties no widget.
pub(crate) fn sync_effect_mod_offset_field_delta(
    rt: &mut Runtime,
    previous: &[EffectModValues],
    current: &[EffectModValues],
) -> (bool, usize) {
    let mut effects_dirty = false;
    let mut published = 0usize;
    for response in current {
        let prev = previous
            .iter()
            .find(|candidate| candidate.node_id == response.node_id);
        for sampled in &response.values {
            let was = prev.and_then(|prev| {
                prev.values
                    .iter()
                    .find(|candidate| candidate.param_idx == sampled.param_idx)
                    .copied()
            });
            let mut publish = |field: String, value: f64, was: Option<f64>| {
                if was == Some(value) {
                    return;
                }
                published += 1;
                effects_dirty |= rt
                    .set_reactive("SEQ", &field, Value::Number(value))
                    .effects_dirty;
            };
            publish(
                effect_mod_offset_field(response.node_id, sampled.param_idx),
                sampled.offset,
                was.map(|was| was.offset),
            );
            publish(
                effect_mod_value_field(response.node_id, sampled.param_idx),
                sampled.value,
                was.map(|was| was.value),
            );
            publish(
                effect_mod_scale_field(response.node_id, sampled.param_idx),
                sampled.scale,
                was.map(|was| was.scale),
            );
        }
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
