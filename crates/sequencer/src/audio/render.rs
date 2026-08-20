/*!
Block rendering and per-block housekeeping around it.

`render_chunk` asks the graph for the next block; around it live the
metronome mix-in, interleaved peak metering, snapshot effect-param dispatch
at step boundaries, and the host transport-clock broadcasts to instruments,
effect modulators, and the DJ mixer.
*/

#[allow(unused_imports)]
use super::*;

pub(super) fn render_chunk(data: &mut AudioCallbackData, output: &mut [f32]) {
    if output.is_empty() {
        return;
    }
    let nframes = output.len() / data.num_channels;
    if nframes == 0 {
        return;
    }
    publish_sampler_modulator_activity(data);
    unsafe {
        data.lg
            .process_next_block(output.as_mut_ptr(), nframes as i32);
    }
}

/// Mix the transport click after recorder capture. This intentionally keeps
/// exported master WAVs and all upstream per-track capture paths click-free.
pub(super) fn mix_metronome(
    metronome: &mut MetronomeState,
    output: &mut [f32],
    num_channels: usize,
    sample_rate: f64,
    block_start_beats: f64,
    bpm: f64,
) {
    if output.is_empty() || num_channels == 0 || bpm <= 0.0 {
        return;
    }
    let nframes = output.len() / num_channels;
    let beats_per_sample = bpm / (sample_rate * 60.0);
    let block_end_beats = block_start_beats + nframes as f64 * beats_per_sample;
    let first_quarter = (block_start_beats - 1.0e-9).ceil().max(0.0) as u64;
    let mut next_quarter = first_quarter;

    for frame in 0..nframes {
        let beat = block_start_beats + frame as f64 * beats_per_sample;
        while (next_quarter as f64) <= beat + 1.0e-9
            && (next_quarter as f64) < block_end_beats + 1.0e-9
        {
            metronome.trigger(sample_rate, next_quarter % 4 == 0);
            next_quarter += 1;
        }
        let click = metronome.sample(sample_rate);
        if click != 0.0 {
            for channel in 0..num_channels {
                output[frame * num_channels + channel] += click;
            }
        }
    }
}

pub(super) fn publish_sampler_modulator_activity(data: &AudioCallbackData) {
    // Covers both per-track pools (0..MAX_TRACKS) and per-rack-slot pools
    // (rack_slot_pool_index, >= MAX_TRACKS) — previously capped at
    // MAX_TRACKS, which left every rack slot's mask permanently zero and
    // forced its voice_modulator nodes through an O(nframes) gate-timeline
    // scan every block instead of the O(1) active-mask check.
    for (pool_id, pool) in data.voice_pools.iter().enumerate() {
        if pool.num_voices == 0 {
            continue;
        }
        let mut mask = 0u64;
        for voice_idx in 0..pool.num_voices.min(MAX_VOICES) {
            if pool.voices[voice_idx].active {
                mask |= 1u64 << voice_idx;
            }
        }
        crate::instruments::voice_modulator::set_sampler_active_mask(pool_id, mask);
    }
}

pub(super) unsafe fn dispatch_snapshot_effect_params_at_step(
    lg: *mut LiveGraph,
    effect_slots: &[EffectSlotSnapshot],
    step: usize,
) {
    for slot in effect_slots {
        if slot.node_id == 0 {
            continue;
        }
        let num_params = slot.num_params as usize;
        let mut param_indices: ArrayVec<usize, MAX_SLOT_PARAMS> = ArrayVec::new();
        for param_idx in 0..num_params.min(MAX_SLOT_PARAMS) {
            param_indices.push(param_idx);
        }
        param_indices.sort_by_key(|param_idx| slot.node_param_idx(*param_idx).unwrap_or(u32::MAX));
        for param_idx in param_indices {
            let Some(idx) = slot.node_param_idx(param_idx) else {
                continue;
            };
            if idx == u32::MAX || param_idx >= slot.defaults.len() {
                continue;
            }
            let (logical_id, idx) = if idx >= crate::instruments::voice_modulator::MOD_PARAM_BASE {
                if slot.modulator_node_id == 0 {
                    continue;
                }
                (
                    slot.modulator_node_id as u64,
                    (idx - crate::instruments::voice_modulator::MOD_PARAM_BASE) as u64,
                )
            } else {
                (slot.node_id as u64, idx as u64)
            };
            let value = resolved_slot_param_value(slot, step, param_idx, slot.defaults[param_idx]);
            if !value.is_finite() {
                continue;
            }
            let span = slot
                .param_node_spans
                .get(param_idx)
                .copied()
                .unwrap_or(1)
                .max(1);
            push_param_span(lg, logical_id, idx, span, value);
        }
    }
}

pub(super) fn compute_host_transport_clock(
    data: &mut AudioCallbackData,
    block_start_sample: u64,
) -> HostTransportClock {
    let playing = data.state.transport.playing.load(Ordering::Relaxed);
    if playing && !data.host_clock_was_playing {
        data.host_clock_play_start_sample = block_start_sample;
    }
    if !playing && data.host_clock_was_playing {
        data.host_clock_play_start_sample = block_start_sample;
    }
    data.host_clock_was_playing = playing;

    let bpm = data.state.transport.bpm.load(Ordering::Relaxed).max(1) as f64;
    let samples_per_bar = data.sample_rate * 240.0 / bpm;
    let elapsed_samples = block_start_sample.saturating_sub(data.host_clock_play_start_sample);
    let bar_phase = (elapsed_samples as f64 / samples_per_bar).fract() as f32;
    let bar_phase_increment = (1.0 / samples_per_bar) as f32;

    HostTransportClock {
        bar_phase,
        bar_phase_increment,
    }
}

pub(super) fn sync_instrument_host_clock_params(data: &mut AudioCallbackData, clock: HostTransportClock) {
    for engine_id in 0..data.state.runtime.engine_voice_counts.len() {
        let voice_count =
            data.state.runtime.engine_voice_counts[engine_id].load(Ordering::Acquire) as usize;
        for voice_idx in 0..voice_count.min(MAX_VOICES) {
            let lid =
                data.state.runtime.engine_voice_lids[engine_id][voice_idx].load(Ordering::Acquire);
            if lid == 0 {
                continue;
            }
            unsafe {
                params_push_wrapper(
                    data.lg.0,
                    ParamMsg {
                        idx: gatepitch::PARAM_CLOCK_PHASE,
                        logical_id: lid,
                        fvalue: clock.bar_phase,
                    },
                );
                params_push_wrapper(
                    data.lg.0,
                    ParamMsg {
                        idx: gatepitch::PARAM_CLOCK_INC,
                        logical_id: lid,
                        fvalue: clock.bar_phase_increment,
                    },
                );
            }
        }
    }

    for pool_id in 0..data.state.runtime.voice_counts.len() {
        let voice_count = data.state.runtime.voice_counts[pool_id].load(Ordering::Acquire) as usize;
        for voice_idx in 0..voice_count.min(MAX_VOICES) {
            let gatepitch_id = data.state.runtime.sampler_gatepitch_node_ids[pool_id][voice_idx]
                .load(Ordering::Acquire);
            if gatepitch_id == 0 {
                continue;
            }
            unsafe {
                params_push_wrapper(
                    data.lg.0,
                    ParamMsg {
                        idx: gatepitch::PARAM_CLOCK_PHASE,
                        logical_id: gatepitch_id as u64,
                        fvalue: clock.bar_phase,
                    },
                );
                params_push_wrapper(
                    data.lg.0,
                    ParamMsg {
                        idx: gatepitch::PARAM_CLOCK_INC,
                        logical_id: gatepitch_id as u64,
                        fvalue: clock.bar_phase_increment,
                    },
                );
            }
        }
    }
}

pub(super) fn sync_effect_modulator_transport_clock_params(
    data: &mut AudioCallbackData,
    clock: HostTransportClock,
) {
    for chain in &data.state.pattern.effect_chains {
        for slot in chain {
            let modulator_id = slot.modulator_node_id.load(Ordering::Relaxed);
            if modulator_id == 0 {
                continue;
            }
            unsafe {
                dispatch_voice_modulator_transport_clock(data.lg.0, modulator_id as u64, clock);
            }
        }
    }

    for track in &data.scheduler_snapshot.tracks {
        let Some(rack) = &track.rack_track else {
            continue;
        };
        for rack_slot in &rack.slots {
            for slot in &rack_slot.effect_slots {
                if slot.modulator_node_id == 0 {
                    continue;
                }
                unsafe {
                    dispatch_voice_modulator_transport_clock(
                        data.lg.0,
                        slot.modulator_node_id as u64,
                        clock,
                    );
                }
            }
        }
    }

    let Ok(bus_effects) = data.bus_effect_runtime.try_lock() else {
        return;
    };
    for bus in bus_effects.iter() {
        for slot in &bus.effect_slots {
            if slot.modulator_node_id == 0 {
                continue;
            }
            unsafe {
                dispatch_voice_modulator_transport_clock(
                    data.lg.0,
                    slot.modulator_node_id as u64,
                    clock,
                );
            }
        }
    }
}

pub(super) fn sync_dj_mixer_transport_phase(data: &mut AudioCallbackData, block_start_sample: u64) {
    let playing = data.state.transport.playing.load(Ordering::Relaxed);
    let bpm = data.state.transport.bpm.load(Ordering::Relaxed).max(1) as f64;
    let total_beats = if playing {
        block_start_sample as f64 * bpm / (data.sample_rate * 60.0)
    } else {
        0.0
    };
    let beat_phase = crate::effects::dj_mixer::transport_beat_phase(total_beats);

    for chain in &data.state.pattern.effect_chains {
        for slot in chain {
            let param_idx = slot.transport_phase_param_idx.load(Ordering::Relaxed);
            if param_idx == crate::effects::NO_TRANSPORT_PHASE_PARAM {
                continue;
            }
            let node_id = slot.node_id.load(Ordering::Relaxed);
            if node_id == 0 {
                continue;
            }
            unsafe {
                dispatch_transport_phase(data.lg.0, node_id as u64, param_idx, beat_phase);
            }
        }
    }
    for track in &data.scheduler_snapshot.tracks {
        let Some(rack) = &track.rack_track else {
            continue;
        };
        for rack_slot in &rack.slots {
            for slot in &rack_slot.effect_slots {
                if slot.transport_phase_param_idx == crate::effects::NO_TRANSPORT_PHASE_PARAM
                    || slot.node_id == 0
                {
                    continue;
                }
                unsafe {
                    dispatch_transport_phase(
                        data.lg.0,
                        slot.node_id as u64,
                        slot.transport_phase_param_idx,
                        beat_phase,
                    );
                }
            }
        }
    }

    let Ok(bus_effects) = data.bus_effect_runtime.try_lock() else {
        return;
    };
    for bus in bus_effects.iter() {
        for slot in &bus.effect_slots {
            let param_idx = slot.transport_phase_param_idx;
            if param_idx == crate::effects::NO_TRANSPORT_PHASE_PARAM || slot.node_id == 0 {
                continue;
            }
            unsafe {
                dispatch_transport_phase(data.lg.0, slot.node_id as u64, param_idx, beat_phase);
            }
        }
    }
}

pub(super) fn interleaved_peak(output: &[f32], num_channels: usize) -> (f32, f32) {
    let mut peak_l = 0.0f32;
    let mut peak_r = 0.0f32;
    if num_channels == 0 {
        return (peak_l, peak_r);
    }
    let nframes = output.len() / num_channels;
    for i in 0..nframes {
        let l = output[i * num_channels].abs();
        if l > peak_l {
            peak_l = l;
        }
        if num_channels > 1 {
            let r = output[i * num_channels + 1].abs();
            if r > peak_r {
                peak_r = r;
            }
        }
    }
    (peak_l, peak_r)
}

pub(super) fn zero_output_frames(output: &mut [f32], start_frame: usize, num_channels: usize) {
    let start = start_frame.saturating_mul(num_channels);
    if start < output.len() {
        output[start..].fill(0.0);
    }
}
