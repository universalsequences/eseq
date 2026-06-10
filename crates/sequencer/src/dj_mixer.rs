use crate::audiograph::NodeVTable;
use std::os::raw::{c_int, c_void};

const MAX_BUFFER_SAMPLES: usize = 480_000;
const HEADER_SLOTS: usize = 64;

const MIN_LENGTH_SEC: f32 = 0.012;
const MAX_LENGTH_SEC: f32 = 0.230;
const DEFAULT_LENGTH_SEC: f32 = 0.230;
const LENGTH_SMOOTH_TAU_SEC: f32 = 0.010;
const GATE_SMOOTH_TAU_SEC: f32 = 0.005;
const MIN_FADE_SAMPLES: f32 = 32.0;
const MAX_FADE_SAMPLES: f32 = 2048.0;

/// Beats per loop for each `div` enum value: 1/16, 1/8, 1/4, 1/2, 1 bar, 2 bars.
const SYNC_DIV_BEATS: [f32; 6] = [0.25, 0.5, 1.0, 2.0, 4.0, 8.0];

const STATE_ENABLED: usize = 0;
const STATE_SPEED: usize = 1;
const STATE_LENGTH_SEC: usize = 2;
const STATE_LOOP: usize = 3;
const STATE_SAMPLE_RATE: usize = 4;
const STATE_WRITE_POS: usize = 5;
const STATE_READ_POS: usize = 6;
const STATE_LOOP_START: usize = 7;
const STATE_LOOP_END: usize = 8;
const STATE_LOOP_SAMPLES_SMOOTH: usize = 9;
const STATE_LENGTH_SMOOTH_K: usize = 10;
const STATE_PREV_LOOP: usize = 11;
const STATE_XFADE_REMAINING: usize = 12;
const STATE_XFADE_TOTAL: usize = 13;
const STATE_XFADE_OLD_READ_POS: usize = 14;
const STATE_CLEAR_FLAG: usize = 15;
const STATE_BPM: usize = 16;
const STATE_SYNC: usize = 17;
const STATE_DIV: usize = 18;
const STATE_WARP: usize = 19;
const STATE_WARP_PHASE: usize = 20;
const STATE_WET_GATE: usize = 21;
const STATE_MOD_ENABLED_DEPTH: usize = 24; // 4 slots
const STATE_MOD_SPEED_DEPTH: usize = 28; // 4 slots
const STATE_MOD_LENGTH_DEPTH: usize = 32; // 4 slots
const STATE_MOD_LOOP_DEPTH: usize = 36; // 4 slots
const STATE_MOD_WARP_DEPTH: usize = 40; // 4 slots
const STATE_BUF_L: usize = HEADER_SLOTS;
const STATE_BUF_R: usize = STATE_BUF_L + MAX_BUFFER_SAMPLES;
const STATE_END: usize = STATE_BUF_R + MAX_BUFFER_SAMPLES;

pub const DJ_MIXER_STATE_SIZE: usize = STATE_END;

pub const DJ_MIXER_PARAM_ENABLED: u64 = STATE_ENABLED as u64;
pub const DJ_MIXER_PARAM_SPEED: u64 = STATE_SPEED as u64;
pub const DJ_MIXER_PARAM_LENGTH_SEC: u64 = STATE_LENGTH_SEC as u64;
pub const DJ_MIXER_PARAM_LOOP: u64 = STATE_LOOP as u64;
pub const DJ_MIXER_PARAM_CLEAR_FLAG: u64 = STATE_CLEAR_FLAG as u64;
pub const DJ_MIXER_PARAM_BPM: u64 = STATE_BPM as u64;
pub const DJ_MIXER_PARAM_SYNC: u64 = STATE_SYNC as u64;
pub const DJ_MIXER_PARAM_DIV: u64 = STATE_DIV as u64;
pub const DJ_MIXER_PARAM_WARP: u64 = STATE_WARP as u64;

pub const DJ_MIXER_PARAM_MOD_ENABLED_DEPTH_1: u64 = STATE_MOD_ENABLED_DEPTH as u64;
pub const DJ_MIXER_PARAM_MOD_ENABLED_DEPTH_2: u64 = STATE_MOD_ENABLED_DEPTH as u64 + 1;
pub const DJ_MIXER_PARAM_MOD_ENABLED_DEPTH_3: u64 = STATE_MOD_ENABLED_DEPTH as u64 + 2;
pub const DJ_MIXER_PARAM_MOD_ENABLED_DEPTH_4: u64 = STATE_MOD_ENABLED_DEPTH as u64 + 3;
pub const DJ_MIXER_PARAM_MOD_SPEED_DEPTH_1: u64 = STATE_MOD_SPEED_DEPTH as u64;
pub const DJ_MIXER_PARAM_MOD_SPEED_DEPTH_2: u64 = STATE_MOD_SPEED_DEPTH as u64 + 1;
pub const DJ_MIXER_PARAM_MOD_SPEED_DEPTH_3: u64 = STATE_MOD_SPEED_DEPTH as u64 + 2;
pub const DJ_MIXER_PARAM_MOD_SPEED_DEPTH_4: u64 = STATE_MOD_SPEED_DEPTH as u64 + 3;
pub const DJ_MIXER_PARAM_MOD_LENGTH_DEPTH_1: u64 = STATE_MOD_LENGTH_DEPTH as u64;
pub const DJ_MIXER_PARAM_MOD_LENGTH_DEPTH_2: u64 = STATE_MOD_LENGTH_DEPTH as u64 + 1;
pub const DJ_MIXER_PARAM_MOD_LENGTH_DEPTH_3: u64 = STATE_MOD_LENGTH_DEPTH as u64 + 2;
pub const DJ_MIXER_PARAM_MOD_LENGTH_DEPTH_4: u64 = STATE_MOD_LENGTH_DEPTH as u64 + 3;
pub const DJ_MIXER_PARAM_MOD_LOOP_DEPTH_1: u64 = STATE_MOD_LOOP_DEPTH as u64;
pub const DJ_MIXER_PARAM_MOD_LOOP_DEPTH_2: u64 = STATE_MOD_LOOP_DEPTH as u64 + 1;
pub const DJ_MIXER_PARAM_MOD_LOOP_DEPTH_3: u64 = STATE_MOD_LOOP_DEPTH as u64 + 2;
pub const DJ_MIXER_PARAM_MOD_LOOP_DEPTH_4: u64 = STATE_MOD_LOOP_DEPTH as u64 + 3;
pub const DJ_MIXER_PARAM_MOD_WARP_DEPTH_1: u64 = STATE_MOD_WARP_DEPTH as u64;
pub const DJ_MIXER_PARAM_MOD_WARP_DEPTH_2: u64 = STATE_MOD_WARP_DEPTH as u64 + 1;
pub const DJ_MIXER_PARAM_MOD_WARP_DEPTH_3: u64 = STATE_MOD_WARP_DEPTH as u64 + 2;
pub const DJ_MIXER_PARAM_MOD_WARP_DEPTH_4: u64 = STATE_MOD_WARP_DEPTH as u64 + 3;

#[inline]
fn finite_or(value: f32, fallback: f32) -> f32 {
    if value.is_finite() {
        value
    } else {
        fallback
    }
}

#[inline]
fn wrap_pos(pos: f32) -> f32 {
    let n = MAX_BUFFER_SAMPLES as f32;
    let mut wrapped = pos % n;
    if wrapped < 0.0 {
        wrapped += n;
    }
    wrapped
}

#[inline]
fn ring_distance(from: f32, to: f32) -> f32 {
    wrap_pos(to - from)
}

#[inline]
unsafe fn read_interpolated(base: *const f32, pos: f32) -> f32 {
    let pos = wrap_pos(pos);
    let idx0 = pos.floor() as usize;
    let frac = pos - idx0 as f32;
    let idx1 = (idx0 + 1) % MAX_BUFFER_SAMPLES;
    let a = *base.add(idx0);
    let b = *base.add(idx1);
    a + (b - a) * frac
}

#[inline]
fn free_loop_samples(length_sec: f32, sample_rate: f32) -> f32 {
    let sr = sample_rate.clamp(8_000.0, 192_000.0);
    (length_sec.clamp(MIN_LENGTH_SEC, MAX_LENGTH_SEC) * sr)
        .floor()
        .clamp(2.0, (MAX_BUFFER_SAMPLES - 2) as f32)
}

#[inline]
fn synced_loop_samples(div: f32, bpm: f32, sample_rate: f32) -> f32 {
    let idx = (div.round() as usize).min(SYNC_DIV_BEATS.len() - 1);
    let beats = SYNC_DIV_BEATS[idx];
    let sr = sample_rate.clamp(8_000.0, 192_000.0);
    (beats * 60.0 / bpm.clamp(20.0, 999.0) * sr)
        .floor()
        .clamp(2.0, (MAX_BUFFER_SAMPLES - 2) as f32)
}

#[inline]
fn fade_samples(loop_samples: f32) -> f32 {
    (loop_samples * 0.08)
        .floor()
        .clamp(MIN_FADE_SAMPLES, MAX_FADE_SAMPLES)
        .min(loop_samples * 0.5)
        .max(1.0)
}

#[inline]
fn cosine_fade(phase: f32) -> f32 {
    let t = phase.clamp(0.0, 1.0);
    0.5 - 0.5 * (std::f32::consts::PI * t).cos()
}

#[inline]
fn start_xfade(
    read_pos: &mut f32,
    xfade_old_read_pos: &mut f32,
    xfade_remaining: &mut f32,
    xfade_total: &mut f32,
    new_read_pos: f32,
    loop_samples: f32,
) {
    *xfade_old_read_pos = *read_pos;
    *read_pos = wrap_pos(new_read_pos);
    *xfade_total = fade_samples(loop_samples);
    *xfade_remaining = *xfade_total;
}

unsafe extern "C" fn dj_mixer_init(
    state: *mut c_void,
    sample_rate: c_int,
    _max_block: c_int,
    _initial_state: *const c_void,
) {
    let s = state as *mut f32;
    let sr = (sample_rate as f32).clamp(8_000.0, 192_000.0);
    let loop_samples = free_loop_samples(DEFAULT_LENGTH_SEC, sr);
    let length_k = 1.0 - (-1.0 / (sr * LENGTH_SMOOTH_TAU_SEC)).exp();

    for i in 0..HEADER_SLOTS {
        *s.add(i) = 0.0;
    }
    *s.add(STATE_ENABLED) = 1.0;
    *s.add(STATE_SPEED) = 1.0;
    *s.add(STATE_LENGTH_SEC) = DEFAULT_LENGTH_SEC;
    *s.add(STATE_SAMPLE_RATE) = sr;
    *s.add(STATE_READ_POS) = wrap_pos(-loop_samples);
    *s.add(STATE_LOOP_START) = wrap_pos(-loop_samples);
    *s.add(STATE_LOOP_SAMPLES_SMOOTH) = loop_samples;
    *s.add(STATE_LENGTH_SMOOTH_K) = length_k;
    *s.add(STATE_BPM) = 120.0;
    *s.add(STATE_DIV) = 4.0;
    *s.add(STATE_WET_GATE) = 1.0;
    for i in STATE_BUF_L..STATE_END {
        *s.add(i) = 0.0;
    }
}

unsafe extern "C" fn dj_mixer_process(
    inp: *const *mut f32,
    out: *const *mut f32,
    nframes: c_int,
    state: *mut c_void,
    _buffers: *mut c_void,
) {
    let s = state as *mut f32;
    let nf = nframes as usize;
    let in0 = *inp.add(0);
    let in1 = *inp.add(1);
    let mod_inputs = [*inp.add(2), *inp.add(3), *inp.add(4), *inp.add(5)];
    let out0 = *out.add(0);
    let out1 = *out.add(1);
    let buf_l = s.add(STATE_BUF_L);
    let buf_r = s.add(STATE_BUF_R);

    let sr = finite_or(*s.add(STATE_SAMPLE_RATE), 44_100.0).clamp(8_000.0, 192_000.0);
    let base_speed = finite_or(*s.add(STATE_SPEED), 1.0).clamp(-1.0, 1.0);
    let enabled_base = finite_or(*s.add(STATE_ENABLED), 1.0);
    let loop_base = finite_or(*s.add(STATE_LOOP), 0.0);
    let mut prev_loop = *s.add(STATE_PREV_LOOP) > 0.5;
    let length_sec = finite_or(*s.add(STATE_LENGTH_SEC), DEFAULT_LENGTH_SEC)
        .clamp(MIN_LENGTH_SEC, MAX_LENGTH_SEC);
    let length_k = finite_or(*s.add(STATE_LENGTH_SMOOTH_K), 0.0).clamp(0.0, 1.0);
    let bpm = finite_or(*s.add(STATE_BPM), 120.0).clamp(20.0, 999.0);
    let sync = *s.add(STATE_SYNC) > 0.5;
    let div = finite_or(*s.add(STATE_DIV), 4.0).clamp(0.0, (SYNC_DIV_BEATS.len() - 1) as f32);
    let warp_base = finite_or(*s.add(STATE_WARP), 0.0).clamp(0.0, 1.0);
    let mut warp_phase = finite_or(*s.add(STATE_WARP_PHASE), 0.0).rem_euclid(1.0);
    let mut wet_gate = finite_or(*s.add(STATE_WET_GATE), 1.0).clamp(0.0, 1.0);
    let gate_k = 1.0 - (-1.0 / (sr * GATE_SMOOTH_TAU_SEC)).exp();

    let depth = |base: usize, slot: usize| -> f32 { unsafe { finite_or(*s.add(base + slot), 0.0) } };
    let enabled_depths = [
        depth(STATE_MOD_ENABLED_DEPTH, 0),
        depth(STATE_MOD_ENABLED_DEPTH, 1),
        depth(STATE_MOD_ENABLED_DEPTH, 2),
        depth(STATE_MOD_ENABLED_DEPTH, 3),
    ];
    let speed_depths = [
        depth(STATE_MOD_SPEED_DEPTH, 0),
        depth(STATE_MOD_SPEED_DEPTH, 1),
        depth(STATE_MOD_SPEED_DEPTH, 2),
        depth(STATE_MOD_SPEED_DEPTH, 3),
    ];
    let length_depths = [
        depth(STATE_MOD_LENGTH_DEPTH, 0),
        depth(STATE_MOD_LENGTH_DEPTH, 1),
        depth(STATE_MOD_LENGTH_DEPTH, 2),
        depth(STATE_MOD_LENGTH_DEPTH, 3),
    ];
    let loop_depths = [
        depth(STATE_MOD_LOOP_DEPTH, 0),
        depth(STATE_MOD_LOOP_DEPTH, 1),
        depth(STATE_MOD_LOOP_DEPTH, 2),
        depth(STATE_MOD_LOOP_DEPTH, 3),
    ];
    let warp_depths = [
        depth(STATE_MOD_WARP_DEPTH, 0),
        depth(STATE_MOD_WARP_DEPTH, 1),
        depth(STATE_MOD_WARP_DEPTH, 2),
        depth(STATE_MOD_WARP_DEPTH, 3),
    ];

    let base_target = if sync {
        synced_loop_samples(div, bpm, sr)
    } else {
        free_loop_samples(length_sec, sr)
    };
    let mut loop_samples = finite_or(*s.add(STATE_LOOP_SAMPLES_SMOOTH), base_target)
        .clamp(2.0, (MAX_BUFFER_SAMPLES - 2) as f32);
    let mut write_pos =
        finite_or(*s.add(STATE_WRITE_POS), 0.0).clamp(0.0, MAX_BUFFER_SAMPLES as f32);
    if write_pos >= MAX_BUFFER_SAMPLES as f32 {
        write_pos = 0.0;
    }
    let mut read_pos = wrap_pos(finite_or(*s.add(STATE_READ_POS), write_pos - loop_samples));
    let mut loop_start = wrap_pos(finite_or(
        *s.add(STATE_LOOP_START),
        write_pos - loop_samples,
    ));
    let mut loop_end = wrap_pos(finite_or(*s.add(STATE_LOOP_END), write_pos));
    let mut xfade_remaining = finite_or(*s.add(STATE_XFADE_REMAINING), 0.0).max(0.0);
    let mut xfade_total = finite_or(*s.add(STATE_XFADE_TOTAL), 0.0).max(0.0);
    let mut xfade_old_read_pos = wrap_pos(finite_or(*s.add(STATE_XFADE_OLD_READ_POS), read_pos));

    if *s.add(STATE_CLEAR_FLAG) != 0.0 {
        for i in 0..(MAX_BUFFER_SAMPLES * 2) {
            *s.add(STATE_BUF_L + i) = 0.0;
        }
        write_pos = 0.0;
        read_pos = wrap_pos(-loop_samples);
        loop_start = read_pos;
        loop_end = 0.0;
        xfade_remaining = 0.0;
        xfade_total = 0.0;
        xfade_old_read_pos = read_pos;
        *s.add(STATE_CLEAR_FLAG) = 0.0;
    }

    for i in 0..nf {
        let input_l = *in0.add(i);
        let input_r = *in1.add(i);

        let mut mod_enabled = 0.0;
        let mut mod_speed = 0.0;
        let mut mod_length = 0.0;
        let mut mod_loop = 0.0;
        let mut mod_warp = 0.0;
        for slot in 0..4 {
            let v = (*mod_inputs[slot].add(i)).clamp(0.0, 1.0);
            mod_enabled += v * enabled_depths[slot];
            mod_speed += v * speed_depths[slot];
            mod_length += v * length_depths[slot];
            mod_loop += v * loop_depths[slot];
            mod_warp += v * warp_depths[slot];
        }

        let eff_enabled = (enabled_base + mod_enabled) > 0.5;
        let eff_loop = (loop_base + mod_loop) > 0.5;
        let loop_active = eff_enabled && eff_loop;
        let eff_warp = (warp_base + mod_warp).clamp(0.0, 1.0);

        // Warp: read-speed wobble whose rate scales with depth — gentle
        // tape warble at low settings, seasick mangling near 1.
        let warp_rate_hz = 0.3 + eff_warp * 5.7;
        warp_phase = (warp_phase + warp_rate_hz / sr).rem_euclid(1.0);
        let warp_offset =
            eff_warp * 0.6 * (warp_phase * std::f32::consts::TAU).sin();
        let speed = (base_speed + mod_speed + warp_offset).clamp(-2.0, 2.0);

        // Length modulation is in octaves around the base (free or synced) length.
        let target_samples = (base_target * 2.0_f32.powf(mod_length.clamp(-3.0, 3.0)))
            .clamp(2.0, (MAX_BUFFER_SAMPLES - 2) as f32);
        loop_samples += length_k * (target_samples - loop_samples);
        loop_samples = loop_samples.clamp(2.0, (MAX_BUFFER_SAMPLES - 2) as f32);

        if loop_active && !prev_loop {
            loop_start = wrap_pos(write_pos - loop_samples);
            loop_end = write_pos;
            read_pos = if speed < 0.0 {
                wrap_pos(loop_end - 1.0)
            } else {
                loop_start
            };
            xfade_remaining = 0.0;
            xfade_total = 0.0;
            xfade_old_read_pos = read_pos;
        }
        prev_loop = loop_active;

        if !loop_active {
            *buf_l.add(write_pos as usize) = input_l;
            *buf_r.add(write_pos as usize) = input_r;
        }

        let mut wet_l = read_interpolated(buf_l as *const f32, read_pos);
        let mut wet_r = read_interpolated(buf_r as *const f32, read_pos);
        if xfade_remaining > 0.0 && xfade_total > 0.0 {
            let old_l = read_interpolated(buf_l as *const f32, xfade_old_read_pos);
            let old_r = read_interpolated(buf_r as *const f32, xfade_old_read_pos);
            let phase = 1.0 - xfade_remaining / xfade_total;
            let mix = cosine_fade(phase);
            wet_l = old_l * (1.0 - mix) + wet_l * mix;
            wet_r = old_r * (1.0 - mix) + wet_r * mix;
            xfade_old_read_pos = wrap_pos(xfade_old_read_pos + speed);
            xfade_remaining -= 1.0;
        }

        // Declicked bypass: smoothed equal-gain crossfade between dry and wet.
        let gate_target = if eff_enabled { 1.0 } else { 0.0 };
        wet_gate += gate_k * (gate_target - wet_gate);
        *out0.add(i) = input_l * (1.0 - wet_gate) + wet_l * wet_gate;
        *out1.add(i) = input_r * (1.0 - wet_gate) + wet_r * wet_gate;

        if !loop_active {
            write_pos += 1.0;
            if write_pos >= MAX_BUFFER_SAMPLES as f32 {
                write_pos = 0.0;
            }
        }

        if speed.abs() > 0.000001 {
            read_pos = wrap_pos(read_pos + speed);
        }

        if loop_active {
            let pos_from_start = ring_distance(loop_start, read_pos);
            if speed >= 0.0 && pos_from_start >= loop_samples {
                start_xfade(
                    &mut read_pos,
                    &mut xfade_old_read_pos,
                    &mut xfade_remaining,
                    &mut xfade_total,
                    loop_start + (pos_from_start - loop_samples),
                    loop_samples,
                );
            } else if speed < 0.0 && pos_from_start > loop_samples {
                start_xfade(
                    &mut read_pos,
                    &mut xfade_old_read_pos,
                    &mut xfade_remaining,
                    &mut xfade_total,
                    loop_start + loop_samples - 1.0,
                    loop_samples,
                );
            }
        } else if speed.abs() > 0.000001 {
            let pos_from_start = ring_distance(loop_start, read_pos);
            if speed >= 0.0 && pos_from_start >= loop_samples {
                let new_start = wrap_pos(write_pos - loop_samples);
                let overshoot = pos_from_start - loop_samples;
                start_xfade(
                    &mut read_pos,
                    &mut xfade_old_read_pos,
                    &mut xfade_remaining,
                    &mut xfade_total,
                    new_start + overshoot,
                    loop_samples,
                );
                loop_start = new_start;
                loop_end = write_pos;
            } else if speed < 0.0 && pos_from_start > loop_samples {
                let new_start = wrap_pos(write_pos - loop_samples);
                start_xfade(
                    &mut read_pos,
                    &mut xfade_old_read_pos,
                    &mut xfade_remaining,
                    &mut xfade_total,
                    new_start + loop_samples - 1.0,
                    loop_samples,
                );
                loop_start = new_start;
                loop_end = write_pos;
            }
        }
    }

    *s.add(STATE_SAMPLE_RATE) = sr;
    *s.add(STATE_LENGTH_SEC) = length_sec;
    *s.add(STATE_WRITE_POS) = write_pos;
    *s.add(STATE_READ_POS) = read_pos;
    *s.add(STATE_LOOP_START) = loop_start;
    *s.add(STATE_LOOP_END) = loop_end;
    *s.add(STATE_LOOP_SAMPLES_SMOOTH) = loop_samples;
    *s.add(STATE_PREV_LOOP) = if prev_loop { 1.0 } else { 0.0 };
    *s.add(STATE_XFADE_REMAINING) = xfade_remaining.max(0.0);
    *s.add(STATE_XFADE_TOTAL) = xfade_total;
    *s.add(STATE_XFADE_OLD_READ_POS) = xfade_old_read_pos;
    *s.add(STATE_WARP_PHASE) = warp_phase;
    *s.add(STATE_WET_GATE) = wet_gate;
}

pub fn dj_mixer_vtable() -> NodeVTable {
    NodeVTable {
        process: Some(dj_mixer_process),
        init: Some(dj_mixer_init),
        reset: None,
        migrate: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ptr;

    fn process_block(state: &mut [f32], left: &[f32], right: &[f32]) -> (Vec<f32>, Vec<f32>) {
        process_block_with_mods(state, left, right, None)
    }

    fn process_block_with_mods(
        state: &mut [f32],
        left: &[f32],
        right: &[f32],
        mods: Option<&[Vec<f32>; 4]>,
    ) -> (Vec<f32>, Vec<f32>) {
        assert_eq!(left.len(), right.len());
        let mut in_l = left.to_vec();
        let mut in_r = right.to_vec();
        let mut out_l = vec![0.0; left.len()];
        let mut out_r = vec![0.0; left.len()];
        let mut zero_mods = [
            vec![0.0f32; left.len()],
            vec![0.0f32; left.len()],
            vec![0.0f32; left.len()],
            vec![0.0f32; left.len()],
        ];
        let mut mod_bufs: Vec<Vec<f32>> = match mods {
            Some(m) => m.to_vec(),
            None => zero_mods.iter_mut().map(std::mem::take).collect(),
        };
        let inputs = [
            in_l.as_mut_ptr(),
            in_r.as_mut_ptr(),
            mod_bufs[0].as_mut_ptr(),
            mod_bufs[1].as_mut_ptr(),
            mod_bufs[2].as_mut_ptr(),
            mod_bufs[3].as_mut_ptr(),
        ];
        let outputs = [out_l.as_mut_ptr(), out_r.as_mut_ptr()];
        unsafe {
            dj_mixer_process(
                inputs.as_ptr(),
                outputs.as_ptr(),
                left.len() as c_int,
                state.as_mut_ptr() as *mut c_void,
                ptr::null_mut(),
            );
        }
        (out_l, out_r)
    }

    fn init_state(sample_rate: i32) -> Vec<f32> {
        let mut state = vec![0.0; DJ_MIXER_STATE_SIZE];
        unsafe {
            dj_mixer_init(
                state.as_mut_ptr() as *mut c_void,
                sample_rate,
                128,
                ptr::null(),
            );
        }
        state
    }

    #[test]
    fn disabled_passes_through_but_keeps_buffer_fresh() {
        let mut state = init_state(44_100);
        state[STATE_ENABLED] = 0.0;
        state[STATE_WET_GATE] = 0.0;
        state[STATE_LOOP] = 1.0;
        state[STATE_LENGTH_SEC] = MIN_LENGTH_SEC;
        let input: Vec<f32> = (0..4096).map(|i| i as f32 / 4096.0).collect();
        let (out_l, _) = process_block(&mut state, &input, &input);
        assert_eq!(out_l, input);
        assert!(state[STATE_WRITE_POS] > 0.0);

        state[STATE_ENABLED] = 1.0;
        let silence = vec![0.0; 512];
        let (wet_l, _) = process_block(&mut state, &silence, &silence);
        assert!(wet_l.iter().any(|sample| sample.abs() > 0.0001));
    }

    #[test]
    fn bypass_transition_is_smooth() {
        let mut state = init_state(44_100);
        state[STATE_LENGTH_SEC] = MIN_LENGTH_SEC;
        let input: Vec<f32> = (0..4096).map(|i| (i as f32 * 0.05).sin()).collect();
        process_block(&mut state, &input, &input);

        // Toggle to bypass mid-stream: the gate should ramp, not jump.
        state[STATE_ENABLED] = 0.0;
        let dc = vec![1.0f32; 512];
        let (out_l, _) = process_block(&mut state, &dc, &dc);
        let max_step = out_l
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0f32, f32::max);
        assert!(max_step < 0.2, "bypass clicked: max step {max_step}");
    }

    #[test]
    fn rolling_mode_is_not_dry_when_speed_changes() {
        let mut state = init_state(44_100);
        state[STATE_SPEED] = 0.5;
        state[STATE_LENGTH_SEC] = MIN_LENGTH_SEC;
        let input: Vec<f32> = (0..4096).map(|i| (i as f32 * 0.01).sin()).collect();
        let (out_l, _) = process_block(&mut state, &input, &input);
        let differs = out_l
            .iter()
            .zip(input.iter())
            .skip(1024)
            .any(|(a, b)| (*a - *b).abs() > 0.001);
        assert!(differs);
        assert!(out_l.iter().all(|sample| sample.is_finite()));
    }

    #[test]
    fn loop_freezes_current_window() {
        let mut state = init_state(44_100);
        state[STATE_LENGTH_SEC] = MIN_LENGTH_SEC;
        let ramp: Vec<f32> = (0..4096).map(|i| i as f32 / 4096.0).collect();
        process_block(&mut state, &ramp, &ramp);

        state[STATE_LOOP] = 1.0;
        let silence = vec![0.0; 2048];
        let (out_l, _) = process_block(&mut state, &silence, &silence);
        assert!(out_l.iter().any(|sample| sample.abs() > 0.0001));
    }

    #[test]
    fn zero_speed_holds_position() {
        let mut state = init_state(44_100);
        state[STATE_LENGTH_SEC] = MIN_LENGTH_SEC;
        let ramp: Vec<f32> = (0..2048).map(|i| i as f32 / 2048.0).collect();
        process_block(&mut state, &ramp, &ramp);

        state[STATE_LOOP] = 1.0;
        state[STATE_SPEED] = 0.0;
        let silence = vec![0.0; 256];
        let (out_l, _) = process_block(&mut state, &silence, &silence);
        let reference = out_l[32];
        assert!(out_l[32..]
            .iter()
            .all(|sample| (*sample - reference).abs() < 0.0001));
    }

    #[test]
    fn reverse_speed_stays_finite() {
        let mut state = init_state(44_100);
        state[STATE_SPEED] = -1.0;
        state[STATE_LENGTH_SEC] = MIN_LENGTH_SEC;
        let input: Vec<f32> = (0..8192).map(|i| (i as f32 * 0.02).sin()).collect();
        let (out_l, out_r) = process_block(&mut state, &input, &input);
        assert!(out_l
            .iter()
            .chain(out_r.iter())
            .all(|sample| sample.is_finite()));
    }

    #[test]
    fn length_parameter_is_clamped_to_sp_range() {
        let mut state = init_state(44_100);
        state[STATE_LENGTH_SEC] = 10.0;
        let silence = vec![0.0; 64];
        process_block(&mut state, &silence, &silence);
        assert_eq!(state[STATE_LENGTH_SEC], MAX_LENGTH_SEC);

        state[STATE_LENGTH_SEC] = 0.001;
        process_block(&mut state, &silence, &silence);
        assert_eq!(state[STATE_LENGTH_SEC], MIN_LENGTH_SEC);
    }

    #[test]
    fn sync_mode_tracks_bpm_loop_length() {
        let mut state = init_state(48_000);
        state[STATE_SYNC] = 1.0;
        state[STATE_DIV] = 2.0; // 1/4 note
        state[STATE_BPM] = 120.0;
        // One quarter at 120bpm = 0.5s = 24_000 samples.
        let silence = vec![0.0; 48_000];
        process_block(&mut state, &silence, &silence);
        let smoothed = state[STATE_LOOP_SAMPLES_SMOOTH];
        assert!(
            (smoothed - 24_000.0).abs() < 100.0,
            "expected ~24000, got {smoothed}"
        );
    }

    #[test]
    fn loop_modulation_engages_loop_per_sample() {
        let mut state = init_state(44_100);
        state[STATE_LENGTH_SEC] = MIN_LENGTH_SEC;
        state[STATE_MOD_LOOP_DEPTH] = 1.0; // slot 1 drives loop
        let ramp: Vec<f32> = (0..4096).map(|i| i as f32 / 4096.0).collect();
        process_block(&mut state, &ramp, &ramp);

        // Gate the loop on via mod input mid-block; output should keep sounding
        // from the frozen window even with silent input.
        let n = 2048;
        let mods = [
            vec![1.0f32; n],
            vec![0.0f32; n],
            vec![0.0f32; n],
            vec![0.0f32; n],
        ];
        let silence = vec![0.0; n];
        let (out_l, _) = process_block_with_mods(&mut state, &silence, &silence, Some(&mods));
        assert!(out_l.iter().any(|sample| sample.abs() > 0.0001));
        assert_eq!(state[STATE_PREV_LOOP], 1.0);
    }

    #[test]
    fn warp_wobbles_playback() {
        let mut state = init_state(44_100);
        state[STATE_WARP] = 1.0;
        state[STATE_LENGTH_SEC] = MAX_LENGTH_SEC;
        let input: Vec<f32> = (0..16_384).map(|i| (i as f32 * 0.05).sin()).collect();
        let (out_l, _) = process_block(&mut state, &input, &input);
        assert!(out_l.iter().all(|sample| sample.is_finite()));
        let differs = out_l
            .iter()
            .zip(input.iter())
            .skip(4096)
            .any(|(a, b)| (*a - *b).abs() > 0.01);
        assert!(differs, "warp should detune output away from input");
    }
}
