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
const STATE_WARP_NEXT_BOUNDARY: usize = 36; // next Beats segment boundary (source frames)
const STATE_WARP_SLICE_PROJECT_FRAME_START: usize = 37;
const STATE_WARP_XFADE_REMAINING: usize = 38;
const STATE_WARP_PREV_PLAYHEAD: usize = 39;
const STATE_WARP_SAMPLE_BPM: usize = 40;
const STATE_WARP_PROJECT_BPM: usize = 41;
const STATE_WARP_SLICE_SOURCE_FRAME_START: usize = 42;
const STATE_WARP_LAST_TARGET_RATIO: usize = 43;
const STATE_SOURCE_SAMPLE_RATE: usize = 44;
const STATE_SCRUB_OFFSET: usize = 45;
const STATE_SCRUB_SMOOTH: usize = 46;
const STATE_MOD_SPEED_SOURCE: usize = 47;
const STATE_MOD_SPEED_DEPTH: usize = 48;
const STATE_MOD_SCRUB_SOURCE: usize = 49;
const STATE_MOD_SCRUB_DEPTH: usize = 50;
const STATE_MOD_SR_SOURCE: usize = 51;
const STATE_MOD_SR_DEPTH: usize = 52;
const STATE_MOD_WARP_BPM_SOURCE: usize = 53;
const STATE_MOD_WARP_BPM_DEPTH: usize = 54;
const STATE_MOD_START_SOURCE: usize = 55;
const STATE_MOD_START_DEPTH: usize = 56;
const STATE_MOD_END_SOURCE: usize = 57;
const STATE_MOD_END_DEPTH: usize = 58;
const STATE_MOD_SPEED_LANE2_SOURCE: usize = 59;
const STATE_MOD_SPEED_LANE2_DEPTH: usize = 60;
const STATE_MOD_SCRUB_LANE2_SOURCE: usize = 61;
const STATE_MOD_SCRUB_LANE2_DEPTH: usize = 62;
const STATE_MOD_SR_LANE2_SOURCE: usize = 63;
const STATE_MOD_SR_LANE2_DEPTH: usize = 64;
const STATE_MOD_WARP_BPM_LANE2_SOURCE: usize = 65;
const STATE_MOD_WARP_BPM_LANE2_DEPTH: usize = 66;
const STATE_MOD_START_LANE2_SOURCE: usize = 67;
const STATE_MOD_START_LANE2_DEPTH: usize = 68;
const STATE_MOD_END_LANE2_SOURCE: usize = 69;
const STATE_MOD_END_LANE2_DEPTH: usize = 70;
const STATE_MOD_SPEED_LANE3_SOURCE: usize = 71;
const STATE_MOD_SPEED_LANE3_DEPTH: usize = 72;
const STATE_MOD_SCRUB_LANE3_SOURCE: usize = 73;
const STATE_MOD_SCRUB_LANE3_DEPTH: usize = 74;
const STATE_MOD_SR_LANE3_SOURCE: usize = 75;
const STATE_MOD_SR_LANE3_DEPTH: usize = 76;
const STATE_MOD_WARP_BPM_LANE3_SOURCE: usize = 77;
const STATE_MOD_WARP_BPM_LANE3_DEPTH: usize = 78;
const STATE_MOD_START_LANE3_SOURCE: usize = 79;
const STATE_MOD_START_LANE3_DEPTH: usize = 80;
const STATE_MOD_END_LANE3_SOURCE: usize = 81;
const STATE_MOD_END_LANE3_DEPTH: usize = 82;
const STATE_MOD_SPEED_LANE4_SOURCE: usize = 83;
const STATE_MOD_SPEED_LANE4_DEPTH: usize = 84;
const STATE_MOD_SCRUB_LANE4_SOURCE: usize = 85;
const STATE_MOD_SCRUB_LANE4_DEPTH: usize = 86;
const STATE_MOD_SR_LANE4_SOURCE: usize = 87;
const STATE_MOD_SR_LANE4_DEPTH: usize = 88;
const STATE_MOD_WARP_BPM_LANE4_SOURCE: usize = 89;
const STATE_MOD_WARP_BPM_LANE4_DEPTH: usize = 90;
const STATE_MOD_START_LANE4_SOURCE: usize = 91;
const STATE_MOD_START_LANE4_DEPTH: usize = 92;
const STATE_MOD_END_LANE4_SOURCE: usize = 93;
const STATE_MOD_END_LANE4_DEPTH: usize = 94;
const STATE_SCRUB_SMOOTH_TIME_MS: usize = 95;
const STATE_LAST_ENV_AMP: usize = 96;
const STATE_RETRIGGER_PLAYHEAD: usize = 97;
const STATE_RETRIGGER_DIRECTION: usize = 98;
const STATE_RETRIGGER_AMP: usize = 99;
const STATE_RETRIGGER_SR_PHASE: usize = 100;
const STATE_RETRIGGER_SR_HELD_L: usize = 101;
const STATE_RETRIGGER_SR_HELD_R: usize = 102;
const STATE_LAST_READ_HEAD: usize = 103;
const STATE_RETRIGGER_FADE_REMAINING: usize = 104;
const STATE_WARP_PRESERVE: usize = 105; // Beats grid division (warp_grid::PRESERVE_*)
const STATE_WARP_SEG_LOOP_MODE: usize = 106; // 0=off, 1=loop, 2=ping-pong tail fill
const STATE_WARP_SEG_ENVELOPE: usize = 107; // 0..1 per-segment decay amount
const STATE_WARP_SEG_TAIL_DIR: usize = 108; // ±1, ping-pong tail direction
const STATE_WARP_LAST_PRESERVE: usize = 109;
const STATE_TIMELINE_COUNT: usize = 110;
const STATE_TIMELINE_BASE: usize = 111;
const TIMELINE_EVENT_WIDTH: usize = 3 + GBE_AUX_CAP;
const TIMELINE_FRAME: usize = 0;
const TIMELINE_KIND: usize = 1;
const TIMELINE_AUX_COUNT: usize = 2;
const TIMELINE_AUX_BASE: usize = 3;
pub const SAMPLER_TIMELINE_CAPACITY: usize = crate::sequencer::MAX_STEPS;
pub const SAMPLER_STATE_SIZE: usize =
    STATE_TIMELINE_BASE + SAMPLER_TIMELINE_CAPACITY * TIMELINE_EVENT_WIDTH;
pub const SAMPLER_PARAM_ENABLED: u64 = STATE_ENABLED as u64;

pub const SAMPLER_EVENT_AUX_ENABLED: usize = 0;
pub const SAMPLER_EVENT_AUX_VELOCITY: usize = 1;
pub const SAMPLER_EVENT_AUX_SPEED: usize = 2;
pub const SAMPLER_EVENT_AUX_GATE_SAMPLES: usize = 3;
pub const SAMPLER_EVENT_AUX_TRANSPOSE: usize = 4;
pub const SAMPLER_EVENT_AUX_ATTACK_SAMPLES: usize = 5;
pub const SAMPLER_EVENT_AUX_RELEASE_SAMPLES: usize = 6;
pub const SAMPLER_EVENT_AUX_GATE_MODE: usize = 7;
pub const SAMPLER_EVENT_AUX_START_POINT: usize = 8;
pub const SAMPLER_EVENT_AUX_END_POINT: usize = 9;
pub const SAMPLER_EVENT_AUX_REVERSE: usize = 10;
pub const SAMPLER_EVENT_AUX_LOOP_MODE: usize = 11;
pub const SAMPLER_EVENT_AUX_LOOP_XFADE_SAMPLES: usize = 12;
pub const SAMPLER_EVENT_AUX_SR_HZ: usize = 13;
pub const SAMPLER_EVENT_AUX_WARP_ENABLED: usize = 14;
pub const SAMPLER_EVENT_AUX_WARP_MODE: usize = 15;
pub const SAMPLER_EVENT_AUX_WARP_RATIO: usize = 16;
pub const SAMPLER_EVENT_AUX_WARP_SAMPLE_BPM: usize = 17;
pub const SAMPLER_EVENT_AUX_WARP_PROJECT_BPM: usize = 18;
pub const SAMPLER_EVENT_AUX_WARP_PTR_LO: usize = 19;
pub const SAMPLER_EVENT_AUX_WARP_PTR_HI: usize = 20;
pub const SAMPLER_EVENT_AUX_SCRUB_OFFSET: usize = 21;
pub const SAMPLER_EVENT_AUX_WARP_PRESERVE: usize = 22;
pub const SAMPLER_EVENT_AUX_WARP_SEG_LOOP_MODE: usize = 23;
pub const SAMPLER_EVENT_AUX_WARP_SEG_ENVELOPE: usize = 24;
const SAMPLER_EVENT_AUX_REQUIRED_NOTE_ON_COUNT: usize = SAMPLER_EVENT_AUX_SCRUB_OFFSET + 1;
pub const SAMPLER_EVENT_AUX_NOTE_ON_COUNT: usize = 25;

// Envelope phase constants
const ENV_IDLE: f32 = 0.0;
const ENV_ATTACK: f32 = 1.0;
const ENV_SUSTAIN: f32 = 2.0;
const ENV_RELEASE: f32 = 3.0;
const ENV_RETRIGGER: f32 = 4.0; // legacy retrigger phase, migrated to ATTACK at process time

// Minimum release to prevent clicks (in samples, ~1.5ms at 44100)
const MIN_RELEASE_SAMPLES: f32 = 64.0;
// Retrigger fade-down duration. A new trigger while the old envelope is nonzero
// first fades the previous output to silence, then starts the new amp envelope.
const RETRIGGER_FADE_SECONDS: f32 = 0.004;
const WARP_XFADE_SECONDS: f32 = 0.005;
const MOD_INPUT_COUNT: usize = crate::voice_modulator::SLOT_COUNT;
const SR_MIN_HZ: f32 = 2_000.0;
const SR_MAX_HZ: f32 = 44_100.0;
const SPEED_MIN: f32 = -4.0;
const SPEED_MAX: f32 = 4.0;
const WARP_BPM_MIN: f32 = 20.0;
const WARP_BPM_MAX: f32 = 400.0;
// Warp mode enum (matches the "mode" param labels in builtin_sampler()).
pub const WARP_MODE_BEATS: i32 = 0;
pub const WARP_MODE_TONES: i32 = 1;
pub const WARP_MODE_TEXTURE: i32 = 2;
pub const WARP_MODE_REPITCH: i32 = 3;
// Beats-mode tail fill when the segment runs out of natural content before
// the next boundary is due (project slower than sample).
pub const SEG_LOOP_OFF: i32 = 0;
pub const SEG_LOOP_FORWARD: i32 = 1;
pub const SEG_LOOP_PINGPONG: i32 = 2;
pub const WARP_PRESERVE_DEFAULT: i32 = crate::warp_grid::PRESERVE_TRANSIENTS;
pub const WARP_SEG_LOOP_MODE_DEFAULT: i32 = SEG_LOOP_FORWARD;
pub const WARP_SEG_ENVELOPE_DEFAULT: f32 = 0.0;
const SCRUB_SMOOTH_TIME_MS_DEFAULT: f32 = 6.0;
const SCRUB_RELEASE_EPSILON: f32 = 0.000_1;

const MOD_SPEED_LANES: [(usize, usize); SAMPLER_MOD_LANES_PER_PARAM] = [
    (STATE_MOD_SPEED_SOURCE, STATE_MOD_SPEED_DEPTH),
    (STATE_MOD_SPEED_LANE2_SOURCE, STATE_MOD_SPEED_LANE2_DEPTH),
    (STATE_MOD_SPEED_LANE3_SOURCE, STATE_MOD_SPEED_LANE3_DEPTH),
    (STATE_MOD_SPEED_LANE4_SOURCE, STATE_MOD_SPEED_LANE4_DEPTH),
];
const MOD_SCRUB_LANES: [(usize, usize); SAMPLER_MOD_LANES_PER_PARAM] = [
    (STATE_MOD_SCRUB_SOURCE, STATE_MOD_SCRUB_DEPTH),
    (STATE_MOD_SCRUB_LANE2_SOURCE, STATE_MOD_SCRUB_LANE2_DEPTH),
    (STATE_MOD_SCRUB_LANE3_SOURCE, STATE_MOD_SCRUB_LANE3_DEPTH),
    (STATE_MOD_SCRUB_LANE4_SOURCE, STATE_MOD_SCRUB_LANE4_DEPTH),
];
const MOD_SR_LANES: [(usize, usize); SAMPLER_MOD_LANES_PER_PARAM] = [
    (STATE_MOD_SR_SOURCE, STATE_MOD_SR_DEPTH),
    (STATE_MOD_SR_LANE2_SOURCE, STATE_MOD_SR_LANE2_DEPTH),
    (STATE_MOD_SR_LANE3_SOURCE, STATE_MOD_SR_LANE3_DEPTH),
    (STATE_MOD_SR_LANE4_SOURCE, STATE_MOD_SR_LANE4_DEPTH),
];
const MOD_WARP_BPM_LANES: [(usize, usize); SAMPLER_MOD_LANES_PER_PARAM] = [
    (STATE_MOD_WARP_BPM_SOURCE, STATE_MOD_WARP_BPM_DEPTH),
    (
        STATE_MOD_WARP_BPM_LANE2_SOURCE,
        STATE_MOD_WARP_BPM_LANE2_DEPTH,
    ),
    (
        STATE_MOD_WARP_BPM_LANE3_SOURCE,
        STATE_MOD_WARP_BPM_LANE3_DEPTH,
    ),
    (
        STATE_MOD_WARP_BPM_LANE4_SOURCE,
        STATE_MOD_WARP_BPM_LANE4_DEPTH,
    ),
];
const MOD_START_LANES: [(usize, usize); SAMPLER_MOD_LANES_PER_PARAM] = [
    (STATE_MOD_START_SOURCE, STATE_MOD_START_DEPTH),
    (STATE_MOD_START_LANE2_SOURCE, STATE_MOD_START_LANE2_DEPTH),
    (STATE_MOD_START_LANE3_SOURCE, STATE_MOD_START_LANE3_DEPTH),
    (STATE_MOD_START_LANE4_SOURCE, STATE_MOD_START_LANE4_DEPTH),
];
const MOD_END_LANES: [(usize, usize); SAMPLER_MOD_LANES_PER_PARAM] = [
    (STATE_MOD_END_SOURCE, STATE_MOD_END_DEPTH),
    (STATE_MOD_END_LANE2_SOURCE, STATE_MOD_END_LANE2_DEPTH),
    (STATE_MOD_END_LANE3_SOURCE, STATE_MOD_END_LANE3_DEPTH),
    (STATE_MOD_END_LANE4_SOURCE, STATE_MOD_END_LANE4_DEPTH),
];
const ALL_MOD_LANES: [(usize, usize); SAMPLER_MOD_LANES_PER_PARAM * 6] = [
    MOD_SPEED_LANES[0],
    MOD_SPEED_LANES[1],
    MOD_SPEED_LANES[2],
    MOD_SPEED_LANES[3],
    MOD_SCRUB_LANES[0],
    MOD_SCRUB_LANES[1],
    MOD_SCRUB_LANES[2],
    MOD_SCRUB_LANES[3],
    MOD_SR_LANES[0],
    MOD_SR_LANES[1],
    MOD_SR_LANES[2],
    MOD_SR_LANES[3],
    MOD_WARP_BPM_LANES[0],
    MOD_WARP_BPM_LANES[1],
    MOD_WARP_BPM_LANES[2],
    MOD_WARP_BPM_LANES[3],
    MOD_START_LANES[0],
    MOD_START_LANES[1],
    MOD_START_LANES[2],
    MOD_START_LANES[3],
    MOD_END_LANES[0],
    MOD_END_LANES[1],
    MOD_END_LANES[2],
    MOD_END_LANES[3],
];

fn sampler_playback_step(source_sample_rate: f32, host_sample_rate: f32, rate: f32) -> f32 {
    let source_sample_rate = source_sample_rate.max(1.0);
    let host_sample_rate = host_sample_rate.max(1.0);
    rate * source_sample_rate / host_sample_rate
}

fn source_frames_to_host_frames(
    source_frames: f32,
    source_sample_rate: f32,
    host_sample_rate: f32,
) -> f32 {
    let source_sample_rate = source_sample_rate.max(1.0);
    let host_sample_rate = host_sample_rate.max(1.0);
    source_frames * host_sample_rate / source_sample_rate
}

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

unsafe fn modulation_input(
    inp: *const *mut f32,
    n_inputs: usize,
    source: f32,
    frame: usize,
) -> f32 {
    let source_idx = source.round() as i32;
    if source_idx <= 0 {
        return 0.0;
    }
    let input_idx = (source_idx - 1) as usize;
    if input_idx >= MOD_INPUT_COUNT || input_idx >= n_inputs {
        return 0.0;
    }
    let ptr = *inp.add(input_idx);
    if ptr.is_null() {
        0.0
    } else {
        (*ptr.add(frame)).clamp(-1.0, 1.0)
    }
}

unsafe fn modulation_lane_sum(
    inp: *const *mut f32,
    n_inputs: usize,
    state: *mut f32,
    lanes: &[(usize, usize); SAMPLER_MOD_LANES_PER_PARAM],
    frame: usize,
) -> f32 {
    lanes.iter().fold(0.0, |sum, (source_idx, depth_idx)| {
        let source = *state.add(*source_idx);
        let depth = *state.add(*depth_idx);
        sum + modulation_input(inp, n_inputs, source, frame) * depth
    })
}

fn modulated_normalized(base: f32, modulation: f32) -> f32 {
    (base + modulation).clamp(0.0, 1.0)
}

fn retrigger_fade_samples(sample_rate: f32) -> f32 {
    (sample_rate.max(1.0) * RETRIGGER_FADE_SECONDS).max(1.0)
}

fn retrigger_tail_gain(remaining_samples: f32, total_samples: f32) -> f32 {
    let progress = 1.0 - (remaining_samples / total_samples.max(1.0)).clamp(0.0, 1.0);
    0.5 * (1.0 + (std::f32::consts::PI * progress).cos())
}

fn advance_retrigger_playhead(
    playhead: f32,
    direction: f32,
    step: f32,
    start_sample: usize,
    end_sample: usize,
    loop_mode: i32,
) -> (f32, f32, bool) {
    let start = start_sample as f32;
    let end = end_sample as f32;
    let mut next_playhead = playhead + step * direction;
    let mut next_direction = direction;
    let past_forward = next_playhead >= end;
    let past_reverse = next_playhead < start;

    if !(past_forward || past_reverse) {
        return (next_playhead, next_direction, true);
    }

    if loop_mode == 2 || loop_mode == 3 {
        if loop_mode == 3 {
            next_direction = -next_direction;
            next_playhead = if past_forward {
                end_sample.saturating_sub(1) as f32
            } else {
                start
            };
        } else if next_direction >= 0.0 {
            let len = (end - start).max(1.0);
            next_playhead = start + (next_playhead - start).rem_euclid(len);
        } else {
            let len = (end - start).max(1.0);
            next_playhead = start + (next_playhead - start).rem_euclid(len);
        }
        (
            next_playhead.clamp(start, end_sample.saturating_sub(1) as f32),
            next_direction,
            true,
        )
    } else {
        (next_playhead, next_direction, false)
    }
}

fn accumulated_scrub_read_head(
    playhead: &mut f32,
    scrub_smooth: &mut f32,
    target_scrub: f32,
    region_len: f32,
    start_sample: f32,
    end_sample: f32,
    sample_rate: f32,
    smooth_time_ms: f32,
) -> f32 {
    let max_read_head = end_sample.max(start_sample);
    let target_scrub = target_scrub.clamp(-1.0, 1.0);
    if target_scrub.abs() <= SCRUB_RELEASE_EPSILON && scrub_smooth.abs() > SCRUB_RELEASE_EPSILON {
        *playhead = (*playhead + *scrub_smooth * region_len).clamp(start_sample, max_read_head);
        *scrub_smooth = 0.0;
        return *playhead;
    }
    if target_scrub.abs() <= SCRUB_RELEASE_EPSILON {
        *scrub_smooth = 0.0;
        return (*playhead).clamp(start_sample, max_read_head);
    }
    if scrub_smooth.abs() > SCRUB_RELEASE_EPSILON && target_scrub.signum() != scrub_smooth.signum()
    {
        *playhead = (*playhead + *scrub_smooth * region_len).clamp(start_sample, max_read_head);
        *scrub_smooth = 0.0;
    }

    if target_scrub.abs() > scrub_smooth.abs() {
        let smooth_seconds = smooth_time_ms.clamp(0.0, 5000.0) * 0.001;
        let scrub_coeff = (1.0 / (sample_rate * smooth_seconds)).clamp(0.0001, 1.0);
        *scrub_smooth += (target_scrub - *scrub_smooth) * scrub_coeff;
    }
    (*playhead + *scrub_smooth * region_len).clamp(start_sample, max_read_head)
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
pub const PARAM_WARP_PRESERVE: u64 = STATE_WARP_PRESERVE as u64;
pub const PARAM_WARP_SEG_LOOP_MODE: u64 = STATE_WARP_SEG_LOOP_MODE as u64;
pub const PARAM_WARP_SEG_ENVELOPE: u64 = STATE_WARP_SEG_ENVELOPE as u64;
pub const PARAM_SOURCE_SAMPLE_RATE: u64 = STATE_SOURCE_SAMPLE_RATE as u64;
pub const PARAM_SCRUB_OFFSET: u64 = STATE_SCRUB_OFFSET as u64;
pub const PARAM_SCRUB_SMOOTH: u64 = STATE_SCRUB_SMOOTH as u64;
pub const PARAM_SCRUB_SMOOTH_TIME_MS: u64 = STATE_SCRUB_SMOOTH_TIME_MS as u64;
pub const PARAM_MOD_SPEED_SOURCE: u64 = STATE_MOD_SPEED_SOURCE as u64;
pub const PARAM_MOD_SPEED_DEPTH: u64 = STATE_MOD_SPEED_DEPTH as u64;
pub const PARAM_MOD_SCRUB_SOURCE: u64 = STATE_MOD_SCRUB_SOURCE as u64;
pub const PARAM_MOD_SCRUB_DEPTH: u64 = STATE_MOD_SCRUB_DEPTH as u64;
pub const PARAM_MOD_SR_SOURCE: u64 = STATE_MOD_SR_SOURCE as u64;
pub const PARAM_MOD_SR_DEPTH: u64 = STATE_MOD_SR_DEPTH as u64;
pub const PARAM_MOD_WARP_BPM_SOURCE: u64 = STATE_MOD_WARP_BPM_SOURCE as u64;
pub const PARAM_MOD_WARP_BPM_DEPTH: u64 = STATE_MOD_WARP_BPM_DEPTH as u64;
pub const PARAM_MOD_START_SOURCE: u64 = STATE_MOD_START_SOURCE as u64;
pub const PARAM_MOD_START_DEPTH: u64 = STATE_MOD_START_DEPTH as u64;
pub const PARAM_MOD_END_SOURCE: u64 = STATE_MOD_END_SOURCE as u64;
pub const PARAM_MOD_END_DEPTH: u64 = STATE_MOD_END_DEPTH as u64;
pub const SAMPLER_MOD_LANES_PER_PARAM: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SamplerModTargetParam {
    pub destination: &'static str,
    pub lane: usize,
    pub source_param: u64,
    pub depth_param: u64,
}

pub const SAMPLER_MOD_TARGET_PARAMS: [SamplerModTargetParam; SAMPLER_MOD_LANES_PER_PARAM * 6] = [
    SamplerModTargetParam {
        destination: "speed",
        lane: 1,
        source_param: STATE_MOD_SPEED_SOURCE as u64,
        depth_param: STATE_MOD_SPEED_DEPTH as u64,
    },
    SamplerModTargetParam {
        destination: "speed",
        lane: 2,
        source_param: STATE_MOD_SPEED_LANE2_SOURCE as u64,
        depth_param: STATE_MOD_SPEED_LANE2_DEPTH as u64,
    },
    SamplerModTargetParam {
        destination: "speed",
        lane: 3,
        source_param: STATE_MOD_SPEED_LANE3_SOURCE as u64,
        depth_param: STATE_MOD_SPEED_LANE3_DEPTH as u64,
    },
    SamplerModTargetParam {
        destination: "speed",
        lane: 4,
        source_param: STATE_MOD_SPEED_LANE4_SOURCE as u64,
        depth_param: STATE_MOD_SPEED_LANE4_DEPTH as u64,
    },
    SamplerModTargetParam {
        destination: "scrub",
        lane: 1,
        source_param: STATE_MOD_SCRUB_SOURCE as u64,
        depth_param: STATE_MOD_SCRUB_DEPTH as u64,
    },
    SamplerModTargetParam {
        destination: "scrub",
        lane: 2,
        source_param: STATE_MOD_SCRUB_LANE2_SOURCE as u64,
        depth_param: STATE_MOD_SCRUB_LANE2_DEPTH as u64,
    },
    SamplerModTargetParam {
        destination: "scrub",
        lane: 3,
        source_param: STATE_MOD_SCRUB_LANE3_SOURCE as u64,
        depth_param: STATE_MOD_SCRUB_LANE3_DEPTH as u64,
    },
    SamplerModTargetParam {
        destination: "scrub",
        lane: 4,
        source_param: STATE_MOD_SCRUB_LANE4_SOURCE as u64,
        depth_param: STATE_MOD_SCRUB_LANE4_DEPTH as u64,
    },
    SamplerModTargetParam {
        destination: "sr",
        lane: 1,
        source_param: STATE_MOD_SR_SOURCE as u64,
        depth_param: STATE_MOD_SR_DEPTH as u64,
    },
    SamplerModTargetParam {
        destination: "sr",
        lane: 2,
        source_param: STATE_MOD_SR_LANE2_SOURCE as u64,
        depth_param: STATE_MOD_SR_LANE2_DEPTH as u64,
    },
    SamplerModTargetParam {
        destination: "sr",
        lane: 3,
        source_param: STATE_MOD_SR_LANE3_SOURCE as u64,
        depth_param: STATE_MOD_SR_LANE3_DEPTH as u64,
    },
    SamplerModTargetParam {
        destination: "sr",
        lane: 4,
        source_param: STATE_MOD_SR_LANE4_SOURCE as u64,
        depth_param: STATE_MOD_SR_LANE4_DEPTH as u64,
    },
    SamplerModTargetParam {
        destination: "bpm",
        lane: 1,
        source_param: STATE_MOD_WARP_BPM_SOURCE as u64,
        depth_param: STATE_MOD_WARP_BPM_DEPTH as u64,
    },
    SamplerModTargetParam {
        destination: "bpm",
        lane: 2,
        source_param: STATE_MOD_WARP_BPM_LANE2_SOURCE as u64,
        depth_param: STATE_MOD_WARP_BPM_LANE2_DEPTH as u64,
    },
    SamplerModTargetParam {
        destination: "bpm",
        lane: 3,
        source_param: STATE_MOD_WARP_BPM_LANE3_SOURCE as u64,
        depth_param: STATE_MOD_WARP_BPM_LANE3_DEPTH as u64,
    },
    SamplerModTargetParam {
        destination: "bpm",
        lane: 4,
        source_param: STATE_MOD_WARP_BPM_LANE4_SOURCE as u64,
        depth_param: STATE_MOD_WARP_BPM_LANE4_DEPTH as u64,
    },
    SamplerModTargetParam {
        destination: "start",
        lane: 1,
        source_param: STATE_MOD_START_SOURCE as u64,
        depth_param: STATE_MOD_START_DEPTH as u64,
    },
    SamplerModTargetParam {
        destination: "start",
        lane: 2,
        source_param: STATE_MOD_START_LANE2_SOURCE as u64,
        depth_param: STATE_MOD_START_LANE2_DEPTH as u64,
    },
    SamplerModTargetParam {
        destination: "start",
        lane: 3,
        source_param: STATE_MOD_START_LANE3_SOURCE as u64,
        depth_param: STATE_MOD_START_LANE3_DEPTH as u64,
    },
    SamplerModTargetParam {
        destination: "start",
        lane: 4,
        source_param: STATE_MOD_START_LANE4_SOURCE as u64,
        depth_param: STATE_MOD_START_LANE4_DEPTH as u64,
    },
    SamplerModTargetParam {
        destination: "end",
        lane: 1,
        source_param: STATE_MOD_END_SOURCE as u64,
        depth_param: STATE_MOD_END_DEPTH as u64,
    },
    SamplerModTargetParam {
        destination: "end",
        lane: 2,
        source_param: STATE_MOD_END_LANE2_SOURCE as u64,
        depth_param: STATE_MOD_END_LANE2_DEPTH as u64,
    },
    SamplerModTargetParam {
        destination: "end",
        lane: 3,
        source_param: STATE_MOD_END_LANE3_SOURCE as u64,
        depth_param: STATE_MOD_END_LANE3_DEPTH as u64,
    },
    SamplerModTargetParam {
        destination: "end",
        lane: 4,
        source_param: STATE_MOD_END_LANE4_SOURCE as u64,
        depth_param: STATE_MOD_END_LANE4_DEPTH as u64,
    },
];

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
        *s.add(STATE_SOURCE_SAMPLE_RATE) = (*init.add(1)).max(1.0);
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
    *s.add(STATE_LAST_ENV_AMP) = 0.0;
    *s.add(STATE_RETRIGGER_PLAYHEAD) = 0.0;
    *s.add(STATE_RETRIGGER_DIRECTION) = 1.0;
    *s.add(STATE_RETRIGGER_AMP) = 0.0;
    *s.add(STATE_RETRIGGER_SR_PHASE) = 0.0;
    *s.add(STATE_RETRIGGER_SR_HELD_L) = 0.0;
    *s.add(STATE_RETRIGGER_SR_HELD_R) = 0.0;
    *s.add(STATE_LAST_READ_HEAD) = 0.0;
    *s.add(STATE_RETRIGGER_FADE_REMAINING) = 0.0;
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
    *s.add(STATE_WARP_NEXT_BOUNDARY) = 0.0;
    *s.add(STATE_WARP_PRESERVE) = WARP_PRESERVE_DEFAULT as f32;
    *s.add(STATE_WARP_SEG_LOOP_MODE) = WARP_SEG_LOOP_MODE_DEFAULT as f32;
    *s.add(STATE_WARP_SEG_ENVELOPE) = WARP_SEG_ENVELOPE_DEFAULT;
    *s.add(STATE_WARP_SEG_TAIL_DIR) = 1.0;
    *s.add(STATE_WARP_LAST_PRESERVE) = WARP_PRESERVE_DEFAULT as f32;
    *s.add(STATE_WARP_SLICE_PROJECT_FRAME_START) = 0.0;
    *s.add(STATE_WARP_XFADE_REMAINING) = 0.0;
    *s.add(STATE_WARP_PREV_PLAYHEAD) = 0.0;
    *s.add(STATE_WARP_SAMPLE_BPM) = 120.0;
    *s.add(STATE_WARP_PROJECT_BPM) = 120.0;
    *s.add(STATE_WARP_SLICE_SOURCE_FRAME_START) = 0.0;
    *s.add(STATE_WARP_LAST_TARGET_RATIO) = 1.0;
    if initial_state.is_null() {
        *s.add(STATE_SOURCE_SAMPLE_RATE) = 44_100.0;
    }
    *s.add(STATE_SCRUB_OFFSET) = 0.0;
    *s.add(STATE_SCRUB_SMOOTH) = 0.0;
    *s.add(STATE_SCRUB_SMOOTH_TIME_MS) = SCRUB_SMOOTH_TIME_MS_DEFAULT;
    *s.add(STATE_TIMELINE_COUNT) = 0.0;
    for (source_idx, depth_idx) in ALL_MOD_LANES {
        *s.add(source_idx) = 0.0;
        *s.add(depth_idx) = 0.0;
    }
}

/// extern "C" process — reads sample data from buffer, writes to output.
///
/// Envelope state machine (persists across blocks):
///   IDLE → (trigger) → ATTACK → SUSTAIN → (gate-off) → RELEASE → IDLE
///
/// gate_samples=0 is treated as an explicit note-off regardless of gate_mode,
/// so keyboard release always triggers the release phase.
unsafe fn sampler_process_segment(
    inp: *const *mut f32,
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
    let speed = (*s.add(STATE_SPEED)).clamp(SPEED_MIN, SPEED_MAX);
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
    let mut last_env_amp = *s.add(STATE_LAST_ENV_AMP);
    let mut retrigger_playhead = *s.add(STATE_RETRIGGER_PLAYHEAD);
    let mut retrigger_direction = *s.add(STATE_RETRIGGER_DIRECTION);
    let mut retrigger_amp = *s.add(STATE_RETRIGGER_AMP);
    let mut retrigger_sr_phase = *s.add(STATE_RETRIGGER_SR_PHASE);
    let mut retrigger_sr_held_l = *s.add(STATE_RETRIGGER_SR_HELD_L);
    let mut retrigger_sr_held_r = *s.add(STATE_RETRIGGER_SR_HELD_R);
    let mut last_read_head = *s.add(STATE_LAST_READ_HEAD);
    let mut retrigger_fade_remaining = *s.add(STATE_RETRIGGER_FADE_REMAINING);
    let base_start_point = (*s.add(STATE_START_POINT)).clamp(0.0, 1.0);
    let base_end_point = (*s.add(STATE_END_POINT)).clamp(0.0, 1.0);
    let enabled = *s.add(STATE_ENABLED);
    let reverse_param = *s.add(STATE_REVERSE) > 0.5;
    let loop_mode_param = (*s.add(STATE_LOOP_MODE)).round().clamp(0.0, 3.0) as i32;
    let loop_xfade_samples = (*s.add(STATE_LOOP_XFADE_SAMPLES)).max(0.0);
    let base_sr_hz = *s.add(STATE_SR_HZ);
    let mut play_direction = *s.add(STATE_PLAY_DIRECTION);
    let mut sr_phase = *s.add(STATE_SR_PHASE);
    let mut sr_held_l = *s.add(STATE_SR_HELD_L);
    let mut sr_held_r = *s.add(STATE_SR_HELD_R);
    let sample_rate = (*s.add(STATE_SAMPLE_RATE)).max(1.0);
    let retrigger_fade_samples = retrigger_fade_samples(sample_rate);
    let source_sample_rate = (*s.add(STATE_SOURCE_SAMPLE_RATE)).max(1.0);
    let warp_enabled = *s.add(STATE_WARP_ENABLED) > 0.5;
    let warp_mode = (*s.add(STATE_WARP_MODE)).round() as i32;
    let mut warp_ratio = (*s.add(STATE_WARP_RATIO)).clamp(0.01, 32.0);
    let base_warp_sample_bpm = (*s.add(STATE_WARP_SAMPLE_BPM)).clamp(WARP_BPM_MIN, WARP_BPM_MAX);
    let warp_project_bpm = (*s.add(STATE_WARP_PROJECT_BPM)).clamp(1.0, 400.0);
    let onset_ptr = crate::analysis::unpack_ptr(
        *s.add(STATE_WARP_ONSET_TABLE_PTR_LO),
        *s.add(STATE_WARP_ONSET_TABLE_PTR_HI),
    );
    let onset_table = if warp_enabled && warp_mode == WARP_MODE_BEATS && !onset_ptr.is_null() {
        Some(&*onset_ptr)
    } else {
        None
    };
    let mut next_boundary_src = *s.add(STATE_WARP_NEXT_BOUNDARY);
    let mut seg_host_start = *s.add(STATE_WARP_SLICE_PROJECT_FRAME_START);
    let mut warp_xfade_remaining = (*s.add(STATE_WARP_XFADE_REMAINING)).max(0.0);
    let mut warp_prev_playhead = *s.add(STATE_WARP_PREV_PLAYHEAD);
    let mut seg_source_start = *s.add(STATE_WARP_SLICE_SOURCE_FRAME_START);
    let mut last_warp_target_ratio = (*s.add(STATE_WARP_LAST_TARGET_RATIO)).clamp(0.01, 32.0);
    let warp_preserve = (*s.add(STATE_WARP_PRESERVE)).round() as i32;
    let seg_loop_mode = (*s.add(STATE_WARP_SEG_LOOP_MODE)).round() as i32;
    let seg_envelope = (*s.add(STATE_WARP_SEG_ENVELOPE)).clamp(0.0, 1.0);
    let mut seg_tail_dir = if *s.add(STATE_WARP_SEG_TAIL_DIR) < 0.0 {
        -1.0_f32
    } else {
        1.0_f32
    };
    let mut last_warp_preserve = (*s.add(STATE_WARP_LAST_PRESERVE)).round() as i32;
    let scrub_offset = (*s.add(STATE_SCRUB_OFFSET)).clamp(-1.0, 1.0);
    let mut scrub_smooth = *s.add(STATE_SCRUB_SMOOTH);
    let scrub_smooth_time_ms = (*s.add(STATE_SCRUB_SMOOTH_TIME_MS)).clamp(0.0, 5000.0);
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
        *s.add(STATE_LAST_ENV_AMP) = 0.0;
        *s.add(STATE_LAST_READ_HEAD) = 0.0;
        *s.add(STATE_RETRIGGER_FADE_REMAINING) = 0.0;
        return;
    }

    // Compute effective sample region from normalized start/end points
    let block_start_mod = modulation_lane_sum(inp, MOD_INPUT_COUNT, s, &MOD_START_LANES, 0);
    let block_end_mod = modulation_lane_sum(inp, MOD_INPUT_COUNT, s, &MOD_END_LANES, 0);
    let start_point = modulated_normalized(base_start_point, block_start_mod);
    let end_point = modulated_normalized(base_end_point, block_end_mod);
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
        *s.add(STATE_LAST_ENV_AMP) = 0.0;
        *s.add(STATE_LAST_READ_HEAD) = 0.0;
        *s.add(STATE_RETRIGGER_FADE_REMAINING) = 0.0;
        return;
    }

    let region_len = (end_sample.saturating_sub(start_sample)).max(1) as f32;
    // Beats runs off the uniform beat grid (bpm + anchor); the onset table is
    // only consulted for Preserve=Transients boundary snapping, so warp no
    // longer requires analysis to have finished.
    let warp_active = warp_enabled && warp_mode == WARP_MODE_BEATS;
    // Tones/Texture fall back to re-pitch until their granular engine lands.
    let warp_repitch = warp_enabled && warp_mode != WARP_MODE_BEATS;
    let reverse = reverse_param && !warp_active;
    let loop_mode = loop_mode_param;
    let loop_xfade = loop_xfade_samples.min(region_len * 0.5);
    let block_speed_mod = modulation_lane_sum(inp, MOD_INPUT_COUNT, s, &MOD_SPEED_LANES, 0);
    let effective_speed = (speed + block_speed_mod).clamp(SPEED_MIN, SPEED_MAX);
    let block_sr_mod = modulation_lane_sum(inp, MOD_INPUT_COUNT, s, &MOD_SR_LANES, 0);
    let sr_hz = (base_sr_hz + block_sr_mod).clamp(SR_MIN_HZ, SR_MAX_HZ);
    let block_warp_bpm_mod = modulation_lane_sum(inp, MOD_INPUT_COUNT, s, &MOD_WARP_BPM_LANES, 0);
    let warp_sample_bpm =
        (base_warp_sample_bpm + block_warp_bpm_mod).clamp(WARP_BPM_MIN, WARP_BPM_MAX);
    let warp_target_ratio = if warp_enabled {
        (warp_project_bpm / warp_sample_bpm).clamp(0.01, 32.0)
    } else {
        warp_ratio
    };
    let effective_rate = effective_speed * (2.0_f32).powf(transpose / 12.0);
    let step_rate = effective_rate.abs().max(0.0);
    let playback_step = sampler_playback_step(source_sample_rate, sample_rate, step_rate);
    let sr_step = if sr_hz > 0.0 {
        (sample_rate / sr_hz).max(1.0)
    } else {
        1.0
    };
    let sr_reduced = sr_hz > 0.0 && sr_hz < sample_rate * 0.98 && sr_hz < 44_100.0 * 0.98;
    let amplitude = velocity * gain;
    let eff_release = release_samples.max(MIN_RELEASE_SAMPLES);
    let warp_ratio_slew = (1.0 / (sample_rate * 0.050)).clamp(0.0001, 1.0);
    // The beat grid anchors at the region start: trimming the start point to
    // a downbeat is what aligns the grid (same contract as v0.1).
    let warp_anchor = start_sample as f64;
    let warp_onsets: &[u32] = onset_table
        .map(|table| table.onsets_frames.as_slice())
        .unwrap_or(&[]);
    let next_seg_boundary = |after_src: f32| -> f32 {
        crate::warp_grid::next_segment_boundary(
            after_src as f64,
            warp_anchor,
            warp_sample_bpm as f64,
            source_sample_rate as f64,
            warp_preserve,
            warp_onsets,
        ) as f32
    };
    let reset_warp_seg = |seg_source_start: &mut f32,
                          seg_host_start: &mut f32,
                          next_boundary_src: &mut f32,
                          warp_xfade_remaining: &mut f32,
                          warp_prev_playhead: &mut f32,
                          seg_tail_dir: &mut f32,
                          playhead: f32,
                          gate_counter: f32| {
        *seg_source_start = playhead;
        *seg_host_start = gate_counter;
        *next_boundary_src = next_seg_boundary(playhead);
        *warp_xfade_remaining = 0.0;
        *warp_prev_playhead = playhead;
        *seg_tail_dir = 1.0;
    };

    // ── Trigger detection ──
    // playhead==0 means params just reset it. Distinguish fresh vs retrigger:
    if playhead == 0.0 {
        let old_read_head = last_read_head;
        let old_play_direction = play_direction;
        let old_sr_phase = sr_phase;
        let old_sr_held_l = sr_held_l;
        let old_sr_held_r = sr_held_r;
        play_direction = if reverse { -1.0 } else { 1.0 };
        playhead = if reverse {
            (end_sample.saturating_sub(1)) as f32
        } else {
            start_sample as f32
        };
        if warp_active {
            playhead = start_sample as f32;
            reset_warp_seg(
                &mut seg_source_start,
                &mut seg_host_start,
                &mut next_boundary_src,
                &mut warp_xfade_remaining,
                &mut warp_prev_playhead,
                &mut seg_tail_dir,
                playhead,
                0.0,
            );
        }
        gate_counter = 0.0; // reset real-time duration counter
        sr_phase = 0.0;
        scrub_smooth = 0.0;
        if env_level > 0.001 || last_out_l.abs() > 0.000_1 || last_out_r.abs() > 0.000_1 {
            // Voice was still audible. Keep rendering the outgoing waveform as a
            // short stolen tail while the new trigger starts its normal attack.
            retrigger_out_l = last_out_l;
            retrigger_out_r = last_out_r;
            retrigger_playhead =
                old_read_head.clamp(start_sample as f32, end_sample.saturating_sub(1) as f32);
            retrigger_direction = old_play_direction;
            retrigger_amp = last_env_amp.max(0.0);
            retrigger_sr_phase = old_sr_phase;
            retrigger_sr_held_l = old_sr_held_l;
            retrigger_sr_held_r = old_sr_held_r;
            retrigger_fade_remaining = retrigger_fade_samples;
        } else {
            // Voice was silent → clean attack from 0
            retrigger_out_l = 0.0;
            retrigger_out_r = 0.0;
            retrigger_amp = 0.0;
            retrigger_fade_remaining = 0.0;
        }
        env_phase = ENV_ATTACK;
        env_level = 0.0;
        release_level = 0.0;
    }

    let warp_ratio_changed = (warp_target_ratio - last_warp_target_ratio).abs() > 0.0001;
    let warp_preserve_changed = warp_preserve != last_warp_preserve;
    if warp_active && (warp_ratio_changed || warp_preserve_changed) {
        // Tempo/sample BPM changes and preserve-grid changes invalidate the
        // current Beats segment schedule. Re-derive the position from musical
        // time elapsed so playback lands at the musically-correct spot, then
        // let normal segment advancement take over from the new grid.
        let old_playhead = playhead;
        warp_ratio = warp_target_ratio;
        last_warp_target_ratio = warp_target_ratio;
        last_warp_preserve = warp_preserve;
        let src_elapsed =
            source_frames_to_host_frames(gate_counter, sample_rate, source_sample_rate)
                * warp_ratio;
        let ideal_src = (start_sample as f32 + src_elapsed)
            .clamp(start_sample as f32, (end_sample.saturating_sub(1)) as f32);
        playhead = ideal_src;
        seg_source_start = ideal_src;
        seg_host_start = gate_counter;
        next_boundary_src = next_seg_boundary(ideal_src);
        seg_tail_dir = 1.0;
        if (playhead - old_playhead).abs() > 1.0 {
            warp_prev_playhead = old_playhead;
            warp_xfade_remaining = (sample_rate * WARP_XFADE_SECONDS).max(1.0);
        } else {
            warp_prev_playhead = playhead;
        }
    } else if !warp_active {
        last_warp_target_ratio = warp_target_ratio;
        last_warp_preserve = warp_preserve;
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
        if warp_active || warp_repitch {
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
                reset_warp_seg(
                    &mut seg_source_start,
                    &mut seg_host_start,
                    &mut next_boundary_src,
                    &mut warp_xfade_remaining,
                    &mut warp_prev_playhead,
                    &mut seg_tail_dir,
                    playhead,
                    gate_counter,
                );
            }
        }

        // Past end of sample region: stop
        if playhead >= end_sample as f32 || playhead < start_sample as f32 {
            *out0.add(i) = 0.0;
            *out1.add(i) = 0.0;
            env_phase = ENV_IDLE;
            env_level = 0.0;
            last_env_amp = 0.0;
            *s.add(STATE_PLAYING) = 0.0;
            for j in (i + 1)..nf {
                *out0.add(j) = 0.0;
                *out1.add(j) = 0.0;
            }
            break;
        }

        // ── Envelope state machine (per sample) ──
        // Uses chained `if` (not else-if) so phase transitions within a
        // single sample flow through immediately.
        if env_phase == ENV_RETRIGGER {
            env_phase = ENV_ATTACK;
            env_level = 0.0;
        }

        if env_phase == ENV_ATTACK {
            if attack_samples > 0.0 {
                env_level += 1.0 / attack_samples;
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
                last_env_amp = 0.0;
                for j in (i + 1)..nf {
                    *out0.add(j) = 0.0;
                    *out1.add(j) = 0.0;
                }
                break;
            }
        }

        // ── Read sample with linear interpolation ──

        let mut warp_silent = false;
        if warp_active && play_direction >= 0.0 {
            // Fire the next segment at its scheduled host-time boundary. The
            // new segment's host start is the *scheduled* boundary time (not
            // gate_counter) so quantization error can't accumulate.
            let next_boundary_host = seg_host_start
                + source_frames_to_host_frames(
                    next_boundary_src - seg_source_start,
                    source_sample_rate,
                    sample_rate,
                ) / warp_ratio;
            // `next_boundary_src > seg_source_start` guards against stale
            // segment state (e.g. warp toggled on mid-note): a boundary at or
            // behind the segment start must never fire a jump.
            if gate_counter >= next_boundary_host
                && next_boundary_src > seg_source_start
                && (next_boundary_src as usize) < end_sample
            {
                warp_prev_playhead = playhead;
                playhead = next_boundary_src;
                seg_source_start = next_boundary_src;
                seg_host_start = next_boundary_host;
                next_boundary_src = next_seg_boundary(next_boundary_src);
                seg_tail_dir = 1.0;
                warp_xfade_remaining = (sample_rate * WARP_XFADE_SECONDS).max(1.0);
            } else if playhead >= next_boundary_src && next_boundary_src > seg_source_start {
                // Natural content ran out before the boundary is due
                // (project slower than sample): fill per the tail mode.
                let tail_start = seg_source_start + 0.5 * (next_boundary_src - seg_source_start);
                match seg_loop_mode {
                    SEG_LOOP_FORWARD => {
                        warp_prev_playhead = playhead;
                        playhead = tail_start;
                        warp_xfade_remaining = (sample_rate * WARP_XFADE_SECONDS).max(1.0);
                    }
                    SEG_LOOP_PINGPONG => {
                        playhead = next_boundary_src;
                        seg_tail_dir = -1.0;
                    }
                    _ => {
                        // Hold at the boundary reading silence so the next
                        // segment's transient isn't consumed early and the
                        // voice can't run off the region end while waiting.
                        playhead = next_boundary_src;
                        warp_silent = true;
                    }
                }
            } else if seg_tail_dir < 0.0 {
                // Ping-pong tail: bounce forward again at the tail midpoint.
                let tail_start = seg_source_start + 0.5 * (next_boundary_src - seg_source_start);
                if playhead <= tail_start {
                    seg_tail_dir = 1.0;
                }
            }
        }

        let scrub_mod = modulation_lane_sum(inp, MOD_INPUT_COUNT, s, &MOD_SCRUB_LANES, i);
        let target_scrub = (scrub_offset + scrub_mod).clamp(-1.0, 1.0);
        let read_head = accumulated_scrub_read_head(
            &mut playhead,
            &mut scrub_smooth,
            target_scrub,
            region_len,
            start_sample as f32,
            (end_sample.saturating_sub(1)) as f32,
            sample_rate,
            scrub_smooth_time_ms,
        );

        let (mut sample_l, mut sample_r) = if warp_silent {
            (0.0, 0.0)
        } else {
            read_interpolated(sample_data, sample_len, channel_count, read_head)
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
            warp_prev_playhead += playback_step;
            warp_xfade_remaining -= 1.0;
        }
        if warp_active && play_direction >= 0.0 && seg_envelope > 0.001 {
            // Per-segment decay, restarting at every boundary. High values
            // gate each segment down hard (classic choppy Beats sound) and
            // mask tail-loop repetition at extreme slow-downs.
            let seg_len_host = source_frames_to_host_frames(
                next_boundary_src - seg_source_start,
                source_sample_rate,
                sample_rate,
            ) / warp_ratio;
            if seg_len_host > 1.0 {
                let t = (gate_counter - seg_host_start).max(0.0);
                let tau = seg_len_host * (4.0 + (0.15 - 4.0) * seg_envelope);
                let seg_gain = (-t / tau.max(1.0)).exp();
                sample_l *= seg_gain;
                sample_r *= seg_gain;
            }
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
        let mut tail_l = 0.0;
        let mut tail_r = 0.0;
        if retrigger_fade_remaining > 0.0 && retrigger_amp > 0.0 {
            let tail_in_region =
                retrigger_playhead >= start_sample as f32 && retrigger_playhead < end_sample as f32;
            let (mut raw_tail_l, mut raw_tail_r) = if tail_in_region {
                read_interpolated(sample_data, sample_len, channel_count, retrigger_playhead)
            } else {
                (retrigger_out_l, retrigger_out_r)
            };

            if sr_reduced && tail_in_region {
                if retrigger_sr_phase <= 0.0 {
                    retrigger_sr_held_l = raw_tail_l;
                    retrigger_sr_held_r = raw_tail_r;
                    retrigger_sr_phase += sr_step;
                }
                retrigger_sr_phase -= 1.0;
                raw_tail_l = retrigger_sr_held_l;
                raw_tail_r = retrigger_sr_held_r;
            }

            let tail_gain = retrigger_tail_gain(retrigger_fade_remaining, retrigger_fade_samples);
            tail_l = raw_tail_l * retrigger_amp * tail_gain;
            tail_r = raw_tail_r * retrigger_amp * tail_gain;
            retrigger_fade_remaining -= 1.0;

            if tail_in_region {
                let (next_playhead, next_direction, tail_active) = advance_retrigger_playhead(
                    retrigger_playhead,
                    retrigger_direction,
                    playback_step,
                    start_sample,
                    end_sample,
                    loop_mode,
                );
                retrigger_playhead = next_playhead;
                retrigger_direction = next_direction;
                if !tail_active {
                    retrigger_fade_remaining = 0.0;
                    retrigger_amp = 0.0;
                }
            }

            if retrigger_fade_remaining <= 0.0 {
                retrigger_fade_remaining = 0.0;
                retrigger_amp = 0.0;
            }
        }

        *out0.add(i) = sample_l * env_amp + tail_l;
        *out1.add(i) = sample_r * env_amp + tail_r;
        last_out_l = *out0.add(i);
        last_out_r = *out1.add(i);
        last_env_amp = env_amp;
        last_read_head = read_head;

        playhead += if warp_active && play_direction >= 0.0 {
            playback_step * seg_tail_dir
        } else if warp_repitch {
            // Re-pitch: fold the tempo ratio into the resample rate so pitch
            // rides with tempo (tape-style). No segmentation, no artifacts.
            playback_step * warp_ratio * play_direction
        } else {
            playback_step * play_direction
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
    *s.add(STATE_LAST_ENV_AMP) = last_env_amp;
    *s.add(STATE_RETRIGGER_PLAYHEAD) = retrigger_playhead;
    *s.add(STATE_RETRIGGER_DIRECTION) = retrigger_direction;
    *s.add(STATE_RETRIGGER_AMP) = retrigger_amp;
    *s.add(STATE_RETRIGGER_SR_PHASE) = retrigger_sr_phase;
    *s.add(STATE_RETRIGGER_SR_HELD_L) = retrigger_sr_held_l;
    *s.add(STATE_RETRIGGER_SR_HELD_R) = retrigger_sr_held_r;
    *s.add(STATE_RETRIGGER_FADE_REMAINING) = retrigger_fade_remaining;
    *s.add(STATE_PLAY_DIRECTION) = play_direction;
    *s.add(STATE_SR_PHASE) = sr_phase;
    *s.add(STATE_SR_HELD_L) = sr_held_l;
    *s.add(STATE_SR_HELD_R) = sr_held_r;
    *s.add(STATE_WARP_RATIO) = warp_ratio;
    *s.add(STATE_WARP_NEXT_BOUNDARY) = next_boundary_src;
    *s.add(STATE_WARP_SLICE_PROJECT_FRAME_START) = seg_host_start;
    *s.add(STATE_WARP_XFADE_REMAINING) = warp_xfade_remaining;
    *s.add(STATE_WARP_PREV_PLAYHEAD) = warp_prev_playhead;
    *s.add(STATE_WARP_SLICE_SOURCE_FRAME_START) = seg_source_start;
    *s.add(STATE_WARP_LAST_TARGET_RATIO) = last_warp_target_ratio;
    *s.add(STATE_WARP_SEG_TAIL_DIR) = seg_tail_dir;
    *s.add(STATE_WARP_LAST_PRESERVE) = last_warp_preserve as f32;
    *s.add(STATE_SCRUB_SMOOTH) = scrub_smooth;
    *s.add(STATE_LAST_READ_HEAD) = last_read_head;
}

unsafe extern "C" fn sampler_begin_event_slice(
    state: *mut c_void,
    _block_serial: u64,
    _slice_start: c_int,
    _slice_nframes: c_int,
) {
    *(state as *mut f32).add(STATE_TIMELINE_COUNT) = 0.0;
}

unsafe extern "C" fn sampler_schedule_event(
    state: *mut c_void,
    event: *const GraphBlockEvent,
) -> bool {
    if event.is_null() {
        return false;
    }
    let event = &*event;
    if event.kind != GBE_NOTE_ON && event.kind != GBE_GATE_OFF {
        return false;
    }
    if event.kind == GBE_NOTE_ON
        && event.aux_count < SAMPLER_EVENT_AUX_REQUIRED_NOTE_ON_COUNT as u32
    {
        return false;
    }
    let s = state as *mut f32;
    let count = (*s.add(STATE_TIMELINE_COUNT)).max(0.0) as usize;
    if count >= SAMPLER_TIMELINE_CAPACITY {
        return false;
    }
    let base = STATE_TIMELINE_BASE + count * TIMELINE_EVENT_WIDTH;
    *s.add(base + TIMELINE_FRAME) = event.frame_offset as f32;
    *s.add(base + TIMELINE_KIND) = event.kind as f32;
    let aux_count = (event.aux_count as usize).min(GBE_AUX_CAP);
    *s.add(base + TIMELINE_AUX_COUNT) = aux_count as f32;
    for i in 0..aux_count {
        *s.add(base + TIMELINE_AUX_BASE + i) = event.aux[i];
    }
    *s.add(STATE_TIMELINE_COUNT) = (count + 1) as f32;
    true
}

unsafe fn sampler_apply_timeline_event(state: *mut f32, base: usize) -> bool {
    let kind = *state.add(base + TIMELINE_KIND) as u32;
    if kind == GBE_GATE_OFF {
        *state.add(STATE_GATE_SAMPLES) = 0.0;
        return true;
    }
    if kind != GBE_NOTE_ON {
        return false;
    }

    let aux_count = (*state.add(base + TIMELINE_AUX_COUNT)).max(0.0) as usize;
    if aux_count < SAMPLER_EVENT_AUX_REQUIRED_NOTE_ON_COUNT {
        return false;
    }
    let aux = |idx: usize| *state.add(base + TIMELINE_AUX_BASE + idx);
    let aux_or = |idx: usize, default: f32| {
        if idx < aux_count {
            aux(idx)
        } else {
            default
        }
    };
    *state.add(STATE_ENABLED) = aux(SAMPLER_EVENT_AUX_ENABLED);
    *state.add(STATE_VELOCITY) = aux(SAMPLER_EVENT_AUX_VELOCITY);
    *state.add(STATE_SPEED) = aux(SAMPLER_EVENT_AUX_SPEED);
    *state.add(STATE_GATE_SAMPLES) = aux(SAMPLER_EVENT_AUX_GATE_SAMPLES);
    *state.add(STATE_TRANSPOSE) = aux(SAMPLER_EVENT_AUX_TRANSPOSE);
    *state.add(STATE_ATTACK_SAMPLES) = aux(SAMPLER_EVENT_AUX_ATTACK_SAMPLES);
    *state.add(STATE_RELEASE_SAMPLES) = aux(SAMPLER_EVENT_AUX_RELEASE_SAMPLES);
    *state.add(STATE_GATE_MODE) = aux(SAMPLER_EVENT_AUX_GATE_MODE);
    *state.add(STATE_START_POINT) = aux(SAMPLER_EVENT_AUX_START_POINT);
    *state.add(STATE_END_POINT) = aux(SAMPLER_EVENT_AUX_END_POINT);
    *state.add(STATE_REVERSE) = aux(SAMPLER_EVENT_AUX_REVERSE);
    *state.add(STATE_LOOP_MODE) = aux(SAMPLER_EVENT_AUX_LOOP_MODE);
    *state.add(STATE_LOOP_XFADE_SAMPLES) = aux(SAMPLER_EVENT_AUX_LOOP_XFADE_SAMPLES);
    *state.add(STATE_SR_HZ) = aux(SAMPLER_EVENT_AUX_SR_HZ);
    *state.add(STATE_WARP_ENABLED) = aux(SAMPLER_EVENT_AUX_WARP_ENABLED);
    *state.add(STATE_WARP_MODE) = aux(SAMPLER_EVENT_AUX_WARP_MODE);
    *state.add(STATE_WARP_RATIO) = aux(SAMPLER_EVENT_AUX_WARP_RATIO);
    *state.add(STATE_WARP_SAMPLE_BPM) = aux(SAMPLER_EVENT_AUX_WARP_SAMPLE_BPM);
    *state.add(STATE_WARP_PROJECT_BPM) = aux(SAMPLER_EVENT_AUX_WARP_PROJECT_BPM);
    *state.add(STATE_WARP_ONSET_TABLE_PTR_LO) = aux(SAMPLER_EVENT_AUX_WARP_PTR_LO);
    *state.add(STATE_WARP_ONSET_TABLE_PTR_HI) = aux(SAMPLER_EVENT_AUX_WARP_PTR_HI);
    *state.add(STATE_SCRUB_OFFSET) = aux(SAMPLER_EVENT_AUX_SCRUB_OFFSET);
    *state.add(STATE_WARP_PRESERVE) = aux_or(
        SAMPLER_EVENT_AUX_WARP_PRESERVE,
        WARP_PRESERVE_DEFAULT as f32,
    );
    *state.add(STATE_WARP_SEG_LOOP_MODE) = aux_or(
        SAMPLER_EVENT_AUX_WARP_SEG_LOOP_MODE,
        WARP_SEG_LOOP_MODE_DEFAULT as f32,
    );
    *state.add(STATE_WARP_SEG_ENVELOPE) = aux_or(
        SAMPLER_EVENT_AUX_WARP_SEG_ENVELOPE,
        WARP_SEG_ENVELOPE_DEFAULT,
    );
    *state.add(STATE_PLAYHEAD) = 0.0;
    *state.add(STATE_PLAYING) = 1.0;
    true
}

unsafe fn sampler_process_segment_at(
    inp: *const *mut f32,
    out: *const *mut f32,
    offset: usize,
    nframes: usize,
    state: *mut c_void,
    buffers: *mut c_void,
) {
    if nframes == 0 {
        return;
    }

    let mut shifted_inputs = [std::ptr::null_mut(); MOD_INPUT_COUNT];
    let shifted_input_ptr = if inp.is_null() {
        std::ptr::null()
    } else {
        for (i, slot) in shifted_inputs.iter_mut().enumerate() {
            let ptr = *inp.add(i);
            *slot = if ptr.is_null() { ptr } else { ptr.add(offset) };
        }
        shifted_inputs.as_ptr()
    };

    let shifted_outputs = [(*out.add(0)).add(offset), (*out.add(1)).add(offset)];
    sampler_process_segment(
        shifted_input_ptr,
        shifted_outputs.as_ptr(),
        nframes as c_int,
        state,
        buffers,
    );
}

unsafe extern "C" fn sampler_process(
    inp: *const *mut f32,
    out: *const *mut f32,
    nframes: c_int,
    state: *mut c_void,
    buffers: *mut c_void,
) {
    if nframes <= 0 {
        return;
    }
    let nf = nframes as usize;
    let s = state as *mut f32;
    let event_count = (*s.add(STATE_TIMELINE_COUNT))
        .max(0.0)
        .min(SAMPLER_TIMELINE_CAPACITY as f32) as usize;
    if event_count == 0 {
        sampler_process_segment(inp, out, nframes, state, buffers);
        return;
    }

    let mut rendered = 0usize;
    for event_index in 0..event_count {
        let base = STATE_TIMELINE_BASE + event_index * TIMELINE_EVENT_WIDTH;
        let event_frame = (*s.add(base + TIMELINE_FRAME)).max(0.0) as usize;
        let event_frame = event_frame.min(nf);
        if event_frame > rendered {
            sampler_process_segment_at(inp, out, rendered, event_frame - rendered, state, buffers);
            rendered = event_frame;
        }
        let _ = sampler_apply_timeline_event(s, base);
    }
    if rendered < nf {
        sampler_process_segment_at(inp, out, rendered, nf - rendered, state, buffers);
    }
}

pub fn sampler_vtable() -> NodeVTable {
    NodeVTable {
        process: Some(sampler_process),
        init: Some(sampler_init),
        reset: None,
        migrate: None,
        begin_event_slice: Some(sampler_begin_event_slice),
        schedule_event: Some(sampler_schedule_event),
    }
}

/// Load a WAV file into an audiograph buffer and retain the trimmed mono data
/// needed by the offline analyzer.
pub fn load_wav_buffer(lg: *mut LiveGraph, wav_path: &Path) -> Result<LoadedSample, String> {
    let decoded = eseqlisp::audio::sample::load_wav_file(wav_path).map_err(|e| {
        let file_len = std::fs::metadata(wav_path).map(|meta| meta.len()).ok();
        eprintln!(
            "sampler: failed to open WAV {}; exists={}; len={file_len:?}; error={e}",
            wav_path.display(),
            wav_path.exists()
        );
        format!("Failed to open WAV {}: {e}", wav_path.display())
    })?;

    for warning in &decoded.warnings {
        eprintln!(
            "sampler: decoded {} with WAV warning: {warning}",
            wav_path.display(),
        );
    }

    let channels = decoded.channels as usize;
    let stereo: Vec<f32> = if channels == 1 {
        decoded
            .samples
            .iter()
            .flat_map(|&sample| [sample, sample])
            .collect()
    } else if channels == 2 {
        decoded.samples.clone()
    } else {
        decoded
            .samples
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
    let decoded_peak = stereo
        .iter()
        .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
    let trimmed_peak = trimmed
        .iter()
        .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
    eprintln!(
        "sampler: decoded {} successfully; sample_rate={}; channels={}; frames={}; decoded_samples={}; stereo_frames={}; skipped_frames={}; trimmed_frames={}; decoded_peak={:.6}; trimmed_peak={:.6}",
        wav_path.display(),
        decoded.sample_rate,
        decoded.channels,
        decoded.frames,
        decoded.samples.len(),
        stereo.len() / 2,
        skip,
        trimmed.len() / 2,
        decoded_peak,
        trimmed_peak
    );

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
        sample_rate: decoded.sample_rate,
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
    source_sample_rate: u32,
    name: &str,
) -> Result<SamplerTrack, String> {
    let initial_state = [buffer_id as f32, source_sample_rate.max(1) as f32];
    let c_name = CString::new(name).unwrap();

    let node_id = unsafe {
        add_node(
            lg,
            sampler_vtable(),
            SAMPLER_STATE_SIZE * std::mem::size_of::<f32>(),
            c_name.as_ptr(),
            MOD_INPUT_COUNT as i32,
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

pub unsafe fn queue_sampler_host_sample_rate_update(
    lg: *mut LiveGraph,
    node_id: i32,
    sample_rate: u32,
) -> bool {
    let value = sample_rate.max(1) as f32;
    crate::audiograph::write_node_state(lg, node_id, STATE_SAMPLE_RATE, &value, 1)
}

/// Load a WAV file, create an audiograph buffer and sampler node.
pub fn create_sampler_track(lg: *mut LiveGraph, wav_path: &Path) -> Result<SamplerTrack, String> {
    let loaded = load_wav_buffer(lg, wav_path)?;
    create_sampler_node(lg, loaded.buffer_id, loaded.sample_rate, &loaded.name)
}

#[cfg(test)]
mod tests {
    use super::{
        accumulated_scrub_read_head, advance_retrigger_playhead, modulation_lane_sum,
        retrigger_fade_samples, sampler_begin_event_slice, sampler_init, sampler_playback_step,
        sampler_process, sampler_schedule_event, source_frames_to_host_frames, BufferDesc,
        SAMPLER_EVENT_AUX_ATTACK_SAMPLES, SAMPLER_EVENT_AUX_ENABLED, SAMPLER_EVENT_AUX_END_POINT,
        SAMPLER_EVENT_AUX_GATE_MODE, SAMPLER_EVENT_AUX_GATE_SAMPLES, SAMPLER_EVENT_AUX_LOOP_MODE,
        SAMPLER_EVENT_AUX_LOOP_XFADE_SAMPLES, SAMPLER_EVENT_AUX_NOTE_ON_COUNT,
        SAMPLER_EVENT_AUX_RELEASE_SAMPLES, SAMPLER_EVENT_AUX_REVERSE,
        SAMPLER_EVENT_AUX_SCRUB_OFFSET, SAMPLER_EVENT_AUX_SPEED, SAMPLER_EVENT_AUX_SR_HZ,
        SAMPLER_EVENT_AUX_START_POINT, SAMPLER_EVENT_AUX_TRANSPOSE, SAMPLER_EVENT_AUX_VELOCITY,
        SAMPLER_EVENT_AUX_WARP_ENABLED, SAMPLER_EVENT_AUX_WARP_MODE,
        SAMPLER_EVENT_AUX_WARP_PRESERVE, SAMPLER_EVENT_AUX_WARP_PROJECT_BPM,
        SAMPLER_EVENT_AUX_WARP_PTR_HI, SAMPLER_EVENT_AUX_WARP_PTR_LO, SAMPLER_EVENT_AUX_WARP_RATIO,
        SAMPLER_EVENT_AUX_WARP_SAMPLE_BPM, SAMPLER_EVENT_AUX_WARP_SEG_ENVELOPE,
        SAMPLER_EVENT_AUX_WARP_SEG_LOOP_MODE, SAMPLER_MOD_LANES_PER_PARAM, SAMPLER_STATE_SIZE,
        SCRUB_SMOOTH_TIME_MS_DEFAULT, SEG_LOOP_FORWARD, SEG_LOOP_OFF, SEG_LOOP_PINGPONG,
        STATE_ATTACK_SAMPLES, STATE_BUFFER_ID, STATE_END_POINT, STATE_GAIN, STATE_GATE_SAMPLES,
        STATE_MOD_SPEED_DEPTH, STATE_MOD_SPEED_LANE2_DEPTH, STATE_MOD_SPEED_LANE2_SOURCE,
        STATE_MOD_SPEED_SOURCE, STATE_PLAYHEAD, STATE_PLAYING, STATE_SAMPLE_RATE,
        STATE_SOURCE_SAMPLE_RATE, STATE_START_POINT, STATE_VELOCITY, STATE_WARP_ENABLED,
        STATE_WARP_LAST_PRESERVE, STATE_WARP_LAST_TARGET_RATIO, STATE_WARP_MODE,
        STATE_WARP_NEXT_BOUNDARY, STATE_WARP_PRESERVE, STATE_WARP_PROJECT_BPM, STATE_WARP_RATIO,
        STATE_WARP_SAMPLE_BPM, STATE_WARP_SEG_ENVELOPE, STATE_WARP_SEG_LOOP_MODE,
        STATE_WARP_SLICE_SOURCE_FRAME_START, WARP_MODE_BEATS, WARP_MODE_REPITCH,
    };
    use crate::audiograph::{GraphBlockEvent, GBE_AUX_CAP, GBE_NOTE_ON};
    use std::ffi::c_void;

    fn sampler_note_on_event(frame_offset: u32) -> GraphBlockEvent {
        let mut event = GraphBlockEvent {
            logical_id: 1,
            frame_offset,
            sequence: 0,
            kind: GBE_NOTE_ON,
            aux_count: SAMPLER_EVENT_AUX_NOTE_ON_COUNT as u32,
            aux: [0.0; GBE_AUX_CAP],
        };
        event.aux[SAMPLER_EVENT_AUX_ENABLED] = 1.0;
        event.aux[SAMPLER_EVENT_AUX_VELOCITY] = 1.0;
        event.aux[SAMPLER_EVENT_AUX_SPEED] = 1.0;
        event.aux[SAMPLER_EVENT_AUX_GATE_SAMPLES] = 64.0;
        event.aux[SAMPLER_EVENT_AUX_TRANSPOSE] = 0.0;
        event.aux[SAMPLER_EVENT_AUX_ATTACK_SAMPLES] = 0.0;
        event.aux[SAMPLER_EVENT_AUX_RELEASE_SAMPLES] = 64.0;
        event.aux[SAMPLER_EVENT_AUX_GATE_MODE] = 1.0;
        event.aux[SAMPLER_EVENT_AUX_START_POINT] = 0.0;
        event.aux[SAMPLER_EVENT_AUX_END_POINT] = 1.0;
        event.aux[SAMPLER_EVENT_AUX_REVERSE] = 0.0;
        event.aux[SAMPLER_EVENT_AUX_LOOP_MODE] = 1.0;
        event.aux[SAMPLER_EVENT_AUX_LOOP_XFADE_SAMPLES] = 0.0;
        event.aux[SAMPLER_EVENT_AUX_SR_HZ] = 44_100.0;
        event.aux[SAMPLER_EVENT_AUX_WARP_ENABLED] = 0.0;
        event.aux[SAMPLER_EVENT_AUX_WARP_MODE] = 0.0;
        event.aux[SAMPLER_EVENT_AUX_WARP_RATIO] = 1.0;
        event.aux[SAMPLER_EVENT_AUX_WARP_SAMPLE_BPM] = 120.0;
        event.aux[SAMPLER_EVENT_AUX_WARP_PROJECT_BPM] = 120.0;
        event.aux[SAMPLER_EVENT_AUX_WARP_PTR_LO] = 0.0;
        event.aux[SAMPLER_EVENT_AUX_WARP_PTR_HI] = 0.0;
        event.aux[SAMPLER_EVENT_AUX_SCRUB_OFFSET] = 0.0;
        event.aux[SAMPLER_EVENT_AUX_WARP_PRESERVE] = crate::warp_grid::PRESERVE_TRANSIENTS as f32;
        event.aux[SAMPLER_EVENT_AUX_WARP_SEG_LOOP_MODE] = SEG_LOOP_FORWARD as f32;
        event.aux[SAMPLER_EVENT_AUX_WARP_SEG_ENVELOPE] = 0.0;
        event
    }

    #[test]
    fn playback_step_preserves_44100_wav_pitch_at_48000_host_rate() {
        let step = sampler_playback_step(44_100.0, 48_000.0, 1.0);

        assert!((step - 0.91875).abs() < 0.00001);
    }

    #[test]
    fn playback_step_preserves_48000_wav_pitch_at_44100_host_rate() {
        let step = sampler_playback_step(48_000.0, 44_100.0, 1.0);

        assert!((step - 1.0884354).abs() < 0.00001);
    }

    #[test]
    fn source_frame_durations_convert_to_host_frame_durations() {
        let host_frames = source_frames_to_host_frames(44_100.0, 44_100.0, 48_000.0);

        assert!((host_frames - 48_000.0).abs() < 0.001);
    }

    #[test]
    fn retrigger_fade_is_four_milliseconds_at_host_rate() {
        assert!((retrigger_fade_samples(48_000.0) - 192.0).abs() < 0.001);
        assert!((retrigger_fade_samples(44_100.0) - 176.4).abs() < 0.001);
    }

    #[test]
    fn retrigger_tail_advances_outgoing_sample_position() {
        let (playhead, direction, active) =
            advance_retrigger_playhead(100.0, 1.0, 2.5, 0, 1_000, 1);

        assert!(active);
        assert_eq!(direction, 1.0);
        assert_eq!(playhead, 102.5);
    }

    #[test]
    fn retrigger_tail_wraps_looped_outgoing_sample_position() {
        let (playhead, direction, active) =
            advance_retrigger_playhead(998.0, 1.0, 5.0, 100, 1_000, 2);

        assert!(active);
        assert_eq!(direction, 1.0);
        assert_eq!(playhead, 103.0);
    }

    #[test]
    fn sampler_process_preserves_stereo_channels_during_playback_and_retrigger_tail() {
        let mut state = [0.0_f32; SAMPLER_STATE_SIZE];
        let initial = [0.0_f32, 44_100.0];
        unsafe {
            sampler_init(
                state.as_mut_ptr() as *mut c_void,
                44_100,
                64,
                initial.as_ptr() as *const c_void,
            );
        }
        state[STATE_BUFFER_ID] = 0.0;
        state[STATE_SOURCE_SAMPLE_RATE] = 44_100.0;
        state[STATE_SAMPLE_RATE] = 44_100.0;
        state[STATE_PLAYING] = 1.0;
        state[STATE_PLAYHEAD] = 0.0;
        state[STATE_GAIN] = 1.0;
        state[STATE_VELOCITY] = 1.0;
        state[STATE_ATTACK_SAMPLES] = 0.0;
        state[STATE_GATE_SAMPLES] = f32::MAX;
        state[STATE_END_POINT] = 1.0;

        let mut sample = [
            0.30, -0.70, 0.35, -0.65, 0.40, -0.60, 0.45, -0.55, 0.50, -0.50, 0.55, -0.45, 0.60,
            -0.40, 0.65, -0.35,
        ];
        let desc = [BufferDesc {
            buffer: sample.as_mut_ptr(),
            size: 8,
            channel_count: 2,
        }];
        let mut left = [0.0_f32; 8];
        let mut right = [0.0_f32; 8];
        let outputs = [left.as_mut_ptr(), right.as_mut_ptr()];
        let inputs: [*mut f32; 0] = [];

        unsafe {
            sampler_process(
                inputs.as_ptr(),
                outputs.as_ptr(),
                8,
                state.as_mut_ptr() as *mut c_void,
                desc.as_ptr() as *mut c_void,
            );
        }

        assert!(
            left.iter().any(|v| v.abs() > 0.01),
            "left channel was silent during normal playback: {left:?}"
        );
        assert!(
            right.iter().any(|v| v.abs() > 0.01),
            "right channel was silent during normal playback: {right:?}"
        );
        assert_ne!(left[0], right[0]);

        state[STATE_PLAYHEAD] = 0.0;
        left.fill(0.0);
        right.fill(0.0);

        unsafe {
            sampler_process(
                inputs.as_ptr(),
                outputs.as_ptr(),
                4,
                state.as_mut_ptr() as *mut c_void,
                desc.as_ptr() as *mut c_void,
            );
        }

        assert!(
            left.iter().any(|v| v.abs() > 0.01),
            "left channel was silent during retrigger tail: {left:?}"
        );
        assert!(
            right.iter().any(|v| v.abs() > 0.01),
            "right channel was silent during retrigger tail: {right:?}"
        );
        assert_ne!(left[0], right[0]);
    }

    // 1 kHz host+source rate keeps the numbers tiny: sample at 60 BPM with a
    // 1/4 grid puts a segment boundary every 1000 source frames.
    fn beats_warp_test_state(project_bpm: f32, seg_loop_mode: i32) -> [f32; SAMPLER_STATE_SIZE] {
        let mut state = [0.0_f32; SAMPLER_STATE_SIZE];
        let initial = [0.0_f32, 1_000.0];
        unsafe {
            sampler_init(
                state.as_mut_ptr() as *mut c_void,
                1_000,
                64,
                initial.as_ptr() as *const c_void,
            );
        }
        state[STATE_PLAYING] = 1.0;
        state[STATE_PLAYHEAD] = 0.0;
        state[STATE_GAIN] = 1.0;
        state[STATE_VELOCITY] = 1.0;
        state[STATE_ATTACK_SAMPLES] = 0.0;
        state[STATE_GATE_SAMPLES] = f32::MAX;
        state[STATE_END_POINT] = 1.0;
        state[STATE_WARP_ENABLED] = 1.0;
        state[STATE_WARP_MODE] = WARP_MODE_BEATS as f32;
        state[STATE_WARP_PRESERVE] = crate::warp_grid::PRESERVE_1_4 as f32;
        state[STATE_WARP_SEG_LOOP_MODE] = seg_loop_mode as f32;
        state[STATE_WARP_SAMPLE_BPM] = 60.0;
        state[STATE_WARP_PROJECT_BPM] = project_bpm;
        let ratio = project_bpm / 60.0;
        state[STATE_WARP_RATIO] = ratio;
        state[STATE_WARP_LAST_TARGET_RATIO] = ratio;
        state
    }

    fn beats_warp_test_state_at_rate(
        host_sample_rate: i32,
        source_sample_rate: f32,
        sample_bpm: f32,
        project_bpm: f32,
        preserve: i32,
        seg_loop_mode: i32,
    ) -> [f32; SAMPLER_STATE_SIZE] {
        let mut state = [0.0_f32; SAMPLER_STATE_SIZE];
        let initial = [0.0_f32, source_sample_rate];
        unsafe {
            sampler_init(
                state.as_mut_ptr() as *mut c_void,
                host_sample_rate,
                256,
                initial.as_ptr() as *const c_void,
            );
        }
        state[STATE_PLAYING] = 1.0;
        state[STATE_PLAYHEAD] = 0.0;
        state[STATE_GAIN] = 1.0;
        state[STATE_VELOCITY] = 1.0;
        state[STATE_ATTACK_SAMPLES] = 0.0;
        state[STATE_GATE_SAMPLES] = f32::MAX;
        state[STATE_END_POINT] = 1.0;
        state[STATE_WARP_ENABLED] = 1.0;
        state[STATE_WARP_MODE] = WARP_MODE_BEATS as f32;
        state[STATE_WARP_PRESERVE] = preserve as f32;
        state[STATE_WARP_LAST_PRESERVE] = preserve as f32;
        state[STATE_WARP_SEG_LOOP_MODE] = seg_loop_mode as f32;
        state[STATE_WARP_SAMPLE_BPM] = sample_bpm;
        state[STATE_WARP_PROJECT_BPM] = project_bpm;
        let ratio = project_bpm / sample_bpm;
        state[STATE_WARP_RATIO] = ratio;
        state[STATE_WARP_LAST_TARGET_RATIO] = ratio;
        state
    }

    fn render_beats(
        state: &mut [f32; SAMPLER_STATE_SIZE],
        sample: &mut [f32],
        frames: usize,
    ) -> Vec<f32> {
        let desc = [BufferDesc {
            buffer: sample.as_mut_ptr(),
            size: sample.len() as std::os::raw::c_int,
            channel_count: 1,
        }];
        let mut rendered = Vec::with_capacity(frames);
        let mut left = [0.0_f32; 250];
        let mut right = [0.0_f32; 250];
        let inputs: [*mut f32; 0] = [];
        let mut remaining = frames;
        while remaining > 0 {
            let n = remaining.min(250);
            let outputs = [left.as_mut_ptr(), right.as_mut_ptr()];
            unsafe {
                sampler_process(
                    inputs.as_ptr(),
                    outputs.as_ptr(),
                    n as i32,
                    state.as_mut_ptr() as *mut c_void,
                    desc.as_ptr() as *mut c_void,
                );
            }
            rendered.extend_from_slice(&left[..n]);
            remaining -= n;
        }
        rendered
    }

    #[test]
    fn beats_warp_fires_segments_at_scheduled_host_times_when_speeding_up() {
        // Project at 120 vs sample at 60: ratio 2.0, so the boundary at
        // source frame 1000 is due at host frame 500.
        let mut state = beats_warp_test_state(120.0, SEG_LOOP_FORWARD);
        let mut sample: Vec<f32> = (0..4_000).map(|i| i as f32 * 0.000_1).collect();
        let out = render_beats(&mut state, &mut sample, 700);

        // Before the boundary: native-rate ramp playback.
        assert!((out[499] - 0.049_9).abs() < 0.001, "got {}", out[499]);
        // After the boundary (+xfade of 5 frames at 1 kHz): reading from
        // source frame ~1000 onward.
        assert!((out[520] - 0.102).abs() < 0.002, "got {}", out[520]);
    }

    #[test]
    fn beats_warp_slowdown_fill_loops_tail_instead_of_going_silent() {
        // Project at 30 vs sample at 60: ratio 0.5. Natural content for the
        // first segment runs out at host frame 1000; the boundary is due at
        // host frame 2000.
        let mut dc: Vec<f32> = vec![1.0; 4_000];

        // Fill off: silence between content end and the next boundary.
        let mut state = beats_warp_test_state(30.0, SEG_LOOP_OFF);
        let out = render_beats(&mut state, &mut dc.clone(), 2_500);
        assert!(out[900] > 0.9, "content should play, got {}", out[900]);
        assert!(
            out[1_500].abs() < 0.001,
            "fill=off should hold silence, got {}",
            out[1_500]
        );
        assert!(
            out[2_100] > 0.9,
            "next segment should fire at host 2000, got {}",
            out[2_100]
        );

        // Fill loop: the tail loops, so audio stays continuous.
        let mut state = beats_warp_test_state(30.0, SEG_LOOP_FORWARD);
        let out = render_beats(&mut state, &mut dc, 2_500);
        for (i, &v) in out.iter().enumerate().take(2_400).skip(10) {
            assert!(v > 0.5, "fill=loop went quiet at host frame {i}: {v}");
        }
    }

    #[test]
    fn beats_warp_one_sixteenth_differs_from_unwarped_playback() {
        // Ratio 0.5 means host frame 600 should be around source frame 350 in
        // 1/16 Beats mode: boundary at source 250 fires at host 500, then the
        // new segment plays natively for 100 frames. Unwarped playback would
        // read source frame 600.
        let mut state = beats_warp_test_state(30.0, SEG_LOOP_FORWARD);
        state[STATE_WARP_PRESERVE] = crate::warp_grid::PRESERVE_1_16 as f32;
        let mut sample: Vec<f32> = (0..4_000).map(|i| i as f32 * 0.001).collect();
        let out = render_beats(&mut state, &mut sample, 700);

        assert!(
            (out[600] - 0.350).abs() < 0.01,
            "1/16 Beats should read warped source frame ~350 at host 600, got {}",
            out[600]
        );
        assert!(
            (out[600] - 0.600).abs() > 0.1,
            "1/16 Beats collapsed to unwarped playback at host 600: {}",
            out[600]
        );
    }

    #[test]
    fn beats_warp_small_divisions_differ_from_unwarped_at_audio_rate() {
        let host_sample_rate = 44_100;
        let source_sample_rate = 44_100.0;
        let sample_bpm = 174.0;
        let project_bpm = 120.0;
        let sample_len = host_sample_rate as usize * 2;
        let sample: Vec<f32> = (0..sample_len)
            .map(|i| ((i % 44_100) as f32 / 44_100.0) * 2.0 - 1.0)
            .collect();

        let mut plain_state = beats_warp_test_state_at_rate(
            host_sample_rate,
            source_sample_rate,
            sample_bpm,
            project_bpm,
            crate::warp_grid::PRESERVE_1_4,
            SEG_LOOP_FORWARD,
        );
        plain_state[STATE_WARP_ENABLED] = 0.0;
        let plain = render_beats(&mut plain_state, &mut sample.clone(), 24_000);

        for preserve in [
            crate::warp_grid::PRESERVE_1_8,
            crate::warp_grid::PRESERVE_1_16,
            crate::warp_grid::PRESERVE_TRANSIENTS,
        ] {
            let mut state = beats_warp_test_state_at_rate(
                host_sample_rate,
                source_sample_rate,
                sample_bpm,
                project_bpm,
                preserve,
                SEG_LOOP_FORWARD,
            );
            let warped = render_beats(&mut state, &mut sample.clone(), 24_000);
            let rms_diff = plain[8_000..24_000]
                .iter()
                .zip(&warped[8_000..24_000])
                .map(|(a, b)| {
                    let d = a - b;
                    d * d
                })
                .sum::<f32>()
                / 16_000.0;
            let rms_diff = rms_diff.sqrt();
            assert!(
                rms_diff > 0.02,
                "preserve={preserve} collapsed to plain playback at audio rate; rms diff {rms_diff}"
            );
        }
    }

    #[test]
    fn beats_warp_one_sixteenth_does_not_stall_after_f32_boundary_roundtrip() {
        let host_sample_rate = 48_000;
        let source_sample_rate = 44_100.0;
        let sample_bpm = 121.896;
        let project_bpm = 153.0;
        let start_sample = 11_674.0_f32;
        let sample_len = 299_008_usize;
        let mut state = beats_warp_test_state_at_rate(
            host_sample_rate,
            source_sample_rate,
            sample_bpm,
            project_bpm,
            crate::warp_grid::PRESERVE_1_16,
            SEG_LOOP_FORWARD,
        );
        state[STATE_START_POINT] = start_sample / sample_len as f32;
        let mut sample = vec![1.0_f32; sample_len];

        let _ = render_beats(&mut state, &mut sample, 20_000);

        assert!(
            state[STATE_WARP_SLICE_SOURCE_FRAME_START] > 27_000.0,
            "1/16 Beats stalled after the second boundary: seg_src={}, next_src={}",
            state[STATE_WARP_SLICE_SOURCE_FRAME_START],
            state[STATE_WARP_NEXT_BOUNDARY]
        );
        assert!(
            state[STATE_WARP_NEXT_BOUNDARY] > state[STATE_WARP_SLICE_SOURCE_FRAME_START] + 0.25,
            "next boundary must remain ahead of the active segment: seg_src={}, next_src={}",
            state[STATE_WARP_SLICE_SOURCE_FRAME_START],
            state[STATE_WARP_NEXT_BOUNDARY]
        );
    }

    #[test]
    fn scheduled_beats_note_on_small_divisions_differ_from_unwarped_at_audio_rate() {
        let host_sample_rate = 44_100;
        let sample_bpm = 174.0;
        let project_bpm = 120.0;
        let sample_len = host_sample_rate as usize * 2;
        let sample: Vec<f32> = (0..sample_len)
            .map(|i| ((i % 44_100) as f32 / 44_100.0) * 2.0 - 1.0)
            .collect();
        let render_scheduled = |preserve: i32, warp_enabled: bool| {
            let mut state = [0.0_f32; SAMPLER_STATE_SIZE];
            let initial = [0.0_f32, host_sample_rate as f32];
            unsafe {
                sampler_init(
                    state.as_mut_ptr() as *mut c_void,
                    host_sample_rate,
                    256,
                    initial.as_ptr() as *const c_void,
                );
            }
            let mut event = sampler_note_on_event(0);
            event.aux[SAMPLER_EVENT_AUX_GATE_SAMPLES] = f32::MAX;
            event.aux[SAMPLER_EVENT_AUX_WARP_ENABLED] = if warp_enabled { 1.0 } else { 0.0 };
            event.aux[SAMPLER_EVENT_AUX_WARP_MODE] = WARP_MODE_BEATS as f32;
            event.aux[SAMPLER_EVENT_AUX_WARP_RATIO] = project_bpm / sample_bpm;
            event.aux[SAMPLER_EVENT_AUX_WARP_SAMPLE_BPM] = sample_bpm;
            event.aux[SAMPLER_EVENT_AUX_WARP_PROJECT_BPM] = project_bpm;
            event.aux[SAMPLER_EVENT_AUX_WARP_PRESERVE] = preserve as f32;
            event.aux[SAMPLER_EVENT_AUX_WARP_SEG_LOOP_MODE] = SEG_LOOP_FORWARD as f32;
            unsafe {
                sampler_begin_event_slice(state.as_mut_ptr().cast(), 1, 0, 256);
                assert!(sampler_schedule_event(state.as_mut_ptr().cast(), &event));
            }
            let mut sample = sample.clone();
            let desc = [BufferDesc {
                buffer: sample.as_mut_ptr(),
                size: sample.len() as std::os::raw::c_int,
                channel_count: 1,
            }];
            let mut rendered = Vec::with_capacity(24_000);
            let mut left = [0.0_f32; 256];
            let mut right = [0.0_f32; 256];
            let outputs = [left.as_mut_ptr(), right.as_mut_ptr()];
            let inputs: [*mut f32; 0] = [];
            let mut remaining = 24_000usize;
            let mut block_serial = 1_u64;
            while remaining > 0 {
                let n = remaining.min(256);
                unsafe {
                    sampler_process(
                        inputs.as_ptr(),
                        outputs.as_ptr(),
                        n as i32,
                        state.as_mut_ptr() as *mut c_void,
                        desc.as_ptr() as *mut c_void,
                    );
                }
                rendered.extend_from_slice(&left[..n]);
                remaining -= n;
                block_serial += 1;
                unsafe {
                    sampler_begin_event_slice(
                        state.as_mut_ptr().cast(),
                        block_serial,
                        rendered.len() as i32,
                        remaining.min(256) as i32,
                    );
                }
            }
            rendered
        };

        let plain = render_scheduled(crate::warp_grid::PRESERVE_1_4, false);
        for preserve in [
            crate::warp_grid::PRESERVE_1_8,
            crate::warp_grid::PRESERVE_1_16,
            crate::warp_grid::PRESERVE_TRANSIENTS,
        ] {
            let warped = render_scheduled(preserve, true);
            let rms_diff = plain[8_000..24_000]
                .iter()
                .zip(&warped[8_000..24_000])
                .map(|(a, b)| {
                    let d = a - b;
                    d * d
                })
                .sum::<f32>()
                / 16_000.0;
            let rms_diff = rms_diff.sqrt();
            assert!(
                rms_diff > 0.02,
                "scheduled preserve={preserve} collapsed to plain playback; rms diff {rms_diff}"
            );
        }
    }

    #[test]
    fn beats_warp_live_preserve_change_recomputes_current_segment_grid() {
        let mut state = beats_warp_test_state(30.0, SEG_LOOP_FORWARD);
        state[STATE_WARP_PRESERVE] = crate::warp_grid::PRESERVE_1_4 as f32;
        state[STATE_WARP_LAST_PRESERVE] = crate::warp_grid::PRESERVE_1_4 as f32;
        let mut sample: Vec<f32> = (0..4_000).map(|i| i as f32 * 0.001).collect();

        let _ = render_beats(&mut state, &mut sample, 100);
        assert!(
            (state[STATE_WARP_NEXT_BOUNDARY] - 1_000.0).abs() < 0.001,
            "1/4 grid should still target source 1000 before the live edit"
        );

        state[STATE_WARP_PRESERVE] = crate::warp_grid::PRESERVE_1_16 as f32;
        let _ = render_beats(&mut state, &mut sample, 8);

        assert_eq!(
            state[STATE_WARP_LAST_PRESERVE],
            crate::warp_grid::PRESERVE_1_16 as f32
        );
        assert!(
            (state[STATE_WARP_NEXT_BOUNDARY] - 250.0).abs() < 0.001,
            "live preserve edit should recompute the active grid to source 250, got {}",
            state[STATE_WARP_NEXT_BOUNDARY]
        );
    }

    #[test]
    fn repitch_warp_advances_read_head_at_tempo_ratio() {
        let mut state = [0.0_f32; SAMPLER_STATE_SIZE];
        let initial = [0.0_f32, 44_100.0];
        unsafe {
            sampler_init(
                state.as_mut_ptr() as *mut c_void,
                44_100,
                64,
                initial.as_ptr() as *const c_void,
            );
        }
        state[STATE_PLAYING] = 1.0;
        state[STATE_PLAYHEAD] = 0.0;
        state[STATE_GAIN] = 1.0;
        state[STATE_VELOCITY] = 1.0;
        state[STATE_ATTACK_SAMPLES] = 0.0;
        state[STATE_GATE_SAMPLES] = f32::MAX;
        state[STATE_END_POINT] = 1.0;
        state[STATE_WARP_ENABLED] = 1.0;
        state[STATE_WARP_MODE] = WARP_MODE_REPITCH as f32;
        // 240 BPM sample in a 120 BPM project: read head must advance at
        // 120/240 = 0.5 source frames per host frame.
        state[STATE_WARP_SAMPLE_BPM] = 240.0;
        state[STATE_WARP_PROJECT_BPM] = 120.0;
        state[STATE_WARP_RATIO] = 0.5;
        state[STATE_WARP_LAST_TARGET_RATIO] = 0.5;

        // Mono linear ramp: interpolated read at head h yields h * 0.001.
        let mut sample: Vec<f32> = (0..512).map(|i| i as f32 * 0.001).collect();
        let desc = [BufferDesc {
            buffer: sample.as_mut_ptr(),
            size: 512,
            channel_count: 1,
        }];
        let mut left = [0.0_f32; 64];
        let mut right = [0.0_f32; 64];
        let outputs = [left.as_mut_ptr(), right.as_mut_ptr()];
        let inputs: [*mut f32; 0] = [];

        unsafe {
            sampler_process(
                inputs.as_ptr(),
                outputs.as_ptr(),
                64,
                state.as_mut_ptr() as *mut c_void,
                desc.as_ptr() as *mut c_void,
            );
        }

        for (i, &value) in left.iter().enumerate() {
            let expected = i as f32 * 0.5 * 0.001;
            assert!(
                (value - expected).abs() < 0.000_05,
                "frame {i}: expected {expected}, got {value} (full: {left:?})"
            );
        }
        // Read head landed halfway through the rendered block's frame count.
        assert!((state[STATE_PLAYHEAD] - 32.0).abs() < 0.001);
    }

    #[test]
    fn sampler_scheduled_note_on_starts_at_local_frame() {
        let mut state = [0.0_f32; SAMPLER_STATE_SIZE];
        let initial = [0.0_f32, 44_100.0];
        unsafe {
            sampler_init(
                state.as_mut_ptr() as *mut c_void,
                44_100,
                64,
                initial.as_ptr() as *const c_void,
            );
        }

        let mut sample = [
            1.0_f32, 1.0, 0.8, 0.8, 0.6, 0.6, 0.4, 0.4, 0.2, 0.2, 0.0, 0.0,
        ];
        let mut buffers = [BufferDesc {
            buffer: sample.as_mut_ptr(),
            size: 6,
            channel_count: 2,
        }];

        unsafe {
            sampler_begin_event_slice(state.as_mut_ptr().cast(), 1, 0, 8);
            let event = sampler_note_on_event(3);
            assert!(sampler_schedule_event(state.as_mut_ptr().cast(), &event));
        }

        let mut left = [0.0f32; 8];
        let mut right = [0.0f32; 8];
        let outputs = [left.as_mut_ptr(), right.as_mut_ptr()];
        unsafe {
            sampler_process(
                std::ptr::null(),
                outputs.as_ptr(),
                8,
                state.as_mut_ptr().cast(),
                buffers.as_mut_ptr().cast(),
            );
        }

        assert_eq!(&left[..3], &[0.0, 0.0, 0.0]);
        assert_eq!(&right[..3], &[0.0, 0.0, 0.0]);
        assert!((left[3] - 0.8).abs() < 0.0001, "left={left:?}");
        assert!((right[3] - 0.8).abs() < 0.0001, "right={right:?}");
    }

    #[test]
    fn sampler_note_on_aux_applies_beats_preserve_before_segment_reset() {
        for (preserve, expected_boundary) in [
            (crate::warp_grid::PRESERVE_1_8, 500.0_f32),
            (crate::warp_grid::PRESERVE_1_16, 250.0_f32),
            (crate::warp_grid::PRESERVE_TRANSIENTS, 250.0_f32),
        ] {
            let mut state = [0.0_f32; SAMPLER_STATE_SIZE];
            let initial = [0.0_f32, 1_000.0];
            unsafe {
                sampler_init(
                    state.as_mut_ptr() as *mut c_void,
                    1_000,
                    64,
                    initial.as_ptr() as *const c_void,
                );
            }
            // Prove the note-on aux value, not the initialized state, drives
            // the first boundary.
            state[STATE_WARP_PRESERVE] = crate::warp_grid::PRESERVE_1_4 as f32;

            let mut sample = vec![1.0_f32; 4_000];
            let mut buffers = [BufferDesc {
                buffer: sample.as_mut_ptr(),
                size: sample.len() as std::os::raw::c_int,
                channel_count: 1,
            }];
            let mut event = sampler_note_on_event(0);
            event.aux[SAMPLER_EVENT_AUX_WARP_ENABLED] = 1.0;
            event.aux[SAMPLER_EVENT_AUX_WARP_MODE] = WARP_MODE_BEATS as f32;
            event.aux[SAMPLER_EVENT_AUX_WARP_RATIO] = 2.0;
            event.aux[SAMPLER_EVENT_AUX_WARP_SAMPLE_BPM] = 60.0;
            event.aux[SAMPLER_EVENT_AUX_WARP_PROJECT_BPM] = 120.0;
            event.aux[SAMPLER_EVENT_AUX_WARP_PRESERVE] = preserve as f32;
            event.aux[SAMPLER_EVENT_AUX_WARP_SEG_LOOP_MODE] = SEG_LOOP_PINGPONG as f32;
            event.aux[SAMPLER_EVENT_AUX_WARP_SEG_ENVELOPE] = 0.5;

            unsafe {
                sampler_begin_event_slice(state.as_mut_ptr().cast(), 1, 0, 8);
                assert!(sampler_schedule_event(state.as_mut_ptr().cast(), &event));
            }

            let mut left = [0.0f32; 8];
            let mut right = [0.0f32; 8];
            let outputs = [left.as_mut_ptr(), right.as_mut_ptr()];
            unsafe {
                sampler_process(
                    std::ptr::null(),
                    outputs.as_ptr(),
                    8,
                    state.as_mut_ptr().cast(),
                    buffers.as_mut_ptr().cast(),
                );
            }

            assert_eq!(state[STATE_WARP_PRESERVE], preserve as f32);
            assert_eq!(state[STATE_WARP_SEG_LOOP_MODE], SEG_LOOP_PINGPONG as f32);
            assert!((state[STATE_WARP_SEG_ENVELOPE] - 0.5).abs() < 0.0001);
            assert!(
                (state[STATE_WARP_NEXT_BOUNDARY] - expected_boundary).abs() < 0.001,
                "preserve={preserve}: expected boundary {expected_boundary}, got {}",
                state[STATE_WARP_NEXT_BOUNDARY]
            );
            assert!(
                left.iter().any(|value| value.abs() > 0.001),
                "preserve={preserve}: scheduled warp note rendered silence"
            );
        }
    }

    #[test]
    fn modulation_lane_sum_adds_multiple_sampler_lanes() {
        let mut lfo = [0.25_f32];
        let mut env = [0.5_f32];
        let inputs = [lfo.as_mut_ptr(), env.as_mut_ptr()];
        let mut state = [0.0_f32; SAMPLER_STATE_SIZE];
        state[STATE_MOD_SPEED_SOURCE] = 1.0;
        state[STATE_MOD_SPEED_DEPTH] = 0.8;
        state[STATE_MOD_SPEED_LANE2_SOURCE] = 2.0;
        state[STATE_MOD_SPEED_LANE2_DEPTH] = -4.0;
        let lanes = [
            (STATE_MOD_SPEED_SOURCE, STATE_MOD_SPEED_DEPTH),
            (STATE_MOD_SPEED_LANE2_SOURCE, STATE_MOD_SPEED_LANE2_DEPTH),
            (0, 0),
            (0, 0),
        ];
        assert_eq!(lanes.len(), SAMPLER_MOD_LANES_PER_PARAM);

        let sum = unsafe {
            modulation_lane_sum(inputs.as_ptr(), inputs.len(), state.as_mut_ptr(), &lanes, 0)
        };

        assert!((sum + 1.8).abs() < 0.00001, "sum was {sum}");
    }

    #[test]
    fn accumulated_scrub_commits_offset_when_scrub_releases() {
        let mut playhead = 500.0;
        let mut scrub_smooth = 0.25;

        let read_head = accumulated_scrub_read_head(
            &mut playhead,
            &mut scrub_smooth,
            0.0,
            1_000.0,
            0.0,
            999.0,
            48_000.0,
            SCRUB_SMOOTH_TIME_MS_DEFAULT,
        );

        assert_eq!(read_head, 750.0);
        assert_eq!(playhead, 750.0);
        assert_eq!(scrub_smooth, 0.0);
    }

    #[test]
    fn accumulated_scrub_offsets_read_head_while_active_without_committing() {
        let mut playhead = 500.0;
        let mut scrub_smooth = 0.0;

        let read_head = accumulated_scrub_read_head(
            &mut playhead,
            &mut scrub_smooth,
            1.0,
            1_000.0,
            0.0,
            999.0,
            48_000.0,
            SCRUB_SMOOTH_TIME_MS_DEFAULT,
        );

        assert!(
            read_head > playhead,
            "read_head={read_head} playhead={playhead}"
        );
        assert_eq!(playhead, 500.0);
        assert!(scrub_smooth > 0.0);
    }

    #[test]
    fn accumulated_scrub_holds_peak_while_pulse_falls() {
        let mut playhead = 500.0;
        let mut scrub_smooth = 0.25;

        let read_head = accumulated_scrub_read_head(
            &mut playhead,
            &mut scrub_smooth,
            0.1,
            1_000.0,
            0.0,
            999.0,
            48_000.0,
            SCRUB_SMOOTH_TIME_MS_DEFAULT,
        );

        assert_eq!(read_head, 750.0);
        assert_eq!(playhead, 500.0);
        assert_eq!(scrub_smooth, 0.25);
    }

    #[test]
    fn accumulated_scrub_commits_before_opposite_direction_scrub() {
        let mut playhead = 500.0;
        let mut scrub_smooth = 0.25;

        let read_head = accumulated_scrub_read_head(
            &mut playhead,
            &mut scrub_smooth,
            -1.0,
            1_000.0,
            0.0,
            999.0,
            48_000.0,
            SCRUB_SMOOTH_TIME_MS_DEFAULT,
        );

        assert!(read_head < 750.0);
        assert_eq!(playhead, 750.0);
        assert!(scrub_smooth < 0.0);
    }
}
