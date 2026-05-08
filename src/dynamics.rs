use crate::audiograph::NodeVTable;
use std::os::raw::{c_int, c_void};

const STATE_ENABLED: usize = 0;
const STATE_MODE: usize = 1; // 0=Glue, 1=404, 2=Hybrid
const STATE_AMOUNT: usize = 2;
const STATE_ATTACK: usize = 3; // 0=fast, 1=punch, 2=glue, 3=slow
const STATE_RELEASE: usize = 4; // 0=fast, 1=bounce, 2=auto, 3=smooth
const STATE_LOW_CUT_HZ: usize = 5;
const STATE_DRIVE: usize = 6;
const STATE_OUTPUT_DB: usize = 7;
const STATE_MIX: usize = 8;
const STATE_SAMPLE_RATE: usize = 9;
const STATE_SC_X1_L: usize = 10;
const STATE_SC_Y1_L: usize = 11;
const STATE_SC_X1_R: usize = 12;
const STATE_SC_Y1_R: usize = 13;
const STATE_ENV_FAST: usize = 14;
const STATE_ENV_SLOW: usize = 15;
const STATE_GAIN_DB: usize = 16;
pub const DYNAMICS_STATE_SIZE: usize = 17;

pub const DYNAMICS_PARAM_ENABLED: u64 = STATE_ENABLED as u64;
pub const DYNAMICS_PARAM_MODE: u64 = STATE_MODE as u64;
pub const DYNAMICS_PARAM_AMOUNT: u64 = STATE_AMOUNT as u64;
pub const DYNAMICS_PARAM_ATTACK: u64 = STATE_ATTACK as u64;
pub const DYNAMICS_PARAM_RELEASE: u64 = STATE_RELEASE as u64;
pub const DYNAMICS_PARAM_LOW_CUT_HZ: u64 = STATE_LOW_CUT_HZ as u64;
pub const DYNAMICS_PARAM_DRIVE: u64 = STATE_DRIVE as u64;
pub const DYNAMICS_PARAM_OUTPUT_DB: u64 = STATE_OUTPUT_DB as u64;
pub const DYNAMICS_PARAM_MIX: u64 = STATE_MIX as u64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DynamicsMode {
    Glue,
    FourOhFour,
    Hybrid,
}

impl DynamicsMode {
    fn from_param(value: f32) -> Self {
        match value.round() as i32 {
            0 => Self::Glue,
            1 => Self::FourOhFour,
            _ => Self::Hybrid,
        }
    }
}

#[inline]
fn db_to_amp(db: f32) -> f32 {
    (10.0_f32).powf(db / 20.0)
}

#[inline]
fn amp_to_db(amp: f32) -> f32 {
    20.0 * amp.max(1.0e-9).log10()
}

#[inline]
fn time_coef(ms: f32, sample_rate: f32) -> f32 {
    if ms <= 0.0 {
        1.0
    } else {
        1.0 - (-1.0 / (ms * 0.001 * sample_rate.max(1.0))).exp()
    }
}

#[inline]
fn attack_ms(mode: DynamicsMode, idx: usize) -> f32 {
    let idx = idx.min(3);
    match mode {
        DynamicsMode::Glue => [0.3, 3.0, 10.0, 30.0][idx],
        DynamicsMode::FourOhFour => [0.05, 0.5, 2.0, 8.0][idx],
        DynamicsMode::Hybrid => [0.1, 1.5, 6.0, 20.0][idx],
    }
}

#[inline]
fn release_ms(idx: usize) -> f32 {
    [100.0, 300.0, 600.0, 1200.0][idx.min(3)]
}

#[inline]
fn compression_gain_db(input_db: f32, threshold_db: f32, ratio: f32, knee_db: f32) -> f32 {
    let ratio = ratio.max(1.0);
    if ratio <= 1.0001 {
        return 0.0;
    }

    let slope = 1.0 - 1.0 / ratio;
    let over_db = input_db - threshold_db;
    let knee_db = knee_db.max(0.0);

    if knee_db <= 0.0001 {
        return if over_db > 0.0 { -over_db * slope } else { 0.0 };
    }

    let half_knee = knee_db * 0.5;
    if over_db <= -half_knee {
        0.0
    } else if over_db >= half_knee {
        -over_db * slope
    } else {
        let x = over_db + half_knee;
        -(slope * x * x) / (2.0 * knee_db)
    }
}

#[inline]
fn sidechain_highpass(
    input: f32,
    cutoff_hz: f32,
    sample_rate: f32,
    x1: &mut f32,
    y1: &mut f32,
) -> f32 {
    let cutoff = cutoff_hz.clamp(20.0, 250.0);
    let rc = 1.0 / (std::f32::consts::TAU * cutoff);
    let dt = 1.0 / sample_rate.max(1.0);
    let alpha = rc / (rc + dt);
    let y = alpha * (*y1 + input - *x1);
    *x1 = input;
    *y1 = y;
    y
}

#[inline]
fn soft_clip(input: f32, drive: f32, mode: DynamicsMode) -> f32 {
    let shaped_drive = match mode {
        DynamicsMode::Glue => drive * 0.45,
        DynamicsMode::FourOhFour => 0.18 + drive * 1.35,
        DynamicsMode::Hybrid => 0.08 + drive * 0.85,
    }
    .clamp(0.0, 1.5);

    if shaped_drive <= 0.0001 {
        input
    } else {
        let gain = 1.0 + shaped_drive * 10.0;
        let clipped = (input * gain).tanh() / gain.tanh().max(0.0001);
        let hard_limit = clipped.clamp(-1.2, 1.2);
        input + (hard_limit - input) * shaped_drive.min(1.0)
    }
}

#[inline]
fn target_gain_db(mode: DynamicsMode, detector_db: f32, amount: f32) -> f32 {
    let amount = amount.clamp(0.0, 1.0);
    match mode {
        DynamicsMode::Glue => {
            let threshold = -4.0 - amount * 22.0;
            let ratio = 1.4 + amount * 8.6;
            let gain = compression_gain_db(detector_db, threshold, ratio, 8.0);
            gain + amount * 2.5
        }
        DynamicsMode::FourOhFour => {
            let threshold = -18.0 - amount * 18.0;
            let ratio = 3.0 + amount * 17.0;
            let downward = compression_gain_db(detector_db, threshold, ratio, 12.0);
            let quietness = ((-18.0 - detector_db) / 42.0).clamp(0.0, 1.0);
            let sustain = quietness * amount * 15.0;
            downward + sustain + amount * 3.0
        }
        DynamicsMode::Hybrid => {
            let glue = target_gain_db(DynamicsMode::Glue, detector_db, amount);
            let sp = target_gain_db(DynamicsMode::FourOhFour, detector_db, amount * 0.72);
            glue * 0.68 + sp * 0.32
        }
    }
}

unsafe extern "C" fn dynamics_init(
    state: *mut c_void,
    sample_rate: c_int,
    _max_block: c_int,
    _initial_state: *const c_void,
) {
    let s = state as *mut f32;
    *s.add(STATE_ENABLED) = 1.0;
    *s.add(STATE_MODE) = 2.0;
    *s.add(STATE_AMOUNT) = 0.45;
    *s.add(STATE_ATTACK) = 1.0;
    *s.add(STATE_RELEASE) = 2.0;
    *s.add(STATE_LOW_CUT_HZ) = 90.0;
    *s.add(STATE_DRIVE) = 0.18;
    *s.add(STATE_OUTPUT_DB) = 0.0;
    *s.add(STATE_MIX) = 1.0;
    *s.add(STATE_SAMPLE_RATE) = sample_rate as f32;
    *s.add(STATE_SC_X1_L) = 0.0;
    *s.add(STATE_SC_Y1_L) = 0.0;
    *s.add(STATE_SC_X1_R) = 0.0;
    *s.add(STATE_SC_Y1_R) = 0.0;
    *s.add(STATE_ENV_FAST) = 0.0;
    *s.add(STATE_ENV_SLOW) = 0.0;
    *s.add(STATE_GAIN_DB) = 0.0;
}

unsafe extern "C" fn dynamics_process(
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

    if *s.add(STATE_ENABLED) <= 0.5 {
        std::ptr::copy_nonoverlapping(in0 as *const f32, out0, nf);
        std::ptr::copy_nonoverlapping(in1 as *const f32, out1, nf);
        *s.add(STATE_ENV_FAST) = 0.0;
        *s.add(STATE_ENV_SLOW) = 0.0;
        *s.add(STATE_GAIN_DB) = 0.0;
        return;
    }

    let mode = DynamicsMode::from_param(*s.add(STATE_MODE));
    let amount = (*s.add(STATE_AMOUNT)).clamp(0.0, 1.0);
    let attack_idx = (*s.add(STATE_ATTACK)).round().clamp(0.0, 3.0) as usize;
    let release_idx = (*s.add(STATE_RELEASE)).round().clamp(0.0, 3.0) as usize;
    let low_cut = (*s.add(STATE_LOW_CUT_HZ)).clamp(20.0, 250.0);
    let drive = (*s.add(STATE_DRIVE)).clamp(0.0, 1.0);
    let output = db_to_amp((*s.add(STATE_OUTPUT_DB)).clamp(-12.0, 12.0));
    let mix = (*s.add(STATE_MIX)).clamp(0.0, 1.0);
    let sr = (*s.add(STATE_SAMPLE_RATE)).max(1.0);

    let attack_coef = time_coef(attack_ms(mode, attack_idx), sr);
    let release_fast_coef = time_coef(release_ms(release_idx), sr);
    let release_slow_coef = time_coef(1400.0, sr);
    let gain_smooth = time_coef(2.0, sr);

    let mut sc_x1_l = *s.add(STATE_SC_X1_L);
    let mut sc_y1_l = *s.add(STATE_SC_Y1_L);
    let mut sc_x1_r = *s.add(STATE_SC_X1_R);
    let mut sc_y1_r = *s.add(STATE_SC_Y1_R);
    let mut env_fast = *s.add(STATE_ENV_FAST);
    let mut env_slow = *s.add(STATE_ENV_SLOW);
    let mut gain_db = *s.add(STATE_GAIN_DB);

    for i in 0..nf {
        let input_l = *in0.add(i);
        let input_r = *in1.add(i);
        let sc_l = sidechain_highpass(input_l, low_cut, sr, &mut sc_x1_l, &mut sc_y1_l);
        let sc_r = sidechain_highpass(input_r, low_cut, sr, &mut sc_x1_r, &mut sc_y1_r);
        let detector = sc_l.abs().max(sc_r.abs());

        let fast_coef = if detector > env_fast {
            attack_coef
        } else {
            release_fast_coef
        };
        env_fast += fast_coef * (detector - env_fast);

        let slow_coef = if detector > env_slow {
            attack_coef * 0.5
        } else {
            release_slow_coef
        };
        env_slow += slow_coef * (detector - env_slow);

        let env = if release_idx == 2 {
            env_fast.max(env_slow * 0.62)
        } else {
            env_fast
        };
        let detector_db = amp_to_db(env);
        let desired_gain_db = target_gain_db(mode, detector_db, amount);
        gain_db += gain_smooth * (desired_gain_db - gain_db);

        let gain = db_to_amp(gain_db);
        let wet_l = soft_clip(input_l * gain, drive, mode) * output;
        let wet_r = soft_clip(input_r * gain, drive, mode) * output;
        *out0.add(i) = input_l + (wet_l - input_l) * mix;
        *out1.add(i) = input_r + (wet_r - input_r) * mix;
    }

    *s.add(STATE_SC_X1_L) = sc_x1_l;
    *s.add(STATE_SC_Y1_L) = sc_y1_l;
    *s.add(STATE_SC_X1_R) = sc_x1_r;
    *s.add(STATE_SC_Y1_R) = sc_y1_r;
    *s.add(STATE_ENV_FAST) = env_fast;
    *s.add(STATE_ENV_SLOW) = env_slow;
    *s.add(STATE_GAIN_DB) = gain_db;
}

pub fn dynamics_vtable() -> NodeVTable {
    NodeVTable {
        process: Some(dynamics_process),
        init: Some(dynamics_init),
        reset: None,
        migrate: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn process_block(
        state: &mut [f32; DYNAMICS_STATE_SIZE],
        left: &[f32],
        right: &[f32],
    ) -> (Vec<f32>, Vec<f32>) {
        let mut in_l = left.to_vec();
        let mut in_r = right.to_vec();
        let mut out_l = vec![0.0; left.len()];
        let mut out_r = vec![0.0; right.len()];
        let inputs = [in_l.as_mut_ptr(), in_r.as_mut_ptr()];
        let outputs = [out_l.as_mut_ptr(), out_r.as_mut_ptr()];
        unsafe {
            dynamics_process(
                inputs.as_ptr(),
                outputs.as_ptr(),
                left.len() as c_int,
                state.as_mut_ptr() as *mut c_void,
                std::ptr::null_mut(),
            );
        }
        (out_l, out_r)
    }

    fn init_state() -> [f32; DYNAMICS_STATE_SIZE] {
        let mut state = [0.0; DYNAMICS_STATE_SIZE];
        unsafe {
            dynamics_init(
                state.as_mut_ptr() as *mut c_void,
                48_000,
                512,
                std::ptr::null(),
            );
        }
        state
    }

    fn rms(samples: &[f32]) -> f32 {
        (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
    }

    #[test]
    fn hard_knee_compression_reduces_over_threshold_by_ratio() {
        let gain = compression_gain_db(0.0, -12.0, 4.0, 0.0);
        assert!((gain + 9.0).abs() < 0.001, "gain was {gain}");
    }

    #[test]
    fn soft_knee_starts_before_threshold() {
        let gain = compression_gain_db(-13.0, -12.0, 4.0, 6.0);
        assert!(gain < 0.0);
        assert!(gain > -1.0);
    }

    #[test]
    fn bypass_copies_stereo_input_exactly() {
        let mut state = init_state();
        state[STATE_ENABLED] = 0.0;
        let left = vec![0.0, 0.25, -0.5, 0.75];
        let right = vec![0.1, -0.2, 0.3, -0.4];
        let (out_l, out_r) = process_block(&mut state, &left, &right);
        assert_eq!(out_l, left);
        assert_eq!(out_r, right);
    }

    #[test]
    fn glue_attenuates_hot_signal_with_linked_stereo_gain() {
        let mut state = init_state();
        state[STATE_MODE] = 0.0;
        state[STATE_AMOUNT] = 0.9;
        state[STATE_ATTACK] = 0.0;
        state[STATE_RELEASE] = 1.0;
        state[STATE_DRIVE] = 0.0;
        let left = vec![0.9; 4096];
        let right = vec![0.3; 4096];
        let (out_l, out_r) = process_block(&mut state, &left, &right);
        assert!(rms(&out_l[2048..]) < 0.9);
        let ratio = rms(&out_l[2048..]) / rms(&out_r[2048..]);
        assert!((ratio - 3.0).abs() < 0.08, "ratio was {ratio}");
    }

    #[test]
    fn four_oh_four_mode_lifts_quiet_sustained_input() {
        let mut state = init_state();
        state[STATE_MODE] = 1.0;
        state[STATE_AMOUNT] = 1.0;
        state[STATE_ATTACK] = 1.0;
        state[STATE_RELEASE] = 2.0;
        state[STATE_DRIVE] = 0.0;
        let left = vec![0.04; 8192];
        let right = vec![0.04; 8192];
        let (out_l, _) = process_block(&mut state, &left, &right);
        assert!(rms(&out_l[4096..]) > 0.055);
    }

    #[test]
    fn low_cut_reduces_detector_reaction_to_low_frequency_input() {
        let mut low_cut_low = init_state();
        low_cut_low[STATE_MODE] = 0.0;
        low_cut_low[STATE_AMOUNT] = 1.0;
        low_cut_low[STATE_LOW_CUT_HZ] = 20.0;
        low_cut_low[STATE_DRIVE] = 0.0;
        let mut low_cut_high = low_cut_low;
        low_cut_high[STATE_LOW_CUT_HZ] = 250.0;

        let sr = 48_000.0;
        let left: Vec<f32> = (0..8192)
            .map(|i| (std::f32::consts::TAU * 40.0 * i as f32 / sr).sin() * 0.8)
            .collect();
        let right = left.clone();
        let (out_low, _) = process_block(&mut low_cut_low, &left, &right);
        let (out_high, _) = process_block(&mut low_cut_high, &left, &right);
        assert!(rms(&out_high[4096..]) > rms(&out_low[4096..]));
    }

    #[test]
    fn hot_driven_output_stays_finite_and_bounded() {
        let mut state = init_state();
        state[STATE_MODE] = 1.0;
        state[STATE_AMOUNT] = 1.0;
        state[STATE_DRIVE] = 1.0;
        state[STATE_OUTPUT_DB] = 12.0;
        let left = vec![4.0; 2048];
        let right = vec![-4.0; 2048];
        let (out_l, out_r) = process_block(&mut state, &left, &right);
        for sample in out_l.iter().chain(out_r.iter()) {
            assert!(sample.is_finite());
            assert!(sample.abs() <= 5.0, "sample was {sample}");
        }
    }
}
