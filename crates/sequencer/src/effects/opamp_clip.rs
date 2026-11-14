//! A slew-limited op-amp gain stage into a hard diode clipper, solved as a
//! circuit (the topology of the ProCo RAT).
//!
//! This is the `op-amp` shaper of the Roar engine. Like `triode.rs` it is not
//! a transfer curve: the op-amp cannot move its output faster than its slew
//! rate, so loud highs come out as ramps while lows square off against the
//! diodes, and the clipper's coupling capacitor shifts with the signal.
//!
//! ```text
//!   e --||Cin--+------(+)\
//!              Rb        |  >--+-- vo --||Cc--Rs--+-- vd --Rt--+-- vt --||Co--+-- out
//!             ref   +-(-)/     |                  |            |              |
//!                   |          |               D+ v ^ D-       Ct             Ro
//!                   +--Rf||Cf--+                  |            |              |
//!                   |                            ref          ref            ref
//!                   +--R1--||C1-- ref
//!                   +--R2--||C2-- ref
//! ```
//!
//! * The op-amp is a macromodel of an LM308 with a 30 pF compensation
//!   capacitor: a differential pair that saturates into the compensation
//!   integrator, and an output that stops at the supply rails:
//!
//!   ```text
//!   dvo/dt = SR * tanh((v+ - v-) / Vlin),   Vlin = SR / (2 pi GBW),   |vo| <= Vmax
//!   ```
//!
//!   so both the slew rate and the gain-bandwidth product are physical.
//! * `Rf` is the pedal's distortion pot at one fixed setting (10 k, a gain of
//!   38 at 1 kHz). Higher settings run out of gain-bandwidth inside the audio
//!   band (47 k is 16 dB down at 20 kHz before any clipping).
//! * `R1` is the stock 47 ohm plus 470 ohm in series, the common "Ruetz"
//!   modification. Stock, the two gain legs make a 22 dB shelf that leaves
//!   100 Hz 14 dB below 1 kHz going into the clipper: right for a guitar,
//!   too thin for a general-purpose saturator. With the extra resistance
//!   100 Hz is 3 dB down and 50 Hz 6.5 dB.
//! * `Rt` is the filter control at its minimum resistance (its 32 kHz end);
//!   the Saturator has its own tone control after the stage.
//! * The bias control makes the clipping asymmetric: up to two more diodes in
//!   series with one side (positive bias, the diode that clips the positive
//!   half). An offset could not do it, `Cc` would block it.
//!
//! Component and device values are from memory of the published schematic and
//! of common SPICE models (1N914: Is 2.52 nA, n 1.752); they have not been
//! checked against a unit or a datasheet.
//!
//! Discretization: trapezoidal companions for the capacitors and for the
//! op-amp integrator, except `Cf`, which is backward Euler: it sits between
//! the op-amp output and the low-impedance feedback node (a 4 ns time
//! constant), and a trapezoid would ring on it. The op-amp output is a stiff
//! source, so a step is two scalar solves in sequence: a Newton solve for
//! `vo` through the tanh (one pass, usually), then the clipper node `vd`,
//! which has a closed form in the Wright omega function (a table here). It
//! runs at 4x the host rate inside the shared oversampler (`oversample4.rs`).
//!
//! `tools/saturator/opamp_clip_ref.py` holds the same circuit twice: this
//! algorithm, and a stiff ODE integration it is checked against. The fixtures
//! the tests here assert against come from that ODE solve. Keep them in sync.

use super::oversample4;

const CIN: f64 = 22e-9;
const RB: f64 = 1e6;
const RF: f64 = 10e3;
const CF: f64 = 100e-12;
const R1: f64 = 47.0 + 470.0;
const C1: f64 = 2.2e-6;
const R2: f64 = 560.0;
const C2: f64 = 4.7e-6;
/// Op-amp slew rate, volts per second.
const SLEW_RATE: f64 = 0.3e6;
/// Op-amp gain-bandwidth product, Hz.
const GBW: f64 = 1.0e6;
/// Output swing either side of the reference (9 V single supply).
const VMAX: f64 = 3.5;
const CC: f64 = 4.7e-6;
const RS: f64 = 1e3;
const DIODE_IS: f64 = 2.52e-9;
const DIODE_NVT: f64 = 1.752 * 25.85e-3;
const RT: f64 = 1.5e3;
const CT: f64 = 3.3e-9;
const CO: f64 = 1e-6;
const RO: f64 = 100e3;
const INV_RB: f64 = 1.0 / RB;
const INV_RO: f64 = 1.0 / RO;
/// Extra diodes in series with one side at full bias.
const ASYM_DIODES: f64 = 2.0;

/// Small-signal voltage gain magnitude at 1 kHz, input to output
/// (`opamp_clip_ref.py measure`). The phase there is under a degree.
pub const SMALL_SIGNAL_GAIN: f64 = 37.973_9;
/// Volts at the input for a unit-amplitude signal: the diodes are clipping
/// audibly there (about 4% THD at half of it).
pub const INPUT_VOLTS: f64 = 0.03;
/// The drive is unbounded upstream; past the supply voltage nothing changes.
const MAX_INPUT_VOLTS: f64 = 9.0;

const MAX_OPAMP_ITERS: usize = 8;
// Estimated error at which the op-amp solve stops, volts.
const OPAMP_TOL: f64 = 1.0e-8;
// In units of n Vt: past this the non-conducting diode's current is under
// 1e-7 of its saturation current.
const REVERSE_CUTOFF: f64 = 16.0;

/// Floats of circuit state per channel. All zero is the circuit at rest.
const CIRCUIT_LEN: usize = 15;
const ST_VCIN: usize = 0;
const ST_IIN: usize = 1;
const ST_VO: usize = 2;
const ST_DVO: usize = 3; // the op-amp output's last rate of change
const ST_VF: usize = 4; // across Cf
const ST_VC1: usize = 5;
const ST_I1: usize = 6;
const ST_VC2: usize = 7;
const ST_I2: usize = 8;
const ST_VCC: usize = 9;
const ST_IC: usize = 10;
const ST_VCT: usize = 11;
const ST_IT: usize = 12;
const ST_VCO: usize = 13;
const ST_IO: usize = 14;

const OS: usize = CIRCUIT_LEN;
/// Floats of runtime state per channel ([`process`]).
pub const STATE_LEN: usize = OS + oversample4::STATE_LEN;

/// Sample-rate dependent constants of the discretized linear network.
#[derive(Clone, Copy)]
pub struct Coeffs {
    inv_gin: f64,
    kin: f64,
    inv_g1: f64,
    inv_g2: f64,
    y1: f64,
    y2: f64,
    gf: f64,
    /// The feedback node is `alpha * vo + beta`.
    alpha: f64,
    inv_d: f64,
    /// Half a step of slewing, volts.
    half_slew: f64,
    half_t: f64,
    inv_vlin: f64,
    inv_gc: f64,
    inv_rs: f64,
    inv_gt: f64,
    yt: f64,
    inv_go: f64,
    ko: f64,
    /// The clipper node's linear conductance.
    g_lin: f64,
    inv_g_lin: f64,
    /// One diode per side.
    diode: Diode,
    tables: &'static Tables,
    quiet_samples: f32,
}

impl Coeffs {
    /// For [`process`] at `host_rate` (the circuit is solved at 4x that).
    pub fn new(host_rate: f64) -> Self {
        Self::at_solver_rate(host_rate.max(1_000.0) * oversample4::FACTOR)
    }

    fn at_solver_rate(rate: f64) -> Self {
        let g = |cap: f64| 2.0 * cap * rate;
        let (gin, g1, g2, gc, gt, go) = (g(CIN), g(C1), g(C2), g(CC), g(CT), g(CO));
        let gf = CF * rate;
        let y1 = 1.0 / (R1 + 1.0 / g1);
        let y2 = 1.0 / (R2 + 1.0 / g2);
        let d = 1.0 / RF + gf + y1 + y2;
        let inv_rs = 1.0 / (RS + 1.0 / gc);
        let yt = 1.0 / (RT + 1.0 / gt);
        let g_lin = inv_rs + yt;
        Self {
            g_lin,
            inv_g_lin: 1.0 / g_lin,
            diode: Diode {
                n: DIODE_NVT,
                inv_n: 1.0 / DIODE_NVT,
                ln_c: (DIODE_IS / (g_lin * DIODE_NVT)).ln(),
            },
            inv_gin: 1.0 / gin,
            kin: RB / (RB + 1.0 / gin),
            inv_g1: 1.0 / g1,
            inv_g2: 1.0 / g2,
            y1,
            y2,
            gf,
            alpha: (1.0 / RF + gf) / d,
            inv_d: 1.0 / d,
            half_slew: 0.5 * SLEW_RATE / rate,
            half_t: 0.5 / rate,
            inv_vlin: 2.0 * std::f64::consts::PI * GBW / SLEW_RATE,
            inv_gc: 1.0 / gc,
            inv_rs,
            inv_gt: 1.0 / gt,
            yt,
            inv_go: 1.0 / go,
            ko: RO / (RO + 1.0 / go),
            tables: tables(),
            quiet_samples: oversample4::quiet_samples(rate / oversample4::FACTOR),
        }
    }
}

// tanh and exp tables: values on a uniform grid, cubic Hermite between
// (both functions give their own slope, so only values are stored). The
// interpolation error is under 2e-9, relative for the exponential.
const TABLE_N: usize = 1024;
const TANH_MAX: f64 = 10.0;
const EXP_MAX: f64 = 16.0;

// The clipper's starting point: w(y) solving w + exp(w) = y, as value and
// slope pairs. With one diode conducting, the node equation
// vd + r * Is * exp(vd / n) = open is this in w = vd / n + ln(r * Is / n).
const OMEGA_MIN: f64 = -12.0;
const OMEGA_MAX: f64 = 116.0;

struct Tables {
    tanh: Vec<f64>,
    exp: Vec<f64>,
    omega: Vec<[f64; 2]>,
}

fn tables() -> &'static Tables {
    static TABLES: std::sync::OnceLock<Tables> = std::sync::OnceLock::new();
    TABLES.get_or_init(|| {
        let build = |max: f64, f: &dyn Fn(f64) -> f64| {
            (0..=TABLE_N)
                .map(|i| f(-max + 2.0 * max * i as f64 / TABLE_N as f64))
                .collect::<Vec<_>>()
        };
        let omega = (0..=TABLE_N)
            .map(|i| {
                let y = OMEGA_MIN + (OMEGA_MAX - OMEGA_MIN) * i as f64 / TABLE_N as f64;
                // Newton on w + exp(w) = y, from below where it is monotonic.
                let mut w = if y < 1.0 { y - y.exp() } else { y.ln() };
                for _ in 0..60 {
                    let e = w.exp();
                    w -= (w + e - y) / (1.0 + e);
                }
                [w, 1.0 / (1.0 + w.exp())]
            })
            .collect();
        Tables { tanh: build(TANH_MAX, &f64::tanh), exp: build(EXP_MAX, &f64::exp), omega }
    })
}

/// Build the tables now, off the audio thread ([`Coeffs::new`] otherwise
/// builds them on first use).
pub fn warm_tables() {
    tables();
}

/// Cubic Hermite read of a table over `-max..max`, `x` already inside it.
/// `slope` gives the function's derivative from its value.
#[inline]
fn hermite(table: &[f64], max: f64, x: f64, slope: impl Fn(f64) -> f64) -> f64 {
    let h = 2.0 * max / TABLE_N as f64;
    let t = (x + max) * (1.0 / h);
    let i = (t as usize).min(TABLE_N - 1);
    let u = t - i as f64;
    let (f0, f1) = (table[i], table[i + 1]);
    let (d0, d1) = (slope(f0) * h, slope(f1) * h);
    let u2 = u * u;
    let u3 = u2 * u;
    (2.0 * u3 - 3.0 * u2 + 1.0) * f0
        + (u3 - 2.0 * u2 + u) * d0
        + (-2.0 * u3 + 3.0 * u2) * f1
        + (u3 - u2) * d1
}

#[inline]
fn fast_tanh(tables: &Tables, z: f64) -> f64 {
    hermite(&tables.tanh, TANH_MAX, z.clamp(-TANH_MAX, TANH_MAX), |f| 1.0 - f * f)
}

/// exp(x) for x inside the table.
#[inline]
fn fast_exp(tables: &Tables, x: f64) -> f64 {
    hermite(&tables.exp, EXP_MAX, x.clamp(-EXP_MAX, EXP_MAX), |f| f)
}

/// w with w + exp(w) = y, to about 1e-7.
#[inline]
fn omega(tables: &Tables, y: f64) -> f64 {
    if y <= OMEGA_MIN {
        // exp(w) is under 1e-5 of w here.
        return y;
    }
    let inv_h = TABLE_N as f64 / (OMEGA_MAX - OMEGA_MIN);
    let h = (OMEGA_MAX - OMEGA_MIN) / TABLE_N as f64;
    if y >= OMEGA_MAX {
        // More current than the op-amp can push; not reached in practice.
        let mut w = (y - y.ln()).ln();
        for _ in 0..3 {
            let e = w.exp();
            w -= (w + e - y) / (1.0 + e);
        }
        return w;
    }
    let t = (y - OMEGA_MIN) * inv_h;
    let i = (t as usize).min(TABLE_N - 1);
    let u = t - i as f64;
    let [f0, d0] = tables.omega[i];
    let [f1, d1] = tables.omega[i + 1];
    let u2 = u * u;
    let u3 = u2 * u;
    (2.0 * u3 - 3.0 * u2 + 1.0) * f0
        + (u3 - 2.0 * u2 + u) * d0 * h
        + (-2.0 * u3 + 3.0 * u2) * f1
        + (u3 - u2) * d1 * h
}

/// One side of the clipper: `count` diodes in series.
#[derive(Clone, Copy)]
struct Diode {
    /// n Vt of the string, and its reciprocal.
    n: f64,
    inv_n: f64,
    /// ln(Is / (g n)), `g` the clipper node's linear conductance.
    ln_c: f64,
}

/// The clipper for a bias setting: the diode conducting for vd > 0, and for
/// vd < 0.
#[derive(Clone, Copy)]
struct Diodes {
    pos: Diode,
    neg: Diode,
}

impl Diodes {
    fn new(c: &Coeffs, bias: f64) -> Self {
        let side = |count: f64| {
            if count == 1.0 {
                c.diode
            } else {
                Diode {
                    n: DIODE_NVT * count,
                    inv_n: 1.0 / (DIODE_NVT * count),
                    ln_c: c.diode.ln_c - count.ln(),
                }
            }
        };
        Self {
            pos: side(1.0 + ASYM_DIODES * bias.max(0.0)),
            neg: side(1.0 + ASYM_DIODES * (-bias).max(0.0)),
        }
    }
}

/// Advance the circuit one solver step with `e` volts at the input. Returns
/// the output voltage.
///
/// # Safety
/// `st` must point to [`CIRCUIT_LEN`] valid floats.
#[inline]
unsafe fn solve_step(c: &Coeffs, d: &Diodes, st: *mut f32, e: f64) -> f64 {
    // Trapezoidal companion sources: v[n] = i[n]/g + (v[n-1] + i[n-1]/g).
    // Input coupling: the non-inverting input is the drop across Rb.
    let hin = *st.add(ST_VCIN) as f64 + *st.add(ST_IIN) as f64 * c.inv_gin;
    let vp = (e - hin) * c.kin;
    let iin = vp * INV_RB;
    *st.add(ST_VCIN) = (hin + iin * c.inv_gin) as f32;
    *st.add(ST_IIN) = iin as f32;

    // Feedback network: the inverting input is alpha * vo + beta.
    let h1 = *st.add(ST_VC1) as f64 + *st.add(ST_I1) as f64 * c.inv_g1;
    let h2 = *st.add(ST_VC2) as f64 + *st.add(ST_I2) as f64 * c.inv_g2;
    let beta = (c.y1 * h1 + c.y2 * h2 - c.gf * *st.add(ST_VF) as f64) * c.inv_d;

    // Op-amp: vo = vo_prev + T/2 * (SR * tanh((v+ - v-) / Vlin) + last rate).
    let vo_prev = *st.add(ST_VO) as f64;
    let base = vo_prev + c.half_t * *st.add(ST_DVO) as f64;
    let feedback = c.alpha * c.inv_vlin;
    // Start from where the last rate of change points.
    let mut vo = vo_prev + 2.0 * c.half_t * *st.add(ST_DVO) as f64;
    let mut th = 0.0;
    for _ in 0..MAX_OPAMP_ITERS {
        th = fast_tanh(c.tables, (vp - beta) * c.inv_vlin - feedback * vo);
        let dth = (1.0 - th * th) * feedback;
        let step = (vo - base - c.half_slew * th) / (1.0 + c.half_slew * dth);
        vo -= step;
        // Newton's error after this step is about half the curvature times
        // the step squared; a saturated tanh has none, so slewing and
        // sitting on a rail take one pass.
        let error = step * step * th.abs() * dth * feedback * c.half_slew;
        // The tanh at the new point, to first order.
        th += dth * step;
        if error < OPAMP_TOL {
            break;
        }
    }
    let mut dvo = SLEW_RATE * th;
    if vo.abs() > VMAX {
        // Against a rail: the output stops there.
        vo = VMAX.copysign(vo);
        dvo = 0.0;
    }
    if !vo.is_finite() {
        // Never observed; keep the audio thread safe regardless.
        std::ptr::write_bytes(st, 0, CIRCUIT_LEN);
        return 0.0;
    }
    let vm = c.alpha * vo + beta;
    let i1 = (vm - h1) * c.y1;
    let i2 = (vm - h2) * c.y2;
    *st.add(ST_VO) = vo as f32;
    *st.add(ST_DVO) = dvo as f32;
    *st.add(ST_VF) = (vo - vm) as f32;
    *st.add(ST_VC1) = (h1 + i1 * c.inv_g1) as f32;
    *st.add(ST_I1) = i1 as f32;
    *st.add(ST_VC2) = (h2 + i2 * c.inv_g2) as f32;
    *st.add(ST_I2) = i2 as f32;

    // Clipper node: (u - vd) / rs = Id(vd) + (vd - ht) * yt.
    let hc = *st.add(ST_VCC) as f64 + *st.add(ST_IC) as f64 * c.inv_gc;
    let ht = *st.add(ST_VCT) as f64 + *st.add(ST_IT) as f64 * c.inv_gt;
    let u = vo - hc;
    // Without the diodes the node would sit at `open`. With only the one
    // that conducts for this sign, v + (Is / g) * exp(v / n) = |open|, which
    // is w + exp(w) = y in w = v / n + ln(Is / (g n)): one table read.
    let open = (u * c.inv_rs + ht * c.yt) * c.inv_g_lin;
    let (on, other_inv) = if open < 0.0 { (&d.neg, d.pos.inv_n) } else { (&d.pos, d.neg.inv_n) };
    let y = open.abs() * on.inv_n + on.ln_c;
    let w = omega(c.tables, y);
    let mut v = (w - on.ln_c) * on.n;
    // The other diode's reverse current matters only within a few n Vt of
    // zero, where it is one Newton step from there (the step is under 2 uV,
    // so what is left of it is far below the state's precision). exp(w) is
    // y - w, so the conducting diode needs no exponential.
    let reverse = v * other_inv;
    if reverse < REVERSE_CUTOFF {
        let i_reverse = DIODE_IS * fast_exp(c.tables, -reverse);
        let slope = c.g_lin * (1.0 + (y - w)) + i_reverse * other_inv;
        v += i_reverse / slope;
    }
    let vd = v.copysign(open);
    let ic = (u - vd) * c.inv_rs;
    let it = (vd - ht) * c.yt;
    let vct = ht + it * c.inv_gt;
    *st.add(ST_VCC) = (hc + ic * c.inv_gc) as f32;
    *st.add(ST_IC) = ic as f32;
    *st.add(ST_VCT) = vct as f32;
    *st.add(ST_IT) = it as f32;

    // Output coupling: the output is the drop across Ro.
    let ho = *st.add(ST_VCO) as f64 + *st.add(ST_IO) as f64 * c.inv_go;
    let out = (vct - ho) * c.ko;
    let io = out * INV_RO;
    *st.add(ST_VCO) = (ho + io * c.inv_go) as f32;
    *st.add(ST_IO) = io as f32;
    if !out.is_finite() {
        std::ptr::write_bytes(st, 0, CIRCUIT_LEN);
        return 0.0;
    }
    out
}

/// One host-rate sample through the stage in the unit domain: `x` of
/// amplitude 1 is [`INPUT_VOLTS`] at the input, and the output is scaled so
/// the small-signal gain at 1 kHz is +1. `bias` is the -1..1 bias control
/// (diode asymmetry). The output lags the input by
/// [`oversample4::LATENCY_SAMPLES`].
///
/// # Safety
/// `st` must point to [`STATE_LEN`] valid floats.
#[inline]
pub unsafe fn process(c: &Coeffs, st: *mut f32, x: f32, bias: f32) -> f32 {
    let limit = (MAX_INPUT_VOLTS / INPUT_VOLTS) as f32;
    let x = if x.is_finite() { x.clamp(-limit, limit) } else { 0.0 };
    let bias = if bias.is_finite() { bias.clamp(-1.0, 1.0) } else { 0.0 };
    let scale = 1.0 / (SMALL_SIGNAL_GAIN * INPUT_VOLTS);
    // The circuit rests at zero whatever the bias: it only changes the diodes.
    if oversample4::idle(st, STATE_LEN, OS, x, true, c.quiet_samples) {
        return 0.0;
    }
    let diodes = Diodes::new(c, bias as f64);
    oversample4::process(st.add(OS), x, |v| {
        (solve_step(c, &diodes, st, v as f64 * INPUT_VOLTS) * scale) as f32
    })
}

/// Memoryless midband transfer of the stage in the unit domain, for the
/// shaper display and the stateless `shaper_transfer`: an ideal amplifier
/// into the diodes with the capacitors as shorts, so no slewing. A rational
/// fit of that solve (`tools/saturator/opamp_clip_ref.py` prints its error),
/// the same form as `triode::static_transfer`.
pub fn static_transfer(x: f32) -> f32 {
    let x = x.clamp(-64.0, 64.0);
    if x >= 0.0 {
        STATIC_POS_LIMIT * x / (x.powf(STATIC_POS_KNEE) + STATIC_POS_LIMIT.powf(STATIC_POS_KNEE))
            .powf(1.0 / STATIC_POS_KNEE)
    } else {
        let u = -x;
        -STATIC_NEG_LIMIT * u / (u.powf(STATIC_NEG_KNEE) + STATIC_NEG_LIMIT.powf(STATIC_NEG_KNEE))
            .powf(1.0 / STATIC_NEG_KNEE)
    }
}

// Fit constants, see `static_transfer`. The curve is symmetric.
pub const STATIC_POS_LIMIT: f32 = 0.5530;
pub const STATIC_POS_KNEE: f32 = 2.3059;
pub const STATIC_NEG_LIMIT: f32 = STATIC_POS_LIMIT;
pub const STATIC_NEG_KNEE: f32 = STATIC_POS_KNEE;

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    struct Stage {
        coeffs: Coeffs,
        state: [f32; STATE_LEN],
    }

    impl Stage {
        /// `rate` is the solver rate for `volts` (four times the host rate
        /// `tick` then runs at).
        fn new(rate: f64) -> Self {
            Self { coeffs: Coeffs::at_solver_rate(rate), state: [0.0; STATE_LEN] }
        }
        fn volts(&mut self, e: f64, bias: f64) -> f64 {
            let diodes = Diodes::new(&self.coeffs, bias);
            unsafe { solve_step(&self.coeffs, &diodes, self.state.as_mut_ptr(), e) }
        }
        /// One host-rate sample through the oversampled stage.
        fn tick(&mut self, x: f32, bias: f32) -> f32 {
            unsafe { process(&self.coeffs, self.state.as_mut_ptr(), x, bias) }
        }
    }

    /// For a sine of `amp` volts in steady state: the fraction of the time
    /// the op-amp output spends between the rails, and its fastest rate of
    /// change in volts per second.
    fn transit_and_rate(freq: f64, amp: f64) -> (f64, f64) {
        let rate = 192_000.0;
        let mut stage = Stage::new(rate);
        let n = (rate * 0.05) as usize;
        let (mut transit, mut step, mut last) = (0_usize, 0.0_f64, 0.0_f64);
        for i in 0..n {
            stage.volts(amp * (2.0 * PI * freq * i as f64 / rate).sin(), 0.0);
            let vo = stage.state[ST_VO] as f64;
            if i >= n / 2 {
                transit += (vo.abs() < VMAX - 0.01) as usize;
                step = step.max((vo - last).abs());
            }
            last = vo;
        }
        (transit as f64 / (n - n / 2) as f64, step * rate)
    }

    #[test]
    fn rest_state_is_silent() {
        // Zero state, zero input: nothing moves, whatever the bias. (The
        // clipper table is good to nanovolts, so "nothing" is not exact.)
        let mut stage = Stage::new(192_000.0);
        for bias in [0.0, 1.0, -1.0] {
            for _ in 0..5_000 {
                assert!(stage.volts(0.0, bias).abs() < 1.0e-7);
            }
        }
        assert!(stage.state.iter().all(|v| v.abs() < 1.0e-6));
        // And through `process`, in the unit domain.
        let mut stage = Stage::new(192_000.0);
        for _ in 0..5_000 {
            assert!(stage.tick(0.0, 0.0).abs() < 1.0e-6);
        }
    }

    #[test]
    fn small_signals_pass_at_unity_gain() {
        let rate = 48_000.0;
        let mut stage = Stage::new(4.0 * rate);
        let n = (rate * 0.5) as usize;
        let mut peak = 0.0_f32;
        let mut dot = 0.0_f64;
        let signal = |i: usize| 1.0e-3 * (2.0 * PI * 1000.0 * i as f64 / rate).sin();
        for i in 0..n {
            let y = stage.tick(signal(i) as f32, 0.0);
            if i > n - 480 {
                peak = peak.max(y.abs());
                // Against the input LATENCY_SAMPLES ago: in phase, not inverted.
                dot += signal(i - oversample4::LATENCY_SAMPLES) * y as f64;
            }
        }
        assert!(dot / 240.0 > 0.99e-6, "aligned at LATENCY_SAMPLES: {dot}");
        assert!((peak / 1.0e-3 - 1.0).abs() < 0.01, "small-signal gain {}", peak / 1.0e-3);
    }

    #[test]
    fn matches_the_ode_reference() {
        // Fixtures: tools/saturator/opamp_clip_ref.py (scipy Radau solve of
        // the same circuit), output volts at 192 kHz.
        let fixtures: serde_json::Value =
            serde_json::from_str(include_str!("opamp_clip_fixtures.json")).unwrap();
        let rate = fixtures["rate"].as_f64().unwrap();
        for case in fixtures["cases"].as_array().unwrap() {
            let name = case["name"].as_str().unwrap();
            let floats = |key: &str| -> Vec<f64> {
                case[key].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect()
            };
            let (input, reference) = (floats("input"), floats("output"));
            let bias = case["asym"].as_f64().unwrap();
            let mut stage = Stage::new(rate);
            let mut err = 0.0;
            let mut power = 0.0;
            for (e, want) in input.iter().zip(&reference) {
                let got = stage.volts(*e, bias);
                err += (got - want) * (got - want);
                power += want * want;
            }
            let db = 10.0 * (err / power).log10();
            // Per-case tolerance: see opamp_clip_ref.py. Once the op-amp
            // slews or hits a rail the error is the time step (when an edge
            // starts and stops), not the solve.
            let tolerance = case["tolerance_db"].as_f64().unwrap();
            assert!(db < tolerance, "{name}: error {db:.1} dB relative to the reference");
        }
    }

    #[test]
    fn highs_slew_where_lows_square_off() {
        // The same overdrive at 200 Hz and at 10 kHz. Rail to rail takes the
        // op-amp 2 * VMAX / SLEW_RATE = 23 microseconds: nothing at 200 Hz,
        // which comes out square, and nearly half of every 10 kHz half cycle,
        // which comes out a trapezoid.
        let (low_transit, low_rate) = transit_and_rate(200.0, 8.0);
        let (high_transit, high_rate) = transit_and_rate(10_000.0, 8.0);
        assert!(low_transit < 0.02, "the low tone is square: {low_transit}");
        let expect = (2.0 * VMAX / SLEW_RATE) * 2.0 * 10_000.0;
        assert!((high_transit - expect).abs() < 0.08, "{high_transit}, expected {expect}");
        for rate in [low_rate, high_rate] {
            assert!(rate < 1.02 * SLEW_RATE, "never faster than the slew rate: {rate} V/s");
            assert!(rate > 0.95 * SLEW_RATE, "the edges are slewing: {rate} V/s");
        }
    }

    #[test]
    fn bias_makes_the_clipping_asymmetric() {
        let rate = 48_000.0;
        let peaks = |bias: f32| {
            let mut stage = Stage::new(4.0 * rate);
            let (mut lo, mut hi) = (0.0_f32, 0.0_f32);
            for i in 0..(rate as usize / 2) {
                let x = 8.0 * (2.0 * PI * 220.0 * i as f64 / rate).sin();
                let y = stage.tick(x as f32, bias);
                if i > rate as usize / 4 {
                    lo = lo.min(y);
                    hi = hi.max(y);
                }
            }
            (lo, hi)
        };
        let (lo, hi) = peaks(0.0);
        assert!((hi + lo).abs() < 0.02 * hi, "symmetric at zero bias: {lo}..{hi}");
        let (lo_pos, hi_pos) = peaks(1.0);
        assert!(hi_pos - lo_pos > 1.5 * (hi - lo), "three diodes on one side: {lo_pos}..{hi_pos}");
        let (lo_neg, hi_neg) = peaks(-1.0);
        assert!((hi_neg - lo_neg - (hi_pos - lo_pos)).abs() < 0.02, "mirror images");
    }

    #[test]
    fn full_scale_noise_at_maximum_drive_stays_bounded() {
        let mut stage = Stage::new(192_000.0);
        let mut seed = 0x1234_5678_u32;
        let mut power = 0.0_f64;
        let n = 48_000;
        // One diode drop at the most current the op-amp can push, in the
        // unit domain.
        let drop = (DIODE_NVT * (VMAX / RS / DIODE_IS).ln() / (SMALL_SIGNAL_GAIN * INPUT_VOLTS)) as f32;
        for i in 0..n {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let x = (seed as f32 / u32::MAX as f32 - 0.5) * 2.0 * 300.0;
            let y = stage.tick(x, 0.0);
            // The decimation filters overshoot a clipped edge a little.
            assert!(y.abs() < 1.5 * drop, "sample {i}: {y} is past the diodes ({drop})");
            if i > n / 2 {
                power += (y as f64) * (y as f64);
            }
        }
        let rms = (power / (n / 2) as f64).sqrt();
        assert!(rms > 0.1, "the stage keeps passing (clipped) signal: rms {rms}");
    }

    #[test]
    fn survives_hostile_input() {
        let mut stage = Stage::new(192_000.0);
        let inputs = [f32::NAN, f32::INFINITY, -1.0e9, 1.0e9, 0.0, -3.0];
        for (i, x) in inputs.iter().cycle().take(6000).enumerate() {
            let bias = [1.0, -1.0, f32::NAN, 0.3][i % 4];
            let y = stage.tick(*x, bias);
            assert!(y.is_finite() && y.abs() < 4.0, "{y}");
        }
    }
}
