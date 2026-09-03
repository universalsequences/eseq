/*!
Callback-side event machinery: the consumer end of the scheduler queue.

Owns the deferred-event types (gate-off, retrig, countdown, block) and their
queues. Scheduled events from the scheduler thread are enqueued per block,
counted down sample-accurately, promoted to block events, prioritized (with
mute-group arbitration), and dispatched — ultimately into `fire_resolved` /
`dispatch_retrig_event` / gate-off handling. Also computes swing delays and
cancels pending events when tracks or voices are retired.
*/

#[allow(unused_imports)]
use super::*;

#[derive(Clone, Copy, Debug)]
pub(super) enum GateOffTarget {
    Custom { engine_id: usize, free_patch: bool },
    Sampler { gatepitch_id: i32 },
}

#[derive(Clone, Copy, Debug)]
pub(super) struct GateOffEvent {
    pub(super) track_idx: usize,
    pub(super) logical_id: u64,
    pub(super) target: GateOffTarget,
}

/// One custom (dgen) voice a retrig burst re-excites. The burst re-fires the
/// *same* logical voice rather than allocating a new one, so the running patch
/// restarts its envelopes instead of being voice-stolen mid-roll.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct RetrigCustomVoice {
    pub(super) logical_id: u64,
    pub(super) pitch_hz: f32,
    pub(super) velocity: f32,
}

/// How a retrig repeat re-fires the track.
#[derive(Clone, Copy, Debug)]
pub(super) enum RetrigTarget {
    /// Sampler and modulator tracks: the repeat re-allocates from the step's
    /// stored parameters (the path the retired `Chop` param used).
    Step,
    /// Custom (dgen) tracks: re-trigger the logical voices the initial hit
    /// allocated, with the gate left held.
    Custom {
        voices: [RetrigCustomVoice; MAX_VOICES],
        count: usize,
        /// Pool the voices came from, so each repeat can re-arm its gate-off.
        engine_id: usize,
        free_patch: bool,
        /// Track gate mode at the initial hit. When set, every repeat closes
        /// its gate after `RetrigEvent::gate` samples; otherwise the patch
        /// owns its own envelope and repeats only re-pulse the trigger.
        gated: bool,
    },
}

/// A single repeat of a step's retrig burst (Machinedrum RTRG/RTIM). See
/// `docs/step-retrig-spec.md`.
#[derive(Clone, Copy, Debug)]
pub(super) struct RetrigEvent {
    pub(super) track_idx: usize,
    pub(super) step: usize,
    /// Gate length for this hit, in samples: `min(interval, step duration)`,
    /// so repeats butt together like the MD's restarted amp envelope.
    pub(super) gate: f32,
    pub(super) target: RetrigTarget,
}

#[derive(Debug)]
pub(super) enum CountdownEventKind {
    Scheduled(ScheduledEvent),
    GateOff(GateOffEvent),
    Retrig(RetrigEvent),
}

#[derive(Debug)]
pub(super) struct CountdownEvent {
    pub(super) remaining_samples: f64,
    pub(super) period_samples: f64,
    pub(super) repeats: u32,
    pub(super) pattern_epoch: u64,
    pub(super) seq: u64,
    pub(super) kind: CountdownEventKind,
}

#[derive(Debug)]
pub(super) enum BlockEventKind {
    Scheduled(ScheduledEvent),
    GateOff(GateOffEvent),
    Retrig(RetrigEvent),
}

#[derive(Debug)]
pub(super) struct BlockEvent {
    pub(super) frame_offset: u32,
    pub(super) seq: u64,
    pub(super) kind: BlockEventKind,
}

pub(super) fn swing_delay_samples(
    sample_rate: f64,
    bpm: f64,
    swing_pct: f32,
    resolution: SwingResolution,
) -> f64 {
    let samples_per_quarter = sample_rate * 60.0 / bpm;
    let resolution_samples = resolution.step_beats() * samples_per_quarter;
    ((swing_pct as f64 / 100.0) - 0.5) * 2.0 * resolution_samples
}

pub(super) fn cancel_gate_off_for_lid(
    countdown_events: &mut Vec<CountdownEvent>,
    block_events: &mut Vec<BlockEvent>,
    lid: u64,
) {
    countdown_events.retain(|event| {
        !matches!(
            event.kind,
            CountdownEventKind::GateOff(GateOffEvent { logical_id, .. }) if logical_id == lid
        )
    });
    block_events.retain(|event| {
        !matches!(
            event.kind,
            BlockEventKind::GateOff(GateOffEvent { logical_id, .. }) if logical_id == lid
        )
    });
}

pub(super) fn cancel_retrigs_for_track(
    countdown_events: &mut Vec<CountdownEvent>,
    block_events: &mut Vec<BlockEvent>,
    track_idx: usize,
) {
    countdown_events.retain(|event| {
        !matches!(
            event.kind,
            CountdownEventKind::Retrig(RetrigEvent { track_idx: event_track, .. }) if event_track == track_idx
        )
    });
    block_events.retain(|event| {
        !matches!(
            event.kind,
            BlockEventKind::Retrig(RetrigEvent { track_idx: event_track, .. }) if event_track == track_idx
        )
    });
}

pub(super) fn schedule_gate_off_event(
    data: &mut AudioCallbackData,
    track_idx: usize,
    logical_id: u64,
    source_frame_offset: u32,
    delay_samples: f64,
    target: GateOffTarget,
) {
    cancel_gate_off_for_lid(
        &mut data.countdown_events,
        &mut data.block_events,
        logical_id,
    );
    let event_offset = source_frame_offset as f64 + delay_samples.max(0.0);
    schedule_countdown_or_block_event(
        data,
        event_offset,
        0.0,
        1,
        data.scheduler_snapshot.transport.pattern_epoch,
        CountdownEventKind::GateOff(GateOffEvent {
            track_idx,
            logical_id,
            target,
        }),
    );
}

/// Arm a step's retrig burst. Always cancels the track's pending burst first,
/// which is the Machinedrum "until the next trig on this track" rule.
/// `repeats == u32::MAX` is the infinite (RTRG 127) case.
pub(super) fn schedule_retrig_events(
    data: &mut AudioCallbackData,
    track_idx: usize,
    source_frame_offset: u32,
    first_delay_samples: f64,
    interval_samples: f64,
    repeats: u32,
    step: usize,
    gate: f32,
    target: RetrigTarget,
) {
    cancel_retrigs_for_track(
        &mut data.countdown_events,
        &mut data.block_events,
        track_idx,
    );
    if repeats == 0 {
        return;
    }
    schedule_countdown_or_block_event(
        data,
        source_frame_offset as f64 + first_delay_samples.max(0.0),
        interval_samples.max(1.0),
        repeats,
        data.scheduler_snapshot.transport.pattern_epoch,
        CountdownEventKind::Retrig(RetrigEvent {
            track_idx,
            step,
            gate,
            target,
        }),
    );
}

pub(super) fn dispatch_gate_off_event(
    data: &mut AudioCallbackData,
    event: GateOffEvent,
    frame_offset: u32,
    block_start_sample: u64,
) {
    match event.target {
        GateOffTarget::Custom {
            engine_id,
            free_patch,
        } => {
            if engine_id >= data.custom_engine_pools.len() {
                return;
            }
            if free_patch {
                data.custom_engine_pools[engine_id]
                    .release_free_patch_voice_by_logical_id(event.logical_id);
            } else {
                data.custom_engine_pools[engine_id].release_voice_by_logical_id(
                    event.logical_id,
                    block_start_sample + frame_offset as u64,
                );
            }
            let seq = next_block_event_sequence(data);
            unsafe {
                send_custom_note_off(data.lg.0, event.logical_id, frame_offset, seq);
            }
        }
        GateOffTarget::Sampler { gatepitch_id } => {
            if gatepitch_id > 0 {
                let seq = next_block_event_sequence(data);
                unsafe {
                    send_custom_note_off(data.lg.0, gatepitch_id as u64, frame_offset, seq);
                }
            }
            if event.track_idx >= data.voice_pools.len() {
                return;
            }
            data.voice_pools[event.track_idx].release_voice_by_logical_id(event.logical_id);
            let seq = next_block_event_sequence(data);
            unsafe {
                send_sampler_note_off(data.lg.0, event.logical_id, frame_offset, seq);
            }
        }
    }
}

pub(super) fn dispatch_scheduled_step(
    data: &mut AudioCallbackData,
    frame_offset: u32,
    track_idx: usize,
    step: usize,
    samples_per_step: f32,
    resolved: crate::accumulator::ResolvedStep,
    chord: crate::scheduled_event::ScheduledChordData,
    mut effect_params: Vec<ScheduledEffectParam>,
    instrument_params: ScheduledInstrumentParams,
    instrument_tensor_params: ScheduledInstrumentTensorParams,
    sampler_params: ScheduledSamplerParams,
    instrument_fingerprint: u64,
    rack_macro_values: [Option<f32>; crate::sequencer::RACK_MACRO_COUNT],
) {
    unsafe {
        dispatch_effect_chain_for_track(data.lg.0, &mut effect_params);
    }
    fire_resolved(
        data,
        frame_offset,
        track_idx,
        step,
        Some(step),
        samples_per_step as f64,
        resolved,
        chord,
        instrument_params,
        instrument_tensor_params,
        instrument_fingerprint,
        Some(sampler_params),
        rack_macro_values,
    );
}

pub(super) fn dispatch_scheduled_network_step(
    data: &mut AudioCallbackData,
    frame_offset: u32,
    track_idx: usize,
    key_lock_plock_step: Option<usize>,
    samples_per_step: f32,
    resolved: crate::accumulator::ResolvedStep,
    chord: crate::scheduled_event::ScheduledChordData,
    mut effect_params: Vec<ScheduledEffectParam>,
    instrument_params: ScheduledInstrumentParams,
    instrument_tensor_params: ScheduledInstrumentTensorParams,
    sampler_params: ScheduledSamplerParams,
    instrument_fingerprint: u64,
    rack_macro_values: [Option<f32>; crate::sequencer::RACK_MACRO_COUNT],
) {
    unsafe {
        dispatch_effect_chain_for_track(data.lg.0, &mut effect_params);
    }
    fire_resolved(
        data,
        frame_offset,
        track_idx,
        0,
        key_lock_plock_step,
        samples_per_step as f64,
        resolved,
        chord,
        instrument_params,
        instrument_tensor_params,
        instrument_fingerprint,
        Some(sampler_params),
        rack_macro_values,
    );
}

pub(super) fn dispatch_scheduled_event(
    data: &mut AudioCallbackData,
    event: ScheduledEvent,
    frame_offset: u32,
) {
    match event.kind {
        ScheduledEventKind::ResolvedTrigger {
            track,
            step,
            samples_per_step,
            resolved,
            chord,
            effect_params,
            instrument_params,
            instrument_tensor_params,
            sampler_params,
            instrument_fingerprint,
            rack_macro_values,
        } => {
            dispatch_scheduled_step(
                data,
                frame_offset,
                track,
                step,
                samples_per_step,
                resolved,
                chord,
                effect_params,
                instrument_params,
                instrument_tensor_params,
                sampler_params,
                instrument_fingerprint,
                rack_macro_values,
            );
        }
        ScheduledEventKind::InstrumentParams {
            track,
            instrument_params,
            instrument_tensor_params,
        } => {
            dispatch_instrument_params_to_active_voices(data, track, &instrument_params);
            dispatch_instrument_tensor_params_to_active_voices(
                data,
                track,
                &instrument_tensor_params,
            );
        }
        ScheduledEventKind::EffectParams {
            mut effect_params, ..
        } => unsafe {
            dispatch_effect_chain_for_track(data.lg.0, &mut effect_params);
        },
        ScheduledEventKind::NetworkTrigger {
            track,
            samples_per_step,
            resolved,
            chord,
            effect_params,
            instrument_params,
            instrument_tensor_params,
            sampler_params,
            instrument_fingerprint,
            rack_macro_values,
            seed,
            ..
        } => {
            dispatch_scheduled_network_step(
                data,
                frame_offset,
                track,
                seed.map(|(_, step)| step),
                samples_per_step,
                resolved,
                chord,
                effect_params,
                instrument_params,
                instrument_tensor_params,
                sampler_params,
                instrument_fingerprint,
                rack_macro_values,
            );
        }
    }
}

pub(super) fn scheduled_trigger_track(event: &ScheduledEvent) -> Option<usize> {
    match &event.kind {
        ScheduledEventKind::ResolvedTrigger { track, .. }
        | ScheduledEventKind::NetworkTrigger { track, .. } => Some(*track),
        ScheduledEventKind::InstrumentParams { .. } | ScheduledEventKind::EffectParams { .. } => {
            None
        }
    }
}

pub(super) fn frame_offset_from_remaining(remaining_samples: f64, nframes: usize) -> u32 {
    remaining_samples
        .floor()
        .max(0.0)
        .min(nframes.saturating_sub(1) as f64) as u32
}

pub(super) fn block_event_priority(kind: &BlockEventKind) -> u8 {
    match kind {
        BlockEventKind::GateOff(_) => 0,
        BlockEventKind::Scheduled(ScheduledEvent {
            kind:
                ScheduledEventKind::InstrumentParams { .. } | ScheduledEventKind::EffectParams { .. },
            ..
        }) => 1,
        BlockEventKind::Scheduled(_) | BlockEventKind::Retrig(_) => 2,
    }
}

pub(super) fn try_push_block_event(
    data: &mut AudioCallbackData,
    frame_offset: u32,
    seq: u64,
    kind: BlockEventKind,
) {
    if data.block_events.len() >= SCHEDULED_BLOCK_SCRATCH_CAPACITY {
        data.dropped_scheduled_events = data.dropped_scheduled_events.saturating_add(1);
        if data.trace_audio {
            eprintln!(
                "audio-trace: dropped countdown event; block scratch full capacity={SCHEDULED_BLOCK_SCRATCH_CAPACITY}"
            );
        }
        return;
    }
    data.block_events.push(BlockEvent {
        frame_offset,
        seq,
        kind,
    });
    data.block_events_need_sort = true;
}

pub(super) fn try_push_countdown_event(
    data: &mut AudioCallbackData,
    remaining_samples: f64,
    period_samples: f64,
    repeats: u32,
    pattern_epoch: u64,
    seq: u64,
    kind: CountdownEventKind,
) {
    if data.countdown_events.len() >= SCHEDULED_COUNTDOWN_CAPACITY {
        data.dropped_scheduled_events = data.dropped_scheduled_events.saturating_add(1);
        if data.trace_audio {
            eprintln!(
                "audio-trace: dropped countdown event; countdown pool full capacity={SCHEDULED_COUNTDOWN_CAPACITY}"
            );
        }
        return;
    }
    data.countdown_events.push(CountdownEvent {
        remaining_samples,
        period_samples,
        repeats,
        pattern_epoch,
        seq,
        kind,
    });
}

/// Walk a repeating burst across one block of `nframes`, calling `emit` with
/// each in-block frame offset. Returns the leftover `(offset relative to the
/// next block, remaining repeats)` when the burst outlives this block, or
/// `None` when it finished inside it. `repeats == u32::MAX` is the infinite
/// (RTRG 127) burst: it always has a leftover, so it keeps rolling until
/// something cancels it. Allocation-free, so it is safe on the audio thread.
pub(super) fn walk_repeating_offsets(
    first_offset: f64,
    period_samples: f64,
    repeats: u32,
    nframes: usize,
    mut emit: impl FnMut(u32),
) -> Option<(f64, u32)> {
    let period = period_samples.max(1.0);
    let mut next_offset = first_offset;
    let mut remaining = repeats;
    while remaining > 0 && next_offset < nframes as f64 {
        emit(frame_offset_from_remaining(next_offset, nframes));
        remaining -= 1;
        next_offset += period;
    }
    (remaining > 0).then_some((next_offset - nframes as f64, remaining))
}

pub(super) fn schedule_countdown_or_block_event(
    data: &mut AudioCallbackData,
    event_offset: f64,
    period_samples: f64,
    repeats: u32,
    pattern_epoch: u64,
    kind: CountdownEventKind,
) {
    if repeats == 0 {
        return;
    }
    let nframes = data.current_callback_nframes;
    match kind {
        CountdownEventKind::Scheduled(event) => {
            let seq = data.event_seq;
            data.event_seq = data.event_seq.wrapping_add(1);
            if event_offset < nframes as f64 {
                let frame_offset = frame_offset_from_remaining(event_offset, nframes);
                try_push_block_event(data, frame_offset, seq, BlockEventKind::Scheduled(event));
            } else {
                try_push_countdown_event(
                    data,
                    event_offset - nframes as f64,
                    0.0,
                    1,
                    pattern_epoch,
                    seq,
                    CountdownEventKind::Scheduled(event),
                );
            }
        }
        CountdownEventKind::GateOff(event) => {
            let seq = data.event_seq;
            data.event_seq = data.event_seq.wrapping_add(1);
            if event_offset < nframes as f64 {
                let frame_offset = frame_offset_from_remaining(event_offset, nframes);
                try_push_block_event(data, frame_offset, seq, BlockEventKind::GateOff(event));
            } else {
                try_push_countdown_event(
                    data,
                    event_offset - nframes as f64,
                    0.0,
                    1,
                    pattern_epoch,
                    seq,
                    CountdownEventKind::GateOff(event),
                );
            }
        }
        CountdownEventKind::Retrig(event) => {
            let leftover = walk_repeating_offsets(
                event_offset,
                period_samples.max(1.0),
                repeats,
                nframes,
                |frame_offset| {
                    let seq = data.event_seq;
                    data.event_seq = data.event_seq.wrapping_add(1);
                    try_push_block_event(data, frame_offset, seq, BlockEventKind::Retrig(event));
                },
            );
            if let Some((next_offset, remaining_repeats)) = leftover {
                let seq = data.event_seq;
                data.event_seq = data.event_seq.wrapping_add(1);
                try_push_countdown_event(
                    data,
                    next_offset,
                    period_samples.max(1.0),
                    remaining_repeats,
                    pattern_epoch,
                    seq,
                    CountdownEventKind::Retrig(event),
                );
            }
        }
    }
}

pub(super) fn enqueue_scheduled_event_for_callback(
    data: &mut AudioCallbackData,
    event: ScheduledEvent,
    block_start_sample: u64,
    nframes: usize,
    current_pattern_epoch: u64,
) {
    if event.pattern_epoch != current_pattern_epoch {
        return;
    }
    let seq = data.event_seq;
    data.event_seq = data.event_seq.wrapping_add(1);
    let remaining_samples = if event.sample_time >= block_start_sample {
        (event.sample_time - block_start_sample) as f64
    } else {
        data.late_scheduled_events = data.late_scheduled_events.saturating_add(1);
        0.0
    };
    if remaining_samples < nframes as f64 {
        let frame_offset = frame_offset_from_remaining(remaining_samples, nframes);
        try_push_block_event(data, frame_offset, seq, BlockEventKind::Scheduled(event));
    } else {
        try_push_countdown_event(
            data,
            remaining_samples - nframes as f64,
            0.0,
            1,
            event.pattern_epoch,
            seq,
            CountdownEventKind::Scheduled(event),
        );
    }
}

pub(super) fn drain_scheduled_events_for_callback(
    data: &mut AudioCallbackData,
    block_start_sample: u64,
    nframes: usize,
    current_pattern_epoch: u64,
) {
    while let Some(event) = data.scheduled_events.pop() {
        enqueue_scheduled_event_for_callback(
            data,
            event,
            block_start_sample,
            nframes,
            current_pattern_epoch,
        );
    }
}

pub(super) fn collect_due_countdown_events(
    data: &mut AudioCallbackData,
    nframes: usize,
    current_pattern_epoch: u64,
) {
    let block_len = nframes as f64;
    let mut i = 0usize;
    while i < data.countdown_events.len() {
        let stale = match data.countdown_events[i].kind {
            CountdownEventKind::GateOff(_) => false,
            CountdownEventKind::Scheduled(_) | CountdownEventKind::Retrig(_) => {
                data.countdown_events[i].pattern_epoch != current_pattern_epoch
            }
        };
        if stale {
            data.countdown_events.swap_remove(i);
            continue;
        }
        if data.countdown_events[i].remaining_samples < block_len {
            let mut due = data.countdown_events.swap_remove(i);
            match due.kind {
                CountdownEventKind::Retrig(event) => {
                    let seq = &mut due.seq;
                    let leftover = walk_repeating_offsets(
                        due.remaining_samples,
                        due.period_samples,
                        due.repeats,
                        nframes,
                        |frame_offset| {
                            try_push_block_event(
                                data,
                                frame_offset,
                                *seq,
                                BlockEventKind::Retrig(event),
                            );
                            *seq = data.event_seq;
                            data.event_seq = data.event_seq.wrapping_add(1);
                        },
                    );
                    if let Some((next_offset, remaining_repeats)) = leftover {
                        due.remaining_samples = next_offset;
                        due.repeats = remaining_repeats;
                        data.countdown_events.push(due);
                    }
                }
                CountdownEventKind::Scheduled(event) => {
                    let frame_offset = frame_offset_from_remaining(due.remaining_samples, nframes);
                    try_push_block_event(
                        data,
                        frame_offset,
                        due.seq,
                        BlockEventKind::Scheduled(event),
                    );
                }
                CountdownEventKind::GateOff(event) => {
                    let frame_offset = frame_offset_from_remaining(due.remaining_samples, nframes);
                    try_push_block_event(
                        data,
                        frame_offset,
                        due.seq,
                        BlockEventKind::GateOff(event),
                    );
                }
            }
            continue;
        }
        data.countdown_events[i].remaining_samples -= block_len;
        i += 1;
    }
}

pub(super) fn clear_countdown_events(data: &mut AudioCallbackData) {
    data.countdown_events.clear();
    data.block_events.clear();
    data.block_events_need_sort = false;
}

pub(super) fn clear_transport_countdown_events(data: &mut AudioCallbackData) {
    data.countdown_events
        .retain(|event| matches!(event.kind, CountdownEventKind::GateOff(_)));
    data.block_events
        .retain(|event| matches!(event.kind, BlockEventKind::GateOff(_)));
    data.block_events_need_sort = true;
}

pub(super) fn mute_group_winner_for_block_events(
    track: usize,
    group: u8,
    batch: &[BlockEvent],
    track_mute_groups: impl Fn(usize) -> u8,
) -> usize {
    batch
        .iter()
        .filter_map(|event| match &event.kind {
            BlockEventKind::Scheduled(scheduled) => scheduled_trigger_track(scheduled),
            BlockEventKind::GateOff(_) | BlockEventKind::Retrig(_) => None,
        })
        .filter(|&candidate| track_mute_groups(candidate) == group)
        .max()
        .unwrap_or(track)
}

pub(super) fn dispatch_block_events(data: &mut AudioCallbackData, block_start_sample: u64) {
    while !data.block_events.is_empty() {
        if data.block_events_need_sort {
            data.block_events.sort_unstable_by(|a, b| {
                (b.frame_offset, block_event_priority(&b.kind), b.seq).cmp(&(
                    a.frame_offset,
                    block_event_priority(&a.kind),
                    a.seq,
                ))
            });
            data.block_events_need_sort = false;
        }

        let Some(frame_offset) = data.block_events.last().map(|event| event.frame_offset) else {
            break;
        };
        let mut group_start = data.block_events.len();
        while group_start > 0 && data.block_events[group_start - 1].frame_offset == frame_offset {
            group_start -= 1;
        }

        let mut winning_group_tracks = [false; MAX_TRACKS];
        {
            let group = &data.block_events[group_start..];
            for event in group {
                let Some(track) = (match &event.kind {
                    BlockEventKind::Scheduled(scheduled) => scheduled_trigger_track(scheduled),
                    BlockEventKind::GateOff(_) | BlockEventKind::Retrig(_) => None,
                }) else {
                    continue;
                };
                if track >= data.state.active_track_count() {
                    continue;
                }
                let group_id = data.state.pattern.track_params[track].get_mute_group();
                if group_id == 0 {
                    continue;
                }
                let winner =
                    mute_group_winner_for_block_events(track, group_id, group, |candidate| {
                        data.state
                            .pattern
                            .track_params
                            .get(candidate)
                            .map(|params| params.get_mute_group())
                            .unwrap_or(0)
                    });
                if winner < MAX_TRACKS {
                    winning_group_tracks[winner] = true;
                }
            }
        }

        let release_sample = block_start_sample + frame_offset as u64;
        for (track, is_winner) in winning_group_tracks.iter().copied().enumerate() {
            if is_winner {
                enforce_mute_group_for_winning_track(data, track, release_sample, frame_offset);
            }
        }

        while data
            .block_events
            .last()
            .is_some_and(|event| event.frame_offset == frame_offset)
        {
            let event = data.block_events.pop().unwrap();
            match event.kind {
                BlockEventKind::Scheduled(scheduled) => {
                    let dispatch = match scheduled_trigger_track(&scheduled) {
                        Some(track) if track < data.state.active_track_count() => {
                            let group = data.state.pattern.track_params[track].get_mute_group();
                            group == 0 || winning_group_tracks[track]
                        }
                        Some(_) => false,
                        None => true,
                    };
                    if dispatch {
                        dispatch_scheduled_event(data, scheduled, frame_offset);
                    }
                }
                BlockEventKind::GateOff(gate_off) => {
                    dispatch_gate_off_event(data, gate_off, frame_offset, block_start_sample);
                }
                BlockEventKind::Retrig(retrig) => {
                    dispatch_retrig_event(data, retrig, frame_offset);
                }
            }
        }
    }
}
