/*!
MIDI-FX event transforms, quantization state, and live-keyboard scheduling.
*/

#[allow(unused_imports)]
use super::*;

#[derive(Clone)]
pub(super) struct MidiFxEvent {
    pub(super) offset_beats: f32,
    pub(super) track: usize,
    pub(super) step: usize,
    pub(super) samples_per_step: f32,
    pub(super) step_beats: f32,
    pub(super) resolved: ResolvedStep,
    pub(super) chord: Vec<f32>,
    pub(super) chord_durations: Vec<f32>,
    pub(super) chord_delays: Vec<f32>,
    pub(super) chord_step_transpose: f32,
    pub(super) note_spans: Option<Vec<AccumulatorNoteSpan>>,
    pub(super) arp_phase_beats: f32,
    pub(super) midi_fx_params: Vec<ProcessMidiFxParamOverride>,
    pub(super) effect_params: Vec<ScheduledEffectParam>,
    pub(super) instrument_params: ScheduledInstrumentParams,
    pub(super) instrument_tensor_params: ScheduledInstrumentTensorParams,
    pub(super) sampler_params: ScheduledSamplerParams,
    pub(super) rack_macro_values: [Option<f32>; crate::sequencer::RACK_MACRO_COUNT],
    pub(super) source: EventSource,
}

#[derive(Clone)]
pub(super) struct PendingQuantizedMidiFxEvent {
    pub(super) source_track: usize,
    pub(super) stage_idx: usize,
    pub(super) resume_stage_idx: usize,
    pub(super) deadline_beats: f64,
    pub(super) event: MidiFxEvent,
}

#[derive(Default)]
pub(super) struct MidiFxQuantizerState {
    pending: Vec<PendingQuantizedMidiFxEvent>,
}

impl MidiFxQuantizerState {
    pub(super) fn reset(&mut self) {
        self.pending.clear();
    }

    fn push_or_replace(
        &mut self,
        source_track: usize,
        stage_idx: usize,
        resume_stage_idx: usize,
        deadline_beats: f64,
        mut event: MidiFxEvent,
    ) {
        event.offset_beats = 0.0;
        event.arp_phase_beats = deadline_beats as f32;
        let existing = self.pending.iter_mut().find(|pending| {
            pending.source_track == source_track
                && pending.stage_idx == stage_idx
                && (pending.deadline_beats - deadline_beats).abs() <= 1e-9
        });
        if let Some(pending) = existing {
            if event.resolved.velocity > pending.event.resolved.velocity {
                pending.resume_stage_idx = resume_stage_idx;
                pending.event = event;
            }
        } else {
            self.pending.push(PendingQuantizedMidiFxEvent {
                source_track,
                stage_idx,
                resume_stage_idx,
                deadline_beats,
                event,
            });
        }
    }

    pub(super) fn drain_due(&mut self, up_to_beats: f64) -> Vec<PendingQuantizedMidiFxEvent> {
        let mut due = Vec::new();
        let mut idx = 0;
        while idx < self.pending.len() {
            if self.pending[idx].deadline_beats <= up_to_beats + 1e-9 {
                due.push(self.pending.swap_remove(idx));
            } else {
                idx += 1;
            }
        }
        due.sort_by(|a, b| {
            a.deadline_beats
                .partial_cmp(&b.deadline_beats)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        due
    }
}

#[derive(Clone, Copy)]
pub(super) struct LiveMidiFxNote {
    pub(super) transpose: f32,
    pub(super) velocity: f32,
    pub(super) pending_event: bool,
}

#[derive(Clone, Default)]
pub(super) struct LiveMidiFxTrackState {
    pub(super) notes: Vec<LiveMidiFxNote>,
    pub(super) next_tick_sample: u64,
    pub(super) quantize_next_tick: bool,
}

pub(super) fn midi_fx_event_from_step(
    snapshot: &SequencerSnapshot,
    track_idx: usize,
    step_idx: usize,
    samples_per_step: f32,
    step_beats: f32,
    arp_phase_beats: f32,
    resolved: ResolvedStep,
    effect_params: Vec<ScheduledEffectParam>,
    instrument_params: ScheduledInstrumentParams,
    instrument_tensor_params: ScheduledInstrumentTensorParams,
) -> MidiFxEvent {
    let step = &snapshot.tracks[track_idx].steps[step_idx];
    MidiFxEvent {
        offset_beats: 0.0,
        track: track_idx,
        step: step_idx,
        samples_per_step,
        step_beats,
        resolved,
        chord: step.chord.clone(),
        chord_durations: step.chord_durations.clone(),
        chord_delays: step.chord_delays.clone(),
        chord_step_transpose: step.params[StepParam::Transpose.index()],
        note_spans: Some(track_note_spans_for_trigger(snapshot, track_idx, step_idx)),
        arp_phase_beats,
        midi_fx_params: Vec::new(),
        effect_params,
        instrument_params,
        instrument_tensor_params,
        sampler_params: resolve_sampler_params(snapshot, track_idx, step_idx),
        rack_macro_values: [None; crate::sequencer::RACK_MACRO_COUNT],
        source: EventSource::Step {
            track: track_idx,
            step: step_idx,
            instrument_fingerprint: 0,
        },
    }
}

pub(super) fn midi_fx_event_step_for_track(
    snapshot: &SequencerSnapshot,
    track_idx: usize,
    step_idx: usize,
) -> usize {
    let step_count = snapshot
        .tracks
        .get(track_idx)
        .map(|track| track.steps.len().min(track.params.num_steps.max(1)))
        .unwrap_or(1)
        .max(1);
    step_idx.min(step_count.saturating_sub(1))
}

pub(super) fn midi_fx_event_from_step_event(
    snapshot: &SequencerSnapshot,
    mut event: StepEvent,
    step_idx: usize,
    step_beats: f32,
    offset_beats: f32,
    arp_phase_beats: f32,
    midi_fx_params: Vec<ProcessMidiFxParamOverride>,
) -> MidiFxEvent {
    let step_idx = midi_fx_event_step_for_track(snapshot, event.track, step_idx);
    let chord = event.chord.notes[..event.chord.count].to_vec();
    let chord_durations = event.chord.durations[..event.chord.count].to_vec();
    let chord_delays = event.chord.delays[..event.chord.count].to_vec();
    event.sampler_params = match &event.source {
        EventSource::Network { .. } => event.sampler_params,
        EventSource::Step { .. } => resolve_sampler_params(snapshot, event.track, step_idx),
    };
    MidiFxEvent {
        offset_beats,
        track: event.track,
        step: step_idx,
        samples_per_step: event.samples_per_step,
        step_beats,
        resolved: event.resolved,
        chord,
        chord_durations,
        chord_delays,
        chord_step_transpose: event.chord.step_transpose,
        note_spans: None,
        arp_phase_beats,
        midi_fx_params,
        effect_params: event.effect_params,
        instrument_params: event.instrument_params,
        instrument_tensor_params: event.instrument_tensor_params,
        sampler_params: event.sampler_params,
        rack_macro_values: event.rack_macro_values,
        source: event.source,
    }
}

pub(super) fn rebind_midi_fx_event_to_track(
    snapshot: &SequencerSnapshot,
    mut event: MidiFxEvent,
    target_track: usize,
) -> Option<MidiFxEvent> {
    if target_track >= snapshot.tracks.len() {
        if debug_routing_enabled() {
            eprintln!(
                "[routing] rebind drop reason=target-out-of-range from_track={} target_track={} tracks={} source={} step={}",
                event.track,
                target_track,
                snapshot.tracks.len(),
                event_source_label(&event.source),
                event.step
            );
        }
        return None;
    }
    if event.track == target_track {
        if debug_routing_enabled() {
            eprintln!(
                "[routing] rebind noop track={} source={} step={}",
                event.track,
                event_source_label(&event.source),
                event.step
            );
        }
        return Some(event);
    }
    if debug_routing_enabled() {
        eprintln!(
            "[routing] rebind from_track={} target_track={} source={} step={} transpose={} explicit_fx_params={} explicit_inst_params={}",
            event.track,
            target_track,
            event_source_label(&event.source),
            event.step,
            event.resolved.transpose,
            event.effect_params.len(),
            event.instrument_params.len()
        );
    }
    let explicit_effect_params = std::mem::take(&mut event.effect_params);
    let explicit_instrument_params = std::mem::replace(
        &mut event.instrument_params,
        ScheduledInstrumentParams::new(),
    );
    let explicit_instrument_tensor_params = std::mem::replace(
        &mut event.instrument_tensor_params,
        ScheduledInstrumentTensorParams::new(),
    );
    let target_step = midi_fx_event_step_for_track(snapshot, target_track, event.step);
    event.track = target_track;
    event.step = target_step;
    event.midi_fx_params.clear();
    event.effect_params = resolve_effect_params(snapshot, target_track, target_step);
    event.instrument_params = resolve_instrument_params(snapshot, target_track, target_step);
    event.instrument_tensor_params =
        resolve_instrument_tensor_params(snapshot, target_track, target_step);
    event.sampler_params = resolve_sampler_params(snapshot, target_track, target_step);
    upsert_effect_params(&mut event.effect_params, explicit_effect_params);
    upsert_instrument_params(&mut event.instrument_params, explicit_instrument_params);
    upsert_instrument_tensor_params(
        &mut event.instrument_tensor_params,
        explicit_instrument_tensor_params,
    );
    event.source = match event.source {
        EventSource::Network { seed, neuron, .. } => EventSource::Network {
            seed,
            neuron,
            instrument_fingerprint: 0,
        },
        EventSource::Step { .. } => EventSource::Step {
            track: target_track,
            step: target_step,
            instrument_fingerprint: 0,
        },
    };
    if debug_routing_enabled() {
        eprintln!(
            "[routing] rebind result target_track={} target_step={} fx_params={} inst_params={} sampler_speed={}",
            event.track,
            event.step,
            event.effect_params.len(),
            event.instrument_params.len(),
            event.sampler_params.playback_speed
        );
    }
    Some(event)
}

pub(super) fn midi_fx_window_events_from_step(
    snapshot: &SequencerSnapshot,
    midi_fx_descriptors: &[EffectDescriptor],
    track_idx: usize,
    step_idx: usize,
    samples_per_step: f32,
    step_beats: f32,
    samples_per_quarter: f32,
    arp_phase_beats: f32,
    resolved: ResolvedStep,
    effect_params: Vec<ScheduledEffectParam>,
    instrument_params: ScheduledInstrumentParams,
    instrument_tensor_params: ScheduledInstrumentTensorParams,
) -> Vec<MidiFxEvent> {
    const EPS: f32 = 1e-5;
    const MAX_WINDOWS: usize = 1024;

    let note_spans = track_note_spans_for_trigger(snapshot, track_idx, step_idx);
    if note_spans.is_empty() {
        if debug_routing_enabled() {
            eprintln!(
                "[midi-fx-window] no note spans track={} step={} -> single event",
                track_idx, step_idx
            );
        }
        return vec![midi_fx_event_from_step(
            snapshot,
            track_idx,
            step_idx,
            samples_per_step,
            step_beats,
            arp_phase_beats,
            resolved,
            effect_params,
            instrument_params,
            instrument_tensor_params,
        )];
    }

    let Some(window_beats) =
        midi_fx_clock_tick_beats(snapshot, midi_fx_descriptors, track_idx, step_idx)
    else {
        if debug_routing_enabled() {
            eprintln!(
                "[midi-fx-window] no clock-rate role track={} step={} chain={:?} descriptors={:?} -> single event",
                track_idx,
                step_idx,
                snapshot.tracks[track_idx].params.midi_fx_chain,
                midi_fx_descriptors
                    .iter()
                    .map(|desc| desc.name.as_str())
                    .collect::<Vec<_>>()
            );
        }
        return vec![midi_fx_event_from_step(
            snapshot,
            track_idx,
            step_idx,
            samples_per_step,
            step_beats,
            arp_phase_beats,
            resolved,
            effect_params,
            instrument_params,
            instrument_tensor_params,
        )];
    };
    let window_beats = window_beats.max(EPS);
    let window_samples = (samples_per_quarter * window_beats).round().max(1.0);
    let end_beats = note_spans
        .iter()
        .map(|span| span.end_beats)
        .fold(0.0_f32, f32::max);
    if end_beats <= EPS {
        return Vec::new();
    }

    let window_count = ((end_beats / window_beats).ceil() as usize).min(MAX_WINDOWS);
    if debug_routing_enabled() {
        eprintln!(
            "[midi-fx-window] clocked track={} step={} window_beats={} window_samples={} end_beats={} windows={} note_spans={}",
            track_idx,
            step_idx,
            window_beats,
            window_samples,
            end_beats,
            window_count,
            note_spans.len()
        );
    }
    let mut events = Vec::with_capacity(window_count);
    for window_idx in 0..window_count {
        let window_start = window_idx as f32 * window_beats;
        let window_end = window_start + window_beats;
        let window_spans = note_spans
            .iter()
            .filter(|span| {
                span.end_beats > window_start + EPS && span.start_beats < window_end - EPS
            })
            .map(|span| AccumulatorNoteSpan {
                transpose: span.transpose,
                start_beats: (span.start_beats - window_start).max(0.0),
                end_beats: (span.end_beats - window_start).min(window_beats).max(0.0),
            })
            .filter(|span| span.end_beats > span.start_beats + EPS)
            .collect::<Vec<_>>();
        if window_spans.is_empty() {
            continue;
        }

        let chord = window_spans
            .iter()
            .map(|span| span.transpose)
            .collect::<Vec<_>>();
        let first_transpose = chord.first().copied().unwrap_or(resolved.transpose);
        let mut window_resolved = resolved;
        window_resolved.duration = 1.0;
        window_resolved.transpose = first_transpose;

        events.push(MidiFxEvent {
            offset_beats: window_start,
            track: track_idx,
            step: step_idx,
            samples_per_step: window_samples,
            step_beats: window_beats,
            resolved: window_resolved,
            chord_durations: vec![1.0; chord.len()],
            chord_delays: vec![0.0; chord.len()],
            chord,
            chord_step_transpose: 0.0,
            note_spans: Some(window_spans),
            arp_phase_beats: arp_phase_beats + window_start,
            midi_fx_params: Vec::new(),
            effect_params: effect_params.clone(),
            instrument_params: instrument_params.clone(),
            instrument_tensor_params: instrument_tensor_params.clone(),
            sampler_params: resolve_sampler_params(snapshot, track_idx, step_idx),
            rack_macro_values: [None; crate::sequencer::RACK_MACRO_COUNT],
            source: EventSource::Step {
                track: track_idx,
                step: step_idx,
                instrument_fingerprint: 0,
            },
        });
    }

    events
}

pub(super) fn run_midi_fx_chain_for_track(
    runtime: &mut lisp_host::ScratchControlRuntime,
    snapshot: &SequencerSnapshot,
    source_track: usize,
    events: Vec<MidiFxEvent>,
    quantizer_state: Option<&mut MidiFxQuantizerState>,
    depth: usize,
    debug_accum: bool,
) -> Vec<MidiFxEvent> {
    run_midi_fx_chain_for_track_inner(
        runtime,
        snapshot,
        source_track,
        events,
        quantizer_state,
        0,
        depth,
        [false; MAX_TRACKS],
        debug_accum,
    )
}

pub(super) fn run_midi_fx_chain_for_track_inner(
    runtime: &mut lisp_host::ScratchControlRuntime,
    snapshot: &SequencerSnapshot,
    source_track: usize,
    events: Vec<MidiFxEvent>,
    mut quantizer_state: Option<&mut MidiFxQuantizerState>,
    start_stage_idx: usize,
    depth: usize,
    mut visited_tracks: [bool; MAX_TRACKS],
    debug_accum: bool,
) -> Vec<MidiFxEvent> {
    if source_track >= snapshot.tracks.len() || depth >= MAX_TRACKS {
        if debug_routing_enabled() {
            eprintln!(
                "[midi-fx] skip chain reason=invalid-source-or-depth source_track={} tracks={} depth={}",
                source_track,
                snapshot.tracks.len(),
                depth
            );
        }
        return Vec::new();
    }
    if visited_tracks.get(source_track).copied().unwrap_or(true) {
        if debug_accum || debug_routing_enabled() {
            eprintln!("[midi-fx] dropped recursive route into track={source_track}");
        }
        return Vec::new();
    }
    visited_tracks[source_track] = true;
    let chain = snapshot.tracks[source_track].params.midi_fx_chain.clone();
    if chain.is_empty() {
        if debug_routing_enabled() {
            eprintln!(
                "[midi-fx] skip chain reason=empty-chain source_track={} events={}",
                source_track,
                events.len()
            );
        }
        return events;
    }
    let names = runtime.midi_fx_names();
    let descriptors = runtime.midi_fx_descriptors();
    if debug_routing_enabled() {
        eprintln!(
            "[midi-fx] chain start source_track={} depth={} events={} chain={:?} registered={:?}",
            source_track,
            depth,
            events.len(),
            chain,
            names
        );
    }
    let mut current = events;
    for (stage_idx, fx_name) in chain.into_iter().enumerate().skip(start_stage_idx) {
        let Some(fx_idx) = names
            .iter()
            .position(|name| name.eq_ignore_ascii_case(&fx_name))
        else {
            if debug_accum || debug_routing_enabled() {
                eprintln!("[midi-fx] missing fx name={fx_name:?} track={source_track}");
            }
            continue;
        };
        let mut next = Vec::new();
        for event in current {
            if event.track != source_track {
                if visited_tracks.get(event.track).copied().unwrap_or(true) {
                    if debug_accum || debug_routing_enabled() {
                        eprintln!(
                            "[midi-fx] dropped recursive pending event into track={}",
                            event.track
                        );
                    }
                    continue;
                }
                next.extend(run_midi_fx_chain_for_track_inner(
                    runtime,
                    snapshot,
                    event.track,
                    vec![event],
                    quantizer_state.as_deref_mut(),
                    0,
                    depth + 1,
                    visited_tracks,
                    debug_accum,
                ));
                continue;
            }
            let mut slot_snapshot = snapshot.tracks[event.track]
                .midi_fx_slots
                .get(stage_idx)
                .cloned()
                .unwrap_or_else(crate::effects::EffectSlotSnapshot::new_empty);
            if let Some(desc) = descriptors.get(fx_idx) {
                if slot_snapshot.num_params == 0 && !desc.params.is_empty() {
                    slot_snapshot = crate::effects::EffectSlotSnapshot::new_default(desc, 0);
                }
                apply_process_midi_fx_overrides_to_slot(
                    &mut slot_snapshot,
                    event.step,
                    stage_idx,
                    &fx_name,
                    desc,
                    &event.midi_fx_params,
                );
            }
            let enabled = descriptors
                .get(fx_idx)
                .and_then(|desc| {
                    desc.params
                        .iter()
                        .position(|param| param.name.eq_ignore_ascii_case("enabled"))
                })
                .and_then(|param_idx| {
                    Some(midi_fx_slot_param_value(
                        &slot_snapshot,
                        event.step,
                        param_idx,
                        1.0,
                    ))
                })
                .unwrap_or(1.0);
            if enabled <= 0.5 {
                if debug_routing_enabled() {
                    eprintln!(
                        "[midi-fx] stage skip reason=disabled track={} step={} fx={} stage={} source={} transpose={}",
                        event.track,
                        event.step,
                        fx_name,
                        stage_idx,
                        event_source_label(&event.source),
                        event.resolved.transpose
                    );
                }
                next.push(event);
                continue;
            }
            if let Some(grid_param_idx) = descriptors
                .get(fx_idx)
                .and_then(midi_fx_quantizer_grid_param)
            {
                if let Some(state) = quantizer_state.as_deref_mut() {
                    let grid_beats = midi_fx_timebase_param_beats_from_slot(
                        snapshot,
                        event.track,
                        &slot_snapshot,
                        grid_param_idx,
                        event.step,
                    )
                    .unwrap_or(event.step_beats.max(1.0 / 1024.0));
                    let samples_per_quarter = if event.step_beats > 0.0 {
                        event.samples_per_step as f64 / event.step_beats as f64
                    } else {
                        0.0
                    };
                    let boundary_tolerance_beats = if samples_per_quarter > 0.0 {
                        1.5 / samples_per_quarter
                    } else {
                        1e-9
                    };
                    let event_beats = snap_near_grid_down(
                        event.arp_phase_beats as f64 + event.offset_beats as f64,
                        grid_beats as f64,
                        boundary_tolerance_beats,
                    );
                    let deadline = ceil_to_grid(event_beats, grid_beats as f64);
                    state.push_or_replace(source_track, stage_idx, stage_idx + 1, deadline, event);
                    continue;
                }
                next.push(event);
                continue;
            }
            if debug_routing_enabled() {
                eprintln!(
                    "[midi-fx] invoke track={} step={} fx={} stage={} source={} chord={} note_spans={} offset={} step_beats={} transpose={} vel={} fx_params={} inst_params={} sampler_speed={}",
                    event.track,
                    event.step,
                    fx_name,
                    stage_idx,
                    event_source_label(&event.source),
                    event.chord.len(),
                    event
                        .note_spans
                        .as_ref()
                        .map(|spans| spans.len())
                        .unwrap_or(0),
                    event.offset_beats,
                    event.step_beats,
                    event.resolved.transpose,
                    event.resolved.velocity,
                    event.effect_params.len(),
                    event.instrument_params.len(),
                    event.sampler_params.playback_speed
                );
            }
            runtime.set_position(event.track, event.step);
            match runtime.invoke_midi_fx_with_arp_phase_beats(
                fx_idx,
                event.track,
                event.step,
                0.0,
                event.resolved,
                event.chord.clone(),
                event.chord_durations.clone(),
                event.chord_step_transpose,
                event.note_spans.clone(),
                slot_snapshot,
                event.arp_phase_beats,
                event.step_beats,
                snapshot.tracks[event.track].params.num_steps,
                snapshot.tracks[event.track].effect_slots.clone(),
                snapshot.tracks[event.track].instrument_slot.clone(),
                event.effect_params.clone(),
                event.instrument_params.to_vec(),
            ) {
                Ok(output) => {
                    if debug_routing_enabled() {
                        eprintln!(
                            "[midi-fx] output track={} step={} fx={} suppressed={} emitted={} resolved_transpose={} resolved_vel={} fx_params={} inst_params={}",
                            event.track,
                            event.step,
                            fx_name,
                            output.suppressed,
                            output.emitted.len(),
                            output.resolved.transpose,
                            output.resolved.velocity,
                            output.effect_params.len(),
                            output.instrument_params.len()
                        );
                    }
                    if !output.suppressed {
                        let mut passthrough = event.clone();
                        passthrough.resolved = output.resolved;
                        passthrough.effect_params = output.effect_params.clone();
                        passthrough.instrument_params =
                            scheduled_instrument_params_from_vec(output.instrument_params.clone());
                        next.push(passthrough);
                    }
                    for emitted in output.emitted {
                        let target_track = emitted.track.unwrap_or(event.track);
                        if target_track >= snapshot.tracks.len() {
                            if debug_routing_enabled() {
                                eprintln!(
                                    "[midi-fx] emitted drop reason=target-out-of-range fx={} source_track={} target_track={} tracks={}",
                                    fx_name,
                                    event.track,
                                    target_track,
                                    snapshot.tracks.len()
                                );
                            }
                            continue;
                        }
                        if debug_routing_enabled() {
                            eprintln!(
                                "[midi-fx] emitted fx={} from_track={} target_track={} offset={} transpose={} vel={} emitted_fx_params={} emitted_inst_params={}",
                                fx_name,
                                event.track,
                                target_track,
                                emitted.offset_beats,
                                emitted.resolved.transpose,
                                emitted.resolved.velocity,
                                emitted.effect_params.len(),
                                emitted.instrument_params.len()
                            );
                        }
                        let chord_len = emitted.chord.len();
                        let mut effect_params = emitted.effect_params;
                        let mut instrument_params =
                            scheduled_instrument_params_from_vec(emitted.instrument_params);
                        if target_track == event.track {
                            let explicit_effect_params = effect_params;
                            let explicit_instrument_params = instrument_params;
                            effect_params = event.effect_params.clone();
                            instrument_params = event.instrument_params.clone();
                            upsert_effect_params(&mut effect_params, explicit_effect_params);
                            upsert_instrument_params(
                                &mut instrument_params,
                                explicit_instrument_params,
                            );
                        }
                        let routed = MidiFxEvent {
                            offset_beats: event.offset_beats + emitted.offset_beats,
                            track: event.track,
                            step: event.step,
                            samples_per_step: event.samples_per_step,
                            step_beats: event.step_beats,
                            resolved: emitted.resolved,
                            chord: emitted.chord,
                            chord_durations: emitted.chord_durations,
                            chord_delays: vec![0.0; chord_len],
                            chord_step_transpose: emitted.chord_step_transpose,
                            note_spans: None,
                            arp_phase_beats: event.arp_phase_beats,
                            midi_fx_params: event.midi_fx_params.clone(),
                            effect_params,
                            instrument_params,
                            instrument_tensor_params: event.instrument_tensor_params.clone(),
                            sampler_params: event.sampler_params,
                            rack_macro_values: event.rack_macro_values,
                            source: event.source.clone(),
                        };
                        if target_track == source_track {
                            next.push(routed);
                        } else if visited_tracks.get(target_track).copied().unwrap_or(true) {
                            if debug_accum || debug_routing_enabled() {
                                eprintln!(
                                    "[midi-fx] dropped recursive emit track={source_track} target={target_track}"
                                );
                            }
                        } else if let Some(routed) =
                            rebind_midi_fx_event_to_track(snapshot, routed, target_track)
                        {
                            next.extend(run_midi_fx_chain_for_track_inner(
                                runtime,
                                snapshot,
                                target_track,
                                vec![routed],
                                quantizer_state.as_deref_mut(),
                                0,
                                depth + 1,
                                visited_tracks,
                                debug_accum,
                            ));
                        }
                    }
                }
                Err(err) => {
                    if debug_accum || debug_routing_enabled() {
                        eprintln!(
                            "[midi-fx] invoke err track={} step={} fx={} err={}",
                            event.track, event.step, fx_name, err
                        );
                    }
                    next.push(event);
                }
            }
            if next.len() > 1024 {
                if debug_routing_enabled() {
                    eprintln!(
                        "[midi-fx] truncate stage={} source_track={} len={} max=1024",
                        stage_idx,
                        source_track,
                        next.len()
                    );
                }
                next.truncate(1024);
                break;
            }
        }
        if debug_routing_enabled() {
            eprintln!(
                "[midi-fx] stage done source_track={} stage={} fx={} output_events={}",
                source_track,
                stage_idx,
                fx_name,
                next.len()
            );
        }
        current = next;
    }
    if debug_routing_enabled() {
        eprintln!(
            "[midi-fx] chain done source_track={} depth={} output_events={}",
            source_track,
            depth,
            current.len()
        );
    }
    current
}

pub(super) fn enqueue_midi_fx_events<const QUEUE_CAP: usize>(
    queue: &ScheduledEventQueue<QUEUE_CAP>,
    snapshot: &SequencerSnapshot,
    track_output_events: &mut Vec<TrackOutputEvent>,
    pattern_epoch: u64,
    base_sample_time: u64,
    base_beat: f64,
    samples_per_quarter: f32,
    global_transpose: f32,
    events: Vec<MidiFxEvent>,
) -> bool {
    let mut ok = true;
    for event in events {
        let sample_time = base_sample_time
            .saturating_add((event.offset_beats.max(0.0) * samples_per_quarter).round() as u64);
        let enqueue_track = event.track;
        let enqueue_sample_time = sample_time;
        let chord = chord_data_from_parts(
            &event.chord,
            &event.chord_durations,
            &event.chord_delays,
            event.resolved.duration,
            event.chord_step_transpose,
        );
        let instrument_fingerprint = instrument_sound_fingerprint(
            snapshot,
            event.track,
            &event.instrument_params,
            &event.instrument_tensor_params,
        );
        if debug_routing_enabled() {
            eprintln!(
                "[routing] enqueue source={} track={} step={} sample={} offset={} transpose={} vel={} chord={} fx_params={} inst_params={} sampler_speed={} fingerprint={}",
                event_source_label(&event.source),
                event.track,
                event.step,
                sample_time,
                event.offset_beats,
                event.resolved.transpose,
                event.resolved.velocity,
                chord.count,
                event.effect_params.len(),
                event.instrument_params.len(),
                event.sampler_params.playback_speed,
                instrument_fingerprint
            );
        }
        let enqueued = match event.source {
            EventSource::Network { seed, neuron, .. } => enqueue_network_trigger(
                queue,
                snapshot,
                track_output_events,
                pattern_epoch,
                sample_time,
                base_beat + event.offset_beats as f64,
                samples_per_quarter,
                global_transpose,
                event.track,
                neuron,
                seed,
                event.samples_per_step,
                event.resolved,
                chord,
                event.effect_params,
                event.instrument_params,
                event.instrument_tensor_params,
                event.sampler_params,
                instrument_fingerprint,
                event.rack_macro_values,
            ),
            EventSource::Step { .. } => enqueue_resolved_trigger(
                queue,
                snapshot,
                track_output_events,
                pattern_epoch,
                sample_time,
                base_beat + event.offset_beats as f64,
                samples_per_quarter,
                global_transpose,
                event.track,
                event.step,
                event.samples_per_step,
                event.resolved,
                chord,
                event.effect_params,
                event.instrument_params,
                event.instrument_tensor_params,
                event.sampler_params,
                event.rack_macro_values,
            ),
        };
        if !enqueued {
            if debug_routing_enabled() {
                eprintln!(
                    "[routing] enqueue failed track={} sample={} queue_capacity={}",
                    enqueue_track, enqueue_sample_time, QUEUE_CAP
                );
            }
            ok = false;
            break;
        }
    }
    ok
}

pub(super) fn drain_live_keyboard_inputs(
    live_keyboard_rx: &mpsc::Receiver<KeyboardTrigger>,
    snapshot: &SequencerSnapshot,
    rendered_sample: u64,
    live_tracks: &mut [LiveMidiFxTrackState; MAX_TRACKS],
) {
    while let Ok(trigger) = live_keyboard_rx.try_recv() {
        if trigger.track >= snapshot.tracks.len() || trigger.track >= MAX_TRACKS {
            continue;
        }
        let track_state = &mut live_tracks[trigger.track];
        if trigger.note_off {
            track_state
                .notes
                .retain(|note| note.transpose != trigger.transpose);
            if track_state.notes.is_empty() {
                track_state.next_tick_sample = 0;
                track_state.quantize_next_tick = false;
            }
            continue;
        }
        let was_empty = track_state.notes.is_empty();
        if let Some(note) = track_state
            .notes
            .iter_mut()
            .find(|note| note.transpose == trigger.transpose)
        {
            note.velocity = trigger.velocity;
            note.pending_event = true;
        } else {
            track_state.notes.push(LiveMidiFxNote {
                transpose: trigger.transpose,
                velocity: trigger.velocity,
                pending_event: true,
            });
        }
        if was_empty || track_state.next_tick_sample == 0 {
            track_state.next_tick_sample = rendered_sample;
            track_state.quantize_next_tick = true;
        }
    }
}

pub(super) fn any_live_midi_fx_notes(live_tracks: &[LiveMidiFxTrackState; MAX_TRACKS]) -> bool {
    live_tracks.iter().any(|track| !track.notes.is_empty())
}

pub(super) fn track_has_live_midi_fx_notes(
    live_tracks: &[LiveMidiFxTrackState; MAX_TRACKS],
    snapshot: &SequencerSnapshot,
    midi_fx_descriptors: &[EffectDescriptor],
    track_idx: usize,
) -> bool {
    let has_live_midi_fx_notes = track_idx < MAX_TRACKS
        && track_idx < snapshot.tracks.len()
        && !live_tracks[track_idx].notes.is_empty()
        && !snapshot.tracks[track_idx].params.midi_fx_chain.is_empty()
        && snapshot.tracks[track_idx].params.midi_fx_position == MidiFxPosition::PostAccumulator
        && midi_fx_chain_clock_param(snapshot, midi_fx_descriptors, track_idx).is_some();
    if has_live_midi_fx_notes && debug_routing_enabled() {
        eprintln!(
            "[routing] live-midi-fx owns track={} notes={} chain={:?}",
            track_idx,
            live_tracks[track_idx].notes.len(),
            snapshot.tracks[track_idx].params.midi_fx_chain
        );
    }
    has_live_midi_fx_notes
}

pub(super) fn quantized_live_tick_sample(
    rendered_sample: u64,
    rendered_total_beats: f64,
    live_tick_beats: f32,
    samples_per_quarter: f32,
) -> u64 {
    let beat_phase = rendered_total_beats.rem_euclid(live_tick_beats as f64);
    let beats_to_next_tick = if beat_phase <= 1e-6 {
        0.0
    } else {
        live_tick_beats as f64 - beat_phase
    };
    rendered_sample.saturating_add((beats_to_next_tick * samples_per_quarter as f64).round() as u64)
}

pub(super) fn schedule_live_midi_fx<const QUEUE_CAP: usize>(
    runtime: Option<&mut lisp_host::ScratchControlRuntime>,
    state: &SequencerState,
    snapshot: &SequencerSnapshot,
    queue: &ScheduledEventQueue<QUEUE_CAP>,
    pattern_epoch: u64,
    rendered_sample: u64,
    rendered_total_beats: f64,
    lookahead_samples: u64,
    sample_rate: u32,
    live_tracks: &mut [LiveMidiFxTrackState; MAX_TRACKS],
    debug_accum: bool,
) -> bool {
    let live_active = any_live_midi_fx_notes(live_tracks);
    let Some(runtime) = runtime else {
        if live_active && debug_routing_enabled() {
            eprintln!("[routing] skip live-midi-fx reason=no-scratch-runtime");
        }
        return live_active;
    };
    if snapshot.transport.bpm == 0 {
        return live_active;
    }
    let samples_per_quarter = sample_rate as f32 * 60.0 / snapshot.transport.bpm as f32;
    let horizon = rendered_sample.saturating_add(lookahead_samples);
    let midi_fx_descriptors = runtime.midi_fx_descriptors();
    let mut track_output_events = Vec::new();

    for track_idx in 0..snapshot.tracks.len().min(MAX_TRACKS) {
        if live_tracks[track_idx].notes.is_empty()
            || snapshot.tracks[track_idx].params.midi_fx_chain.is_empty()
            || snapshot.tracks[track_idx].params.midi_fx_position != MidiFxPosition::PostAccumulator
        {
            continue;
        }
        let num_steps = snapshot.tracks[track_idx].params.num_steps.max(1);
        let step = (state.transport.track_playheads[track_idx].load(Ordering::Relaxed) as usize)
            % num_steps;
        let Some(live_tick_beats) =
            midi_fx_clock_tick_beats(snapshot, &midi_fx_descriptors, track_idx, step)
        else {
            let pending_notes = live_tracks[track_idx]
                .notes
                .iter_mut()
                .filter_map(|note| {
                    if note.pending_event {
                        note.pending_event = false;
                        Some(*note)
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>();
            if pending_notes.is_empty() {
                continue;
            }
            let step_beats = snapshot.tracks[track_idx].steps[step]
                .timebase_override
                .unwrap_or(snapshot.tracks[track_idx].params.timebase)
                .step_beats(num_steps) as f32;
            let step_beats = step_beats.max(1.0 / 1024.0);
            let samples_per_step = (samples_per_quarter * step_beats).round().max(1.0);
            let chord = pending_notes
                .iter()
                .map(|note| note.transpose)
                .collect::<Vec<_>>();
            let chord_durations = vec![1.0; chord.len()];
            let chord_delays = vec![0.0; chord.len()];
            let note_spans = pending_notes
                .iter()
                .map(|note| AccumulatorNoteSpan {
                    transpose: note.transpose,
                    start_beats: 0.0,
                    end_beats: step_beats,
                })
                .collect::<Vec<_>>();
            let velocity = pending_notes
                .iter()
                .map(|note| note.velocity)
                .fold(0.0_f32, f32::max)
                .clamp(0.0, 1.0);
            let resolved = ResolvedStep {
                duration: 1.0,
                velocity,
                speed: 1.0,
                aux_a: 0.0,
                aux_b: 0.0,
                transpose: chord[0],
                pan: 0.0,
                chop: 1.0,
            };
            let event = MidiFxEvent {
                offset_beats: 0.0,
                track: track_idx,
                step,
                samples_per_step,
                step_beats,
                resolved,
                chord,
                chord_durations,
                chord_delays,
                chord_step_transpose: 0.0,
                note_spans: Some(note_spans),
                arp_phase_beats: rendered_total_beats as f32,
                midi_fx_params: Vec::new(),
                effect_params: resolve_effect_params(snapshot, track_idx, step),
                instrument_params: resolve_instrument_params(snapshot, track_idx, step),
                instrument_tensor_params: resolve_instrument_tensor_params(
                    snapshot, track_idx, step,
                ),
                sampler_params: resolve_sampler_params(snapshot, track_idx, step),
                rack_macro_values: [None; crate::sequencer::RACK_MACRO_COUNT],
                source: EventSource::Step {
                    track: track_idx,
                    step,
                    instrument_fingerprint: 0,
                },
            };
            let events = run_midi_fx_chain_for_track(
                runtime,
                snapshot,
                track_idx,
                vec![event],
                None,
                0,
                debug_accum,
            );
            if !enqueue_midi_fx_events(
                queue,
                snapshot,
                &mut track_output_events,
                pattern_epoch,
                rendered_sample,
                rendered_total_beats,
                samples_per_quarter,
                0.0,
                events,
            ) {
                break;
            }
            continue;
        };
        for note in &mut live_tracks[track_idx].notes {
            note.pending_event = false;
        }
        if live_tracks[track_idx].next_tick_sample < rendered_sample {
            live_tracks[track_idx].next_tick_sample = rendered_sample;
        }
        while live_tracks[track_idx].next_tick_sample < horizon {
            let notes = live_tracks[track_idx].notes.clone();
            if notes.is_empty() {
                break;
            }
            let live_tick_samples = (samples_per_quarter * live_tick_beats).round().max(1.0) as u64;
            if live_tracks[track_idx].quantize_next_tick {
                live_tracks[track_idx].next_tick_sample = quantized_live_tick_sample(
                    rendered_sample,
                    rendered_total_beats,
                    live_tick_beats,
                    samples_per_quarter,
                );
                live_tracks[track_idx].quantize_next_tick = false;
            }
            let track_boundaries = track_step_boundaries(&snapshot.tracks[track_idx]);
            let cycle_beats = track_boundaries
                .get(snapshot.tracks[track_idx].params.num_steps)
                .copied()
                .unwrap_or(live_tick_beats)
                .max(live_tick_beats) as f64;
            let tick_offset_beats = live_tracks[track_idx]
                .next_tick_sample
                .saturating_sub(rendered_sample) as f64
                / samples_per_quarter as f64;
            let track_position_beats =
                ((rendered_total_beats + tick_offset_beats) % cycle_beats) as f32;
            let velocity = notes
                .iter()
                .map(|note| note.velocity)
                .fold(0.0_f32, f32::max)
                .clamp(0.0, 1.0);
            let mut note_spans = track_active_note_spans_at_beat(
                snapshot,
                track_idx,
                track_position_beats,
                live_tick_beats,
            );
            let mut chord = note_spans
                .iter()
                .map(|note| note.transpose)
                .collect::<Vec<_>>();
            let live_spans = notes
                .iter()
                .map(|note| AccumulatorNoteSpan {
                    transpose: note.transpose,
                    start_beats: 0.0,
                    end_beats: live_tick_beats,
                })
                .collect::<Vec<_>>();
            chord.extend(live_spans.iter().map(|note| note.transpose));
            note_spans.extend(live_spans);
            if chord.is_empty() {
                break;
            }
            let chord_durations = vec![1.0; chord.len()];
            let chord_delays = vec![0.0; chord.len()];
            let first_transpose = chord[0];
            let resolved = ResolvedStep {
                duration: 1.0,
                velocity,
                speed: 1.0,
                aux_a: 0.0,
                aux_b: 0.0,
                transpose: first_transpose,
                pan: 0.0,
                chop: 1.0,
            };
            let event = MidiFxEvent {
                offset_beats: 0.0,
                track: track_idx,
                step,
                samples_per_step: live_tick_samples as f32,
                step_beats: live_tick_beats,
                resolved,
                chord,
                chord_durations,
                chord_delays,
                chord_step_transpose: 0.0,
                note_spans: Some(note_spans),
                arp_phase_beats: (rendered_total_beats + tick_offset_beats) as f32,
                midi_fx_params: Vec::new(),
                effect_params: resolve_effect_params(snapshot, track_idx, step),
                instrument_params: resolve_instrument_params(snapshot, track_idx, step),
                instrument_tensor_params: resolve_instrument_tensor_params(
                    snapshot, track_idx, step,
                ),
                sampler_params: resolve_sampler_params(snapshot, track_idx, step),
                rack_macro_values: [None; crate::sequencer::RACK_MACRO_COUNT],
                source: EventSource::Step {
                    track: track_idx,
                    step,
                    instrument_fingerprint: 0,
                },
            };
            let events = run_midi_fx_chain_for_track(
                runtime,
                snapshot,
                track_idx,
                vec![event],
                None,
                0,
                debug_accum,
            );
            if !enqueue_midi_fx_events(
                queue,
                snapshot,
                &mut track_output_events,
                pattern_epoch,
                live_tracks[track_idx].next_tick_sample,
                rendered_total_beats + tick_offset_beats,
                samples_per_quarter,
                0.0,
                events,
            ) {
                break;
            }
            live_tracks[track_idx].next_tick_sample = live_tracks[track_idx]
                .next_tick_sample
                .saturating_add(live_tick_samples);
        }
    }

    state.set_track_output_current_beat(rendered_total_beats);
    state.append_track_output_events(track_output_events);
    live_active
}

pub(super) fn sample_time_to_beats(
    chunk_start_beats: f64,
    chunk_start_sample: u64,
    sample_time: u64,
    samples_per_quarter: f64,
) -> f64 {
    let sample_delta = sample_time.saturating_sub(chunk_start_sample) as f64;
    chunk_start_beats + sample_delta / samples_per_quarter.max(1.0)
}

pub(super) fn process_neural_boundaries_until(
    neural_runtime: &mut NeuralRuntime,
    cursor_beats: &mut f64,
    cursor_sample: &mut u64,
    target_beats: f64,
    target_sample: u64,
    samples_per_quarter: f64,
    out: &mut Vec<NeuralOutput>,
) {
    if target_beats <= *cursor_beats + 1e-9 {
        return;
    }
    neural_runtime.process_boundaries_with_outputs(
        *cursor_beats,
        target_beats,
        *cursor_sample,
        samples_per_quarter,
        out,
    );
    *cursor_beats = target_beats;
    *cursor_sample = target_sample;
}

pub(super) fn should_reload_neural_runtime(
    loaded_networks: &Option<Vec<crate::neural::ProjectNeuralNetwork>>,
    snapshot_networks: &[crate::neural::ProjectNeuralNetwork],
    last_pattern: usize,
    pattern: usize,
) -> bool {
    last_pattern != pattern
        || loaded_networks
            .as_deref()
            .map(|networks| networks != snapshot_networks)
            .unwrap_or(true)
}
