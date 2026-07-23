/*!
Conversion of resolved step and generated emissions into scheduled queue events.
*/

#[allow(unused_imports)]
use super::*;

pub(super) fn enqueue_resolved_trigger<const QUEUE_CAP: usize>(
    queue: &ScheduledEventQueue<QUEUE_CAP>,
    snapshot: &SequencerSnapshot,
    track_output_events: &mut Vec<TrackOutputEvent>,
    pattern_epoch: u64,
    sample_time: u64,
    event_beat: f64,
    samples_per_quarter: f32,
    global_transpose: f32,
    track_idx: usize,
    step_idx: usize,
    samples_per_step: f32,
    resolved: ResolvedStep,
    chord: ScheduledChordData,
    effect_params: Vec<ScheduledEffectParam>,
    instrument_params: ScheduledInstrumentParams,
    instrument_tensor_params: ScheduledInstrumentTensorParams,
    mut sampler_params: ScheduledSamplerParams,
    rack_macro_values: [Option<f32>; crate::sequencer::RACK_MACRO_COUNT],
) -> bool {
    let (resolved, chord) = apply_fit_to_scale_to_trigger(snapshot, track_idx, resolved, chord);
    let resolved =
        apply_global_transpose_to_resolved(snapshot, track_idx, global_transpose, resolved);
    apply_sampler_instrument_param_overrides(
        snapshot,
        track_idx,
        &mut sampler_params,
        &instrument_params,
    );
    process_trace(snapshot, || {
        format!(
            "enqueue kind=step track={} step={} inst_params={} sampler.attack={} sampler.release={} sampler.speed={}",
            track_idx + 1,
            step_idx,
            instrument_params.len(),
            sampler_params.attack_ms,
            sampler_params.release_ms,
            sampler_params.playback_speed
        )
    });
    let instrument_fingerprint = instrument_sound_fingerprint(
        snapshot,
        track_idx,
        &instrument_params,
        &instrument_tensor_params,
    );
    if chord.count > 0 {
        let max_delay = chord.delays[..chord.count]
            .iter()
            .copied()
            .fold(0.0_f32, f32::max);
        if max_delay > 1e-6 {
            let mut ok = true;
            for note_idx in 0..chord.count {
                let note_delay =
                    chord.delays[note_idx].clamp(StepParam::Delay.min(), StepParam::Delay.max());
                let mut note_chord = ScheduledChordData {
                    count: 1,
                    notes: [0.0; MAX_VOICES],
                    durations: [0.0; MAX_VOICES],
                    delays: [0.0; MAX_VOICES],
                    step_transpose: chord.step_transpose,
                };
                note_chord.notes[0] = chord.notes[note_idx];
                note_chord.durations[0] = chord.durations[note_idx];
                let note_sample_time = sample_time.saturating_add(
                    (note_delay as f64 * samples_per_step.max(0.0) as f64).round() as u64,
                );
                if queue
                    .push(ScheduledEvent {
                        pattern_epoch,
                        sample_time: note_sample_time,
                        kind: ScheduledEventKind::ResolvedTrigger {
                            track: track_idx,
                            step: step_idx,
                            samples_per_step,
                            resolved,
                            chord: note_chord,
                            effect_params: effect_params.clone(),
                            instrument_params: instrument_params.clone(),
                            instrument_tensor_params: instrument_tensor_params.clone(),
                            sampler_params,
                            instrument_fingerprint,
                            rack_macro_values,
                        },
                    })
                    .is_err()
                {
                    ok = false;
                    break;
                }
                let note_beat = event_beat
                    + (note_sample_time.saturating_sub(sample_time) as f64)
                        / samples_per_quarter.max(1.0) as f64;
                record_track_output_event(
                    track_output_events,
                    track_idx,
                    note_sample_time,
                    note_beat,
                    resolved,
                );
            }
            return ok;
        }
    }
    let enqueued = queue
        .push(ScheduledEvent {
            pattern_epoch,
            sample_time,
            kind: ScheduledEventKind::ResolvedTrigger {
                track: track_idx,
                step: step_idx,
                samples_per_step,
                resolved,
                chord,
                effect_params,
                instrument_params,
                instrument_tensor_params,
                sampler_params,
                instrument_fingerprint,
                rack_macro_values,
            },
        })
        .is_ok();
    if enqueued {
        record_track_output_event(
            track_output_events,
            track_idx,
            sample_time,
            event_beat,
            resolved,
        );
    }
    enqueued
}

pub(super) fn step_event_from_resolved(
    snapshot: &SequencerSnapshot,
    track_idx: usize,
    step_idx: usize,
    samples_per_step: f32,
    resolved: ResolvedStep,
    chord: ScheduledChordData,
    effect_params: Vec<ScheduledEffectParam>,
    instrument_params: ScheduledInstrumentParams,
    instrument_tensor_params: ScheduledInstrumentTensorParams,
) -> StepEvent {
    let instrument_fingerprint = instrument_sound_fingerprint(
        snapshot,
        track_idx,
        &instrument_params,
        &instrument_tensor_params,
    );
    StepEvent {
        track: track_idx,
        samples_per_step,
        resolved,
        chord,
        effect_params,
        instrument_params,
        instrument_tensor_params,
        sampler_params: resolve_sampler_params(snapshot, track_idx, step_idx),
        rack_macro_values: [None; crate::sequencer::RACK_MACRO_COUNT],
        source: EventSource::Step {
            track: track_idx,
            step: step_idx,
            instrument_fingerprint,
        },
    }
}

pub(super) fn enqueue_step_event<const QUEUE_CAP: usize>(
    queue: &ScheduledEventQueue<QUEUE_CAP>,
    snapshot: &SequencerSnapshot,
    track_output_events: &mut Vec<TrackOutputEvent>,
    pattern_epoch: u64,
    sample_time: u64,
    event_beat: f64,
    samples_per_quarter: f32,
    global_transpose: f32,
    mut event: StepEvent,
) -> bool {
    match event.source.clone() {
        EventSource::Step { step, .. } => enqueue_resolved_trigger(
            queue,
            snapshot,
            track_output_events,
            pattern_epoch,
            sample_time,
            event_beat,
            samples_per_quarter,
            global_transpose,
            event.track,
            step,
            event.samples_per_step,
            event.resolved,
            event.chord,
            event.effect_params,
            event.instrument_params,
            event.instrument_tensor_params,
            event.sampler_params,
            event.rack_macro_values,
        ),
        EventSource::Network { seed, neuron, .. } => {
            normalize_network_event_destination(snapshot, neuron, seed, &mut event);
            let instrument_fingerprint = instrument_sound_fingerprint(
                snapshot,
                event.track,
                &event.instrument_params,
                &event.instrument_tensor_params,
            );
            enqueue_network_trigger(
                queue,
                snapshot,
                track_output_events,
                pattern_epoch,
                sample_time,
                event_beat,
                samples_per_quarter,
                global_transpose,
                event.track,
                neuron,
                seed,
                event.samples_per_step,
                event.resolved,
                event.chord,
                event.effect_params,
                event.instrument_params,
                event.instrument_tensor_params,
                event.sampler_params,
                instrument_fingerprint,
                event.rack_macro_values,
            )
        }
    }
}

pub(super) fn midi_fx_step_for_step_event(snapshot: &SequencerSnapshot, event: &StepEvent) -> usize {
    let step = match event.source {
        EventSource::Step { step, .. } => step,
        EventSource::Network {
            seed: Some((_, step)),
            ..
        } => step,
        EventSource::Network { .. } => 0,
    };
    midi_fx_event_step_for_track(snapshot, event.track, step)
}

pub(super) fn enqueue_step_event_with_midi_fx<const QUEUE_CAP: usize>(
    queue: &ScheduledEventQueue<QUEUE_CAP>,
    snapshot: &SequencerSnapshot,
    track_output_events: &mut Vec<TrackOutputEvent>,
    runtime: Option<&mut lisp_host::ScratchControlRuntime>,
    quantizer_state: Option<&mut MidiFxQuantizerState>,
    pattern_epoch: u64,
    sample_time: u64,
    event_beat: f64,
    samples_per_quarter: f32,
    global_transpose: f32,
    arp_phase_beats: f32,
    mut event: StepEvent,
    midi_fx_params: Vec<ProcessMidiFxParamOverride>,
    debug_accum: bool,
) -> bool {
    if event.track >= snapshot.tracks.len() {
        if debug_routing_enabled() {
            eprintln!(
                "[routing] skip enqueue_step_event_with_midi_fx reason=track-out-of-range track={} tracks={} source={}",
                event.track,
                snapshot.tracks.len(),
                event_source_label(&event.source)
            );
        }
        return false;
    }
    let run_midi_fx = snapshot.tracks[event.track].params.midi_fx_position
        == MidiFxPosition::PostAccumulator
        && !snapshot.tracks[event.track].params.midi_fx_chain.is_empty();
    let Some(runtime) = runtime else {
        if debug_routing_enabled() {
            eprintln!(
                "[routing] skip midi-fx reason=no-scratch-runtime track={} sample={} source={} chain={:?}",
                event.track,
                sample_time,
                event_source_label(&event.source),
                snapshot.tracks[event.track].params.midi_fx_chain
            );
        }
        return enqueue_step_event(
            queue,
            snapshot,
            track_output_events,
            pattern_epoch,
            sample_time,
            event_beat,
            samples_per_quarter,
            global_transpose,
            event,
        );
    };
    if !run_midi_fx {
        if debug_routing_enabled() {
            eprintln!(
                "[routing] skip midi-fx reason=no-post-accumulator-chain track={} sample={} source={} position={:?} chain={:?}",
                event.track,
                sample_time,
                event_source_label(&event.source),
                snapshot.tracks[event.track].params.midi_fx_position,
                snapshot.tracks[event.track].params.midi_fx_chain
            );
        }
        return enqueue_step_event(
            queue,
            snapshot,
            track_output_events,
            pattern_epoch,
            sample_time,
            event_beat,
            samples_per_quarter,
            global_transpose,
            event,
        );
    }
    if let EventSource::Network { seed, neuron, .. } = event.source.clone() {
        normalize_network_event_destination(snapshot, neuron, seed, &mut event);
    }
    if debug_routing_enabled() {
        eprintln!(
            "[routing] enter midi-fx track={} step={} sample={} source={} chain={:?} transpose={} vel={} inst_params={} sampler_speed={}",
            event.track,
            midi_fx_step_for_step_event(snapshot, &event),
            sample_time,
            event_source_label(&event.source),
            snapshot.tracks[event.track].params.midi_fx_chain,
            event.resolved.transpose,
            event.resolved.velocity,
            event.instrument_params.len(),
            event.sampler_params.playback_speed
        );
    }

    let step = midi_fx_step_for_step_event(snapshot, &event);
    let step_beats = if samples_per_quarter > 0.0 {
        event.samples_per_step / samples_per_quarter
    } else {
        0.0
    };
    let event = midi_fx_event_from_step_event(
        snapshot,
        event,
        step,
        step_beats,
        0.0,
        arp_phase_beats,
        midi_fx_params,
    );
    let events = run_midi_fx_chain_for_track(
        runtime,
        snapshot,
        event.track,
        vec![event],
        quantizer_state,
        0,
        debug_accum,
    );
    if debug_routing_enabled() {
        eprintln!(
            "[routing] midi-fx result count={} base_sample={} samples_per_quarter={}",
            events.len(),
            sample_time,
            samples_per_quarter
        );
    }
    enqueue_midi_fx_events(
        queue,
        snapshot,
        track_output_events,
        pattern_epoch,
        sample_time,
        event_beat,
        samples_per_quarter,
        global_transpose,
        events,
    )
}

pub(super) fn enqueue_neuron_parameter_events<const QUEUE_CAP: usize>(
    queue: &ScheduledEventQueue<QUEUE_CAP>,
    pattern_epoch: u64,
    sample_time: u64,
    parameter_events: NeuronParameterEvents,
) -> bool {
    let mut ok = true;
    for (track, instrument_params) in parameter_events.instrument {
        if instrument_params.is_empty() {
            continue;
        }
        if queue
            .push(ScheduledEvent {
                pattern_epoch,
                sample_time,
                kind: ScheduledEventKind::InstrumentParams {
                    track,
                    instrument_params,
                    instrument_tensor_params: ScheduledInstrumentTensorParams::new(),
                },
            })
            .is_err()
        {
            ok = false;
            break;
        }
    }
    if ok {
        for (track, effect_params) in parameter_events.effects {
            if effect_params.is_empty() {
                continue;
            }
            if queue
                .push(ScheduledEvent {
                    pattern_epoch,
                    sample_time,
                    kind: ScheduledEventKind::EffectParams {
                        track,
                        effect_params,
                    },
                })
                .is_err()
            {
                ok = false;
                break;
            }
        }
    }
    ok
}

pub(super) fn enqueue_neural_output_with_midi_fx<const QUEUE_CAP: usize>(
    queue: &ScheduledEventQueue<QUEUE_CAP>,
    snapshot: &SequencerSnapshot,
    track_output_events: &mut Vec<TrackOutputEvent>,
    runtime: Option<&mut lisp_host::ScratchControlRuntime>,
    mut quantizer_state: Option<&mut MidiFxQuantizerState>,
    pattern_epoch: u64,
    sample_time: u64,
    samples_per_quarter: f32,
    global_transpose: f32,
    arp_phase_beats: f32,
    output: NeuralOutput,
    debug_accum: bool,
) -> bool {
    let mut event = output.event;
    let (seed, neuron) = match event.source.clone() {
        EventSource::Network { seed, neuron, .. } => (seed, neuron),
        EventSource::Step { .. } => {
            return output.emit_trigger
                && enqueue_step_event_with_midi_fx(
                    queue,
                    snapshot,
                    track_output_events,
                    runtime,
                    quantizer_state.as_deref_mut(),
                    pattern_epoch,
                    sample_time,
                    arp_phase_beats as f64,
                    samples_per_quarter,
                    global_transpose,
                    arp_phase_beats,
                    event,
                    Vec::new(),
                    debug_accum,
                );
        }
    };
    if output.emit_trigger {
        normalize_network_event_destination(snapshot, neuron, seed, &mut event);
    }
    let trigger_track = output.emit_trigger.then_some(event.track);
    let parameter_events =
        apply_neuron_output_overrides(snapshot, neuron, trigger_track, &mut event);
    if !enqueue_neuron_parameter_events(queue, pattern_epoch, sample_time, parameter_events) {
        return false;
    }
    if !output.emit_trigger {
        return true;
    }
    enqueue_step_event_with_midi_fx(
        queue,
        snapshot,
        track_output_events,
        runtime,
        quantizer_state,
        pattern_epoch,
        sample_time,
        arp_phase_beats as f64,
        samples_per_quarter,
        global_transpose,
        arp_phase_beats,
        event,
        Vec::new(),
        debug_accum,
    )
}

#[derive(Clone, Copy, Debug)]
pub(super) enum EmittedNetworkEventSource {
    Generator {
        index: usize,
    },
    Process {
        runtime_id: u64,
    },
    Graph {
        graph_index: usize,
        node_index: usize,
    },
}

impl EmittedNetworkEventSource {
    fn event_source_index(self) -> usize {
        match self {
            Self::Generator { index } => index,
            Self::Process { runtime_id } => runtime_id as usize,
            Self::Graph { node_index, .. } => node_index,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Generator { .. } => "generator",
            Self::Process { .. } => "process",
            Self::Graph { .. } => "graph",
        }
    }

    fn owner_index(self) -> usize {
        match self {
            Self::Generator { index } => index,
            Self::Process { runtime_id } => runtime_id as usize,
            Self::Graph {
                graph_index: index, ..
            } => index,
        }
    }

    fn resolve_track(self, emitted_track: Option<usize>) -> Option<usize> {
        match self {
            Self::Generator { .. } | Self::Process { .. } => emitted_track.or(Some(0)),
            Self::Graph { .. } => emitted_track,
        }
    }
}

pub(super) fn enqueue_emitted_network_event_with_midi_fx<const QUEUE_CAP: usize>(
    queue: &ScheduledEventQueue<QUEUE_CAP>,
    snapshot: &SequencerSnapshot,
    track_output_events: &mut Vec<TrackOutputEvent>,
    runtime: Option<&mut lisp_host::ScratchControlRuntime>,
    quantizer_state: Option<&mut MidiFxQuantizerState>,
    pattern_epoch: u64,
    sample_time: u64,
    samples_per_quarter: f32,
    arp_phase_beats: f32,
    global_transpose: f32,
    source: EmittedNetworkEventSource,
    emitted: lisp_host::EmittedAccumulatorEvent,
    debug_accum: bool,
) -> bool {
    let Some(track_idx) = source.resolve_track(emitted.track) else {
        if debug_routing_enabled() {
            eprintln!(
                "[routing] skip emitted-network reason=route-off source={} owner_index={} source_index={} sample={}",
                source.label(),
                source.owner_index(),
                source.event_source_index(),
                sample_time
            );
        }
        return true;
    };
    if track_idx >= snapshot.tracks.len() {
        if debug_routing_enabled() {
            eprintln!(
                "[routing] skip emitted-network reason=track-out-of-range source={} owner_index={} source_index={} track={:?} tracks={} sample={}",
                source.label(),
                source.owner_index(),
                source.event_source_index(),
                emitted.track,
                snapshot.tracks.len(),
                sample_time
            );
        }
        return true;
    }
    if debug_routing_enabled() {
        eprintln!(
            "[routing] emitted-network source={} owner_index={} source_index={} track={} sample={} event_beats={} chain={:?} transpose={} vel={} emitted_fx_params={} emitted_inst_params={}",
            source.label(),
            source.owner_index(),
            source.event_source_index(),
            track_idx,
            sample_time,
            arp_phase_beats,
            snapshot.tracks[track_idx].params.midi_fx_chain,
            emitted.resolved.transpose,
            emitted.resolved.velocity,
            emitted.effect_params.len(),
            emitted.instrument_params.len()
        );
    }

    let chord = chord_data_from_parts(
        &emitted.chord,
        &emitted.chord_durations,
        &[],
        emitted.resolved.duration,
        emitted.chord_step_transpose,
    );
    let mut event = StepEvent {
        track: track_idx,
        samples_per_step: samples_per_quarter,
        resolved: emitted.resolved,
        chord,
        effect_params: resolve_effect_defaults(snapshot, track_idx),
        instrument_params: resolve_instrument_defaults(snapshot, track_idx),
        instrument_tensor_params: resolve_instrument_tensor_defaults(snapshot, track_idx),
        sampler_params: resolve_sampler_defaults(snapshot, track_idx),
        rack_macro_values: [None; crate::sequencer::RACK_MACRO_COUNT],
        source: EventSource::Network {
            seed: None,
            neuron: source.event_source_index(),
            instrument_fingerprint: 0,
        },
    };
    upsert_effect_params(&mut event.effect_params, emitted.effect_params);
    upsert_instrument_params(
        &mut event.instrument_params,
        scheduled_instrument_params_from_vec(emitted.instrument_params),
    );

    enqueue_step_event_with_midi_fx(
        queue,
        snapshot,
        track_output_events,
        runtime,
        quantizer_state,
        pattern_epoch,
        sample_time,
        arp_phase_beats as f64,
        samples_per_quarter,
        global_transpose,
        arp_phase_beats,
        event,
        Vec::new(),
        debug_accum,
    )
}

pub(super) fn enqueue_due_process_emissions<const QUEUE_CAP: usize>(
    queue: &ScheduledEventQueue<QUEUE_CAP>,
    snapshot: &SequencerSnapshot,
    track_output_events: &mut Vec<TrackOutputEvent>,
    scratch_runtime: &mut Option<lisp_host::ScratchControlRuntime>,
    quantizer_state: &mut MidiFxQuantizerState,
    process_runtime: &mut crate::process::ProcessRuntime,
    pattern_epoch: u64,
    chunk_start_beats: f64,
    chunk_start_sample: u64,
    up_to_beat: f64,
    samples_per_quarter: f64,
    debug_accum: bool,
) -> bool {
    for item in process_runtime.take_due_events(up_to_beat) {
        let sample_time = chunk_start_sample.saturating_add(
            ((item.beat - chunk_start_beats).max(0.0) * samples_per_quarter).round() as u64,
        );
        match item.event {
            crate::process::ProcessScheduledEvent::Emission(event) => {
                if debug_routing_enabled() {
                    eprintln!(
                        "[routing] process-emission process={} track={:?} sample={} beat={:.6} transpose={} vel={}",
                        item.process_runtime_id,
                        event.track,
                        sample_time,
                        item.beat,
                        event.resolved.transpose,
                        event.resolved.velocity
                    );
                }
                if !enqueue_emitted_network_event_with_midi_fx(
                    queue,
                    snapshot,
                    track_output_events,
                    scratch_runtime.as_mut(),
                    Some(&mut *quantizer_state),
                    pattern_epoch,
                    sample_time,
                    samples_per_quarter as f32,
                    item.beat as f32,
                    process_runtime.global_transpose(),
                    EmittedNetworkEventSource::Process {
                        runtime_id: item.process_runtime_id,
                    },
                    event,
                    debug_accum,
                ) {
                    return false;
                }
            }
            crate::process::ProcessScheduledEvent::Step(spawned) => {
                if debug_routing_enabled() {
                    eprintln!(
                        "[routing] process-step process={} track={} sample={} beat={:.6} transpose={} vel={} midi_fx_overrides={}",
                        item.process_runtime_id,
                        spawned.event.track,
                        sample_time,
                        item.beat,
                        spawned.event.resolved.transpose,
                        spawned.event.resolved.velocity,
                        spawned.midi_fx_params.len()
                    );
                }
                if !enqueue_step_event_with_midi_fx(
                    queue,
                    snapshot,
                    track_output_events,
                    scratch_runtime.as_mut(),
                    Some(&mut *quantizer_state),
                    pattern_epoch,
                    sample_time,
                    item.beat,
                    samples_per_quarter as f32,
                    process_runtime.global_transpose(),
                    item.beat as f32,
                    spawned.event,
                    spawned.midi_fx_params,
                    debug_accum,
                ) {
                    return false;
                }
            }
        }
    }
    true
}

