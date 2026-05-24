use crate::audiograph::NodeVTable;
use std::os::raw::{c_int, c_void};

// State layout indices (f32 slots)
const STATE_ENABLED: usize = 0;
const STATE_MODE: usize = 1; // 0=LP, 1=HP, 2=BP, 3=notch
const STATE_CUTOFF: usize = 2; // Hz
const STATE_RESONANCE: usize = 3;
const STATE_IC1EQ_L: usize = 4; // SVF integrator state
const STATE_IC2EQ_L: usize = 5;
const STATE_IC1EQ_R: usize = 6;
const STATE_IC2EQ_R: usize = 7;
const STATE_SMOOTH_CUTOFF: usize = 8;
const STATE_SMOOTH_RESO: usize = 9;
const STATE_SAMPLE_RATE: usize = 10;
const STATE_DRIVE: usize = 11;
const STATE_WET: usize = 12;
const STATE_LFO_AMOUNT: usize = 13;
const STATE_LFO_RATE: usize = 14;
const STATE_LFO_SYNCED: usize = 15;
const STATE_LFO_DIVISION: usize = 16;
const STATE_LFO_WAVE: usize = 17;
const STATE_ENV_AMOUNT: usize = 18;
const STATE_ENV_ATTACK_MS: usize = 19;
const STATE_ENV_RELEASE_MS: usize = 20;
const STATE_SLOPE: usize = 21; // 0=12dB, 1=24dB
const STATE_LFO_PHASE_OFFSET: usize = 22; // 0..1 cycle offset
const STATE_BPM: usize = 23;
const STATE_SMOOTH_DRIVE: usize = 24;
const STATE_SMOOTH_WET: usize = 25;
const STATE_SMOOTH_LFO_AMOUNT: usize = 26;
const STATE_SMOOTH_ENV_AMOUNT: usize = 27;
const STATE_LFO_PHASE: usize = 28;
const STATE_ENV_FOLLOW: usize = 29;
const STATE_SH_PHASE: usize = 30;
const STATE_SH_VALUE: usize = 31;
const STATE_IC1EQ2_L: usize = 32;
const STATE_IC2EQ2_L: usize = 33;
const STATE_IC1EQ2_R: usize = 34;
const STATE_IC2EQ2_R: usize = 35;
const STATE_MOD_CUTOFF_DEPTH_1: usize = 36;
const STATE_MOD_CUTOFF_DEPTH_2: usize = 37;
const STATE_MOD_CUTOFF_DEPTH_3: usize = 38;
const STATE_MOD_CUTOFF_DEPTH_4: usize = 39;
pub const FILTER_STATE_SIZE: usize = 40;

// Param indices for external control
pub const FILTER_PARAM_ENABLED: u64 = STATE_ENABLED as u64;
pub const FILTER_PARAM_MODE: u64 = STATE_MODE as u64;
pub const FILTER_PARAM_CUTOFF: u64 = STATE_CUTOFF as u64;
pub const FILTER_PARAM_RESONANCE: u64 = STATE_RESONANCE as u64;
pub const FILTER_PARAM_DRIVE: u64 = STATE_DRIVE as u64;
pub const FILTER_PARAM_WET: u64 = STATE_WET as u64;
pub const FILTER_PARAM_LFO_AMOUNT: u64 = STATE_LFO_AMOUNT as u64;
pub const FILTER_PARAM_LFO_RATE: u64 = STATE_LFO_RATE as u64;
pub const FILTER_PARAM_LFO_SYNCED: u64 = STATE_LFO_SYNCED as u64;
pub const FILTER_PARAM_LFO_DIVISION: u64 = STATE_LFO_DIVISION as u64;
pub const FILTER_PARAM_LFO_WAVE: u64 = STATE_LFO_WAVE as u64;
pub const FILTER_PARAM_ENV_AMOUNT: u64 = STATE_ENV_AMOUNT as u64;
pub const FILTER_PARAM_ENV_ATTACK_MS: u64 = STATE_ENV_ATTACK_MS as u64;
pub const FILTER_PARAM_ENV_RELEASE_MS: u64 = STATE_ENV_RELEASE_MS as u64;
pub const FILTER_PARAM_SLOPE: u64 = STATE_SLOPE as u64;
pub const FILTER_PARAM_LFO_PHASE_OFFSET: u64 = STATE_LFO_PHASE_OFFSET as u64;
pub const FILTER_PARAM_BPM: u64 = STATE_BPM as u64;
pub const FILTER_PARAM_MOD_CUTOFF_DEPTH_1: u64 = STATE_MOD_CUTOFF_DEPTH_1 as u64;
pub const FILTER_PARAM_MOD_CUTOFF_DEPTH_2: u64 = STATE_MOD_CUTOFF_DEPTH_2 as u64;
pub const FILTER_PARAM_MOD_CUTOFF_DEPTH_3: u64 = STATE_MOD_CUTOFF_DEPTH_3 as u64;
pub const FILTER_PARAM_MOD_CUTOFF_DEPTH_4: u64 = STATE_MOD_CUTOFF_DEPTH_4 as u64;

const SYNC_BEATS: [f32; 11] = [
    0.125,     // 1/32
    0.25,      // 1/16
    1.0 / 6.0, // 1/16t
    0.5,       // 1/8
    1.0 / 3.0, // 1/8t
    0.75,      // 1/8.
    1.0,       // 1/4
    2.0 / 3.0, // 1/4t
    1.5,       // 1/4.
    2.0,       // 1/2
    4.0,       // 1
];

#[inline]
fn synced_rate_hz(div_idx: usize, bpm: f32) -> f32 {
    let beats = SYNC_BEATS[div_idx.min(SYNC_BEATS.len() - 1)];
    (bpm.max(20.0) / 60.0) / beats.max(0.0001)
}

#[inline]
fn lfo_value(wave: i32, phase: f32, sh_value: f32) -> f32 {
    match wave {
        1 => 1.0 - 4.0 * (phase - 0.5).abs(), // triangle
        2 => phase * 2.0 - 1.0,               // saw up
        3 => 1.0 - phase * 2.0,               // saw down
        4 => {
            if phase < 0.5 {
                1.0
            } else {
                -1.0
            }
        }
        5 => sh_value,
        _ => (phase * std::f32::consts::TAU).sin(),
    }
}

#[inline]
fn next_noise(seed: f32) -> f32 {
    let x = (seed * 12.9898 + 78.233).sin() * 43_758.547;
    (x.fract() * 2.0 - 1.0).clamp(-1.0, 1.0)
}

#[inline]
fn svf_sample(input: f32, g: f32, k: f32, mode: i32, ic1eq: &mut f32, ic2eq: &mut f32) -> f32 {
    let a1 = 1.0 / (1.0 + g * (g + k));
    let a2 = g * a1;
    let a3 = g * a2;
    let v3 = input - *ic2eq;
    let v1 = a1 * *ic1eq + a2 * v3;
    let v2 = *ic2eq + a2 * *ic1eq + a3 * v3;
    *ic1eq = 2.0 * v1 - *ic1eq;
    *ic2eq = 2.0 * v2 - *ic2eq;
    match mode {
        0 => v2,
        1 => input - k * v1 - v2,
        2 => v1,
        3 => input - k * v1,
        _ => v2,
    }
}

#[inline]
fn drive_sample(input: f32, drive: f32) -> f32 {
    if drive <= 0.0001 {
        input
    } else {
        let gain = 1.0 + drive.clamp(0.0, 1.0) * 14.0;
        (input * gain).tanh() / gain.tanh().max(0.0001)
    }
}

unsafe extern "C" fn filter_init(
    state: *mut c_void,
    sample_rate: c_int,
    _max_block: c_int,
    initial_state: *const c_void,
) {
    let s = state as *mut f32;
    let _ = initial_state;
    *s.add(STATE_ENABLED) = 0.0;
    *s.add(STATE_MODE) = 0.0;
    *s.add(STATE_CUTOFF) = 1000.0;
    *s.add(STATE_RESONANCE) = 1.0;
    *s.add(STATE_IC1EQ_L) = 0.0;
    *s.add(STATE_IC2EQ_L) = 0.0;
    *s.add(STATE_IC1EQ_R) = 0.0;
    *s.add(STATE_IC2EQ_R) = 0.0;
    *s.add(STATE_SMOOTH_CUTOFF) = 1000.0;
    *s.add(STATE_SMOOTH_RESO) = 1.0;
    *s.add(STATE_SAMPLE_RATE) = sample_rate as f32;
    *s.add(STATE_DRIVE) = 0.0;
    *s.add(STATE_WET) = 1.0;
    *s.add(STATE_LFO_AMOUNT) = 0.0;
    *s.add(STATE_LFO_RATE) = 1.0;
    *s.add(STATE_LFO_SYNCED) = 0.0;
    *s.add(STATE_LFO_DIVISION) = 6.0;
    *s.add(STATE_LFO_WAVE) = 0.0;
    *s.add(STATE_ENV_AMOUNT) = 0.0;
    *s.add(STATE_ENV_ATTACK_MS) = 5.0;
    *s.add(STATE_ENV_RELEASE_MS) = 120.0;
    *s.add(STATE_SLOPE) = 0.0;
    *s.add(STATE_LFO_PHASE_OFFSET) = 0.0;
    *s.add(STATE_BPM) = 120.0;
    *s.add(STATE_SMOOTH_DRIVE) = 0.0;
    *s.add(STATE_SMOOTH_WET) = 1.0;
    *s.add(STATE_SMOOTH_LFO_AMOUNT) = 0.0;
    *s.add(STATE_SMOOTH_ENV_AMOUNT) = 0.0;
    *s.add(STATE_LFO_PHASE) = 0.0;
    *s.add(STATE_ENV_FOLLOW) = 0.0;
    *s.add(STATE_SH_PHASE) = 0.0;
    *s.add(STATE_SH_VALUE) = 0.0;
    *s.add(STATE_IC1EQ2_L) = 0.0;
    *s.add(STATE_IC2EQ2_L) = 0.0;
    *s.add(STATE_IC1EQ2_R) = 0.0;
    *s.add(STATE_IC2EQ2_R) = 0.0;
    *s.add(STATE_MOD_CUTOFF_DEPTH_1) = 0.0;
    *s.add(STATE_MOD_CUTOFF_DEPTH_2) = 0.0;
    *s.add(STATE_MOD_CUTOFF_DEPTH_3) = 0.0;
    *s.add(STATE_MOD_CUTOFF_DEPTH_4) = 0.0;
}

unsafe extern "C" fn filter_process(
    inp: *const *mut f32,
    out: *const *mut f32,
    nframes: c_int,
    state: *mut c_void,
    _buffers: *mut c_void,
) {
    let s = state as *mut f32;
    let enabled = *s.add(STATE_ENABLED);
    let nf = nframes as usize;

    let in0 = *inp.add(0);
    let in1 = *inp.add(1);
    let mod_cutoff_inputs = [*inp.add(2), *inp.add(3), *inp.add(4), *inp.add(5)];
    let out0 = *out.add(0);
    let out1 = *out.add(1);

    if enabled <= 0.5 {
        // Bypass: pass-through and reset integrator state to avoid click on re-enable
        *s.add(STATE_IC1EQ_L) = 0.0;
        *s.add(STATE_IC2EQ_L) = 0.0;
        *s.add(STATE_IC1EQ_R) = 0.0;
        *s.add(STATE_IC2EQ_R) = 0.0;
        *s.add(STATE_IC1EQ2_L) = 0.0;
        *s.add(STATE_IC2EQ2_L) = 0.0;
        *s.add(STATE_IC1EQ2_R) = 0.0;
        *s.add(STATE_IC2EQ2_R) = 0.0;
        for i in 0..nf {
            *out0.add(i) = *in0.add(i);
            *out1.add(i) = *in1.add(i);
        }
        return;
    }

    let mode = (*s.add(STATE_MODE)).round() as i32;
    let target_cutoff = *s.add(STATE_CUTOFF);
    let target_reso = *s.add(STATE_RESONANCE);
    let sr = *s.add(STATE_SAMPLE_RATE);
    let target_drive = *s.add(STATE_DRIVE);
    let target_wet = *s.add(STATE_WET);
    let mod_cutoff_depths = [
        *s.add(STATE_MOD_CUTOFF_DEPTH_1),
        *s.add(STATE_MOD_CUTOFF_DEPTH_2),
        *s.add(STATE_MOD_CUTOFF_DEPTH_3),
        *s.add(STATE_MOD_CUTOFF_DEPTH_4),
    ];
    let lfo_rate_hz = if *s.add(STATE_LFO_SYNCED) > 0.5 {
        synced_rate_hz(
            (*s.add(STATE_LFO_DIVISION)).round() as usize,
            *s.add(STATE_BPM),
        )
    } else {
        *s.add(STATE_LFO_RATE)
    }
    .clamp(0.01, 40.0);
    let lfo_wave = (*s.add(STATE_LFO_WAVE)).round() as i32;
    let lfo_phase_offset = *s.add(STATE_LFO_PHASE_OFFSET);
    let slope_24 = *s.add(STATE_SLOPE) > 0.5;
    let env_attack_ms = (*s.add(STATE_ENV_ATTACK_MS)).clamp(0.1, 5000.0);
    let env_release_ms = (*s.add(STATE_ENV_RELEASE_MS)).clamp(1.0, 5000.0);
    let mut smooth_cutoff = *s.add(STATE_SMOOTH_CUTOFF);
    let mut smooth_reso = *s.add(STATE_SMOOTH_RESO);
    let mut smooth_drive = *s.add(STATE_SMOOTH_DRIVE);
    let mut smooth_wet = *s.add(STATE_SMOOTH_WET);
    let mut smooth_lfo_amount = *s.add(STATE_SMOOTH_LFO_AMOUNT);
    let mut smooth_env_amount = *s.add(STATE_SMOOTH_ENV_AMOUNT);
    let mut lfo_phase = *s.add(STATE_LFO_PHASE);
    let mut env_follow = *s.add(STATE_ENV_FOLLOW);
    let mut sh_phase = *s.add(STATE_SH_PHASE);
    let mut sh_value = *s.add(STATE_SH_VALUE);
    let mut ic1eq_l = *s.add(STATE_IC1EQ_L);
    let mut ic2eq_l = *s.add(STATE_IC2EQ_L);
    let mut ic1eq_r = *s.add(STATE_IC1EQ_R);
    let mut ic2eq_r = *s.add(STATE_IC2EQ_R);
    let mut ic1eq2_l = *s.add(STATE_IC1EQ2_L);
    let mut ic2eq2_l = *s.add(STATE_IC2EQ2_L);
    let mut ic1eq2_r = *s.add(STATE_IC1EQ2_R);
    let mut ic2eq2_r = *s.add(STATE_IC2EQ2_R);

    // One-pole smoothing coefficient (~20Hz)
    let smooth_coeff = 1.0 - (-2.0 * std::f32::consts::PI * 20.0 / sr).exp();
    let lfo_inc = lfo_rate_hz / sr.max(1.0);
    let attack_coeff = 1.0 - (-1.0 / (env_attack_ms * 0.001 * sr).max(1.0)).exp();
    let release_coeff = 1.0 - (-1.0 / (env_release_ms * 0.001 * sr).max(1.0)).exp();

    for i in 0..nf {
        // Smooth parameters
        smooth_cutoff += smooth_coeff * (target_cutoff - smooth_cutoff);
        smooth_reso += smooth_coeff * (target_reso - smooth_reso);
        smooth_drive += smooth_coeff * (target_drive - smooth_drive);
        smooth_wet += smooth_coeff * (target_wet - smooth_wet);
        smooth_lfo_amount += smooth_coeff * (*s.add(STATE_LFO_AMOUNT) - smooth_lfo_amount);
        smooth_env_amount += smooth_coeff * (*s.add(STATE_ENV_AMOUNT) - smooth_env_amount);

        let prev_phase = lfo_phase;
        lfo_phase = (lfo_phase + lfo_inc).fract();
        if lfo_wave == 5 && lfo_phase < prev_phase {
            sh_phase += 1.0;
            sh_value = next_noise(sh_phase);
        }
        let lfo = lfo_value(lfo_wave, (lfo_phase + lfo_phase_offset).fract(), sh_value);

        let input_l = *in0.add(i);
        let input_r = *in1.add(i);
        let amp = input_l.abs().max(input_r.abs());
        let env_coeff = if amp > env_follow {
            attack_coeff
        } else {
            release_coeff
        };
        env_follow += env_coeff * (amp - env_follow);

        let host_cutoff_octaves = mod_cutoff_inputs
            .iter()
            .zip(mod_cutoff_depths)
            .map(|(input, depth)| (*input.add(i)).clamp(0.0, 1.0) * depth)
            .sum::<f32>();
        let octave_mod = lfo * smooth_lfo_amount * 4.0
            + env_follow * smooth_env_amount * 4.0
            + host_cutoff_octaves;
        let mod_cutoff = (smooth_cutoff * 2.0_f32.powf(octave_mod)).clamp(20.0, 20_000.0);

        // SVF coefficients: k = 1/Q, where Q = resonance value
        // Higher resonance = higher Q = lower k = more resonant
        let g = (std::f32::consts::PI * mod_cutoff / sr).tan();
        let k = 1.0 / smooth_reso.max(0.5);

        let driven_l = drive_sample(input_l, smooth_drive);
        let driven_r = drive_sample(input_r, smooth_drive);
        let mut wet_l = svf_sample(driven_l, g, k, mode, &mut ic1eq_l, &mut ic2eq_l);
        let mut wet_r = svf_sample(driven_r, g, k, mode, &mut ic1eq_r, &mut ic2eq_r);
        if slope_24 && (mode == 0 || mode == 1) {
            wet_l = svf_sample(wet_l, g, k, mode, &mut ic1eq2_l, &mut ic2eq2_l);
            wet_r = svf_sample(wet_r, g, k, mode, &mut ic1eq2_r, &mut ic2eq2_r);
        }

        let wet = smooth_wet.clamp(0.0, 1.0);
        *out0.add(i) = input_l * (1.0 - wet) + wet_l * wet;
        *out1.add(i) = input_r * (1.0 - wet) + wet_r * wet;
    }

    *s.add(STATE_IC1EQ_L) = ic1eq_l;
    *s.add(STATE_IC2EQ_L) = ic2eq_l;
    *s.add(STATE_IC1EQ_R) = ic1eq_r;
    *s.add(STATE_IC2EQ_R) = ic2eq_r;
    *s.add(STATE_SMOOTH_CUTOFF) = smooth_cutoff;
    *s.add(STATE_SMOOTH_RESO) = smooth_reso;
    *s.add(STATE_SMOOTH_DRIVE) = smooth_drive;
    *s.add(STATE_SMOOTH_WET) = smooth_wet;
    *s.add(STATE_SMOOTH_LFO_AMOUNT) = smooth_lfo_amount;
    *s.add(STATE_SMOOTH_ENV_AMOUNT) = smooth_env_amount;
    *s.add(STATE_LFO_PHASE) = lfo_phase;
    *s.add(STATE_ENV_FOLLOW) = env_follow;
    *s.add(STATE_SH_PHASE) = sh_phase;
    *s.add(STATE_SH_VALUE) = sh_value;
    *s.add(STATE_IC1EQ2_L) = ic1eq2_l;
    *s.add(STATE_IC2EQ2_L) = ic2eq2_l;
    *s.add(STATE_IC1EQ2_R) = ic1eq2_r;
    *s.add(STATE_IC2EQ2_R) = ic2eq2_r;
}

pub fn filter_vtable() -> NodeVTable {
    NodeVTable {
        process: Some(filter_process),
        init: Some(filter_init),
        reset: None,
        migrate: None,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        drive_sample, filter_init, filter_process, lfo_value, synced_rate_hz, FILTER_STATE_SIZE,
        STATE_CUTOFF, STATE_ENABLED, STATE_MOD_CUTOFF_DEPTH_1, STATE_RESONANCE, STATE_WET,
    };
    use std::ffi::c_void;

    #[test]
    fn zero_drive_is_literal_passthrough() {
        for sample in [-1.0, -0.25, 0.0, 0.5, 1.0] {
            assert_eq!(drive_sample(sample, 0.0), sample);
        }
    }

    #[test]
    fn synced_lfo_divisions_follow_quarter_note_bpm() {
        let bpm = 120.0;
        assert!((synced_rate_hz(0, bpm) - 16.0).abs() < f32::EPSILON);
        assert!((synced_rate_hz(6, bpm) - 2.0).abs() < f32::EPSILON);
        assert!((synced_rate_hz(10, bpm) - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn lfo_shapes_are_bipolar() {
        assert!((lfo_value(1, 0.5, 0.0) - 1.0).abs() < f32::EPSILON);
        assert!((lfo_value(2, 0.0, 0.0) + 1.0).abs() < f32::EPSILON);
        assert!((lfo_value(3, 0.0, 0.0) - 1.0).abs() < f32::EPSILON);
        assert_eq!(lfo_value(5, 0.25, 0.42), 0.42);
    }

    #[test]
    fn lfo_phase_offset_moves_wave_position() {
        let phase = (0.0_f32 + 0.25).fract();
        assert!((lfo_value(0, phase, 0.0) - 1.0).abs() < 0.0001);
    }

    #[test]
    fn host_cutoff_modulation_changes_filter_output() {
        fn render(mod_value: f32, depth_octaves: f32) -> Vec<f32> {
            const FRAMES: usize = 512;
            let mut state = vec![0.0_f32; FILTER_STATE_SIZE];
            unsafe {
                filter_init(
                    state.as_mut_ptr().cast::<c_void>(),
                    48_000,
                    FRAMES as i32,
                    std::ptr::null(),
                );
            }
            state[STATE_ENABLED] = 1.0;
            state[STATE_CUTOFF] = 300.0;
            state[STATE_RESONANCE] = 0.1;
            state[STATE_WET] = 1.0;
            state[STATE_MOD_CUTOFF_DEPTH_1] = depth_octaves;

            let mut left = (0..FRAMES)
                .map(|idx| if idx % 2 == 0 { 0.8 } else { -0.8 })
                .collect::<Vec<_>>();
            let mut right = left.clone();
            let mut mod1 = vec![mod_value; FRAMES];
            let mut mod2 = vec![0.0; FRAMES];
            let mut mod3 = vec![0.0; FRAMES];
            let mut mod4 = vec![0.0; FRAMES];
            let inputs = [
                left.as_mut_ptr(),
                right.as_mut_ptr(),
                mod1.as_mut_ptr(),
                mod2.as_mut_ptr(),
                mod3.as_mut_ptr(),
                mod4.as_mut_ptr(),
            ];
            let mut out_l = vec![0.0; FRAMES];
            let mut out_r = vec![0.0; FRAMES];
            let outputs = [out_l.as_mut_ptr(), out_r.as_mut_ptr()];

            unsafe {
                filter_process(
                    inputs.as_ptr(),
                    outputs.as_ptr(),
                    FRAMES as i32,
                    state.as_mut_ptr().cast::<c_void>(),
                    std::ptr::null_mut(),
                );
            }
            out_l
        }

        let unmodulated = render(0.0, 4.0);
        let modulated = render(1.0, 4.0);
        let diff_rms = unmodulated
            .iter()
            .zip(modulated.iter())
            .map(|(a, b)| (a - b).powi(2))
            .sum::<f32>();
        let diff_rms = (diff_rms / unmodulated.len() as f32).sqrt();

        assert!(
            diff_rms > 0.01,
            "expected cutoff modulation to audibly change output, diff_rms={diff_rms}"
        );
    }
}
