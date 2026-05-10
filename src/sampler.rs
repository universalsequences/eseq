use crate::audiograph::*;
use std::ffi::CString;
use std::os::raw::{c_int, c_void};
use std::path::Path;

// State layout indices (f32 slots)
const STATE_BUFFER_ID: usize = 0;
const STATE_PLAYHEAD: usize = 1;
const STATE_PLAYING: usize = 2;
const STATE_GAIN: usize = 3;
const STATE_VELOCITY: usize = 4;
const STATE_SPEED: usize = 5;
const STATE_GATE_SAMPLES: usize = 6; // absolute gate length in samples (computed by audio callback)
const STATE_TRANSPOSE: usize = 7;
const STATE_ATTACK_SAMPLES: usize = 8;
const STATE_RELEASE_SAMPLES: usize = 9;
const STATE_GATE_MODE: usize = 10; // 1.0=gate on, 0.0=gate off
                                   // Persistent envelope state (not settable via params)
const STATE_ENV_PHASE: usize = 11; // 0=idle, 1=attack, 2=sustain, 3=release
const STATE_ENV_LEVEL: usize = 12; // current envelope amplitude 0.0–1.0
const STATE_RELEASE_LEVEL: usize = 13; // level when release began (for linear ramp)
const STATE_GATE_COUNTER: usize = 14; // real-time sample counter for gate duration (increments by 1/sample, not by playback rate)
const STATE_LAST_OUT_L: usize = 15; // last emitted left sample, used for click-free retrigger smoothing
const STATE_LAST_OUT_R: usize = 16; // last emitted right sample, used for click-free retrigger smoothing
const STATE_RETRIGGER_OUT_L: usize = 17; // captured left sample at retrigger start
const STATE_RETRIGGER_OUT_R: usize = 18; // captured right sample at retrigger start
const STATE_START_POINT: usize = 19; // normalized 0.0–1.0 start position in buffer
const STATE_END_POINT: usize = 20; // normalized 0.0–1.0 end position in buffer
const STATE_ENABLED: usize = 21;
const STATE_REVERSE: usize = 22;
const STATE_LOOP_MODE: usize = 23; // 0=one-shot, 1=gate, 2=loop, 3=ping-pong
const STATE_LOOP_XFADE_SAMPLES: usize = 24;
const STATE_SR_HZ: usize = 25;
const STATE_PLAY_DIRECTION: usize = 26;
const STATE_SR_PHASE: usize = 27;
const STATE_SR_HELD_L: usize = 28;
const STATE_SR_HELD_R: usize = 29;
const STATE_SAMPLE_RATE: usize = 30;
const STATE_WARP_ENABLED: usize = 31;
const STATE_WARP_MODE: usize = 32;
const STATE_WARP_RATIO: usize = 33;
const STATE_WARP_ONSET_TABLE_PTR_LO: usize = 34;
const STATE_WARP_ONSET_TABLE_PTR_HI: usize = 35;
const STATE_WARP_CURRENT_SLICE: usize = 36;
const STATE_WARP_SLICE_PROJECT_FRAME_START: usize = 37;
const STATE_WARP_XFADE_REMAINING: usize = 38;
const STATE_WARP_PREV_PLAYHEAD: usize = 39;
const STATE_WARP_SAMPLE_BPM: usize = 40;
const STATE_WARP_PROJECT_BPM: usize = 41;
const STATE_WARP_SLICE_SOURCE_FRAME_START: usize = 42;
const STATE_WARP_LAST_TARGET_RATIO: usize = 43;
pub const SAMPLER_STATE_SIZE: usize = 44;
pub const SAMPLER_PARAM_ENABLED: u64 = STATE_ENABLED as u64;

// Envelope phase constants
const ENV_IDLE: f32 = 0.0;
const ENV_ATTACK: f32 = 1.0;
const ENV_SUSTAIN: f32 = 2.0;
const ENV_RELEASE: f32 = 3.0;
const ENV_RETRIGGER: f32 = 4.0; // smooth fade-down before re-attack

// Minimum release to prevent clicks (in samples, ~1.5ms at 44100)
const MIN_RELEASE_SAMPLES: f32 = 64.0;
// Retrigger crossfade duration (~1ms at 44100). Fades old content to 0 before new attack.
const RETRIGGER_FADE_SAMPLES: f32 = 48.0;
// Minimum attack applied after retrigger to prevent click on ramp-up (~0.2ms at 44100).
// Fresh triggers from silence use the user's attack value directly (even if 0).
const MIN_RETRIGGER_ATTACK: f32 = 8.0;
const WARP_XFADE_SECONDS: f32 = 0.005;

unsafe fn read_interpolated(
    sample_data: *mut f32,
    sample_len: usize,
    channel_count: usize,
    playhead: f32,
) -> (f32, f32) {
    let idx = playhead as usize;
    let frac = playhead - idx as f32;
    let sample_index = idx * channel_count;
    let next_sample_index = (idx + 1) * channel_count;
    let s0_l = if idx < sample_len {
        *sample_data.add(sample_index)
    } else {
        0.0
    };
    let s1_l = if idx + 1 < sample_len {
        *sample_data.add(next_sample_index)
    } else {
        0.0
    };
    let s0_r = if channel_count > 1 && idx < sample_len {
        *sample_data.add(sample_index + 1)
    } else {
        s0_l
    };
    let s1_r = if channel_count > 1 && idx + 1 < sample_len {
        *sample_data.add(next_sample_index + 1)
    } else {
        s1_l
    };
    (s0_l + frac * (s1_l - s0_l), s0_r + frac * (s1_r - s0_r))
}

// Param indices (match state layout for direct write)
pub const PARAM_PLAYHEAD: u64 = STATE_PLAYHEAD as u64;
pub const PARAM_TRIGGER: u64 = STATE_PLAYING as u64;
pub const PARAM_GATE_COUNTER: u64 = STATE_GATE_COUNTER as u64;
pub const PARAM_VELOCITY: u64 = STATE_VELOCITY as u64;
pub const PARAM_SPEED: u64 = STATE_SPEED as u64;
pub const PARAM_GATE_SAMPLES: u64 = STATE_GATE_SAMPLES as u64;
pub const PARAM_TRANSPOSE: u64 = STATE_TRANSPOSE as u64;
pub const PARAM_ATTACK_SAMPLES: u64 = STATE_ATTACK_SAMPLES as u64;
pub const PARAM_RELEASE_SAMPLES: u64 = STATE_RELEASE_SAMPLES as u64;
pub const PARAM_GATE_MODE: u64 = STATE_GATE_MODE as u64;
pub const PARAM_BUFFER_ID: u64 = STATE_BUFFER_ID as u64;
pub const PARAM_START_POINT: u64 = STATE_START_POINT as u64;
pub const PARAM_END_POINT: u64 = STATE_END_POINT as u64;
pub const PARAM_ENABLED: u64 = STATE_ENABLED as u64;
pub const PARAM_REVERSE: u64 = STATE_REVERSE as u64;
pub const PARAM_LOOP_MODE: u64 = STATE_LOOP_MODE as u64;
pub const PARAM_LOOP_XFADE_SAMPLES: u64 = STATE_LOOP_XFADE_SAMPLES as u64;
pub const PARAM_SR_HZ: u64 = STATE_SR_HZ as u64;
pub const PARAM_WARP_ENABLED: u64 = STATE_WARP_ENABLED as u64;
pub const PARAM_WARP_MODE: u64 = STATE_WARP_MODE as u64;
pub const PARAM_WARP_RATIO: u64 = STATE_WARP_RATIO as u64;
pub const PARAM_WARP_ONSET_TABLE_PTR_LO: u64 = STATE_WARP_ONSET_TABLE_PTR_LO as u64;
pub const PARAM_WARP_ONSET_TABLE_PTR_HI: u64 = STATE_WARP_ONSET_TABLE_PTR_HI as u64;
pub const PARAM_WARP_SAMPLE_BPM: u64 = STATE_WARP_SAMPLE_BPM as u64;
pub const PARAM_WARP_PROJECT_BPM: u64 = STATE_WARP_PROJECT_BPM as u64;

pub struct SamplerTrack {
    pub name: String,
    pub node_id: i32,
    pub logical_id: u64,
    pub buffer_id: i32,
}

pub struct LoadedSample {
    pub buffer_id: i32,
    pub name: String,
    pub mono_samples: Vec<f32>,
    pub sample_rate: u32,
    pub frames: usize,
}

/// extern "C" init — called by audiograph when node is created.
unsafe extern "C" fn sampler_init(
    state: *mut c_void,
    sample_rate: c_int,
    _max_block: c_int,
    initial_state: *const c_void,
) {
    let s = state as *mut f32;
    if !initial_state.is_null() {
        let init = initial_state as *const f32;
        *s.add(STATE_BUFFER_ID) = *init.add(0);
    }
    *s.add(STATE_PLAYHEAD) = 0.0;
    *s.add(STATE_PLAYING) = 0.0;
    *s.add(STATE_GAIN) = 0.8;
    *s.add(STATE_VELOCITY) = 1.0;
    *s.add(STATE_SPEED) = 1.0;
    *s.add(STATE_GATE_SAMPLES) = f32::MAX; // ungated by default until first trigger
    *s.add(STATE_TRANSPOSE) = 0.0;
    *s.add(STATE_ATTACK_SAMPLES) = 0.0;
    *s.add(STATE_RELEASE_SAMPLES) = 0.0;
    *s.add(STATE_GATE_MODE) = 1.0; // gate on by default
    *s.add(STATE_ENV_PHASE) = ENV_IDLE;
    *s.add(STATE_ENV_LEVEL) = 0.0;
    *s.add(STATE_RELEASE_LEVEL) = 0.0;
    *s.add(STATE_GATE_COUNTER) = 0.0;
    *s.add(STATE_LAST_OUT_L) = 0.0;
    *s.add(STATE_LAST_OUT_R) = 0.0;
    *s.add(STATE_RETRIGGER_OUT_L) = 0.0;
    *s.add(STATE_RETRIGGER_OUT_R) = 0.0;
    *s.add(STATE_START_POINT) = 0.0;
    *s.add(STATE_END_POINT) = 1.0;
    *s.add(STATE_ENABLED) = 1.0;
    *s.add(STATE_REVERSE) = 0.0;
    *s.add(STATE_LOOP_MODE) = 1.0;
    *s.add(STATE_LOOP_XFADE_SAMPLES) = 0.0;
    *s.add(STATE_SR_HZ) = 44_100.0;
    *s.add(STATE_PLAY_DIRECTION) = 1.0;
    *s.add(STATE_SR_PHASE) = 0.0;
    *s.add(STATE_SR_HELD_L) = 0.0;
    *s.add(STATE_SR_HELD_R) = 0.0;
    *s.add(STATE_SAMPLE_RATE) = sample_rate as f32;
    *s.add(STATE_WARP_ENABLED) = 0.0;
    *s.add(STATE_WARP_MODE) = 0.0;
    *s.add(STATE_WARP_RATIO) = 1.0;
    *s.add(STATE_WARP_ONSET_TABLE_PTR_LO) = 0.0;
    *s.add(STATE_WARP_ONSET_TABLE_PTR_HI) = 0.0;
    *s.add(STATE_WARP_CURRENT_SLICE) = 0.0;
    *s.add(STATE_WARP_SLICE_PROJECT_FRAME_START) = 0.0;
    *s.add(STATE_WARP_XFADE_REMAINING) = 0.0;
    *s.add(STATE_WARP_PREV_PLAYHEAD) = 0.0;
    *s.add(STATE_WARP_SAMPLE_BPM) = 120.0;
    *s.add(STATE_WARP_PROJECT_BPM) = 120.0;
    *s.add(STATE_WARP_SLICE_SOURCE_FRAME_START) = 0.0;
    *s.add(STATE_WARP_LAST_TARGET_RATIO) = 1.0;
}

/// extern "C" process — reads sample data from buffer, writes to output.
///
/// Envelope state machine (persists across blocks):
///   IDLE → (trigger) → ATTACK → SUSTAIN → (gate-off) → RELEASE → IDLE
///
/// gate_samples=0 is treated as an explicit note-off regardless of gate_mode,
/// so keyboard release always triggers the release phase.
unsafe extern "C" fn sampler_process(
    _inp: *const *mut f32,
    out: *const *mut f32,
    nframes: c_int,
    state: *mut c_void,
    buffers: *mut c_void,
) {
    let s = state as *mut f32;
    let buffer_id = (*s.add(STATE_BUFFER_ID)) as usize;
    let mut playhead = *s.add(STATE_PLAYHEAD);
    let playing = *s.add(STATE_PLAYING);
    let gain = *s.add(STATE_GAIN);
    let velocity = *s.add(STATE_VELOCITY);
    let speed = *s.add(STATE_SPEED);
    let gate_samples = *s.add(STATE_GATE_SAMPLES);
    let transpose = *s.add(STATE_TRANSPOSE);
    let attack_samples = *s.add(STATE_ATTACK_SAMPLES);
    let release_samples = *s.add(STATE_RELEASE_SAMPLES);
    let gate_mode = *s.add(STATE_GATE_MODE);
    let mut env_phase = *s.add(STATE_ENV_PHASE);
    let mut env_level = *s.add(STATE_ENV_LEVEL);
    let mut release_level = *s.add(STATE_RELEASE_LEVEL);
    let mut gate_counter = *s.add(STATE_GATE_COUNTER);
    let mut last_out_l = *s.add(STATE_LAST_OUT_L);
    let mut last_out_r = *s.add(STATE_LAST_OUT_R);
    let mut retrigger_out_l = *s.add(STATE_RETRIGGER_OUT_L);
    let mut retrigger_out_r = *s.add(STATE_RETRIGGER_OUT_R);
    let start_point = (*s.add(STATE_START_POINT)).clamp(0.0, 1.0);
    let end_point = (*s.add(STATE_END_POINT)).clamp(0.0, 1.0);
    let enabled = *s.add(STATE_ENABLED);
    let reverse_param = *s.add(STATE_REVERSE) > 0.5;
    let loop_mode_param = (*s.add(STATE_LOOP_MODE)).round().clamp(0.0, 3.0) as i32;
    let loop_xfade_samples = (*s.add(STATE_LOOP_XFADE_SAMPLES)).max(0.0);
    let sr_hz = *s.add(STATE_SR_HZ);
    let mut play_direction = *s.add(STATE_PLAY_DIRECTION);
    let mut sr_phase = *s.add(STATE_SR_PHASE);
    let mut sr_held_l = *s.add(STATE_SR_HELD_L);
    let mut sr_held_r = *s.add(STATE_SR_HELD_R);
    let sample_rate = (*s.add(STATE_SAMPLE_RATE)).max(1.0);
    let warp_enabled = *s.add(STATE_WARP_ENABLED) > 0.5;
    let warp_mode = (*s.add(STATE_WARP_MODE)).round() as i32;
    let mut warp_ratio = (*s.add(STATE_WARP_RATIO)).clamp(0.01, 32.0);
    let warp_sample_bpm = (*s.add(STATE_WARP_SAMPLE_BPM)).clamp(20.0, 400.0);
    let warp_project_bpm = (*s.add(STATE_WARP_PROJECT_BPM)).clamp(1.0, 400.0);
    let warp_target_ratio = if warp_enabled && warp_mode == 0 {
        (warp_project_bpm / warp_sample_bpm).clamp(0.01, 32.0)
    } else {
        warp_ratio
    };
    let onset_ptr = crate::analysis::unpack_ptr(
        *s.add(STATE_WARP_ONSET_TABLE_PTR_LO),
        *s.add(STATE_WARP_ONSET_TABLE_PTR_HI),
    );
    let onset_table = if warp_enabled && warp_mode == 0 && !onset_ptr.is_null() {
        Some(&*onset_ptr)
    } else {
        None
    };
    let mut current_slice = (*s.add(STATE_WARP_CURRENT_SLICE)).max(0.0) as usize;
    let mut slice_project_frame_start = *s.add(STATE_WARP_SLICE_PROJECT_FRAME_START);
    let mut warp_xfade_remaining = (*s.add(STATE_WARP_XFADE_REMAINING)).max(0.0);
    let mut warp_prev_playhead = *s.add(STATE_WARP_PREV_PLAYHEAD);
    let mut slice_source_frame_start = *s.add(STATE_WARP_SLICE_SOURCE_FRAME_START);
    let mut last_warp_target_ratio = (*s.add(STATE_WARP_LAST_TARGET_RATIO)).clamp(0.01, 32.0);

    let buf_desc = buffers as *const BufferDesc;
    let desc = &*buf_desc.add(buffer_id);
    let sample_data = desc.buffer;
    let sample_len = desc.size as usize;

    let out0 = *out.add(0);
    let out1 = *out.add(1);
    let nf = nframes as usize;
    let channel_count = desc.channel_count.max(1) as usize;

    if enabled <= 0.5 {
        for i in 0..nf {
            *out0.add(i) = 0.0;
            *out1.add(i) = 0.0;
        }
        return;
    }

    // Compute effective sample region from normalized start/end points
    let start_sample = (start_point * sample_len as f32) as usize;
    let end_sample = if end_point > start_point {
        (end_point * sample_len as f32) as usize
    } else {
        sample_len
    };

    if playing <= 0.0 || sample_data.is_null() || sample_len == 0 {
        for i in 0..nf {
            *out0.add(i) = 0.0;
            *out1.add(i) = 0.0;
        }
        *s.add(STATE_LAST_OUT_L) = 0.0;
        *s.add(STATE_LAST_OUT_R) = 0.0;
        return;
    }

    let region_len = (end_sample.saturating_sub(start_sample)).max(1) as f32;
    let warp_active = onset_table
        .map(|table| {
            table
                .onsets_frames
                .iter()
                .filter(|&&frame| {
                    let frame = frame as usize;
                    frame > start_sample && frame < end_sample
                })
                .take(1)
                .count()
                >= 1
        })
        .unwrap_or(false);
    if let Some(table) = onset_table {
        if current_slice > table.onsets_frames.len() {
            current_slice = 0;
        }
    }
    let reverse = reverse_param && !warp_active;
    let loop_mode = loop_mode_param;
    let loop_xfade = loop_xfade_samples.min(region_len * 0.5);
    let effective_rate = speed * (2.0_f32).powf(transpose / 12.0);
    let step_rate = effective_rate.abs().max(0.0);
    let sr_step = if sr_hz > 0.0 {
        (sample_rate / sr_hz).max(1.0)
    } else {
        1.0
    };
    let sr_reduced = sr_hz > 0.0 && sr_hz < sample_rate * 0.98 && sr_hz < 44_100.0 * 0.98;
    let amplitude = velocity * gain;
    let eff_release = release_samples.max(MIN_RELEASE_SAMPLES);
    let warp_ratio_slew = (1.0 / (sample_rate * 0.050)).clamp(0.0001, 1.0);
    // After a retrigger fade, use a small minimum attack to avoid click on ramp-up.
    // For fresh triggers from silence this flag stays false → attack=0 stays punchy.
    let mut post_retrigger = false;

    let reset_forward_warp_state = |table: &crate::analysis::OnsetTableShared,
                                    current_slice: &mut usize,
                                    slice_project_frame_start: &mut f32,
                                    slice_source_frame_start: &mut f32,
                                    warp_xfade_remaining: &mut f32,
                                    warp_prev_playhead: &mut f32,
                                    playhead: f32,
                                    gate_counter: f32| {
        *current_slice = table
            .onsets_frames
            .iter()
            .position(|&frame| {
                let frame = frame as usize;
                frame > start_sample && frame < end_sample
            })
            .unwrap_or(table.onsets_frames.len());
        *slice_project_frame_start = gate_counter;
        *slice_source_frame_start = playhead;
        *warp_xfade_remaining = 0.0;
        *warp_prev_playhead = playhead;
    };

    // ── Trigger detection ──
    // playhead==0 means params just reset it. Distinguish fresh vs retrigger:
    if playhead == 0.0 && env_phase != ENV_RETRIGGER {
        play_direction = if reverse { -1.0 } else { 1.0 };
        playhead = if reverse {
            (end_sample.saturating_sub(1)) as f32
        } else {
            start_sample as f32
        };
        if warp_active {
            if let Some(table) = onset_table {
                playhead = start_sample as f32;
                reset_forward_warp_state(
                    table,
                    &mut current_slice,
                    &mut slice_project_frame_start,
                    &mut slice_source_frame_start,
                    &mut warp_xfade_remaining,
                    &mut warp_prev_playhead,
                    playhead,
                    0.0,
                );
            }
        }
        gate_counter = 0.0; // reset real-time duration counter
        sr_phase = 0.0;
        if env_level > 0.001 || last_out_l.abs() > 0.000_1 || last_out_r.abs() > 0.000_1 {
            // Voice was still audible → fade the actual previous output to zero
            // before starting the new waveform attack.
            env_phase = ENV_RETRIGGER;
            env_level = 1.0;
            release_level = 1.0;
            retrigger_out_l = last_out_l;
            retrigger_out_r = last_out_r;
        } else {
            // Voice was silent → clean attack from 0
            env_phase = ENV_ATTACK;
            env_level = 0.0;
            retrigger_out_l = 0.0;
            retrigger_out_r = 0.0;
        }
    }

    if warp_active && (warp_target_ratio - last_warp_target_ratio).abs() > 0.0001 {
        if let Some(table) = onset_table {
            let old_playhead = playhead;
            warp_ratio = warp_target_ratio;
            last_warp_target_ratio = warp_target_ratio;
            slice_project_frame_start = 0.0;
            slice_source_frame_start = start_sample as f32;
            current_slice = table
                .onsets_frames
                .iter()
                .position(|&frame| {
                    let frame = frame as usize;
                    frame > start_sample && frame < end_sample
                })
                .unwrap_or(table.onsets_frames.len());

            while current_slice < table.onsets_frames.len() {
                let next = table.onsets_frames[current_slice] as f32;
                if next as usize >= end_sample {
                    break;
                }
                let next_project_frame =
                    slice_project_frame_start + (next - slice_source_frame_start) / warp_ratio;
                if gate_counter < next_project_frame {
                    break;
                }
                slice_project_frame_start = next_project_frame;
                slice_source_frame_start = next;
                current_slice += 1;
            }

            let elapsed_in_slice = (gate_counter - slice_project_frame_start).max(0.0);
            let mut next_boundary = end_sample as f32;
            if current_slice < table.onsets_frames.len() {
                let next = table.onsets_frames[current_slice] as f32;
                if next as usize >= start_sample && next as usize <= end_sample {
                    next_boundary = next;
                }
            }
            playhead = (slice_source_frame_start + elapsed_in_slice * step_rate)
                .clamp(start_sample as f32, (end_sample.saturating_sub(1)) as f32);
            if warp_ratio < 1.0 && playhead >= next_boundary {
                playhead = next_boundary.min((end_sample.saturating_sub(1)) as f32);
            }
            if (playhead - old_playhead).abs() > 1.0 {
                warp_prev_playhead = old_playhead;
                warp_xfade_remaining = (sample_rate * WARP_XFADE_SECONDS).max(1.0);
            } else {
                warp_prev_playhead = playhead;
            }
        }
    } else if !warp_active {
        last_warp_target_ratio = warp_target_ratio;
    }

    // ── Pre-loop gate-off check (gate may have changed between blocks) ──
    if env_phase == ENV_ATTACK || env_phase == ENV_SUSTAIN {
        if gate_samples <= 0.0 && loop_mode != 0 {
            env_phase = ENV_RELEASE;
            release_level = env_level;
        } else if loop_mode == 1
            && gate_mode > 0.5
            && gate_samples.is_finite()
            && gate_counter >= gate_samples
        {
            env_phase = ENV_RELEASE;
            release_level = env_level;
        } else if (loop_mode == 2 || loop_mode == 3)
            && gate_samples.is_finite()
            && gate_counter >= gate_samples
        {
            env_phase = ENV_RELEASE;
            release_level = env_level;
        }
    }

    for i in 0..nf {
        if warp_active {
            warp_ratio += (warp_target_ratio - warp_ratio) * warp_ratio_slew;
        }
        let past_forward = playhead >= end_sample as f32;
        let past_reverse = playhead < start_sample as f32;
        if (past_forward || past_reverse) && (loop_mode == 2 || loop_mode == 3) {
            if loop_mode == 3 {
                play_direction = -play_direction;
                playhead = if past_forward {
                    (end_sample.saturating_sub(1)) as f32
                } else {
                    start_sample as f32
                };
            } else if play_direction >= 0.0 {
                playhead = start_sample as f32 + (playhead - end_sample as f32);
            } else {
                playhead =
                    (end_sample.saturating_sub(1)) as f32 - ((start_sample as f32) - playhead);
            }
            if warp_active && play_direction >= 0.0 {
                if let Some(table) = onset_table {
                    reset_forward_warp_state(
                        table,
                        &mut current_slice,
                        &mut slice_project_frame_start,
                        &mut slice_source_frame_start,
                        &mut warp_xfade_remaining,
                        &mut warp_prev_playhead,
                        playhead,
                        gate_counter,
                    );
                }
            }
        }

        // Past end of sample region: stop
        if playhead >= end_sample as f32 || playhead < start_sample as f32 {
            *out0.add(i) = 0.0;
            *out1.add(i) = 0.0;
            env_phase = ENV_IDLE;
            env_level = 0.0;
            *s.add(STATE_PLAYING) = 0.0;
            for j in (i + 1)..nf {
                *out0.add(j) = 0.0;
                *out1.add(j) = 0.0;
            }
            break;
        }

        // ── Envelope state machine (per sample) ──
        // Uses chained `if` (not else-if) so phase transitions within a
        // single sample flow through immediately (e.g. retrigger→attack).

        if env_phase == ENV_RETRIGGER {
            // Fade the previously emitted sample to zero, then begin the new attack.
            // The new playhead advances underneath this silent crossfade so we do not
            // jump directly from the old waveform to the new one.
            *out0.add(i) = retrigger_out_l * env_level;
            *out1.add(i) = retrigger_out_r * env_level;
            last_out_l = *out0.add(i);
            last_out_r = *out1.add(i);

            env_level -= 1.0 / RETRIGGER_FADE_SAMPLES;
            playhead += if warp_active && play_direction >= 0.0 {
                step_rate
            } else {
                step_rate * play_direction
            };
            gate_counter += 1.0;
            if env_level <= 0.0 {
                env_level = 0.0;
                env_phase = ENV_ATTACK;
                post_retrigger = true;
                last_out_l = 0.0;
                last_out_r = 0.0;
            }
            continue;
        }

        if env_phase == ENV_ATTACK {
            // After retrigger, enforce a minimum attack to prevent click
            // on the ramp back up (sample data at playhead≈48 != 0).
            // Fresh triggers from silence keep attack=0 for max punch.
            let eff_attack = if post_retrigger {
                attack_samples.max(MIN_RETRIGGER_ATTACK)
            } else {
                attack_samples
            };
            if eff_attack > 0.0 {
                env_level += 1.0 / eff_attack;
            } else {
                env_level = 1.0;
            }
            if env_level >= 1.0 {
                env_level = 1.0;
                env_phase = ENV_SUSTAIN;
            }
        }

        if env_phase == ENV_SUSTAIN {
            if gate_samples <= 0.0 && loop_mode != 0 {
                // Explicit note-off (keyboard release)
                env_phase = ENV_RELEASE;
                release_level = env_level;
            } else if loop_mode == 1 && gate_mode > 0.5 && gate_counter >= gate_samples {
                // Duration gating (real-time counter, independent of playback rate)
                env_phase = ENV_RELEASE;
                release_level = env_level;
            } else if (loop_mode == 2 || loop_mode == 3)
                && gate_samples.is_finite()
                && gate_counter >= gate_samples
            {
                // Sequenced looping notes must still end at their trigger duration.
                env_phase = ENV_RELEASE;
                release_level = env_level;
            } else if loop_mode == 0
                && ((play_direction >= 0.0 && playhead >= (end_sample as f32 - eff_release))
                    || (play_direction < 0.0 && playhead <= (start_sample as f32 + eff_release)))
            {
                // Auto-release near end of sample (gate off = play full sample)
                env_phase = ENV_RELEASE;
                release_level = env_level;
            }
        }

        if env_phase == ENV_RELEASE {
            if release_level > 0.0 {
                env_level -= release_level / eff_release;
            }
            if env_level <= 0.0 {
                env_level = 0.0;
                env_phase = ENV_IDLE;
                *s.add(STATE_PLAYING) = 0.0;
                *out0.add(i) = 0.0;
                *out1.add(i) = 0.0;
                last_out_l = 0.0;
                last_out_r = 0.0;
                for j in (i + 1)..nf {
                    *out0.add(j) = 0.0;
                    *out1.add(j) = 0.0;
                }
                break;
            }
        }

        // ── Read sample with linear interpolation ──

        if warp_active && play_direction >= 0.0 {
            if let Some(table) = onset_table {
                if current_slice < table.onsets_frames.len() {
                    let next = table.onsets_frames[current_slice] as f32;
                    let next_project_frame =
                        slice_project_frame_start + (next - slice_source_frame_start) / warp_ratio;
                    if gate_counter >= next_project_frame {
                        warp_prev_playhead = playhead;
                        slice_project_frame_start = gate_counter;
                        playhead = next;
                        slice_source_frame_start = next;
                        current_slice += 1;
                        warp_xfade_remaining = (sample_rate * WARP_XFADE_SECONDS).max(1.0);
                    }
                }
            }
        }

        let mut warp_silent = false;
        if warp_active && play_direction >= 0.0 {
            if let Some(table) = onset_table {
                if warp_ratio < 1.0 && current_slice < table.onsets_frames.len() {
                    let next = table.onsets_frames[current_slice] as f32;
                    if playhead >= next {
                        warp_silent = true;
                    }
                }
            }
        }

        let (mut sample_l, mut sample_r) = if warp_silent {
            (0.0, 0.0)
        } else {
            read_interpolated(sample_data, sample_len, channel_count, playhead)
        };
        if loop_mode == 2 && loop_xfade > 0.0 {
            let fade_pos = if play_direction >= 0.0 {
                (end_sample as f32 - playhead) / loop_xfade
            } else {
                (playhead - start_sample as f32) / loop_xfade
            };
            if fade_pos <= 1.0 {
                let target_head = if play_direction >= 0.0 {
                    start_sample as f32 + (loop_xfade - (end_sample as f32 - playhead))
                } else {
                    (end_sample.saturating_sub(1)) as f32
                        - (loop_xfade - (playhead - start_sample as f32))
                }
                .clamp(start_sample as f32, (end_sample.saturating_sub(1)) as f32);
                let (wrapped_l, wrapped_r) =
                    read_interpolated(sample_data, sample_len, channel_count, target_head);
                let mix = (1.0 - fade_pos.clamp(0.0, 1.0)).clamp(0.0, 1.0);
                sample_l = sample_l * (1.0 - mix) + wrapped_l * mix;
                sample_r = sample_r * (1.0 - mix) + wrapped_r * mix;
            }
        }
        if warp_active && play_direction >= 0.0 && warp_xfade_remaining > 0.0 {
            let total = (sample_rate * WARP_XFADE_SECONDS).max(1.0);
            let t = 1.0 - (warp_xfade_remaining / total).clamp(0.0, 1.0);
            let old_gain = (t * std::f32::consts::FRAC_PI_2).cos();
            let new_gain = (t * std::f32::consts::FRAC_PI_2).sin();
            let (old_l, old_r) =
                read_interpolated(sample_data, sample_len, channel_count, warp_prev_playhead);
            sample_l = old_l * old_gain + sample_l * new_gain;
            sample_r = old_r * old_gain + sample_r * new_gain;
            warp_prev_playhead += step_rate;
            warp_xfade_remaining -= 1.0;
        }
        if sr_reduced {
            if sr_phase <= 0.0 {
                sr_held_l = sample_l;
                sr_held_r = sample_r;
                sr_phase += sr_step;
            }
            sr_phase -= 1.0;
            sample_l = sr_held_l;
            sample_r = sr_held_r;
        }
        let env_amp = amplitude * env_level;

        *out0.add(i) = sample_l * env_amp;
        *out1.add(i) = sample_r * env_amp;
        last_out_l = *out0.add(i);
        last_out_r = *out1.add(i);

        playhead += if warp_active && play_direction >= 0.0 {
            step_rate
        } else {
            step_rate * play_direction
        };
        gate_counter += 1.0; // real-time counter (1 per sample, independent of transpose/speed)
    }

    // Write back persistent state
    *s.add(STATE_PLAYHEAD) = playhead;
    *s.add(STATE_ENV_PHASE) = env_phase;
    *s.add(STATE_ENV_LEVEL) = env_level;
    *s.add(STATE_RELEASE_LEVEL) = release_level;
    *s.add(STATE_GATE_COUNTER) = gate_counter;
    *s.add(STATE_LAST_OUT_L) = last_out_l;
    *s.add(STATE_LAST_OUT_R) = last_out_r;
    *s.add(STATE_RETRIGGER_OUT_L) = retrigger_out_l;
    *s.add(STATE_RETRIGGER_OUT_R) = retrigger_out_r;
    *s.add(STATE_PLAY_DIRECTION) = play_direction;
    *s.add(STATE_SR_PHASE) = sr_phase;
    *s.add(STATE_SR_HELD_L) = sr_held_l;
    *s.add(STATE_SR_HELD_R) = sr_held_r;
    *s.add(STATE_WARP_RATIO) = warp_ratio;
    *s.add(STATE_WARP_CURRENT_SLICE) = current_slice as f32;
    *s.add(STATE_WARP_SLICE_PROJECT_FRAME_START) = slice_project_frame_start;
    *s.add(STATE_WARP_XFADE_REMAINING) = warp_xfade_remaining;
    *s.add(STATE_WARP_PREV_PLAYHEAD) = warp_prev_playhead;
    *s.add(STATE_WARP_SLICE_SOURCE_FRAME_START) = slice_source_frame_start;
    *s.add(STATE_WARP_LAST_TARGET_RATIO) = last_warp_target_ratio;
}

pub fn sampler_vtable() -> NodeVTable {
    NodeVTable {
        process: Some(sampler_process),
        init: Some(sampler_init),
        reset: None,
        migrate: None,
    }
}

/// Load a WAV file into an audiograph buffer and retain the trimmed mono data
/// needed by the offline analyzer.
pub fn load_wav_buffer(lg: *mut LiveGraph, wav_path: &Path) -> Result<LoadedSample, String> {
    let reader =
        hound::WavReader::open(wav_path).map_err(|e| format!("Failed to open WAV: {e}"))?;
    let spec = reader.spec();
    let channels = spec.channels as usize;

    let samples_f32: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Int => {
            let max_val = (1u32 << (spec.bits_per_sample - 1)) as f32;
            reader
                .into_samples::<i32>()
                .filter_map(|s| s.ok())
                .map(|s| s as f32 / max_val)
                .collect()
        }
        hound::SampleFormat::Float => reader
            .into_samples::<f32>()
            .filter_map(|s| s.ok())
            .collect(),
    };

    let stereo: Vec<f32> = if channels == 1 {
        samples_f32
            .iter()
            .flat_map(|&sample| [sample, sample])
            .collect()
    } else if channels == 2 {
        samples_f32
    } else {
        samples_f32
            .chunks(channels)
            .flat_map(|ch| {
                let left = ch.iter().step_by(2).copied().sum::<f32>()
                    / ch.iter().step_by(2).count() as f32;
                let right = ch.iter().skip(1).step_by(2).copied().sum::<f32>()
                    / ch.iter().skip(1).step_by(2).count().max(1) as f32;
                [left, right]
            })
            .collect()
    };

    // Skip leading silence: scan with 64-sample RMS windows, threshold -60dB (~0.001)
    let skip = {
        const WINDOW: usize = 64;
        const THRESHOLD: f32 = 0.001;
        let thresh_sq = THRESHOLD * THRESHOLD;
        let mut start = 0usize;
        for chunk in stereo.chunks(WINDOW * 2) {
            let frames = chunk.len() / 2;
            let sum_sq: f32 = chunk.iter().map(|s| s * s).sum();
            if frames > 0 && sum_sq / (frames * 2) as f32 > thresh_sq {
                break;
            }
            start += frames;
        }
        start.min(stereo.len() / 2)
    };
    let trimmed = &stereo[skip * 2..];
    let mono_samples: Vec<f32> = trimmed
        .chunks_exact(2)
        .map(|frame| 0.5 * (frame[0] + frame[1]))
        .collect();

    let buffer_id = unsafe { create_buffer(lg, (trimmed.len() / 2) as c_int, 2, trimmed.as_ptr()) };
    if buffer_id < 0 {
        return Err("Failed to create buffer".to_string());
    }

    let name = wav_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("sample")
        .to_string();

    Ok(LoadedSample {
        buffer_id,
        name,
        mono_samples,
        sample_rate: spec.sample_rate,
        frames: trimmed.len() / 2,
    })
}

pub fn create_silent_buffer(lg: *mut LiveGraph) -> Result<i32, String> {
    let silent = [0.0_f32, 0.0_f32];
    let buffer_id = unsafe { create_buffer(lg, 1, 2, silent.as_ptr()) };
    if buffer_id < 0 {
        return Err("Failed to create silent sampler buffer".to_string());
    }
    Ok(buffer_id)
}

/// Create a sampler node from an existing buffer ID.
pub fn create_sampler_node(
    lg: *mut LiveGraph,
    buffer_id: i32,
    name: &str,
) -> Result<SamplerTrack, String> {
    let initial_state = [buffer_id as f32];
    let c_name = CString::new(name).unwrap();

    let node_id = unsafe {
        add_node(
            lg,
            sampler_vtable(),
            SAMPLER_STATE_SIZE * std::mem::size_of::<f32>(),
            c_name.as_ptr(),
            0,
            2,
            initial_state.as_ptr() as *const c_void,
            initial_state.len() * std::mem::size_of::<f32>(),
        )
    };

    if node_id < 0 {
        return Err("Failed to add sampler node".to_string());
    }

    Ok(SamplerTrack {
        name: name.to_string(),
        node_id,
        logical_id: node_id as u64,
        buffer_id,
    })
}

/// Load a WAV file, create an audiograph buffer and sampler node.
pub fn create_sampler_track(lg: *mut LiveGraph, wav_path: &Path) -> Result<SamplerTrack, String> {
    let loaded = load_wav_buffer(lg, wav_path)?;
    create_sampler_node(lg, loaded.buffer_id, &loaded.name)
}
