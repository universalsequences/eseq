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
const STATE_KNEE_DB: usize = 17;
const STATE_INPUT_DB: usize = 18;
pub const DYNAMICS_STATE_SIZE: usize = 19;

pub const DYNAMICS_PARAM_ENABLED: u64 = STATE_ENABLED as u64;
pub const DYNAMICS_PARAM_MODE: u64 = STATE_MODE as u64;
pub const DYNAMICS_PARAM_AMOUNT: u64 = STATE_AMOUNT as u64;
pub const DYNAMICS_PARAM_ATTACK: u64 = STATE_ATTACK as u64;
pub const DYNAMICS_PARAM_RELEASE: u64 = STATE_RELEASE as u64;
pub const DYNAMICS_PARAM_LOW_CUT_HZ: u64 = STATE_LOW_CUT_HZ as u64;
pub const DYNAMICS_PARAM_DRIVE: u64 = STATE_DRIVE as u64;
pub const DYNAMICS_PARAM_OUTPUT_DB: u64 = STATE_OUTPUT_DB as u64;
pub const DYNAMICS_PARAM_MIX: u64 = STATE_MIX as u64;
pub const DYNAMICS_PARAM_KNEE_DB: u64 = STATE_KNEE_DB as u64;
pub const DYNAMICS_PARAM_INPUT_DB: u64 = STATE_INPUT_DB as u64;

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
fn glue_attack_ms(idx: usize) -> f32 {
    [0.3, 3.0, 10.0, 30.0][idx.min(3)]
}

#[inline]
fn glue_release_ms(idx: usize) -> Option<f32> {
    match idx.min(3) {
        0 => Some(100.0),
        1 => Some(300.0),
        2 => None,
        _ => Some(1200.0),
    }
}

#[inline]
fn glue_threshold_db(amount: f32) -> f32 {
    -2.0 - amount.clamp(0.0, 1.0) * 26.0
}

#[inline]
fn glue_ratio(amount: f32) -> f32 {
    2.0 + amount.clamp(0.0, 1.0) * 8.0
}

#[inline]
fn glue_low_cut_hz(idx: usize) -> Option<f32> {
    match idx.min(3) {
        0 => None,
        1 => Some(60.0),
        2 => Some(90.0),
        _ => Some(150.0),
    }
}

#[inline]
fn glue_auto_makeup_db(threshold_db: f32, ratio: f32) -> f32 {
    let slope = 1.0 - 1.0 / ratio.max(1.0);
    (-threshold_db * slope * 0.6).clamp(0.0, 14.0)
}

#[inline]
fn phat_saturate(x: f32, drive: f32) -> f32 {
    let amt = drive.clamp(0.0, 1.0);
    if amt <= 0.0001 {
        return x;
    }
    // Two-stage tube-style: input drive into asymmetric soft-clip, then
    // generous makeup so saturation feels louder/fuller, not quieter.
    let pre = 1.0 + amt * 2.2;
    let bias = 0.22 * amt;
    let xd = x * pre + bias;
    let y = xd.tanh() - bias.tanh();
    let makeup = (1.0 + amt * 0.9) / pre;
    y * makeup
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

#[allow(clippy::too_many_arguments)]
unsafe fn process_glue(
    s: *mut f32,
    in0: *const f32,
    in1: *const f32,
    out0: *mut f32,
    out1: *mut f32,
    nf: usize,
    amount: f32,
    attack_idx: usize,
    release_idx: usize,
    low_cut: f32,
    drive: f32,
    output: f32,
    mix: f32,
    sr: f32,
    knee_db: f32,
    input_gain: f32,
) {
    let threshold_db = glue_threshold_db(amount);
    let ratio = glue_ratio(amount);
    let auto_makeup = db_to_amp(glue_auto_makeup_db(threshold_db, ratio));

    // Standard (1 − e^(−1/(τ·fs))) time coefficients — τ is the time to reach
    // 1 − 1/e ≈ 63.2 % of target. The old `time_coef_lsp` used a 1/√2
    // reference which made every labelled attack/release ~1.7× faster than the
    // user expected.
    let attack_coef = time_coef(glue_attack_ms(attack_idx), sr);
    let auto = glue_release_ms(release_idx).is_none();
    // Fast envelope mirrors a small cap: tracks transients quickly and lets go
    // quickly. In auto mode it defaults to ~100 ms so transient material
    // breathes; in fixed mode the user release sets it.
    let rel_fast_coef = time_coef(glue_release_ms(release_idx).unwrap_or(100.0), sr);
    // Slow envelope mirrors the SSL's larger cap: only charges on *sustained*
    // material, then drains slowly. A 400 ms attack keeps brief transients
    // (snare hits, kicks) from elevating it — only chorus-length loud passages
    // raise it meaningfully. Combined with the fast envelope via `max`, this
    // reproduces the parallel-RC behaviour where transients release fast and
    // long blocks release slow.
    let attack_slow_coef = time_coef(400.0, sr);
    let rel_slow_coef = time_coef(1500.0, sr);

    let low_cut_idx = low_cut.round().clamp(0.0, 3.0) as usize;
    let low_cut_hz = glue_low_cut_hz(low_cut_idx);

    let mut sc_x1_l = *s.add(STATE_SC_X1_L);
    let mut sc_y1_l = *s.add(STATE_SC_Y1_L);
    let mut sc_x1_r = *s.add(STATE_SC_X1_R);
    let mut sc_y1_r = *s.add(STATE_SC_Y1_R);
    let mut env_fast_db = *s.add(STATE_ENV_FAST);
    let mut env_slow_db = *s.add(STATE_ENV_SLOW);
    let mut gain_db = *s.add(STATE_GAIN_DB);

    for i in 0..nf {
        let dry_l = *in0.add(i);
        let dry_r = *in1.add(i);
        let inp_l = dry_l * input_gain;
        let inp_r = dry_r * input_gain;

        // Feedback topology: detector reads the previously-attenuated signal,
        // stereo-linked via max(|L|,|R|) so out-of-phase material doesn't
        // cancel in the detector the way a mono sum would.
        let last_g = db_to_amp(gain_db);
        let det_l = inp_l * last_g;
        let det_r = inp_r * last_g;
        let (sc_l, sc_r) = match low_cut_hz {
            Some(hz) => (
                sidechain_highpass(det_l, hz, sr, &mut sc_x1_l, &mut sc_y1_l),
                sidechain_highpass(det_r, hz, sr, &mut sc_x1_r, &mut sc_y1_r),
            ),
            None => (det_l, det_r),
        };
        let detector_db = amp_to_db(sc_l.abs().max(sc_r.abs()));

        // Fast envelope ("small cap"): user-controlled attack and release.
        if detector_db > env_fast_db {
            env_fast_db += attack_coef * (detector_db - env_fast_db);
        } else {
            env_fast_db += rel_fast_coef * (detector_db - env_fast_db);
        }
        // Slow envelope ("big cap"): only fills on sustained loud passages.
        if detector_db > env_slow_db {
            env_slow_db += attack_slow_coef * (detector_db - env_slow_db);
        } else {
            env_slow_db += rel_slow_coef * (detector_db - env_slow_db);
        }

        let env_db = if auto {
            env_fast_db.max(env_slow_db)
        } else {
            env_fast_db
        };

        gain_db = compression_gain_db(env_db, threshold_db, ratio, knee_db);

        let g = db_to_amp(gain_db) * auto_makeup * output;
        let wet_l = phat_saturate(inp_l * g, drive);
        let wet_r = phat_saturate(inp_r * g, drive);
        // Dry side uses the un-gained input so mix=0 is true bypass; the
        // `in` knob only drives the comp/saturator path.
        *out0.add(i) = dry_l + (wet_l - dry_l) * mix;
        *out1.add(i) = dry_r + (wet_r - dry_r) * mix;
    }

    *s.add(STATE_SC_X1_L) = sc_x1_l;
    *s.add(STATE_SC_Y1_L) = sc_y1_l;
    *s.add(STATE_SC_X1_R) = sc_x1_r;
    *s.add(STATE_SC_Y1_R) = sc_y1_r;
    *s.add(STATE_ENV_FAST) = env_fast_db;
    *s.add(STATE_ENV_SLOW) = env_slow_db;
    *s.add(STATE_GAIN_DB) = gain_db;
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
    *s.add(STATE_KNEE_DB) = 8.0;
    *s.add(STATE_INPUT_DB) = 0.0;
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
    let low_cut_raw = *s.add(STATE_LOW_CUT_HZ);
    let low_cut = low_cut_raw.clamp(20.0, 250.0);
    let drive = (*s.add(STATE_DRIVE)).clamp(0.0, 1.0);
    let output = db_to_amp((*s.add(STATE_OUTPUT_DB)).clamp(-12.0, 12.0));
    let mix = (*s.add(STATE_MIX)).clamp(0.0, 1.0);
    let sr = (*s.add(STATE_SAMPLE_RATE)).max(1.0);
    let input_gain = db_to_amp((*s.add(STATE_INPUT_DB)).clamp(-12.0, 24.0));

    if matches!(mode, DynamicsMode::Glue) {
        let knee_db = (*s.add(STATE_KNEE_DB)).clamp(0.0, 18.0);
        process_glue(
            s,
            in0,
            in1,
            out0,
            out1,
            nf,
            amount,
            attack_idx,
            release_idx,
            low_cut_raw,
            drive,
            output,
            mix,
            sr,
            knee_db,
            input_gain,
        );
        return;
    }

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
        let dry_l = *in0.add(i);
        let dry_r = *in1.add(i);
        let input_l = dry_l * input_gain;
        let input_r = dry_r * input_gain;
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
        *out0.add(i) = dry_l + (wet_l - dry_l) * mix;
        *out1.add(i) = dry_r + (wet_r - dry_r) * mix;
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
        let sr = 48_000.0;
        // 1 kHz sine well above the 150 Hz sidechain HPF so the detector sees
        // a steady level; DC would be filtered out and never trigger GR.
        let left: Vec<f32> = (0..8192)
            .map(|i| (std::f32::consts::TAU * 1000.0 * i as f32 / sr).sin() * 0.9)
            .collect();
        let right: Vec<f32> = (0..8192)
            .map(|i| (std::f32::consts::TAU * 1000.0 * i as f32 / sr).sin() * 0.3)
            .collect();
        let (out_l, out_r) = process_block(&mut state, &left, &right);
        let r_l = rms(&out_l[4096..]);
        let r_r = rms(&out_r[4096..]);
        let in_rms_l = rms(&left[4096..]);
        assert!(
            r_l < in_rms_l,
            "rms_l {r_l} should be below input rms {in_rms_l}"
        );
        let ratio = r_l / r_r;
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
        low_cut_low[STATE_LOW_CUT_HZ] = 0.0;
        low_cut_low[STATE_DRIVE] = 0.0;
        let mut low_cut_high = low_cut_low;
        low_cut_high[STATE_LOW_CUT_HZ] = 3.0;

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
