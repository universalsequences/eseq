use crate::audiograph::NodeVTable;
use std::os::raw::{c_int, c_void};

const MAX_BUFFER_SAMPLES: usize = 96_000;
const HEADER_SLOTS: usize = 32;

const MIN_LENGTH_SEC: f32 = 0.012;
const MAX_LENGTH_SEC: f32 = 0.230;
const DEFAULT_LENGTH_SEC: f32 = 0.230;
const LENGTH_SMOOTH_TAU_SEC: f32 = 0.010;
const MIN_FADE_SAMPLES: f32 = 32.0;
const MAX_FADE_SAMPLES: f32 = 2048.0;

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
const STATE_BUF_L: usize = HEADER_SLOTS;
const STATE_BUF_R: usize = STATE_BUF_L + MAX_BUFFER_SAMPLES;
const STATE_END: usize = STATE_BUF_R + MAX_BUFFER_SAMPLES;

pub const DJ_MIXER_STATE_SIZE: usize = STATE_END;

pub const DJ_MIXER_PARAM_ENABLED: u64 = STATE_ENABLED as u64;
pub const DJ_MIXER_PARAM_SPEED: u64 = STATE_SPEED as u64;
pub const DJ_MIXER_PARAM_LENGTH_SEC: u64 = STATE_LENGTH_SEC as u64;
pub const DJ_MIXER_PARAM_LOOP: u64 = STATE_LOOP as u64;
pub const DJ_MIXER_PARAM_CLEAR_FLAG: u64 = STATE_CLEAR_FLAG as u64;

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
fn target_loop_samples(length_sec: f32, sample_rate: f32) -> f32 {
    let sr = sample_rate.clamp(8_000.0, 192_000.0);
    (length_sec.clamp(MIN_LENGTH_SEC, MAX_LENGTH_SEC) * sr)
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
    let loop_samples = target_loop_samples(DEFAULT_LENGTH_SEC, sr);
    let length_k = 1.0 - (-1.0 / (sr * LENGTH_SMOOTH_TAU_SEC)).exp();

    *s.add(STATE_ENABLED) = 1.0;
    *s.add(STATE_SPEED) = 1.0;
    *s.add(STATE_LENGTH_SEC) = DEFAULT_LENGTH_SEC;
    *s.add(STATE_LOOP) = 0.0;
    *s.add(STATE_SAMPLE_RATE) = sr;
    *s.add(STATE_WRITE_POS) = 0.0;
    *s.add(STATE_READ_POS) = wrap_pos(-loop_samples);
    *s.add(STATE_LOOP_START) = wrap_pos(-loop_samples);
    *s.add(STATE_LOOP_END) = 0.0;
    *s.add(STATE_LOOP_SAMPLES_SMOOTH) = loop_samples;
    *s.add(STATE_LENGTH_SMOOTH_K) = length_k;
    *s.add(STATE_PREV_LOOP) = 0.0;
    *s.add(STATE_XFADE_REMAINING) = 0.0;
    *s.add(STATE_XFADE_TOTAL) = 0.0;
    *s.add(STATE_XFADE_OLD_READ_POS) = 0.0;
    *s.add(STATE_CLEAR_FLAG) = 0.0;
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
    let out0 = *out.add(0);
    let out1 = *out.add(1);
    let buf_l = s.add(STATE_BUF_L);
    let buf_r = s.add(STATE_BUF_R);

    let sr = finite_or(*s.add(STATE_SAMPLE_RATE), 44_100.0).clamp(8_000.0, 192_000.0);
    let speed = finite_or(*s.add(STATE_SPEED), 1.0).clamp(-1.0, 1.0);
    let enabled = *s.add(STATE_ENABLED) > 0.5;
    let loop_enabled = *s.add(STATE_LOOP) > 0.5;
    let loop_active = enabled && loop_enabled;
    let prev_loop = *s.add(STATE_PREV_LOOP) > 0.5;
    let length_sec = finite_or(*s.add(STATE_LENGTH_SEC), DEFAULT_LENGTH_SEC)
        .clamp(MIN_LENGTH_SEC, MAX_LENGTH_SEC);
    let length_k = finite_or(*s.add(STATE_LENGTH_SMOOTH_K), 0.0).clamp(0.0, 1.0);
    let mut loop_samples = finite_or(
        *s.add(STATE_LOOP_SAMPLES_SMOOTH),
        target_loop_samples(length_sec, sr),
    )
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

    for i in 0..nf {
        let input_l = *in0.add(i);
        let input_r = *in1.add(i);

        let target_samples = target_loop_samples(length_sec, sr);
        loop_samples += length_k * (target_samples - loop_samples);
        loop_samples = loop_samples.clamp(2.0, (MAX_BUFFER_SAMPLES - 2) as f32);

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

        if enabled {
            *out0.add(i) = wet_l;
            *out1.add(i) = wet_r;
        } else {
            *out0.add(i) = input_l;
            *out1.add(i) = input_r;
        }

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
    *s.add(STATE_PREV_LOOP) = if loop_active { 1.0 } else { 0.0 };
    *s.add(STATE_XFADE_REMAINING) = xfade_remaining.max(0.0);
    *s.add(STATE_XFADE_TOTAL) = xfade_total;
    *s.add(STATE_XFADE_OLD_READ_POS) = xfade_old_read_pos;
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
        assert_eq!(left.len(), right.len());
        let mut in_l = left.to_vec();
        let mut in_r = right.to_vec();
        let mut out_l = vec![0.0; left.len()];
        let mut out_r = vec![0.0; left.len()];
        let inputs = [in_l.as_mut_ptr(), in_r.as_mut_ptr()];
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
}
