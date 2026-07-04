use crate::audiograph::NodeVTable;
use std::os::raw::{c_int, c_void};

pub const EQ8_NUM_BANDS: usize = 8;

const STATE_ENABLED: usize = 0;
const PARAMS_PER_BAND: usize = 5;
const PARAM_BANDS_BASE: usize = 1;
const PARAM_BAND_ENABLED: usize = 0;
const PARAM_BAND_TYPE: usize = 1;
const PARAM_BAND_FREQ: usize = 2;
const PARAM_BAND_GAIN: usize = 3;
const PARAM_BAND_Q: usize = 4;
pub const EQ8_PARAM_COUNT: usize = PARAM_BANDS_BASE + EQ8_NUM_BANDS * PARAMS_PER_BAND;

const STATE_SAMPLE_RATE: usize = EQ8_PARAM_COUNT;
const BAND_STATE_BASE: usize = STATE_SAMPLE_RATE + 1;
const BAND_STATE_STRIDE: usize = 13;
const BAND_COEFF_B0: usize = 0;
const BAND_COEFF_B1: usize = 1;
const BAND_COEFF_B2: usize = 2;
const BAND_COEFF_A1: usize = 3;
const BAND_COEFF_A2: usize = 4;
const BAND_L_X1: usize = 5;
const BAND_L_X2: usize = 6;
const BAND_L_Y1: usize = 7;
const BAND_L_Y2: usize = 8;
const BAND_R_X1: usize = 9;
const BAND_R_X2: usize = 10;
const BAND_R_Y1: usize = 11;
const BAND_R_Y2: usize = 12;

pub const EQ8_STATE_SIZE: usize = BAND_STATE_BASE + EQ8_NUM_BANDS * BAND_STATE_STRIDE;

pub const EQ8_PARAM_ENABLED: u64 = STATE_ENABLED as u64;

pub const EQ8_FILTER_LOW_SHELF: f32 = 0.0;
pub const EQ8_FILTER_BELL: f32 = 1.0;
pub const EQ8_FILTER_HIGH_SHELF: f32 = 2.0;

const NEUTRAL_GAIN_EPSILON_DB: f32 = 0.000_001;
const MIN_FREQ_HZ: f32 = 20.0;
const MAX_FREQ_HZ: f32 = 20_000.0;
const MIN_Q: f32 = 0.1;
const MAX_Q: f32 = 18.0;
const MIN_GAIN_DB: f32 = -24.0;
const MAX_GAIN_DB: f32 = 24.0;

#[derive(Clone, Copy, Debug)]
pub struct Eq8DefaultBand {
    pub enabled: bool,
    pub filter_type: f32,
    pub freq: f32,
    pub gain: f32,
    pub q: f32,
}

pub const EQ8_DEFAULT_BANDS: [Eq8DefaultBand; EQ8_NUM_BANDS] = [
    Eq8DefaultBand {
        enabled: true,
        filter_type: EQ8_FILTER_LOW_SHELF,
        freq: 80.0,
        gain: 0.0,
        q: 0.707,
    },
    Eq8DefaultBand {
        enabled: true,
        filter_type: EQ8_FILTER_BELL,
        freq: 200.0,
        gain: 0.0,
        q: 1.0,
    },
    Eq8DefaultBand {
        enabled: true,
        filter_type: EQ8_FILTER_BELL,
        freq: 500.0,
        gain: 0.0,
        q: 1.0,
    },
    Eq8DefaultBand {
        enabled: true,
        filter_type: EQ8_FILTER_HIGH_SHELF,
        freq: 8_000.0,
        gain: 0.0,
        q: 0.707,
    },
    Eq8DefaultBand {
        enabled: false,
        filter_type: EQ8_FILTER_BELL,
        freq: 1_500.0,
        gain: 0.0,
        q: 1.0,
    },
    Eq8DefaultBand {
        enabled: false,
        filter_type: EQ8_FILTER_BELL,
        freq: 3_000.0,
        gain: 0.0,
        q: 1.0,
    },
    Eq8DefaultBand {
        enabled: false,
        filter_type: EQ8_FILTER_BELL,
        freq: 6_000.0,
        gain: 0.0,
        q: 1.0,
    },
    Eq8DefaultBand {
        enabled: false,
        filter_type: EQ8_FILTER_BELL,
        freq: 12_000.0,
        gain: 0.0,
        q: 1.0,
    },
];

#[derive(Clone, Copy, Debug)]
struct BiquadCoefficients {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
}

impl BiquadCoefficients {
    const IDENTITY: Self = Self {
        b0: 1.0,
        b1: 0.0,
        b2: 0.0,
        a1: 0.0,
        a2: 0.0,
    };

    fn all_finite(self) -> bool {
        self.b0.is_finite()
            && self.b1.is_finite()
            && self.b2.is_finite()
            && self.a1.is_finite()
            && self.a2.is_finite()
    }

    fn sanitize(self) -> Self {
        if self.all_finite() {
            self
        } else {
            Self::IDENTITY
        }
    }

    fn smooth_toward(&mut self, target: Self, coeff: f32) {
        self.b0 += coeff * (target.b0 - self.b0);
        self.b1 += coeff * (target.b1 - self.b1);
        self.b2 += coeff * (target.b2 - self.b2);
        self.a1 += coeff * (target.a1 - self.a1);
        self.a2 += coeff * (target.a2 - self.a2);
        if !self.all_finite() {
            *self = Self::IDENTITY;
        }
    }

    fn magnitude_at(self, frequency: f32, sample_rate: f32) -> f32 {
        let w = std::f32::consts::TAU * frequency / sample_rate.max(1.0);
        let cos_w = w.cos();
        let cos_2w = (2.0 * w).cos();
        let sin_w = w.sin();
        let sin_2w = (2.0 * w).sin();

        let num_real = self.b0 + self.b1 * cos_w + self.b2 * cos_2w;
        let num_imag = -(self.b1 * sin_w + self.b2 * sin_2w);
        let den_real = 1.0 + self.a1 * cos_w + self.a2 * cos_2w;
        let den_imag = -(self.a1 * sin_w + self.a2 * sin_2w);

        let num_mag = (num_real * num_real + num_imag * num_imag).sqrt();
        let den_mag = (den_real * den_real + den_imag * den_imag).sqrt();
        (num_mag / den_mag.max(1.0e-10)).max(0.0)
    }
}

#[derive(Clone, Copy, Debug)]
struct BiquadRuntime {
    coeffs: BiquadCoefficients,
    l_x1: f32,
    l_x2: f32,
    l_y1: f32,
    l_y2: f32,
    r_x1: f32,
    r_x2: f32,
    r_y1: f32,
    r_y2: f32,
}

impl BiquadRuntime {
    fn identity() -> Self {
        Self {
            coeffs: BiquadCoefficients::IDENTITY,
            l_x1: 0.0,
            l_x2: 0.0,
            l_y1: 0.0,
            l_y2: 0.0,
            r_x1: 0.0,
            r_x2: 0.0,
            r_y1: 0.0,
            r_y2: 0.0,
        }
    }

    fn process_left(&mut self, input: f32) -> f32 {
        let output =
            self.coeffs.b0 * input + self.coeffs.b1 * self.l_x1 + self.coeffs.b2 * self.l_x2
                - self.coeffs.a1 * self.l_y1
                - self.coeffs.a2 * self.l_y2;
        self.l_x2 = self.l_x1;
        self.l_x1 = input;
        self.l_y2 = self.l_y1;
        self.l_y1 = sanitize_sample(output);
        self.l_y1
    }

    fn process_right(&mut self, input: f32) -> f32 {
        let output =
            self.coeffs.b0 * input + self.coeffs.b1 * self.r_x1 + self.coeffs.b2 * self.r_x2
                - self.coeffs.a1 * self.r_y1
                - self.coeffs.a2 * self.r_y2;
        self.r_x2 = self.r_x1;
        self.r_x1 = input;
        self.r_y2 = self.r_y1;
        self.r_y1 = sanitize_sample(output);
        self.r_y1
    }

    fn clear_history(&mut self) {
        self.l_x1 = 0.0;
        self.l_x2 = 0.0;
        self.l_y1 = 0.0;
        self.l_y2 = 0.0;
        self.r_x1 = 0.0;
        self.r_x2 = 0.0;
        self.r_y1 = 0.0;
        self.r_y2 = 0.0;
    }
}

#[inline]
pub const fn eq8_band_param_idx(band: usize, offset: usize) -> u64 {
    (PARAM_BANDS_BASE + band * PARAMS_PER_BAND + offset) as u64
}

#[inline]
pub const fn eq8_band_enabled_param_idx(band: usize) -> u64 {
    eq8_band_param_idx(band, PARAM_BAND_ENABLED)
}

#[inline]
pub const fn eq8_band_type_param_idx(band: usize) -> u64 {
    eq8_band_param_idx(band, PARAM_BAND_TYPE)
}

#[inline]
pub const fn eq8_band_freq_param_idx(band: usize) -> u64 {
    eq8_band_param_idx(band, PARAM_BAND_FREQ)
}

#[inline]
pub const fn eq8_band_gain_param_idx(band: usize) -> u64 {
    eq8_band_param_idx(band, PARAM_BAND_GAIN)
}

#[inline]
pub const fn eq8_band_q_param_idx(band: usize) -> u64 {
    eq8_band_param_idx(band, PARAM_BAND_Q)
}

#[inline]
const fn band_param_base(band: usize) -> usize {
    PARAM_BANDS_BASE + band * PARAMS_PER_BAND
}

#[inline]
const fn band_state_base(band: usize) -> usize {
    BAND_STATE_BASE + band * BAND_STATE_STRIDE
}

#[inline]
fn sanitize_sample(sample: f32) -> f32 {
    if sample.is_finite() {
        sample.clamp(-32.0, 32.0)
    } else {
        0.0
    }
}

#[inline]
fn clamp_freq(freq: f32, sample_rate: f32) -> f32 {
    let nyquist_guard = (sample_rate.max(1.0) * 0.45).max(MIN_FREQ_HZ + 1.0);
    freq.clamp(MIN_FREQ_HZ, MAX_FREQ_HZ.min(nyquist_guard))
}

fn calculate_coefficients(
    filter_type: f32,
    frequency: f32,
    gain_db: f32,
    q: f32,
    sample_rate: f32,
) -> BiquadCoefficients {
    let sr = sample_rate.max(1.0);
    let freq = clamp_freq(frequency, sr);
    let gain = gain_db.clamp(MIN_GAIN_DB, MAX_GAIN_DB);
    if gain.abs() <= NEUTRAL_GAIN_EPSILON_DB {
        return BiquadCoefficients::IDENTITY;
    }
    let q = q.clamp(MIN_Q, MAX_Q);
    let a = 10.0_f32.powf(gain / 40.0);
    let w0 = std::f32::consts::TAU * freq / sr;
    let cos_w0 = w0.cos();
    let sin_w0 = w0.sin();
    let alpha = sin_w0 / (2.0 * q);

    let (b0, b1, b2, a0, a1, a2) = match filter_type.round() as i32 {
        0 => {
            let sqrt_a = a.sqrt();
            (
                a * ((a + 1.0) - (a - 1.0) * cos_w0 + 2.0 * sqrt_a * alpha),
                2.0 * a * ((a - 1.0) - (a + 1.0) * cos_w0),
                a * ((a + 1.0) - (a - 1.0) * cos_w0 - 2.0 * sqrt_a * alpha),
                (a + 1.0) + (a - 1.0) * cos_w0 + 2.0 * sqrt_a * alpha,
                -2.0 * ((a - 1.0) + (a + 1.0) * cos_w0),
                (a + 1.0) + (a - 1.0) * cos_w0 - 2.0 * sqrt_a * alpha,
            )
        }
        2 => {
            let sqrt_a = a.sqrt();
            (
                a * ((a + 1.0) + (a - 1.0) * cos_w0 + 2.0 * sqrt_a * alpha),
                -2.0 * a * ((a - 1.0) + (a + 1.0) * cos_w0),
                a * ((a + 1.0) + (a - 1.0) * cos_w0 - 2.0 * sqrt_a * alpha),
                (a + 1.0) - (a - 1.0) * cos_w0 + 2.0 * sqrt_a * alpha,
                2.0 * ((a - 1.0) - (a + 1.0) * cos_w0),
                (a + 1.0) - (a - 1.0) * cos_w0 - 2.0 * sqrt_a * alpha,
            )
        }
        _ => (
            1.0 + alpha * a,
            -2.0 * cos_w0,
            1.0 - alpha * a,
            1.0 + alpha / a,
            -2.0 * cos_w0,
            1.0 - alpha / a,
        ),
    };

    if a0.abs() <= 1.0e-12 || !a0.is_finite() {
        return BiquadCoefficients::IDENTITY;
    }

    BiquadCoefficients {
        b0: b0 / a0,
        b1: b1 / a0,
        b2: b2 / a0,
        a1: a1 / a0,
        a2: a2 / a0,
    }
    .sanitize()
}

unsafe fn read_runtime(s: *const f32, band: usize) -> BiquadRuntime {
    let base = band_state_base(band);
    BiquadRuntime {
        coeffs: BiquadCoefficients {
            b0: *s.add(base + BAND_COEFF_B0),
            b1: *s.add(base + BAND_COEFF_B1),
            b2: *s.add(base + BAND_COEFF_B2),
            a1: *s.add(base + BAND_COEFF_A1),
            a2: *s.add(base + BAND_COEFF_A2),
        }
        .sanitize(),
        l_x1: *s.add(base + BAND_L_X1),
        l_x2: *s.add(base + BAND_L_X2),
        l_y1: *s.add(base + BAND_L_Y1),
        l_y2: *s.add(base + BAND_L_Y2),
        r_x1: *s.add(base + BAND_R_X1),
        r_x2: *s.add(base + BAND_R_X2),
        r_y1: *s.add(base + BAND_R_Y1),
        r_y2: *s.add(base + BAND_R_Y2),
    }
}

unsafe fn write_runtime(s: *mut f32, band: usize, runtime: BiquadRuntime) {
    let base = band_state_base(band);
    *s.add(base + BAND_COEFF_B0) = runtime.coeffs.b0;
    *s.add(base + BAND_COEFF_B1) = runtime.coeffs.b1;
    *s.add(base + BAND_COEFF_B2) = runtime.coeffs.b2;
    *s.add(base + BAND_COEFF_A1) = runtime.coeffs.a1;
    *s.add(base + BAND_COEFF_A2) = runtime.coeffs.a2;
    *s.add(base + BAND_L_X1) = runtime.l_x1;
    *s.add(base + BAND_L_X2) = runtime.l_x2;
    *s.add(base + BAND_L_Y1) = runtime.l_y1;
    *s.add(base + BAND_L_Y2) = runtime.l_y2;
    *s.add(base + BAND_R_X1) = runtime.r_x1;
    *s.add(base + BAND_R_X2) = runtime.r_x2;
    *s.add(base + BAND_R_Y1) = runtime.r_y1;
    *s.add(base + BAND_R_Y2) = runtime.r_y2;
}

unsafe extern "C" fn eq8_init(
    state: *mut c_void,
    sample_rate: c_int,
    _max_block: c_int,
    _initial_state: *const c_void,
) {
    let s = state as *mut f32;
    *s.add(STATE_ENABLED) = 1.0;
    for (band, default) in EQ8_DEFAULT_BANDS.iter().enumerate() {
        let base = band_param_base(band);
        *s.add(base + PARAM_BAND_ENABLED) = if default.enabled { 1.0 } else { 0.0 };
        *s.add(base + PARAM_BAND_TYPE) = default.filter_type;
        *s.add(base + PARAM_BAND_FREQ) = default.freq;
        *s.add(base + PARAM_BAND_GAIN) = default.gain;
        *s.add(base + PARAM_BAND_Q) = default.q;
        write_runtime(s, band, BiquadRuntime::identity());
    }
    *s.add(STATE_SAMPLE_RATE) = sample_rate as f32;
}

unsafe extern "C" fn eq8_process(
    inp: *const *mut f32,
    out: *const *mut f32,
    nframes: c_int,
    state: *mut c_void,
    _buffers: *mut c_void,
) {
    let s = state as *mut f32;
    let nf = nframes.max(0) as usize;
    let in_l = *inp.add(0);
    let in_r = *inp.add(1);
    let out_l = *out.add(0);
    let out_r = *out.add(1);

    let mut runtimes = [BiquadRuntime::identity(); EQ8_NUM_BANDS];
    for (band, runtime) in runtimes.iter_mut().enumerate() {
        *runtime = read_runtime(s, band);
    }

    if *s.add(STATE_ENABLED) <= 0.5 {
        for runtime in &mut runtimes {
            runtime.coeffs = BiquadCoefficients::IDENTITY;
            runtime.clear_history();
        }
        for i in 0..nf {
            *out_l.add(i) = *in_l.add(i);
            *out_r.add(i) = *in_r.add(i);
        }
        for (band, runtime) in runtimes.into_iter().enumerate() {
            write_runtime(s, band, runtime);
        }
        return;
    }

    let sample_rate = (*s.add(STATE_SAMPLE_RATE)).max(1.0);
    let smooth_coeff = 1.0 - (-2.0 * std::f32::consts::PI * 20.0 / sample_rate).exp();
    let mut target_coeffs = [BiquadCoefficients::IDENTITY; EQ8_NUM_BANDS];
    for (band, target) in target_coeffs.iter_mut().enumerate() {
        let base = band_param_base(band);
        let band_enabled = *s.add(base + PARAM_BAND_ENABLED) > 0.5;
        if band_enabled {
            *target = calculate_coefficients(
                *s.add(base + PARAM_BAND_TYPE),
                *s.add(base + PARAM_BAND_FREQ),
                *s.add(base + PARAM_BAND_GAIN),
                *s.add(base + PARAM_BAND_Q),
                sample_rate,
            );
        }
    }

    for i in 0..nf {
        let mut left = *in_l.add(i);
        let mut right = *in_r.add(i);
        for (runtime, target) in runtimes.iter_mut().zip(target_coeffs) {
            runtime.coeffs.smooth_toward(target, smooth_coeff);
            left = runtime.process_left(left);
            right = runtime.process_right(right);
        }
        *out_l.add(i) = sanitize_sample(left);
        *out_r.add(i) = sanitize_sample(right);
    }

    for (band, runtime) in runtimes.into_iter().enumerate() {
        write_runtime(s, band, runtime);
    }
}

pub fn eq8_vtable() -> NodeVTable {
    NodeVTable {
        process: Some(eq8_process),
        init: Some(eq8_init),
        reset: None,
        migrate: None,
        ..NodeVTable::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::c_void;

    const FRAMES: usize = 8192;
    const SAMPLE_RATE: i32 = 48_000;

    fn init_state() -> Vec<f32> {
        let mut state = vec![0.0_f32; EQ8_STATE_SIZE];
        unsafe {
            eq8_init(
                state.as_mut_ptr().cast::<c_void>(),
                SAMPLE_RATE,
                FRAMES as i32,
                std::ptr::null(),
            );
        }
        state
    }

    fn render(state: &mut [f32], input_l: &[f32], input_r: &[f32]) -> (Vec<f32>, Vec<f32>) {
        assert_eq!(input_l.len(), input_r.len());
        let mut left = input_l.to_vec();
        let mut right = input_r.to_vec();
        let inputs = [left.as_mut_ptr(), right.as_mut_ptr()];
        let mut out_l = vec![0.0_f32; input_l.len()];
        let mut out_r = vec![0.0_f32; input_l.len()];
        let outputs = [out_l.as_mut_ptr(), out_r.as_mut_ptr()];
        unsafe {
            eq8_process(
                inputs.as_ptr(),
                outputs.as_ptr(),
                input_l.len() as i32,
                state.as_mut_ptr().cast::<c_void>(),
                std::ptr::null_mut(),
            );
        }
        (out_l, out_r)
    }

    fn sine(freq: f32, frames: usize) -> Vec<f32> {
        (0..frames)
            .map(|idx| {
                (std::f32::consts::TAU * freq * idx as f32 / SAMPLE_RATE as f32).sin() * 0.25
            })
            .collect()
    }

    fn rms_tail(samples: &[f32]) -> f32 {
        let tail = &samples[samples.len() / 2..];
        (tail.iter().map(|sample| sample * sample).sum::<f32>() / tail.len() as f32).sqrt()
    }

    #[test]
    fn bypass_is_literal_stereo_identity() {
        let mut state = init_state();
        state[STATE_ENABLED] = 0.0;
        let input_l = sine(440.0, 512);
        let input_r = sine(880.0, 512);
        let (out_l, out_r) = render(&mut state, &input_l, &input_r);
        assert_eq!(out_l, input_l);
        assert_eq!(out_r, input_r);
    }

    #[test]
    fn zero_gain_defaults_are_identity() {
        let mut state = init_state();
        let input_l = sine(440.0, 1024);
        let input_r = sine(1234.0, 1024);
        let (out_l, out_r) = render(&mut state, &input_l, &input_r);
        assert_eq!(out_l, input_l);
        assert_eq!(out_r, input_r);
    }

    #[test]
    fn bell_boost_and_cut_change_target_frequency() {
        let input = sine(1_000.0, FRAMES);
        let silent = vec![0.0; FRAMES];

        let mut boost_state = init_state();
        boost_state[eq8_band_freq_param_idx(1) as usize] = 1_000.0;
        boost_state[eq8_band_gain_param_idx(1) as usize] = 6.0;
        boost_state[eq8_band_q_param_idx(1) as usize] = 1.0;
        let (boosted, _) = render(&mut boost_state, &input, &silent);

        let mut cut_state = init_state();
        cut_state[eq8_band_freq_param_idx(1) as usize] = 1_000.0;
        cut_state[eq8_band_gain_param_idx(1) as usize] = -6.0;
        cut_state[eq8_band_q_param_idx(1) as usize] = 1.0;
        let (cut, _) = render(&mut cut_state, &input, &silent);

        let input_rms = rms_tail(&input);
        assert!(rms_tail(&boosted) > input_rms * 1.65);
        assert!(rms_tail(&cut) < input_rms * 0.70);
    }

    #[test]
    fn shelf_responses_match_expected_regions() {
        let low_shelf = calculate_coefficients(EQ8_FILTER_LOW_SHELF, 120.0, 6.0, 0.707, 48_000.0);
        let high_shelf =
            calculate_coefficients(EQ8_FILTER_HIGH_SHELF, 8_000.0, 6.0, 0.707, 48_000.0);
        assert!(low_shelf.magnitude_at(40.0, 48_000.0) > 1.7);
        assert!(low_shelf.magnitude_at(5_000.0, 48_000.0) < 1.05);
        assert!(high_shelf.magnitude_at(12_000.0, 48_000.0) > 1.7);
        assert!(high_shelf.magnitude_at(200.0, 48_000.0) < 1.05);
    }

    #[test]
    fn stereo_histories_are_independent() {
        let mut state = init_state();
        state[eq8_band_freq_param_idx(1) as usize] = 1_000.0;
        state[eq8_band_gain_param_idx(1) as usize] = 9.0;
        let left = sine(1_000.0, FRAMES);
        let right = vec![0.0; FRAMES];
        let (_out_l, out_r) = render(&mut state, &left, &right);
        assert!(out_r.iter().all(|sample| sample.abs() <= 1.0e-7));
    }

    #[test]
    fn fast_parameter_changes_remain_finite() {
        let mut state = init_state();
        let chunk = 256;
        for idx in 0..24 {
            state[eq8_band_freq_param_idx(2) as usize] = if idx % 2 == 0 { 40.0 } else { 18_000.0 };
            state[eq8_band_gain_param_idx(2) as usize] = if idx % 3 == 0 { 24.0 } else { -24.0 };
            state[eq8_band_q_param_idx(2) as usize] = if idx % 2 == 0 { 0.1 } else { 18.0 };
            let input = sine(80.0 + idx as f32 * 331.0, chunk);
            let (out_l, out_r) = render(&mut state, &input, &input);
            assert!(out_l
                .iter()
                .chain(out_r.iter())
                .all(|sample| sample.is_finite()));
        }
    }
}
