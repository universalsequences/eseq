pub mod audiograph;
pub mod engine;
mod device;
#[allow(unused_imports)]
use device::*;
mod graph_dispatch;
#[allow(unused_imports)]
use graph_dispatch::*;
mod state;
#[allow(unused_imports)]
use state::*;
mod voice_pool;
#[allow(unused_imports)]
use voice_pool::*;
mod params;
#[allow(unused_imports)]
use params::*;
mod events;
#[allow(unused_imports)]
use events::*;
mod rack;
#[allow(unused_imports)]
use rack::*;
mod fire;
#[allow(unused_imports)]
use fire::*;

use arrayvec::ArrayVec;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::Stream;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::audiograph::*;
use crate::effects::gatepitch;
use crate::effects::{EffectSlotSnapshot, EffectSlotState, MAX_SLOT_PARAMS};
use crate::recorder::MasterRecorder;
use crate::sampler::{
    PARAM_ATTACK_SAMPLES, PARAM_LOOP_XFADE_SAMPLES, PARAM_RELEASE_SAMPLES, PARAM_WARP_PROJECT_BPM,
    SAMPLER_EVENT_AUX_ATTACK_SAMPLES, SAMPLER_EVENT_AUX_ENABLED, SAMPLER_EVENT_AUX_END_POINT,
    SAMPLER_EVENT_AUX_GATE_MODE, SAMPLER_EVENT_AUX_GATE_SAMPLES, SAMPLER_EVENT_AUX_LOOP_MODE,
    SAMPLER_EVENT_AUX_LOOP_XFADE_SAMPLES, SAMPLER_EVENT_AUX_NOTE_ON_COUNT,
    SAMPLER_EVENT_AUX_RELEASE_SAMPLES, SAMPLER_EVENT_AUX_REVERSE, SAMPLER_EVENT_AUX_SCRUB_OFFSET,
    SAMPLER_EVENT_AUX_SPEED, SAMPLER_EVENT_AUX_SR_HZ, SAMPLER_EVENT_AUX_START_POINT,
    SAMPLER_EVENT_AUX_TRANSPOSE, SAMPLER_EVENT_AUX_VELOCITY, SAMPLER_EVENT_AUX_WARP_ENABLED,
    SAMPLER_EVENT_AUX_WARP_MODE, SAMPLER_EVENT_AUX_WARP_PRESERVE,
    SAMPLER_EVENT_AUX_WARP_PROJECT_BPM, SAMPLER_EVENT_AUX_WARP_PTR_HI,
    SAMPLER_EVENT_AUX_WARP_PTR_LO, SAMPLER_EVENT_AUX_WARP_RATIO, SAMPLER_EVENT_AUX_WARP_SAMPLE_BPM,
    SAMPLER_EVENT_AUX_WARP_SEG_ENVELOPE, SAMPLER_EVENT_AUX_WARP_SEG_LOOP_MODE,
};
use crate::scheduled_event::{
    resolved_chord_transpose, ScheduledEffectParam, ScheduledEvent, ScheduledEventKind,
    ScheduledEventQueue, ScheduledInstrumentParam, ScheduledInstrumentParamTarget,
    ScheduledInstrumentParams, ScheduledInstrumentTensorParam, ScheduledInstrumentTensorParams,
    ScheduledSamplerParams,
};
use crate::sequencer::{
    rack_slot_pool_index, sync_beats, BusId, CustomInstrumentRunMode, InstrumentType,
    KeyboardTrigger, RackRouting, RackSlotParam, RackSlotSnapshot, RackTrackSnapshot,
    SequencerSnapshot, SequencerState, StepParam, SwingResolution, MAX_INSTRUMENT_ENGINES,
    MAX_RACK_SLOTS, MAX_SAMPLER_POOLS, MAX_TRACKS,
};
use crate::app::BusGateRuntimeState;
use crate::voice::{VoicePool, MAX_VOICES};

pub const FALLBACK_SAMPLE_RATE: u32 = 44_100;
const CUSTOM_ENGINE_RELEASE_TAIL_SECONDS: f64 = 20.0;
const SCHEDULED_EVENT_QUEUE_CAPACITY: usize = 4096;
const SCHEDULED_COUNTDOWN_CAPACITY: usize =
    SCHEDULED_EVENT_QUEUE_CAPACITY + MAX_TRACKS * MAX_VOICES * 2 + MAX_TRACKS;
const SCHEDULED_BLOCK_SCRATCH_CAPACITY: usize =
    SCHEDULED_EVENT_QUEUE_CAPACITY + MAX_TRACKS * MAX_VOICES * 2 + MAX_TRACKS;

fn render_chunk(data: &mut AudioCallbackData, output: &mut [f32]) {
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
fn mix_metronome(
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

fn publish_sampler_modulator_activity(data: &AudioCallbackData) {
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
        crate::voice_modulator::set_sampler_active_mask(pool_id, mask);
    }
}

fn bus_gate_state_at(
    sequence: &crate::sequencer::BusGateSequence,
    total_beats: f64,
) -> (f32, usize) {
    const EPS: f64 = 1e-9;
    let ns = sequence.num_steps.clamp(1, crate::sequencer::MAX_STEPS);
    let mut starts = [0.0f64; crate::sequencer::MAX_STEPS];
    let mut durations = [0.0f64; crate::sequencer::MAX_STEPS];
    let mut accum = 0.0f64;
    for step in 0..ns {
        let timebase = sequence.timebase_plocks[step].unwrap_or(sequence.timebase);
        let duration = timebase.step_beats(ns).max(EPS);
        let sync = sync_beats(sequence.syncs[step]);
        if sync > EPS {
            accum = ceil_to_grid(accum, sync);
        }
        starts[step] = accum;
        durations[step] = duration;
        accum += duration;
    }
    let sync0 = sync_beats(sequence.syncs[0]);
    if sync0 > EPS {
        accum = ceil_to_grid(accum, sync0).max(EPS);
    }
    if accum <= EPS {
        return (1.0, 0);
    }

    let pos = total_beats.rem_euclid(accum);
    let mut active_step = None;
    for idx in 0..ns {
        if pos + EPS >= starts[idx] && pos < starts[idx] + durations[idx] {
            active_step = Some(idx);
            break;
        }
    }
    let step = active_step.unwrap_or_else(|| {
        let idx = starts[..ns].partition_point(|&start| start <= pos);
        idx.saturating_sub(1).min(ns - 1)
    });
    if active_step.is_none() {
        return (0.0, step);
    }

    if !sequence.steps[step] {
        return (0.0, step);
    }
    let local = pos - starts[step];
    let gate_duration = durations[step] * sequence.durations[step].clamp(0.0, 1.0) as f64;
    if local <= gate_duration + EPS {
        (sequence.velocities[step].clamp(0.0, 1.0), step)
    } else {
        (0.0, step)
    }
}

fn bus_gate_target_at(sequence: &crate::sequencer::BusGateSequence, total_beats: f64) -> f32 {
    bus_gate_state_at(sequence, total_beats).0
}

fn ceil_to_grid(value: f64, grid: f64) -> f64 {
    let rem = value % grid;
    if rem > 1e-9 {
        value + (grid - rem)
    } else {
        value
    }
}

unsafe fn dispatch_snapshot_effect_params_at_step(
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
        param_indices.sort_by_key(|param_idx| {
            slot.param_node_indices
                .get(*param_idx)
                .copied()
                .unwrap_or(*param_idx as u32)
        });
        for param_idx in param_indices {
            let idx = slot
                .param_node_indices
                .get(param_idx)
                .copied()
                .unwrap_or(param_idx as u32);
            if idx == u32::MAX || param_idx >= slot.defaults.len() {
                continue;
            }
            let (logical_id, idx) = if idx >= crate::voice_modulator::MOD_PARAM_BASE {
                if slot.modulator_node_id == 0 {
                    continue;
                }
                (
                    slot.modulator_node_id as u64,
                    (idx - crate::voice_modulator::MOD_PARAM_BASE) as u64,
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

fn sync_bus_gate_params(data: &mut AudioCallbackData, block_start_sample: u64) {
    let playing = data.state.transport.playing.load(Ordering::Relaxed);
    let bpm = data.state.transport.bpm.load(Ordering::Relaxed).max(1) as f64;
    if playing && !data.bus_gate_was_playing {
        data.bus_gate_play_start_sample = block_start_sample;
        for clock in &mut data.bus_gate_clocks {
            clock.last_target = f32::NAN;
            clock.last_step = None;
        }
    }
    if !playing && data.bus_gate_was_playing {
        for clock in &mut data.bus_gate_clocks {
            clock.last_target = f32::NAN;
            clock.last_step = None;
        }
    }
    data.bus_gate_was_playing = playing;

    let elapsed_samples = block_start_sample.saturating_sub(data.bus_gate_play_start_sample);
    let total_beats = elapsed_samples as f64 * bpm / (data.sample_rate * 60.0);
    let Ok(gates) = data.bus_gate_runtime.try_lock() else {
        return;
    };
    let gates = gates.clone();
    let mut playheads = Vec::with_capacity(gates.len());

    data.bus_gate_clocks
        .retain(|clock| gates.iter().any(|gate| gate.id == clock.id));

    for gate in gates {
        if gate.gate_id <= 0 {
            continue;
        }
        let (target, step) = if playing {
            bus_gate_state_at(&gate.sequence, total_beats)
        } else {
            (1.0, 0)
        };
        playheads.push((gate.id, step));
        let clock_idx = data
            .bus_gate_clocks
            .iter()
            .position(|clock| clock.id == gate.id)
            .unwrap_or_else(|| {
                data.bus_gate_clocks.push(BusGateClock {
                    id: gate.id,
                    last_target: f32::NAN,
                    last_step: None,
                });
                data.bus_gate_clocks.len() - 1
            });
        let clock = &mut data.bus_gate_clocks[clock_idx];
        if clock.last_step != Some(step) {
            clock.last_step = Some(step);
            unsafe {
                dispatch_snapshot_effect_params_at_step(data.lg.0, &gate.effect_slots, step);
            }
        }
        if (clock.last_target - target).abs() <= 0.0001 {
            continue;
        }
        clock.last_target = target;
        unsafe {
            crate::audiograph::params_push_wrapper(
                data.lg.0,
                crate::audiograph::ParamMsg {
                    idx: crate::effects::stereo_panner::STEREO_PANNER_PARAM_VOLUME,
                    logical_id: gate.gate_id as u64,
                    fvalue: target,
                },
            );
        }
    }
    if let Ok(mut shared_playheads) = data.bus_gate_playheads.try_lock() {
        *shared_playheads = playheads;
    }
}

fn compute_host_transport_clock(
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

fn sync_instrument_host_clock_params(data: &mut AudioCallbackData, clock: HostTransportClock) {
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

fn sync_effect_modulator_transport_clock_params(
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

    let Ok(gates) = data.bus_gate_runtime.try_lock() else {
        return;
    };
    for gate in gates.iter() {
        for slot in &gate.effect_slots {
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

fn sync_dj_mixer_transport_phase(data: &mut AudioCallbackData, block_start_sample: u64) {
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

    let Ok(gates) = data.bus_gate_runtime.try_lock() else {
        return;
    };
    for gate in gates.iter() {
        for slot in &gate.effect_slots {
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

fn interleaved_peak(output: &[f32], num_channels: usize) -> (f32, f32) {
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

fn zero_output_frames(output: &mut [f32], start_frame: usize, num_channels: usize) {
    let start = start_frame.saturating_mul(num_channels);
    if start < output.len() {
        output[start..].fill(0.0);
    }
}

fn audio_callback(data: &mut AudioCallbackData, output: &mut [f32]) {
    let callback_start = Instant::now();
    let nframes = output.len() / data.num_channels;
    data.current_callback_nframes = nframes;
    data.trace_callback_counter = data.trace_callback_counter.wrapping_add(1);
    let num_tracks = data.state.active_track_count();
    let topology_epoch = data.state.transport.topology_epoch.load(Ordering::Relaxed);
    if num_tracks != data.last_num_tracks || topology_epoch != data.last_topology_epoch {
        if data.trace_audio {
            eprintln!(
                "audio-trace: topology reset tracks {}->{} epoch {}->{} rendered_samples={}",
                data.last_num_tracks,
                num_tracks,
                data.last_topology_epoch,
                topology_epoch,
                data.rendered_samples.load(Ordering::Acquire),
            );
            data.trace_render_probe_blocks = data.trace_render_probe_blocks.max(12);
        }
        reset_audio_runtime_for_track_topology(data, num_tracks);
    }
    if data.state.topology_edit_in_flight() {
        data.scheduled_events.clear();
        clear_countdown_events(data);
        data.event_seq = 0;
    }
    let scheduler_snapshot_version = data.state.scheduler_snapshot_version();
    if scheduler_snapshot_version != data.scheduler_snapshot_version {
        data.scheduler_snapshot = data.state.latest_scheduler_snapshot();
        data.scheduler_snapshot_version = scheduler_snapshot_version;
    }
    let block_start_sample = data.rendered_samples.load(Ordering::Acquire);
    let block_end_sample = block_start_sample + nframes as u64;
    let transport_playing = data.state.transport.playing.load(Ordering::Relaxed);
    if transport_playing && !data.transport_was_playing {
        data.transport_beats = 0.0;
        data.metronome = MetronomeState::default();
    }
    if transport_playing {
        // Timestamp at callback entry, before graph work, so the UI can
        // interpolate between blocks rather than reading a stale playhead.
        data.state
            .transport
            .record_clock
            .publish(data.transport_beats, callback_start);
    } else {
        data.transport_beats = 0.0;
        data.metronome = MetronomeState::default();
    }
    data.transport_was_playing = transport_playing;
    let host_transport_clock = compute_host_transport_clock(data, block_start_sample);
    sync_bus_gate_params(data, block_start_sample);
    sync_instrument_host_clock_params(data, host_transport_clock);
    sync_effect_modulator_transport_clock_params(data, host_transport_clock);
    sync_dj_mixer_transport_phase(data, block_start_sample);

    // Sync voice pools against current runtime bindings. Project loads can
    // replace tracks in-place, so growth-only sync leaves dead logical IDs.
    for t in 0..num_tracks {
        sync_sampler_voice_pool(&data.state, t, &mut data.voice_pools[t]);

        if let Some(engine_id) = track_engine_id(&data.state, t) {
            sync_custom_engine_pool(
                &data.state,
                engine_id,
                &mut data.custom_engine_pools[engine_id],
            );
        }
    }
    sync_rack_voice_pools(data, num_tracks);
    sync_free_patch_transport_routes(data, num_tracks);

    // Process keyboard triggers
    let mut processed_keyboard_trigger = false;
    while let Ok(kt) = data.keyboard_rx.try_recv() {
        processed_keyboard_trigger = true;
        if kt.track >= num_tracks {
            continue;
        }
        let instrument_type = InstrumentType::from_runtime_flag(
            data.state.runtime.instrument_type_flags[kt.track].load(Ordering::Relaxed),
        );
        let is_custom = instrument_type == InstrumentType::Custom;
        let track_polyphonic = data.state.pattern.track_params[kt.track].is_polyphonic();
        let track_max_polyphony = data.state.pattern.track_params[kt.track].get_max_polyphony();
        data.voice_pools[kt.track].polyphonic = track_polyphonic;
        let base_note_offset = f32::from_bits(
            data.state.pattern.instrument_base_note_offsets[kt.track].load(Ordering::Relaxed),
        );

        if kt.note_off {
            if let Some(active_note) =
                take_active_keyboard_note(&mut data.active_keyboard_notes, kt.track, kt.transpose)
            {
                release_active_keyboard_note(data, active_note, 0, block_end_sample);
            }
        } else {
            // Note-on: allocate voice and trigger
            enforce_mute_group_for_winning_track(data, kt.track, block_start_sample, 0);
            let resolved_transpose = resolve_live_keyboard_transpose(
                &data.state,
                data.accumulator_states[kt.track],
                kt.track,
                kt.transpose,
            );
            if instrument_type == InstrumentType::Rack {
                let rack = data
                    .scheduler_snapshot
                    .tracks
                    .get(kt.track)
                    .and_then(|track| track.rack_track.clone());
                if let Some(rack) = rack {
                    if !fire_live_keyboard_rack_note(data, kt.track, &kt, resolved_transpose, rack)
                    {
                        continue;
                    }
                } else {
                    continue;
                }
            } else if is_custom {
                let Some(engine_id) = track_engine_id(&data.state, kt.track) else {
                    continue;
                };
                let free_patch = track_custom_run_mode(&data.state, kt.track)
                    == CustomInstrumentRunMode::FreePatch;
                let allocation = if free_patch {
                    let Some(allocation) = data.custom_engine_pools[engine_id]
                        .allocate_free_patch_voice(kt.track, kt.track, resolved_transpose)
                    else {
                        continue;
                    };
                    allocation
                } else {
                    data.custom_engine_pools[engine_id].allocate_voice(
                        kt.track,
                        kt.track,
                        resolved_transpose,
                        track_polyphonic,
                        track_max_polyphony,
                    )
                };
                let voice_idx = allocation.voice_idx;
                data.custom_engine_pools[engine_id].note_voice_allocated(engine_id, voice_idx);
                let voice_lid = allocation.logical_id;
                let default_params =
                    resolve_snapshot_instrument_defaults(&data.scheduler_snapshot, kt.track);
                let default_tensor_params =
                    resolve_live_instrument_tensor_defaults(&data.state, kt.track);
                let key_locked_params = key_locked_live_instrument_params(
                    &data.state,
                    kt.track,
                    resolved_transpose,
                    base_note_offset,
                    None,
                    &default_params,
                );
                let fingerprint = instrument_param_bundle_fingerprint(
                    engine_id,
                    base_note_offset,
                    &key_locked_params,
                    &default_tensor_params,
                );
                let synth_id = data.state.runtime.engine_synth_node_ids[engine_id][voice_idx]
                    .load(Ordering::Relaxed);
                let modulator_id = data.state.runtime.engine_modulator_node_ids[engine_id]
                    [voice_idx]
                    .load(Ordering::Relaxed);
                if voice_lid == 0 || synth_id == 0 || modulator_id == 0 {
                    continue;
                }
                if data.trace_audio {
                    let enabled = data.custom_engine_pools[engine_id].enabled_voice_count;
                    eprintln!(
                        "audio-trace: keyboard custom note-on track={} engine={engine_id} voice={voice_idx} lid={voice_lid} synth={synth_id} mod={modulator_id} enabled_voices={enabled} poly={track_polyphonic} stolen={}",
                        kt.track, allocation.stole_active_voice,
                    );
                    data.trace_render_probe_blocks = data.trace_render_probe_blocks.max(12);
                }
                let pitch_hz = custom_pitch_hz(resolved_transpose, base_note_offset);
                cancel_gate_off_for_lid(
                    &mut data.countdown_events,
                    &mut data.block_events,
                    voice_lid,
                );
                unsafe {
                    route_custom_voice_to_consumer(
                        data.lg.0,
                        &data.state,
                        engine_id,
                        voice_idx,
                        allocation.previous_route,
                        kt.track,
                    );
                    if data.custom_engine_pools[engine_id].voices[voice_idx].fingerprint
                        != fingerprint
                    {
                        dispatch_instrument_params_to_voice(
                            data.lg.0,
                            synth_id as u64,
                            modulator_id as u64,
                            &key_locked_params,
                        );
                        dispatch_instrument_tensor_params_to_voice(
                            data.lg.0,
                            synth_id as u64,
                            &default_tensor_params,
                        );
                    }
                }
                data.custom_engine_pools[engine_id].voices[voice_idx].fingerprint = fingerprint;
                if allocation.stole_active_voice || !track_polyphonic || free_patch {
                    let off_seq = next_block_event_sequence(data);
                    unsafe {
                        send_custom_note_off(data.lg.0, voice_lid, 0, off_seq);
                    }
                }
                let on_seq = next_block_event_sequence(data);
                unsafe {
                    send_custom_trigger(data.lg.0, voice_lid, 0, on_seq, pitch_hz, kt.velocity);
                }
                store_active_keyboard_note(
                    &mut data.active_keyboard_notes,
                    kt.track,
                    kt.transpose,
                    midi_note_from_transpose(resolved_transpose, base_note_offset),
                    &[ActiveKeyboardVoice {
                        logical_id: voice_lid,
                        gatepitch_id: 0,
                        target: ActiveKeyboardVoiceTarget::Custom {
                            engine_id,
                            free_patch,
                        },
                    }],
                );
            } else {
                let voice = data.voice_pools[kt.track]
                    .allocate_voice_retriggering_same_note(resolved_transpose);
                let voice_lid = voice.logical_id;
                if voice_lid == 0 {
                    continue;
                }
                let tp = &data.state.pattern.track_params[kt.track];
                let Some(kb_inst_slot) = data
                    .scheduler_snapshot
                    .tracks
                    .get(kt.track)
                    .map(|track| &track.instrument_slot)
                else {
                    continue;
                };
                let kb_default =
                    |param_idx: usize| kb_inst_slot.defaults.get(param_idx).copied().unwrap_or(0.0);
                let kb_instrument_params =
                    resolve_snapshot_instrument_defaults(&data.scheduler_snapshot, kt.track);
                let attack_samples = kb_default(0) * data.sample_rate as f32 / 1000.0;
                let release_samples = kb_default(1) * data.sample_rate as f32 / 1000.0;
                let gate_mode = if tp.is_gate_on() { 1.0 } else { 0.0 };
                let kb_start = kb_default(2);
                let kb_end = kb_default(3);
                let kb_enabled = kb_default(4);
                let kb_reverse = kb_default(5);
                let kb_loop_mode = kb_default(6);
                let kb_loop_xfade_samples = kb_default(7) * data.sample_rate as f32 / 1000.0;
                let kb_sr_hz = kb_default(8);
                let kb_playback_speed = kb_default(12);
                let kb_warp_preserve = snapshot_slot_default_node_param_value(
                    kb_inst_slot,
                    crate::sampler::PARAM_WARP_PRESERVE,
                    crate::sampler::WARP_PRESERVE_DEFAULT as f32,
                );
                let kb_warp_seg_loop_mode = snapshot_slot_default_node_param_value(
                    kb_inst_slot,
                    crate::sampler::PARAM_WARP_SEG_LOOP_MODE,
                    crate::sampler::WARP_SEG_LOOP_MODE_DEFAULT as f32,
                );
                let kb_warp_seg_envelope = snapshot_slot_default_node_param_value(
                    kb_inst_slot,
                    crate::sampler::PARAM_WARP_SEG_ENVELOPE,
                    crate::sampler::WARP_SEG_ENVELOPE_DEFAULT,
                );
                let (
                    kb_warp_enabled,
                    kb_warp_mode,
                    kb_warp_ratio,
                    kb_warp_sample_bpm,
                    kb_warp_project_bpm,
                    kb_warp_ptr_lo,
                    kb_warp_ptr_hi,
                ) = sampler_warp_runtime(
                    &data.state,
                    kt.track,
                    kb_default(9),
                    kb_default(10),
                    kb_default(11),
                );
                if voice.modulator_id > 0 {
                    let gatepitch_seq = next_event_sequence_from(&mut data.event_seq);
                    unsafe {
                        dispatch_sampler_modulator_params_to_voice(
                            data.lg.0,
                            voice.modulator_id as u64,
                            &kb_instrument_params,
                        );
                        send_custom_trigger(
                            data.lg.0,
                            voice.gatepitch_id as u64,
                            0,
                            gatepitch_seq,
                            custom_pitch_hz(resolved_transpose + base_note_offset, 0.0),
                            kt.velocity,
                        );
                    }
                }
                let sampler_seq = next_event_sequence_from(&mut data.event_seq);
                unsafe {
                    send_keyboard_trigger(
                        data.lg.0,
                        voice_lid,
                        0,
                        sampler_seq,
                        resolved_transpose + base_note_offset,
                        kt.velocity,
                        kb_playback_speed,
                        attack_samples,
                        release_samples,
                        gate_mode,
                        kb_start,
                        kb_end,
                        kb_enabled,
                        kb_reverse,
                        kb_loop_mode,
                        kb_loop_xfade_samples,
                        kb_sr_hz,
                        kb_warp_enabled,
                        kb_warp_mode,
                        kb_warp_ratio,
                        kb_warp_sample_bpm,
                        kb_warp_project_bpm,
                        kb_warp_ptr_lo,
                        kb_warp_ptr_hi,
                        kb_warp_preserve,
                        kb_warp_seg_loop_mode,
                        kb_warp_seg_envelope,
                        kb_default(13),
                    );
                    dispatch_sampler_extra_params_to_voice(
                        data.lg.0,
                        voice_lid,
                        &kb_instrument_params,
                    );
                }
                store_active_keyboard_note(
                    &mut data.active_keyboard_notes,
                    kt.track,
                    kt.transpose,
                    midi_note_from_transpose(resolved_transpose, base_note_offset),
                    &[ActiveKeyboardVoice {
                        logical_id: voice_lid,
                        gatepitch_id: voice.gatepitch_id,
                        target: ActiveKeyboardVoiceTarget::Sampler { pool_id: kt.track },
                    }],
                );
            }
            data.state.transport.trigger_flash[kt.track].store(255, Ordering::Relaxed);
        }
    }
    for track in 0..num_tracks {
        data.state.replace_live_notes(
            track,
            data.active_keyboard_notes[track]
                .iter()
                .filter_map(|note| note.and_then(|note| note.midi_note)),
        );
    }
    if processed_keyboard_trigger {
        sync_free_patch_transport_routes(data, num_tracks);
    }

    // Schedule accumulator reset on play-start or pattern change; consumed at next step 0.
    {
        let playing = data.state.transport.playing.load(Ordering::Relaxed);
        let pattern = data.state.current_scene_index() as u32;
        if (!data.last_playing && playing) || data.last_pattern != pattern {
            // Pattern changes and fresh playback should always reapply custom instrument params
            // even if a voice slot is being reused from an older sound state.
            for pool in &mut data.custom_engine_pools {
                pool.invalidate_sound_cache();
            }
            data.pending_accum_reset = [true; MAX_TRACKS];
        }
        if !playing && data.last_playing {
            data.scheduled_events.clear();
            clear_transport_countdown_events(data);
        }
        data.last_playing = playing;
        data.last_pattern = pattern;
    }

    // Push BPM to per-voice modulators when it changes. Track Filter/Delay
    // inserts are descriptor-managed on the control side.
    let bpm = data.state.transport.bpm.load(Ordering::Relaxed);
    if bpm != data.last_bpm {
        data.last_bpm = bpm;
        let bpm_f = bpm as f32;
        for engine in &data.state.runtime.engine_modulator_node_ids {
            for node in engine {
                let logical_id = node.load(Ordering::Relaxed);
                if logical_id != 0 {
                    unsafe {
                        dispatch_voice_modulator_bpm(data.lg.0, logical_id as u64, bpm_f);
                    }
                }
            }
        }
        for pool in &data.voice_pools {
            for voice in pool.voices.iter().take(pool.num_voices) {
                if voice.modulator_id > 0 {
                    unsafe {
                        dispatch_voice_modulator_bpm(data.lg.0, voice.modulator_id as u64, bpm_f);
                    }
                }
                if voice.logical_id != 0 {
                    unsafe {
                        params_push_wrapper(
                            data.lg.0,
                            ParamMsg {
                                idx: PARAM_WARP_PROJECT_BPM,
                                logical_id: voice.logical_id,
                                fvalue: bpm_f,
                            },
                        );
                    }
                }
            }
        }
    }

    let mod_reset_counter = data
        .state
        .transport
        .mod_reset_counter
        .load(Ordering::Relaxed);
    if mod_reset_counter != data.last_mod_reset_counter {
        data.last_mod_reset_counter = mod_reset_counter;
        for engine in &data.state.runtime.engine_modulator_node_ids {
            for node in engine {
                let logical_id = node.load(Ordering::Relaxed);
                if logical_id != 0 {
                    unsafe {
                        params_push_wrapper(
                            data.lg.0,
                            ParamMsg {
                                idx: crate::voice_modulator::PARAM_RESET_COUNTER as u64,
                                logical_id: logical_id as u64,
                                fvalue: mod_reset_counter as f32,
                            },
                        );
                    }
                }
            }
        }
    }

    let current_pattern_epoch = data.state.transport.pattern_epoch.load(Ordering::Relaxed);
    collect_due_countdown_events(data, nframes, current_pattern_epoch);
    drain_scheduled_events_for_callback(data, block_start_sample, nframes, current_pattern_epoch);
    dispatch_block_events(data, block_start_sample);

    let custom_release_tail_samples =
        (CUSTOM_ENGINE_RELEASE_TAIL_SECONDS * data.sample_rate).round() as u64;
    for engine_id in 0..data.state.runtime.engine_voice_counts.len() {
        if data.state.runtime.engine_voice_counts[engine_id].load(Ordering::Acquire) == 0 {
            continue;
        }
        let minimum_enabled_voices = usize::from(custom_engine_requires_idle_voice(
            data, engine_id, num_tracks,
        ));
        data.custom_engine_pools[engine_id].shrink_released_voices(
            engine_id,
            block_end_sample,
            custom_release_tail_samples,
            minimum_enabled_voices,
        );
    }

    let probe_render = data.trace_audio && data.trace_render_probe_blocks > 0;
    if probe_render {
        eprintln!(
            "audio-trace: render-start callback={} nframes={nframes} tracks={num_tracks} countdown_len={} rendered_samples={block_start_sample}",
            data.trace_callback_counter,
            data.countdown_events.len(),
        );
    }
    let render_start = Instant::now();
    render_chunk(data, output);
    let render_elapsed = render_start.elapsed();
    if probe_render {
        let (chunk_peak_l, chunk_peak_r) = interleaved_peak(output, data.num_channels);
        eprintln!(
            "audio-trace: render-done callback={} nframes={nframes} elapsed_us={} peak_l={chunk_peak_l:.6} peak_r={chunk_peak_r:.6}",
            data.trace_callback_counter,
            render_elapsed.as_micros(),
        );
        data.trace_render_probe_blocks -= 1;
    }
    if render_elapsed.as_millis() >= 10 {
        eprintln!(
            "audio: slow render_chunk; nframes={nframes} elapsed_ms={} countdown_len={} block_start_sample={block_start_sample}",
            render_elapsed.as_millis(),
            data.countdown_events.len(),
        );
    }
    data.rendered_samples
        .store(block_end_sample, Ordering::Release);
    data.state.set_audio_rendered_sample(block_end_sample);

    data.master_recorder.capture(output);

    if transport_playing
        && data
            .state
            .transport
            .metronome_enabled
            .load(Ordering::Relaxed)
    {
        let bpm = data.state.transport.bpm.load(Ordering::Relaxed) as f64;
        mix_metronome(
            &mut data.metronome,
            output,
            data.num_channels,
            data.sample_rate,
            data.transport_beats,
            bpm,
        );
    }
    if transport_playing {
        let bpm = data.state.transport.bpm.load(Ordering::Relaxed) as f64;
        data.transport_beats += nframes as f64 * bpm / (data.sample_rate * 60.0);
    }

    // Scan interleaved output for peak levels
    let (peak_l, peak_r) = interleaved_peak(output, data.num_channels);
    data.state
        .transport
        .peak_l
        .store(peak_l.to_bits(), Ordering::Relaxed);
    data.state
        .transport
        .peak_r
        .store(peak_r.to_bits(), Ordering::Relaxed);

    if data.trace_audio {
        let active_custom_voices: usize = data
            .custom_engine_pools
            .iter()
            .map(|pool| {
                pool.voices
                    .iter()
                    .take(pool.num_voices)
                    .filter(|v| v.active)
                    .count()
            })
            .sum();
        let active_sampler_voices: usize = data
            .voice_pools
            .iter()
            .map(|pool| {
                pool.voices
                    .iter()
                    .take(pool.num_voices)
                    .filter(|v| v.active)
                    .count()
            })
            .sum();
        let active_voices = active_custom_voices + active_sampler_voices;
        if active_voices > 0 && peak_l <= 0.000001 && peak_r <= 0.000001 {
            data.trace_silent_active_callbacks =
                data.trace_silent_active_callbacks.saturating_add(1);
            if data.trace_silent_active_callbacks == 16
                || data.trace_silent_active_callbacks % 128 == 0
            {
                eprintln!(
                    "audio-trace: silent while voices active callbacks={} streak={} tracks={num_tracks} custom_active={active_custom_voices} sampler_active={active_sampler_voices} rendered_samples={} topology_epoch={} playing={} countdown_len={} late_events={} dropped_events={}",
                    data.trace_callback_counter,
                    data.trace_silent_active_callbacks,
                    data.rendered_samples.load(Ordering::Acquire),
                    topology_epoch,
                    data.state.transport.playing.load(Ordering::Relaxed),
                    data.countdown_events.len(),
                    data.late_scheduled_events,
                    data.dropped_scheduled_events,
                );
            }
        } else {
            data.trace_silent_active_callbacks = 0;
        }

        let sample_rate = data.sample_rate.max(1.0) as u64;
        let callbacks_per_second = (sample_rate / nframes.max(1) as u64).max(1);
        if data.trace_callback_counter % callbacks_per_second == 0 {
            eprintln!(
                "audio-trace: heartbeat callbacks={} rendered_samples={} tracks={num_tracks} active_custom={active_custom_voices} active_sampler={active_sampler_voices} peak_l={peak_l:.6} peak_r={peak_r:.6} topology_epoch={} cpu_load_pct={:.1}",
                data.trace_callback_counter,
                data.rendered_samples.load(Ordering::Acquire),
                topology_epoch,
                f32::from_bits(data.state.transport.cpu_load_pct.load(Ordering::Relaxed)),
            );
            let mod_stats = crate::voice_modulator::take_process_stats();
            if mod_stats.calls > 0 {
                eprintln!(
                    "audio-trace: modulator-stats calls={} rendered={} disabled_custom={} disabled_sampler={} all_slots_off={} unbound_rendered={} rendered_frames={} disabled_frames={} all_slots_off_frames={}",
                    mod_stats.calls,
                    mod_stats.rendered_calls,
                    mod_stats.disabled_custom_skips,
                    mod_stats.disabled_sampler_skips,
                    mod_stats.all_slots_off_calls,
                    mod_stats.unbound_rendered_calls,
                    mod_stats.rendered_frames,
                    mod_stats.disabled_frames,
                    mod_stats.all_slots_off_frames,
                );
                for stats in mod_stats.engines {
                    eprintln!(
                        "audio-trace: modulator-engine engine={} enabled={} calls={} rendered={} disabled={} rendered_frames={} disabled_frames={}",
                        stats.engine_id,
                        stats.enabled_voices,
                        stats.calls,
                        stats.rendered_calls,
                        stats.disabled_skips,
                        stats.rendered_frames,
                        stats.disabled_frames,
                    );
                }
                for stats in mod_stats.sampler_tracks {
                    eprintln!(
                        "audio-trace: modulator-sampler track={} active_mask=0x{:03x} calls={} rendered={} disabled={} rendered_frames={} disabled_frames={}",
                        stats.track_idx,
                        stats.active_mask,
                        stats.calls,
                        stats.rendered_calls,
                        stats.disabled_skips,
                        stats.rendered_frames,
                        stats.disabled_frames,
                    );
                }
            }
        }
    }

    publish_active_voice_counts(data, num_tracks);

    if nframes > 0 {
        let elapsed_secs = callback_start.elapsed().as_secs_f32();
        let block_budget_secs = nframes as f32 / data.sample_rate as f32;
        let raw_load_pct = if block_budget_secs > 0.0 {
            (elapsed_secs / block_budget_secs) * 100.0
        } else {
            0.0
        };
        let prev_load_pct =
            f32::from_bits(data.state.transport.cpu_load_pct.load(Ordering::Relaxed));
        let smoothed_load_pct = if prev_load_pct <= 0.0 {
            raw_load_pct
        } else {
            prev_load_pct * 0.97 + raw_load_pct * 0.03
        };
        data.state
            .transport
            .cpu_load_pct
            .store(smoothed_load_pct.to_bits(), Ordering::Relaxed);
    }
}

/// Build a cpal output stream that drives the audiograph.
pub fn build_output_stream(
    lg: *mut LiveGraph,
    state: Arc<SequencerState>,
    sample_rate: u32,
    num_channels: usize,
    block_size: usize,
    master_recorder: Arc<MasterRecorder>,
    keyboard_rx: std::sync::mpsc::Receiver<KeyboardTrigger>,
    bus_gate_runtime: Arc<Mutex<Vec<BusGateRuntimeState>>>,
    bus_gate_playheads: Arc<Mutex<Vec<(BusId, usize)>>>,
) -> Result<Stream, String> {
    // CPAL does not expose portable output latency. Use the configured output
    // block as the sensible default; users can tune this transport value when
    // their device/OS path has additional latency.
    state.transport.record_latency_seconds.store(
        (block_size as f32 / sample_rate.max(1) as f32).to_bits(),
        Ordering::Release,
    );
    // Initialize voice pools from state
    let mut voice_pools: Vec<VoicePool> =
        (0..MAX_SAMPLER_POOLS).map(|_| VoicePool::new()).collect();
    let mut custom_engine_pools: Vec<CustomEnginePool> = (0..MAX_INSTRUMENT_ENGINES)
        .map(|_| CustomEnginePool::new())
        .collect();

    // Pre-populate voice pools for any existing tracks
    let num_tracks = state.active_track_count();
    for t in 0..num_tracks {
        sync_sampler_voice_pool(&state, t, &mut voice_pools[t]);

        if let Some(engine_id) = track_engine_id(&state, t) {
            sync_custom_engine_pool(&state, engine_id, &mut custom_engine_pools[engine_id]);
        }
    }

    let scheduled_events = Arc::new(ScheduledEventQueue::new());
    let rendered_samples = Arc::new(AtomicU64::new(0));
    let (audio_keyboard_tx, audio_keyboard_rx) = std::sync::mpsc::channel();
    let (live_keyboard_tx, live_keyboard_rx) = std::sync::mpsc::channel();
    {
        let state_for_keyboard_router = Arc::clone(&state);
        let _ = std::thread::Builder::new()
            .name("keyboard-midi-fx-router".to_string())
            .spawn(move || {
                while let Ok(trigger) = keyboard_rx.recv() {
                    if trigger.note_off {
                        let _ = live_keyboard_tx.send(trigger);
                        let _ = audio_keyboard_tx.send(trigger);
                        continue;
                    }
                    let use_midi_fx = trigger.track
                        < state_for_keyboard_router.active_track_count()
                        && !state_for_keyboard_router.pattern.track_params[trigger.track]
                            .midi_fx_chain()
                            .is_empty();
                    if use_midi_fx {
                        let _ = live_keyboard_tx.send(trigger);
                    } else {
                        let _ = audio_keyboard_tx.send(trigger);
                    }
                }
            });
    }
    let initial_topology_epoch = state.transport.topology_epoch.load(Ordering::Relaxed);
    let initial_scheduler_snapshot_version = state.scheduler_snapshot_version();
    let initial_scheduler_snapshot = state.latest_scheduler_snapshot();
    let trace_audio = env_flag("TINYSEQ_AUDIO_TRACE", false);
    crate::voice_modulator::set_process_stats_enabled(trace_audio);
    if trace_audio {
        eprintln!("audio-trace: enabled");
    }

    let mut cb_data = AudioCallbackData {
        lg: LiveGraphPtr(lg),
        state,
        num_channels,
        sample_rate: sample_rate as f64,
        last_bpm: 0,
        last_mod_reset_counter: 0,
        voice_pools,
        custom_engine_pools,
        scheduler_snapshot: initial_scheduler_snapshot,
        scheduler_snapshot_version: initial_scheduler_snapshot_version,
        active_keyboard_notes: [[None; MAX_VOICES]; MAX_TRACKS],
        keyboard_rx: audio_keyboard_rx,
        master_recorder,
        accumulator_states: [crate::accumulator::AccumulatorRuntimeState::default(); MAX_TRACKS],
        last_playing: false,
        last_pattern: u32::MAX,
        last_num_tracks: num_tracks,
        last_topology_epoch: initial_topology_epoch,
        host_clock_was_playing: false,
        host_clock_play_start_sample: 0,
        free_patch_transport_routes: [FreePatchTransportRouteState::default(); MAX_TRACKS],
        pending_accum_reset: [false; MAX_TRACKS],
        scheduled_events: Arc::clone(&scheduled_events),
        countdown_events: Vec::with_capacity(SCHEDULED_COUNTDOWN_CAPACITY),
        block_events: Vec::with_capacity(SCHEDULED_BLOCK_SCRATCH_CAPACITY),
        block_events_need_sort: false,
        current_callback_nframes: block_size,
        rendered_samples: Arc::clone(&rendered_samples),
        bus_gate_runtime,
        bus_gate_playheads,
        bus_gate_clocks: Vec::new(),
        bus_gate_was_playing: false,
        bus_gate_play_start_sample: 0,
        dropped_scheduled_events: 0,
        late_scheduled_events: 0,
        event_seq: 0,
        trace_audio,
        trace_callback_counter: 0,
        trace_render_probe_blocks: 0,
        trace_silent_active_callbacks: 0,
        transport_beats: 0.0,
        transport_was_playing: false,
        metronome: MetronomeState::default(),
    };
    crate::scheduler::spawn_scheduler_thread(
        Arc::clone(&cb_data.state),
        sample_rate,
        block_size,
        rendered_samples,
        scheduled_events,
        live_keyboard_rx,
    );

    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or("No output device available")?;

    let config = cpal::StreamConfig {
        channels: num_channels as u16,
        sample_rate: cpal::SampleRate(sample_rate),
        buffer_size: cpal::BufferSize::Fixed(block_size as u32),
    };

    let stream = device
        .build_output_stream(
            &config,
            move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                audio_callback(&mut cb_data, data);
            },
            |err| eprintln!("Audio stream error: {err}"),
            None,
        )
        .map_err(|e| format!("Failed to build output stream: {e}"))?;

    stream
        .play()
        .map_err(|e| format!("Failed to play stream: {e}"))?;

    Ok(stream)
}

/// Query the default output device, preserving the system sample rate when possible.
pub fn query_device_config() -> Result<(u32, u16), String> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or("No output device available")?;
    let default_config = device
        .default_output_config()
        .map_err(|e| format!("Failed to get default config: {e}"))?;
    let ranges: Vec<OutputFormatRange> = device
        .supported_output_configs()
        .map_err(|e| format!("Failed to query supported output configs: {e}"))?
        .map(|range| OutputFormatRange {
            channels: range.channels(),
            min_sample_rate: range.min_sample_rate().0,
            max_sample_rate: range.max_sample_rate().0,
            supports_f32: range.sample_format() == cpal::SampleFormat::F32,
        })
        .collect();
    let selected = select_output_config(
        default_config.sample_rate().0,
        default_config.channels(),
        ranges,
    )
    .ok_or_else(|| {
        let device_name = device
            .name()
            .unwrap_or_else(|_| "default output device".to_string());
        format!(
            "{device_name} does not support f32 output at either {} Hz or its default {} Hz rate",
            FALLBACK_SAMPLE_RATE,
            default_config.sample_rate().0
        )
    })?;

    Ok((selected.sample_rate, selected.channels))
}

#[cfg(test)]
mod tests;
