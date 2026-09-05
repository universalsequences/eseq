//! Grampian-inspired two-spring waveguide, independent of the RE-201 fit.
//!
//! Each branch has a forward propagation leg and a return leg. The pickup is
//! between them: first-arrival time is NOT the feedback round-trip time.
//! Dispersion uses cascaded conjugate-pole allpasses, with pole frequency and
//! bandwidth in Hz (no integer z^-K stretch or phase-bank switching). A third,
//! short, band-limited branch represents the fast high-frequency precursor.
//! The tank is linear; drive belongs to the Space Echo electronics, not an
//! impulse-normalized, unidentifiable "drip" waveshaper.

use serde::{Deserialize, Serialize};
use std::f32::consts::{PI, TAU};

const PATHS: usize = 3;
const MAX_SECTIONS: usize = 128;
const DELAY_LEN: usize = 65536; // covers validated delays at 192 kHz / lowest tension
const FILTER_STATE: usize = 0; // HP + four LP input biquads, per path
const FEEDBACK: usize = FILTER_STATE + PATHS * 10;
const DAMP: usize = FEEDBACK + PATHS;
const SHELF: usize = DAMP + PATHS;
const WRITE: usize = SHELF + PATHS;
const ACTIVE: usize = WRITE + 1;
const QUIET: usize = ACTIVE + 1;
const ENVELOPE: usize = QUIET + 1;
const AP_STATE: usize = ENVELOPE + 1;
const DELAYS: usize = AP_STATE + PATHS * 2 * MAX_SECTIONS * 2;
const SCATTER: usize = DELAYS + PATHS * 2 * DELAY_LEN;
pub const STATE_LEN: usize = SCATTER + PATHS * DELAY_LEN;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Dispersion {
    pub sections: usize,
    pub pole_hz: f32,
    pub bandwidth_hz: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PathParams {
    pub dispersion: [Dispersion; 3],
    pub forward_s: f32,
    pub return_s: f32,
    /// Delayed boundary reflection, seconds. Zero bypasses scattering.
    pub scatter_s: f32,
    /// Lossless reflection allpass coefficient (prompt vs delayed return).
    pub scatter: f32,
    pub t60_s: f32,
    pub damping_hz: f32,
    pub shelf_hz: f32,
    pub shelf_gain: f32,
    pub highpass_hz: f32,
    /// Input transducer resonance, outside the feedback loop.
    pub highpass_q: f32,
    pub lowpass_hz: f32,
    pub gain: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Params {
    pub paths: [PathParams; PATHS],
}

impl Default for Params {
    fn default() -> Self {
        Self {
            paths: [
                PathParams {
                    dispersion: [
                        Dispersion {
                            sections: 40,
                            pole_hz: 4566.205,
                            bandwidth_hz: 2990.4854,
                        },
                        Dispersion {
                            sections: 40,
                            pole_hz: 5629.909,
                            bandwidth_hz: 1373.7612,
                        },
                        Dispersion {
                            sections: 40,
                            pole_hz: 6089.8003,
                            bandwidth_hz: 617.3548,
                        },
                    ],
                    forward_s: 0.013806314,
                    return_s: 0.011261084,
                    scatter_s: 0.0305,
                    scatter: 0.30647343,
                    t60_s: 6.61088,
                    damping_hz: 14114.904,
                    shelf_hz: 1262.8733,
                    shelf_gain: 0.9012612,
                    highpass_hz: 219.48396,
                    highpass_q: std::f32::consts::FRAC_1_SQRT_2,
                    lowpass_hz: 4514.2866,
                    gain: 1.0,
                },
                PathParams {
                    dispersion: [
                        Dispersion {
                            sections: 40,
                            pole_hz: 4490.864,
                            bandwidth_hz: 2546.683,
                        },
                        Dispersion {
                            sections: 40,
                            pole_hz: 5526.009,
                            bandwidth_hz: 1239.3225,
                        },
                        Dispersion {
                            sections: 40,
                            pole_hz: 6000.0,
                            bandwidth_hz: 503.97784,
                        },
                    ],
                    forward_s: 0.022027025,
                    return_s: 0.011061424,
                    scatter_s: 0.038,
                    scatter: 0.32576406,
                    t60_s: 3.4313042,
                    damping_hz: 5603.1045,
                    shelf_hz: 415.47064,
                    shelf_gain: 0.89951384,
                    highpass_hz: 357.82718,
                    highpass_q: std::f32::consts::FRAC_1_SQRT_2,
                    lowpass_hz: 5128.477,
                    gain: 0.80322754,
                },
                PathParams {
                    dispersion: [
                        Dispersion {
                            sections: 6,
                            pole_hz: 7300.0,
                            bandwidth_hz: 2200.0,
                        },
                        Dispersion {
                            sections: 0,
                            pole_hz: 7300.0,
                            bandwidth_hz: 2200.0,
                        },
                        Dispersion {
                            sections: 0,
                            pole_hz: 7300.0,
                            bandwidth_hz: 2200.0,
                        },
                    ],
                    forward_s: 0.001,
                    return_s: 0.008160958,
                    scatter_s: 0.0,
                    scatter: 0.0,
                    t60_s: 1.0672294,
                    damping_hz: 17233.254,
                    shelf_hz: 400.0,
                    shelf_gain: 1.0,
                    highpass_hz: 6198.56,
                    highpass_q: 3.5078776,
                    lowpass_hz: 11675.518,
                    gain: 0.030298282,
                },
            ],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidPath(usize),
    InvalidRateOrTension,
    NonFiniteInput,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidPath(i) => write!(f, "invalid Grampian path {i}"),
            Self::InvalidRateOrTension => f.write_str("invalid Grampian sample rate or tension"),
            Self::NonFiniteInput => f.write_str("non-finite Grampian input"),
        }
    }
}
impl std::error::Error for Error {}

impl Params {
    pub fn validate(&self) -> Result<(), Error> {
        for (i, p) in self.paths.iter().enumerate() {
            let values = [
                p.forward_s,
                p.return_s,
                p.scatter_s,
                p.scatter,
                p.t60_s,
                p.damping_hz,
                p.shelf_hz,
                p.shelf_gain,
                p.highpass_hz,
                p.highpass_q,
                p.lowpass_hz,
                p.gain,
            ];
            if !values.iter().all(|v| v.is_finite())
                || p.dispersion.iter().any(|d| d.sections > MAX_SECTIONS)
                || p.dispersion.iter().map(|d| d.sections).sum::<usize>() > MAX_SECTIONS
                || p.dispersion.iter().any(|d| {
                    !d.pole_hz.is_finite()
                        || !d.bandwidth_hz.is_finite()
                        || !(500.0..=12000.0).contains(&d.pole_hz)
                        || !(100.0..=10000.0).contains(&d.bandwidth_hz)
                })
                || p.forward_s < 0.0001
                || p.forward_s > 0.08
                || p.return_s < 0.0001
                || p.return_s > 0.12
                || !(0.0..=0.08).contains(&p.scatter_s)
                || !(0.0..=0.7).contains(&p.scatter)
                || p.t60_s < 0.05
                || p.t60_s > 10.0
                || p.highpass_hz < 20.0
                || !(0.5..=8.0).contains(&p.highpass_q)
                || p.lowpass_hz <= p.highpass_hz
                || p.lowpass_hz > 18000.0
                || p.damping_hz < 100.0
                || p.damping_hz > 20000.0
                || !(100.0..=3000.0).contains(&p.shelf_hz)
                || !(0.3..=1.0).contains(&p.shelf_gain)
                || p.gain < 0.0
                || p.gain > 3.0
            {
                return Err(Error::InvalidPath(i));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
}

impl Biquad {
    fn band_edge(hz: f32, sr: f32, highpass: bool, q: f32) -> Self {
        let w = TAU * hz.clamp(10.0, sr * 0.45) / sr;
        let alpha = w.sin() / (2.0 * q);
        let norm = 1.0 / (1.0 + alpha);
        let c = w.cos();
        let b0 = (if highpass { 1.0 + c } else { 1.0 - c }) * 0.5 * norm;
        Self {
            b0,
            b1: if highpass { -2.0 * b0 } else { 2.0 * b0 },
            b2: b0,
            a1: -2.0 * c * norm,
            a2: (1.0 - alpha) * norm,
        }
    }

    fn memory_bound(self) -> Option<f32> {
        let a1 = self.a1 as f64;
        let a2 = self.a2 as f64;
        let disc = a1 * a1 - 4.0 * a2;
        let radius = if disc >= 0.0 {
            (a1.abs() + disc.sqrt()) * 0.5
        } else {
            a2.sqrt()
        };
        if !(0.0..1.0).contains(&radius) {
            return None;
        }
        let settling = if radius > 0.0 {
            -16.0 / radius.ln()
        } else {
            0.0
        };
        Some((2.0 * (1.0 + radius) / (1.0 - radius) + settling) as f32)
    }

    #[inline]
    fn tick(self, x: f32, z: &mut [f32]) -> f32 {
        let y = self.b0 * x + z[0];
        z[0] = self.b1 * x - self.a1 * y + z[1];
        z[1] = self.b2 * x - self.a2 * y;
        y
    }
}

/// Centered cubic Lagrange interpolation. Its stationary magnitude is <= 1
/// for fractions in [0, 1], unlike overshooting interpolators, while its HF
/// loss is much smaller than linear interpolation in a feedback waveguide.
#[derive(Clone, Copy)]
struct FractionalDelay {
    samples: f32,
    whole: usize,
    weights: [f32; 4],
}

impl FractionalDelay {
    fn new(samples: f32) -> Self {
        // Two samples keep every read causal even in the scattering loop.
        let samples = samples.clamp(2.0, (DELAY_LEN - 3) as f32);
        let whole = samples as usize;
        let u = samples - whole as f32;
        Self {
            samples,
            whole,
            weights: [
                -u * (1.0 - u) * (2.0 - u) / 6.0,
                (1.0 + u) * (1.0 - u) * (2.0 - u) / 2.0,
                (1.0 + u) * u * (2.0 - u) / 2.0,
                -(1.0 + u) * u * (1.0 - u) / 6.0,
            ],
        }
    }

    #[inline]
    fn read(self, w: usize, buffer: &[f32]) -> f32 {
        let a = (w + DELAY_LEN - self.whole + 1) % DELAY_LEN;
        self.weights[0] * buffer[a]
            + self.weights[1] * buffer[(a + DELAY_LEN - 1) % DELAY_LEN]
            + self.weights[2] * buffer[(a + DELAY_LEN - 2) % DELAY_LEN]
            + self.weights[3] * buffer[(a + DELAY_LEN - 3) % DELAY_LEN]
    }
}

#[derive(Clone)]
struct PathCoeffs {
    dispersion: [(usize, Biquad); 3],
    forward: FractionalDelay,
    back: FractionalDelay,
    scatter_delay: Option<FractionalDelay>,
    scatter: f32,
    feedback: f32,
    damping: f32,
    shelf: f32,
    shelf_gain: f32,
    hp: Biquad,
    lp: [Biquad; 4],
    gain: f32,
}

#[derive(Clone)]
pub struct Coeffs {
    paths: [PathCoeffs; PATHS],
    quiet_guard: f32,
    envelope_decay: f32,
}

impl Coeffs {
    /// Tension scales propagation time and pole frequencies together; topology
    /// and allpass count never change. Invalid offline/user parameters fail
    /// here, before they can become runtime indices or unstable coefficients.
    pub fn new(params: &Params, sample_rate: f32, tension: f32) -> Result<Self, Error> {
        params.validate()?;
        if !sample_rate.is_finite()
            || !(8000.0..=192000.0).contains(&sample_rate)
            || !tension.is_finite()
            || !(0.0..=1.0).contains(&tension)
        {
            return Err(Error::InvalidRateOrTension);
        }
        let sr = sample_rate;
        let scale = 2.0f32.powf(2.0 * (tension.clamp(0.0, 1.0) - 0.5));
        let paths = std::array::from_fn(|i| {
            let p = &params.paths[i];
            let dispersion = std::array::from_fn(|j| {
                let d = &p.dispersion[j];
                let hz = (d.pole_hz * scale).min(sr * 0.40);
                let r = (-PI * d.bandwidth_hz * scale / sr).exp();
                let a1 = -2.0 * r * (TAU * hz / sr).cos();
                let a2 = r * r;
                (
                    d.sections,
                    Biquad {
                        b0: a2,
                        b1: a1,
                        b2: 1.0,
                        a1,
                        a2,
                    },
                )
            });
            let forward = FractionalDelay::new(p.forward_s / scale * sr);
            let back = FractionalDelay::new(p.return_s / scale * sr);
            // Both allpass legs and the one-sample feedback register count
            // toward low-frequency round-trip time. Damping adds HF loss;
            // its magnitude and that of the fractional delays never exceed 1.
            let ap_dc: f32 = dispersion
                .iter()
                .map(|(n, ap)| *n as f32 * 2.0 * (1.0 - ap.a2) / (1.0 + ap.a1 + ap.a2))
                .sum();
            let scatter_delay =
                (p.scatter_s > 0.0).then(|| FractionalDelay::new(p.scatter_s / scale * sr));
            let scatter_dc =
                scatter_delay.map_or(0.0, |d| d.samples) * (1.0 + p.scatter) / (1.0 - p.scatter);
            let round_trip = (forward.samples + back.samples + scatter_dc + 1.0 + 2.0 * ap_dc) / sr;
            PathCoeffs {
                dispersion,
                forward,
                back,
                scatter_delay,
                scatter: p.scatter,
                feedback: 10.0f32
                    .powf(-3.0 * round_trip / (p.t60_s * scale.powf(-0.4)))
                    .min(0.9995),
                damping: 1.0 - (-TAU * (p.damping_hz * scale).min(sr * 0.45) / sr).exp(),
                shelf: 1.0 - (-TAU * p.shelf_hz * scale / sr).exp(),
                shelf_gain: p.shelf_gain,
                hp: Biquad::band_edge(p.highpass_hz * scale, sr, true, p.highpass_q),
                // Eighth-order Butterworth transition: suppress the backward
                // side of the pole resonances outside the torsional band.
                lp: [0.5097956, 0.6013449, 0.8999762, 2.5629154]
                    .map(|q| Biquad::band_edge(p.lowpass_hz * scale, sr, false, q)),
                gain: p.gain,
            }
        });
        // A quiet output is not an empty delay line. Wait longer than a
        // conservative maximum round trip PLUS pole settling before sleeping,
        // including the loose-tension case. Never freeze unconsumed packets.
        let mut quiet_guard = 0.0f32;
        for (i, p) in paths.iter().enumerate() {
            let p: &PathCoeffs = p;
            // Input transducer/filter memory also matters, especially
            // when testing short delay paths with low cutoff frequencies.
            let mut filter_bound = 0.0;
            for filter in std::iter::once(&p.hp).chain(&p.lp) {
                filter_bound += filter.memory_bound().ok_or(Error::InvalidPath(i))?;
            }
            for radius in [1.0 - p.damping, 1.0 - p.shelf] {
                if radius > 0.0 {
                    filter_bound += -16.0 / radius.ln();
                }
            }
            let dispersion_bound: f32 = p
                .dispersion
                .iter()
                .map(|(n, ap)| {
                    let r = ap.a2.sqrt();
                    *n as f32 * 2.0 * (1.0 + r) / (1.0 - r)
                })
                .sum();
            let settling = p
                .dispersion
                .iter()
                .map(|(_, ap)| -16.0 / ap.a2.sqrt().ln())
                .fold(0.0f32, f32::max);
            let scatter_samples = p.scatter_delay.map_or(0.0, |d| d.samples);
            let scatter_bound = scatter_samples * (1.0 + p.scatter) / (1.0 - p.scatter);
            let scatter_settle = if p.scatter > 0.0 {
                -16.0 * scatter_samples / p.scatter.ln()
            } else {
                0.0
            };
            quiet_guard = quiet_guard.max(
                p.forward.samples
                    + p.back.samples
                    + scatter_bound
                    + scatter_settle
                    + 2.0 * dispersion_bound
                    + settling
                    + filter_bound,
            );
        }
        Ok(Self {
            paths,
            quiet_guard,
            envelope_decay: (-1.0 / (0.15 * sr)).exp(),
        })
    }
}

#[inline]
fn delay(x: f32, d: FractionalDelay, w: usize, buffer: &mut [f32]) -> f32 {
    buffer[w] = x;
    d.read(w, buffer)
}

/// Returns mid and side, with the precursor centered. Stereo is a pickup mix
/// of the SAME two springs, not a second detuned tank. Width scales side only.
#[inline]
pub fn process(input: f32, c: &Coeffs, state: &mut [f32]) -> (f32, f32) {
    debug_assert!(state.len() >= STATE_LEN);
    // Ignore excitation below -160 dBFS for activity detection; tiny DC/filter
    // residuals otherwise prevent sleep forever (especially without FTZ).
    if input.abs() > 1e-8 {
        state[ACTIVE] = 1.0;
        state[QUIET] = 0.0;
    } else if state[ACTIVE] == 0.0 {
        return (0.0, 0.0);
    } else {
        state[QUIET] = (state[QUIET] + 1.0).min(c.quiet_guard + 1.0);
        if state[QUIET] > c.quiet_guard && state[ENVELOPE] < 1e-5 {
            // One bounded clear on entering sleep, not every silent frame.
            // A later transient must not resurrect a frozen old tail.
            state[..STATE_LEN].fill(0.0);
            return (0.0, 0.0);
        }
    }
    let w = state[WRITE] as usize;
    let mut outputs = [0.0; PATHS];
    for (i, p) in c.paths.iter().enumerate() {
        let filter = FILTER_STATE + i * 10;
        let mut x = p.hp.tick(input, &mut state[filter..filter + 2]);
        for (j, lp) in p.lp.iter().enumerate() {
            x = lp.tick(x, &mut state[filter + 2 + j * 2..filter + 4 + j * 2]);
        }
        let mut wave = x + state[FEEDBACK + i];
        for leg in 0..2 {
            let base = DELAYS + (i * 2 + leg) * DELAY_LEN;
            wave = delay(
                wave,
                if leg == 0 { p.forward } else { p.back },
                w,
                &mut state[base..base + DELAY_LEN],
            );
            let mut base = AP_STATE + (i * 2 + leg) * MAX_SECTIONS * 2;
            for (count, ap) in &p.dispersion {
                for z in state[base..base + count * 2].chunks_exact_mut(2) {
                    wave = ap.tick(wave, z);
                }
                base += count * 2;
            }
            if leg == 0 {
                outputs[i] = wave * p.gain;
            }
        }
        if let Some(d) = p.scatter_delay {
            // Boundary scattering is in the RETURN path, so first arrivals
            // remain sharp. It produces the measured weak half-return and
            // stronger delayed return, then increases late echo density.
            let base = SCATTER + i * DELAY_LEN;
            let delayed = d.read(w, &state[base..base + DELAY_LEN]);
            let y = delayed - p.scatter * wave;
            state[base + w] = wave + p.scatter * y;
            wave = y;
        }
        let damp = state[DAMP + i] + p.damping * (wave - state[DAMP + i]);
        state[DAMP + i] = damp;
        let low = state[SHELF + i] + p.shelf * (damp - state[SHELF + i]);
        state[SHELF + i] = low;
        // Convex combination of unity and a passive one-pole: magnitude <= 1.
        // Low modes may outlive the midrange without extending the entire tail.
        state[FEEDBACK + i] = (p.shelf_gain * damp + (1.0 - p.shelf_gain) * low) * p.feedback;
    }
    state[WRITE] = ((w + 1) % DELAY_LEN) as f32;
    let level: f32 = outputs.iter().map(|x| x.abs()).sum();
    // Re-arm the propagation guard on late packets too. An input-only timer
    // would expire early in a long tail and could then cut a quiet gap before
    // the next audible return, despite the initially conservative wait.
    if level >= 1e-5 {
        state[QUIET] = 0.0;
    }
    state[ENVELOPE] = level.max(state[ENVELOPE] * c.envelope_decay);
    (
        outputs[0] + outputs[1] + outputs[2],
        outputs[0] - outputs[1],
    )
}

pub fn render(
    params: &Params,
    sample_rate: f32,
    input: &[f32],
    tension: f32,
) -> Result<Vec<f32>, Error> {
    let coeffs = Coeffs::new(params, sample_rate, tension)?;
    if input.iter().any(|x| !x.is_finite()) {
        return Err(Error::NonFiniteInput);
    }
    let mut state = vec![0.0; STATE_LEN];
    Ok(input
        .iter()
        .map(|&x| process(x, &coeffs, &mut state).0)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grampian_shipped_parameters_match_verified_fit() {
        let reference: Params = serde_json::from_str(include_str!(
            "../../../tests/fixtures/spring/grampian-fit.json"
        ))
        .unwrap();
        assert_eq!(Params::default(), reference);
        reference.validate().unwrap();
    }

    #[test]
    fn grampian_dispersion_delays_highs_without_integer_stretch() {
        for sr in [44100.0, 48000.0, 96000.0] {
            let c = Coeffs::new(&Params::default(), sr, 0.5).unwrap();
            let p = &c.paths[0];
            let group = |hz: f32| -> f32 {
                let w = TAU * hz / sr;
                p.dispersion
                    .iter()
                    .map(|(n, ap)| {
                        let r = ap.a2.sqrt();
                        let theta = (-ap.a1 / (2.0 * r)).acos();
                        *n as f32 / sr
                            * ((1.0 - r * r) / (1.0 + r * r - 2.0 * r * (w - theta).cos())
                                + (1.0 - r * r) / (1.0 + r * r - 2.0 * r * (w + theta).cos()))
                    })
                    .sum()
            };
            assert!(group(5000.0) - group(1000.0) > 0.015, "sr={sr}");
            assert!((group(1000.0) + p.forward.samples / sr - 0.01625).abs() < 0.001);
            assert!((group(5000.0) + p.forward.samples / sr - 0.0355).abs() < 0.002);
            assert!(group(6000.0) > group(5500.0) && group(5500.0) > group(5000.0));
        }
    }

    #[test]
    fn grampian_fractional_delay_is_passive_and_continuous_at_integer_crossings() {
        for fraction in 0..=100 {
            let d = FractionalDelay::new(17.0 + fraction as f32 / 100.0);
            for bin in 0..=512 {
                let w = std::f64::consts::PI * bin as f64 / 512.0;
                let re: f64 = d
                    .weights
                    .iter()
                    .enumerate()
                    .map(|(i, c)| *c as f64 * (w * i as f64).cos())
                    .sum();
                let im: f64 = d
                    .weights
                    .iter()
                    .enumerate()
                    .map(|(i, c)| *c as f64 * (w * i as f64).sin())
                    .sum();
                assert!(re * re + im * im < 1.000002);
            }
        }
        let buffer: Vec<f32> = (0..DELAY_LEN).map(|i| (i as f32 * 0.2).sin()).collect();
        for w in [0, 2, 35, DELAY_LEN - 1] {
            let a = FractionalDelay::new(17.99999).read(w, &buffer);
            let b = FractionalDelay::new(18.00001).read(w, &buffer);
            assert!((a - b).abs() < 1e-4);
        }
    }

    #[test]
    fn grampian_sample_rates_preserve_band_decay() {
        let mut baseline: Option<Vec<f64>> = None;
        for sr in [48000.0, 44100.0, 96000.0] {
            let mut input = vec![0.0; (sr * 3.0) as usize];
            input[0] = 1.0;
            let ir = render(&Params::default(), sr, &input, 0.5).unwrap();
            let mut ratios = Vec::new();
            for (lo, hi) in [(400.0, 1200.0), (2000.0, 4000.0), (4000.0, 6000.0)] {
                let hp = Biquad::band_edge(lo, sr, true, std::f32::consts::FRAC_1_SQRT_2);
                let lp = Biquad::band_edge(hi, sr, false, std::f32::consts::FRAC_1_SQRT_2);
                let (mut h, mut l) = ([0.0; 2], [0.0; 2]);
                let (mut total, mut tail) = (0.0f64, 0.0f64);
                for (i, x) in ir.iter().enumerate() {
                    let y = lp.tick(hp.tick(*x, &mut h), &mut l) as f64;
                    total += y * y;
                    if i >= sr as usize {
                        tail += y * y;
                    }
                }
                ratios.push(10.0 * (tail / total).log10());
            }
            if let Some(reference) = &baseline {
                for (a, b) in reference.iter().zip(&ratios) {
                    assert!(
                        (a - b).abs() < 2.5,
                        "sr={sr}: decay {ratios:?} vs {reference:?}"
                    );
                }
            } else {
                baseline = Some(ratios);
            }
        }
    }

    #[test]
    fn grampian_rejects_invalid_coefficients_before_rendering() {
        let mut params = Params::default();
        params.paths[0].dispersion[0].sections = usize::MAX;
        assert!(Coeffs::new(&params, 48000.0, 0.5).is_err());
        params = Params::default();
        params.paths[0].scatter = 1.0;
        assert!(Coeffs::new(&params, 48000.0, 0.5).is_err());
        for sr in [0.0, f32::NAN, f32::INFINITY, 384000.0] {
            assert!(Coeffs::new(&Params::default(), sr, 0.5).is_err());
        }
        assert!(Coeffs::new(&Params::default(), 48000.0, f32::NAN).is_err());
    }

    #[test]
    fn grampian_rendered_first_packets_follow_reference() {
        for sr in [44100.0, 48000.0, 96000.0] {
            // Gabor probes localize a packet in frequency AND time. Unlike a
            // whole-tail centroid, this cannot pass with a reversed chirp.
            for (hz, arrival) in [(1000.0, 0.01625), (3000.0, 0.02025), (5000.0, 0.0355)] {
                let input: Vec<f32> = (0..(sr * 0.09) as usize)
                    .map(|i| {
                        let t = i as f32 / sr - 0.005;
                        (TAU * hz * t).cos() * (-0.5 * (t / 0.0012).powi(2)).exp()
                    })
                    .collect();
                let ir = render(&Params::default(), sr, &input, 0.5).unwrap();
                let lo = ((arrival + 0.005 - 0.004) * sr) as usize;
                let hi = ((arrival + 0.005 + 0.004) * sr) as usize;
                let peak = ir[lo..hi]
                    .iter()
                    .enumerate()
                    .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
                    .unwrap()
                    .0
                    + lo;
                assert!(
                    (peak as f32 / sr - 0.005 - arrival).abs() < 0.003,
                    "sr={sr} hz={hz} arrival={}",
                    peak as f32 / sr - 0.005
                );
                assert!(ir[peak].abs() > 0.01);
            }
        }
    }

    #[test]
    fn grampian_idle_guard_preserves_late_packets_across_long_gaps() {
        let mut params = Params::default();
        for (i, p) in params.paths.iter_mut().enumerate() {
            for d in &mut p.dispersion {
                d.sections = 0;
            }
            p.gain = if i == 0 { 1.0 } else { 0.0 };
            p.scatter_s = 0.0;
            p.forward_s = 0.08;
            p.return_s = 0.12;
            p.t60_s = 10.0;
            p.damping_hz = 20000.0;
            p.shelf_gain = 1.0;
            p.highpass_hz = 500.0;
            p.highpass_q = std::f32::consts::FRAC_1_SQRT_2;
            p.lowpass_hz = 2000.0;
        }
        let c = Coeffs::new(&params, 8000.0, 0.0).unwrap();
        let mut reference_c = c.clone();
        reference_c.quiet_guard = f32::INFINITY; // test oracle: identical DSP, never sleep
        let mut actual = vec![0.0; STATE_LEN];
        let mut reference = actual.clone();
        let mut late_packets = 0;
        for i in 0..8000 * 25 {
            let x = if i == 0 { 1.0 } else { 0.0 };
            let a = process(x, &c, &mut actual).0;
            let b = process(x, &reference_c, &mut reference).0;
            if b.abs() > 3e-5 {
                assert!(
                    (a - b).abs() < 1e-6,
                    "cut a return at {} s",
                    i as f32 / 8000.0
                );
                if i > 8000 * 10 {
                    late_packets += 1;
                }
            }
        }
        assert!(
            late_packets > 10,
            "fixture must exercise audible late returns"
        );
    }

    #[test]
    fn grampian_sleep_clears_tail_and_reopens_from_rest() {
        let c = Coeffs::new(&Params::default(), 48000.0, 0.5).unwrap();
        let mut state = vec![0.0; STATE_LEN];
        process(0.2, &c, &mut state);
        for _ in 0..48000 * 10 {
            // A preamp/DC block can leave a nonzero sub-audible residual.
            // Activity must not stay latched forever because of that value.
            process(1e-12, &c, &mut state);
        }
        assert_eq!(state[ACTIVE], 0.0);
        assert!(state.iter().all(|&x| x == 0.0));
        let mut fresh = vec![0.0; STATE_LEN];
        for i in 0..4800 {
            let x = if i == 0 { 0.2 } else { 0.0 };
            assert_eq!(process(x, &c, &mut state), process(x, &c, &mut fresh));
        }
    }

    #[test]
    fn grampian_decays_and_tension_sweep_stays_bounded() {
        for sr in [44100.0, 48000.0, 96000.0] {
            let params = Params::default();
            params.validate().unwrap();
            for tension in [0.0, 0.5, 1.0] {
                let mut input = vec![0.0; (sr * 5.0) as usize];
                input[0] = 1.0;
                let ir = render(&params, sr, &input, tension).unwrap();
                assert!(ir.iter().all(|v| v.is_finite() && v.abs() < 4.0));
                let total: f64 = ir.iter().map(|&v| (v as f64).powi(2)).sum();
                let tail: f64 = ir[(sr * 4.0) as usize..]
                    .iter()
                    .map(|&v| (v as f64).powi(2))
                    .sum();
                assert!(
                    total > 1e-4 && tail / total < 0.001,
                    "sr={sr} tension={tension} tail={}",
                    tail / total
                );
            }
            let mut state = vec![0.0; STATE_LEN];
            for block in 0..500 {
                let c = Coeffs::new(&params, sr, block as f32 / 499.0).unwrap();
                for frame in 0..128 {
                    let x = if block % 31 == 0 && frame == 0 {
                        2.0
                    } else {
                        0.0
                    };
                    let (mid, side) = process(x, &c, &mut state);
                    assert!(mid.is_finite() && side.is_finite() && mid.abs() < 10.0);
                }
            }
        }
    }
}
