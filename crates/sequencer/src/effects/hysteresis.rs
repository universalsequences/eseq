//! An audio transformer with a hysteretic iron core, solved as a circuit.
//!
//! This is the `transformer` shaper of the Roar engine. Like `triode.rs` it
//! is not a transfer curve: the core's flux is the integral of the voltage
//! across the winding, so how hard it saturates depends on frequency. Lows
//! reach saturation long before highs, and once the core is full the
//! winding's inductance collapses and the rest of that half-cycle (and any
//! highs riding on it) drops out until the flux turns around.
//!
//! ```text
//!   x --[driver, rails]-- e(t) --Rs--+---------+------+-- out (1:1)
//!                                    |         |      |
//!                                 primary     Idc     RL
//!                                 (N turns    (bias)  |
//!                                  on core)    |      |
//!                                   gnd       gnd    gnd
//! ```
//!
//! The primary is a nonlinear inductor: `v = N A dB/dt`, `B = mu0 (H + M)`,
//! with `H = N i / l` the field of the magnetising current and `M` the
//! magnetisation of the core, which follows the Jiles-Atherton model (Jiles &
//! Atherton, "Theory of ferromagnetic hysteresis", 1986):
//!
//! ```text
//!   He   = H + alpha M                         effective field
//!   Man  = Ms (coth(He/a) - a/He)              anhysteretic (Langevin)
//!   M    = Mirr + c (Man - Mirr)               reversible wall bowing
//!   dMirr/dH = (Man - Mirr) / (delta k - alpha (Man - Mirr)),  delta = sign(dH/dt)
//! ```
//!
//! which combine to
//!
//! ```text
//!   dM/dH = [ (1-c)(Man - M) / ((1-c) delta k - alpha (Man - M)) + c Man' ]
//!           / (1 - c alpha Man')
//! ```
//!
//! Just after a reversal the raw equation gives a negative susceptibility,
//! which no material has. The usual guard is applied: the irreversible term
//! is dropped while `delta (Man - M) <= 0`, and its denominator is kept
//! positive.
//!
//! **The core and the winding are chosen, not measured.** The numbers below
//! describe a plausible small 50% nickel-iron line output transformer (1.5 T,
//! 8.6 H, 600 ohm source), picked so that it behaves like one; they are not a
//! fit to any particular part, and this shaper should not be described as a
//! model of a named device. What is checked is that the solver integrates
//! this circuit correctly.
//!
//! The driver in front of the transformer has supply rails ([`RAIL`]): an
//! ideal source would let a saturated core pass ever larger spikes.
//!
//! Discretization: the flux is integrated with the trapezoidal rule, the
//! magnetisation law along `H` with a theta rule, and each step is one scalar
//! solve for the new `H` ([`solve_step`]). The Langevin function is read
//! from a cubic Hermite table. It runs at 4x the host rate inside the shared
//! oversampler (`oversample4.rs`).
//!
//! Rest state and remanence: the all-zero state is the demagnetised core. A
//! core that has been driven stays magnetised when the signal stops, with no
//! voltage across the winding, so silence is silent either way; when the idle
//! bypass parks the circuit after two seconds of silence it also
//! demagnetises it, inaudibly.
//!
//! `tools/saturator/hysteresis_ref.py` holds the same circuit twice: this
//! exact algorithm, and a stiff ODE integration in time that it is checked
//! against and that the fixtures here come from. Keep them in sync.

use super::oversample4;

const MU0: f64 = 4.0e-7 * std::f64::consts::PI;
const INV_MU0: f64 = 1.0 / MU0;

// Core (Jiles-Atherton), chosen: a 50% nickel-iron lamination.
const CORE_MS: f64 = 1.2e6; // A/m, 1.5 T
const CORE_A: f64 = 20.0; // A/m
const CORE_ALPHA: f64 = 4.0e-6;
const CORE_K: f64 = 8.0; // A/m
const CORE_C: f64 = 0.25;

// Winding and circuit, chosen: 1600 turns on 0.42 cm^2 with an 8 cm path,
// driven from 600 ohm into a 10 k load.
const TURNS: f64 = 1600.0;
const AREA: f64 = 0.42e-4;
const PATH: f64 = 0.08;
const RS: f64 = 600.0;
const RL: f64 = 10e3;

/// Load divider seen by the driver once the core draws no current.
const KDIV: f64 = RL / (RS + RL);
const RTH: f64 = RS * RL / (RS + RL);
/// Volts dropped across the source per A/m of field.
const RH: f64 = RTH * PATH / TURNS;
/// Volt-seconds per tesla.
const NA: f64 = TURNS * AREA;
const PIN: f64 = (1.0 - CORE_C) * CORE_K;
const MS_OVER_A: f64 = CORE_MS / CORE_A;
const INV_A: f64 = 1.0 / CORE_A;

/// Volts at the driver output for a unit-amplitude signal: a 60 Hz tone at 1
/// swings the core to 1.3 T, the onset of saturation. Onset scales with
/// frequency (flux is the integral of voltage): 30 Hz is there at 0.5, 240 Hz
/// at 4.
pub const INPUT_VOLTS: f64 = 33.0;
/// The driver's swing limit, in units of input.
pub const RAIL: f64 = 3.0;
/// A/m of standing field per unit of the bias control: the DC magnetising
/// current of a single-ended stage. It parks the core part-way up its curve
/// (about 0.9 T at half, 1.25 T at full), so one polarity saturates first.
pub const BIAS_H: f64 = 120.0;
/// The drive is unbounded upstream; the driver is hard on its rails long
/// before this.
const MAX_INPUT: f32 = 1.0e3;

const MAX_NEWTON_ITERS: usize = 12;
// The solve stops when a step is this small (A/m, plus a relative part).
const NEWTON_TOL_H: f64 = 1.0e-4;
const NEWTON_TOL_REL: f64 = 1.0e-5;
// Per A/m; see the convergence test in `solve_step`.
const CURVATURE_FLOOR: f64 = 1.0e-2;
/// Slope of the irreversible pull where M meets the anhysteretic curve.
const RATE_AT_CURVE: f64 = 1.0 / CORE_K;

/// Floats of circuit state per channel.
const CIRCUIT_LEN: usize = 4;
const ST_H: usize = 0; // A/m
const ST_M: usize = 1; // A/m
const ST_V: usize = 2; // winding volts at the last step
const ST_DH: usize = 3; // last step's change in H, the next step's predictor
const OS: usize = CIRCUIT_LEN;
/// Floats of runtime state per channel ([`process`]).
pub const STATE_LEN: usize = OS + oversample4::STATE_LEN;

/// Sample-rate dependent constants.
#[derive(Clone, Copy)]
pub struct Coeffs {
    /// T / (2 N A): tesla per volt of (v[n] + v[n-1]).
    c1: f64,
    /// A/m of magnetisation given up per A/m of field at constant flux
    /// demand: 1 + c1 RH / mu0.
    m_slope: f64,
    /// Unit-domain output per winding volt.
    scale: f64,
    table: &'static Table,
    quiet_samples: f32,
}

impl Coeffs {
    /// For [`process`] at `host_rate` (the circuit is solved at 4x that).
    pub fn new(host_rate: f64) -> Self {
        Self::at_solver_rate(host_rate.max(1_000.0) * oversample4::FACTOR)
    }

    fn at_solver_rate(rate: f64) -> Self {
        let c1 = 1.0 / (2.0 * rate * NA);
        Self {
            c1,
            m_slope: 1.0 + c1 * RH * INV_MU0,
            scale: 1.0 / (small_signal_gain(1000.0) * INPUT_VOLTS),
            table: table(),
            quiet_samples: oversample4::quiet_samples(rate / oversample4::FACTOR),
        }
    }
}

/// Winding volts per driver volt for a vanishing signal at `freq` (the
/// demagnetised core is then a linear inductor, a highpass against `RTH`).
fn small_signal_gain(freq: f64) -> f64 {
    let dman = MS_OVER_A / 3.0;
    let chi0 = CORE_C * dman / (1.0 - CORE_C * CORE_ALPHA * dman);
    let inductance = MU0 * (1.0 + chi0) * TURNS * TURNS * AREA / PATH;
    let w = 2.0 * std::f64::consts::PI * freq * inductance;
    KDIV * w / RTH.hypot(w)
}

/// The Langevin function coth(q) - 1/q and its derivative.
fn langevin_law(q: f64) -> (f64, f64) {
    let aq = q.abs();
    if aq < 1.0e-2 {
        let q2 = q * q;
        (
            q * (1.0 / 3.0 - q2 / 45.0 + 2.0 * q2 * q2 / 945.0),
            1.0 / 3.0 - q2 / 15.0 + 2.0 * q2 * q2 / 189.0,
        )
    } else {
        let e = (-2.0 * aq).exp();
        let inv = 1.0 / aq;
        let inv_1me = 1.0 / (1.0 - e);
        (
            ((1.0 + e) * inv_1me - inv).copysign(q),
            inv * inv - 4.0 * e * inv_1me * inv_1me,
        )
    }
}

// Langevin table: value and slope on a uniform grid, cubic Hermite between.
// Past its ends exp(-2 |q|) is below 2e-14 and the function is 1 - 1/|q|.
const TABLE_N: usize = 2048;
const TABLE_Q: f64 = 16.0;

type Table = Vec<[f64; 2]>;

fn table() -> &'static Table {
    static TABLE: std::sync::OnceLock<Table> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| {
        (0..=TABLE_N)
            .map(|i| {
                let (value, slope) =
                    langevin_law(-TABLE_Q + 2.0 * TABLE_Q * i as f64 / TABLE_N as f64);
                [value, slope]
            })
            .collect()
    })
}

/// Build the Langevin table now, off the audio thread ([`Coeffs::new`]
/// otherwise builds it on first use).
pub fn warm_tables() {
    table();
}

/// [`langevin_law`], read from the table.
#[inline]
fn langevin(table: &Table, q: f64) -> (f64, f64) {
    if q.abs() >= TABLE_Q {
        let inv = 1.0 / q.abs();
        return ((1.0 - inv).copysign(q), inv * inv);
    }
    const INV_H: f64 = TABLE_N as f64 / (2.0 * TABLE_Q);
    const H: f64 = 2.0 * TABLE_Q / TABLE_N as f64;
    let t = (q + TABLE_Q) * INV_H;
    let i = (t as usize).min(TABLE_N - 1);
    let u = t - i as f64;
    let [f0, d0] = table[i];
    let [f1, d1] = table[i + 1];
    let (d0, d1) = (d0 * H, d1 * H);
    let u2 = u * u;
    let u3 = u2 * u;
    let value = (2.0 * u3 - 3.0 * u2 + 1.0) * f0
        + (u3 - 2.0 * u2 + u) * d0
        + (-2.0 * u3 + 3.0 * u2) * f1
        + (u3 - u2) * d1;
    let slope = ((6.0 * u2 - 6.0 * u) * (f0 - f1)
        + (3.0 * u2 - 4.0 * u + 1.0) * d0
        + (3.0 * u2 - 2.0 * u) * d1)
        * INV_H;
    (value, slope)
}

/// The core at (`h`, `m`): everything about its susceptibility dM/dH that
/// does not depend on which way the field is moving.
struct Core {
    /// Reversible part of dM/dH (domain wall bowing).
    rev: f64,
    /// Slope of the anhysteretic curve, dMan/dHe.
    dman: f64,
    /// Man - M: which side the anhysteretic curve is on, and how far.
    diff: f64,
    /// 1 / (1 - c alpha Man').
    inv_d: f64,
}

const _: () = assert!(CORE_C * CORE_ALPHA * MS_OVER_A / 3.0 <= 0.02);

#[inline]
fn core(table: &Table, h: f64, m: f64) -> Core {
    let (lv, ld) = langevin(table, (h + CORE_ALPHA * m) * INV_A);
    let dman = MS_OVER_A * ld;
    // 1 / (1 - y) as a series: y = c alpha Man' is at most 0.02 (the assert
    // above), so the dropped y^4 is under 2e-7.
    let y = CORE_C * CORE_ALPHA * dman;
    let inv_d = 1.0 + y * (1.0 + y * (1.0 + y));
    Core { rev: CORE_C * dman * inv_d, dman, diff: CORE_MS * lv - m, inv_d }
}

/// Irreversible part of dM/dH at distance `d = |Man - M|` from the
/// anhysteretic curve, for a field moving toward it, and how much it changes
/// per A/m of `d`. For a field moving away from the curve it is zero (the
/// guard against the raw equation's negative susceptibility).
#[inline]
fn pull(d: f64, inv_d: f64) -> (f64, f64) {
    let den = PIN - CORE_ALPHA * d;
    if den > 0.1 * PIN {
        let inv_den = 1.0 / den;
        ((1.0 - CORE_C) * d * inv_den * inv_d, (1.0 - CORE_C) * PIN * inv_den * inv_den * inv_d)
    } else {
        // Keep the denominator positive.
        let k = (1.0 - CORE_C) / (0.1 * PIN) * inv_d;
        (k * d, k)
    }
}

#[inline]
fn sign(x: f64) -> f64 {
    if x > 0.0 {
        1.0
    } else if x < 0.0 {
        -1.0
    } else {
        0.0
    }
}

/// The driver's swing limit: clean below the rails, firm at them.
#[inline]
fn rail(x: f64) -> f64 {
    let r = x * (1.0 / RAIL);
    let r2 = r * r;
    let t = r2 * r2;
    if t < 0.02 {
        // (1 + t)^(-1/4) by its series, to 2e-8: most signals never get
        // near the rails.
        x * (1.0 + t * (-0.25 + t * (5.0 / 32.0 - t * (15.0 / 128.0))))
    } else {
        x / (1.0 + t).sqrt().sqrt()
    }
}

/// Advance the circuit one solver step with `x` units at the driver input
/// and `hdc` A/m of standing field. Returns the winding voltage.
///
/// The unknown is the new field `h1`. The trapezoidal flux equation is
/// linear, so it gives the new magnetisation directly, `m(h1)`; what is
/// solved is the magnetisation law integrated along H from `(h0, m0)` to
/// `(h1, m(h1))` with a theta rule:
///
/// ```text
///   m(h1) - m0 = dh ((1 - theta) chi(h0, m0) + theta chi(h1, m(h1)))
/// ```
///
/// `theta` is 1/2 (trapezoidal, second order) while the step is short against
/// the few A/m over which the irreversible pull toward the anhysteretic curve
/// relaxes, and tends to 1 (backward Euler, which lands on that curve without
/// overshooting it) as the step outgrows them: in saturation one step moves
/// H by hundreds of A/m. An explicit rule there overshoots the curve and the
/// step equation grows spurious roots.
///
/// The residual is monotonic in `h1`, so the iteration is bracketed. It is
/// Newton's, except that where the pull is active the residual is close to a
/// parabola in the distance moved (the pull grows linearly with it), and the
/// step goes to that parabola's root.
///
/// # Safety
/// `st` must point to [`CIRCUIT_LEN`] valid floats.
#[inline]
unsafe fn solve_step(c: &Coeffs, st: *mut f32, x: f64, hdc: f64) -> f64 {
    let vth = KDIV * INPUT_VOLTS * rail(x);
    let h0 = *st.add(ST_H) as f64;
    let m0 = *st.add(ST_M) as f64;
    // Flux: mu0 (h1 + m1) = B0 + c1 (v1 + v0), v1 = vth - RH (h1 - hdc)
    //   =>  m1 = m_at0 - m_slope (h1 - h0)
    let rhs = MU0 * (h0 + m0) + c.c1 * (vth + RH * hdc + *st.add(ST_V) as f64);
    let m_at0 = rhs * INV_MU0 - c.m_slope * h0;
    let gap = m_at0 - m0;
    // Start of the step. The pull only acts for a field moving toward the
    // side the anhysteretic curve is on.
    let start = core(c.table, h0, m0);
    let side = sign(start.diff);
    let (irr0, rate0) = pull(start.diff.abs(), start.inv_d);

    // Work in u = |h1 - h0| along the direction delta the field moves:
    //   psi(u) = delta (m(h1) - m0) - u mean(u),  decreasing and concave.
    let mut h1 = h0 + *st.add(ST_DH) as f64;
    let (mut lo, mut hi) = (f64::NEG_INFINITY, f64::INFINITY);
    for _ in 0..MAX_NEWTON_ITERS {
        let dh = h1 - h0;
        let delta = if dh != 0.0 { sign(dh) } else { sign(gap) };
        if delta == 0.0 {
            break;
        }
        let u = dh.abs();
        let m1 = m_at0 - c.m_slope * dh;
        let end = core(c.table, h1, m1);
        let (irr1, rate1) =
            if delta * end.diff > 0.0 { pull(end.diff.abs(), end.inv_d) } else { (0.0, 0.0) };
        let chi1 = end.rev + irr1;
        let (theta, dtheta, chi0) = if delta == side {
            let z = u * rate0;
            let inv = 1.0 / (1.0 + z);
            // theta and d(theta)/du.
            (0.5 * (1.0 + 2.0 * z) * inv, 0.5 * rate0 * inv * inv, start.rev + irr0)
        } else {
            (0.5, 0.0, start.rev)
        };
        let mean = (1.0 - theta) * chi0 + theta * chi1;
        let psi = delta * (m1 - m0) - u * mean;
        if delta * psi > 0.0 {
            lo = h1;
        } else {
            hi = h1;
        }
        // How fast the distance to the anhysteretic curve changes with u.
        let gd = end.dman * (1.0 - CORE_ALPHA * c.m_slope) + c.m_slope;
        let flat = -c.m_slope - mean - u * dtheta * (chi1 - chi0);
        let (du, d1, d2) = if irr1 > 0.0 {
            let d1 = flat - theta * u * rate1 * gd;
            let d2 = -2.0 * theta * rate1 * gd;
            (parabola_root(psi, d1, d2), d1, d2)
        } else {
            // No pull at the trial point (the field moves away from the
            // curve, or the flux puts M past it): psi is a line here...
            let du = -psi / flat;
            let to_curve = -delta * end.diff / gd; // >= 0
            if du > to_curve {
                // ...up to where M meets the curve; continue from there on
                // the parabola the pull makes beyond it.
                let d1 = flat - theta * (u + to_curve) * RATE_AT_CURVE * gd;
                let d2 = -2.0 * theta * RATE_AT_CURVE * gd;
                (to_curve + parabola_root(psi + flat * to_curve, d1, d2), d1, d2)
            } else {
                (du, flat, 0.0)
            }
        };
        let mut next = h1 + delta * du;
        let bisected = next != h1 && !(lo < next && next < hi);
        if bisected {
            // Overshot a point already known to be on the other side.
            next = 0.5 * (lo + hi);
        }
        let step = next - h1;
        h1 = next;
        // Done when the step is negligible, or when what a step of this size
        // leaves behind is: Newton's error is the step squared times the
        // curvature over twice the slope (the parabola step leaves less),
        // with a floor for the curvature the model ignores.
        let tol = NEWTON_TOL_H + NEWTON_TOL_REL * h1.abs();
        let small = step * step * (d2.abs() + 2.0 * CURVATURE_FLOOR * d1.abs())
            < 2.0 * tol * d1.abs();
        if step.abs() < tol || (small && !bisected) {
            break;
        }
    }
    if !h1.is_finite() {
        // Never observed; keep the audio thread safe regardless.
        std::ptr::write_bytes(st, 0, CIRCUIT_LEN);
        return 0.0;
    }
    let v = vth - RH * (h1 - hdc);
    *st.add(ST_DH) = (h1 - h0) as f32;
    *st.add(ST_H) = h1 as f32;
    *st.add(ST_M) = (m_at0 - c.m_slope * (h1 - h0)).clamp(-CORE_MS, CORE_MS) as f32;
    *st.add(ST_V) = v as f32;
    v
}

/// The root nearest zero of `psi + d1 s + d2 s^2 / 2` for a decreasing,
/// concave residual (`d1 < 0`, `d2 <= 0`); the tangent's root if the parabola
/// has none.
#[inline]
fn parabola_root(psi: f64, d1: f64, d2: f64) -> f64 {
    2.0 * psi / (-d1 + (d1 * d1 - 2.0 * d2 * psi).max(0.0).sqrt())
}

/// One host-rate sample through the transformer in the unit domain: `x` of
/// amplitude 1 is [`INPUT_VOLTS`] at the driver, and the output is scaled so
/// the small-signal gain at 1 kHz is +1. `bias` is the -1..1 bias control.
/// The output lags the input by `oversample4::LATENCY_SAMPLES`.
///
/// # Safety
/// `st` must point to [`STATE_LEN`] valid floats.
#[inline]
pub unsafe fn process(c: &Coeffs, st: *mut f32, x: f32, bias: f32) -> f32 {
    let x = if x.is_finite() { x.clamp(-MAX_INPUT, MAX_INPUT) } else { 0.0 };
    let hdc = bias.clamp(-1.0, 1.0) as f64 * BIAS_H;
    // Idle only without standing current: with it the core rests elsewhere.
    if oversample4::idle(st, STATE_LEN, OS, x, oversample4::bias_at_rest(bias), c.quiet_samples) {
        return 0.0;
    }
    oversample4::process(st.add(OS), x, |v| (solve_step(c, st, v as f64, hdc) * c.scale) as f32)
}

/// Display curve. A flux-driven core has no memoryless transfer, so this is
/// the level of a 60 Hz tone's fundamental at the output against its level at
/// the input, extended odd-symmetrically: where the core starts to give at
/// that frequency and how completely. Same rational form as the triode's
/// (`tools/saturator/hysteresis_ref.py` fits it), dual maintained with the
/// `roar-shaper` fragment shaders.
pub fn static_transfer(x: f32) -> f32 {
    let u = x.clamp(-64.0, 64.0).abs();
    let y = STATIC_POS_LIMIT * u
        / (u.powf(STATIC_POS_KNEE) + STATIC_POS_LIMIT.powf(STATIC_POS_KNEE))
            .powf(1.0 / STATIC_POS_KNEE);
    y.copysign(x)
}

// Fit constants, see `static_transfer`. The curve is symmetric.
pub const STATIC_POS_LIMIT: f32 = 1.5393;
pub const STATIC_POS_KNEE: f32 = 5.0860;
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
        /// `rate` is the solver rate for `unit` (four times the host rate
        /// `tick` then runs at).
        fn new(rate: f64) -> Self {
            Self { coeffs: Coeffs::at_solver_rate(rate), state: [0.0; STATE_LEN] }
        }
        /// One solver step, unit domain in and out.
        fn unit(&mut self, x: f64, bias: f64) -> f64 {
            unsafe {
                solve_step(&self.coeffs, self.state.as_mut_ptr(), x, bias * BIAS_H)
                    * self.coeffs.scale
            }
        }
        /// One host-rate sample through the oversampled circuit.
        fn tick(&mut self, x: f32, bias: f32) -> f32 {
            unsafe { process(&self.coeffs, self.state.as_mut_ptr(), x, bias) }
        }
    }

    /// Total harmonic distortion (harmonics 2..=10 over the fundamental) of
    /// the last `cycles` whole cycles of a sine through the oversampled stage.
    fn thd(freq: f64, amp: f64) -> f64 {
        let rate = 48_000.0;
        let mut stage = Stage::new(4.0 * rate);
        let period = (rate / freq).round() as usize;
        let n = period * 8;
        let out: Vec<f64> = (0..n)
            .map(|i| stage.tick((amp * (2.0 * PI * i as f64 / period as f64).sin()) as f32, 0.0) as f64)
            .collect();
        let seg = &out[n - 2 * period..];
        let bin = |k: usize| {
            let (mut re, mut im) = (0.0, 0.0);
            for (i, y) in seg.iter().enumerate() {
                let ph = 2.0 * PI * (k * i) as f64 / period as f64;
                re += y * ph.cos();
                im += y * ph.sin();
            }
            re.hypot(im)
        };
        let fund = bin(1);
        (2..=10).map(|k| bin(k).powi(2)).sum::<f64>().sqrt() / fund
    }

    #[test]
    fn demagnetised_core_at_rest_is_silent() {
        let mut stage = Stage::new(192_000.0);
        for _ in 0..20_000 {
            assert_eq!(stage.unit(0.0, 0.0), 0.0);
        }
        assert!(stage.state[..CIRCUIT_LEN].iter().all(|v| *v == 0.0));
    }

    #[test]
    fn small_signals_pass_at_unity_gain() {
        let rate = 48_000.0;
        let mut stage = Stage::new(4.0 * rate);
        let n = (rate * 0.2) as usize;
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
        assert!(dot / 240.0 > 0.996e-6, "aligned at LATENCY_SAMPLES: {dot}");
        assert!((peak / 1.0e-3 - 1.0).abs() < 0.01, "small-signal gain {}", peak / 1.0e-3);
    }

    #[test]
    fn matches_the_ode_reference() {
        // Fixtures: tools/saturator/hysteresis_ref.py (scipy Radau solve of
        // the same circuit in time), unit-domain output at 192 kHz.
        let fixtures: serde_json::Value =
            serde_json::from_str(include_str!("hysteresis_fixtures.json")).unwrap();
        let rate = fixtures["rate"].as_f64().unwrap();
        for case in fixtures["cases"].as_array().unwrap() {
            let name = case["name"].as_str().unwrap();
            let floats = |key: &str| -> Vec<f64> {
                case[key].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect()
            };
            let (input, reference) = (floats("input"), floats("output"));
            let bias = case["bias"].as_f64().unwrap();
            let mut stage = Stage::new(rate);
            let mut err = 0.0;
            let mut power = 0.0;
            for (x, want) in input.iter().zip(&reference) {
                let got = stage.unit(*x, bias);
                err += (got - want) * (got - want);
                power += want * want;
            }
            let db = 10.0 * (err / power).log10();
            let tolerance = case["tolerance_db"].as_f64().unwrap();
            assert!(db < tolerance, "{name}: error {db:.1} dB relative to the reference");
        }
    }

    #[test]
    fn lows_saturate_long_before_highs() {
        // Flux is the integral of voltage: at the same level a 50 Hz tone is
        // deep in saturation while a 2 kHz tone is nowhere near it.
        let low = thd(50.0, 1.5);
        let high = thd(2000.0, 1.5);
        assert!(low > 0.25, "50 Hz at 1.5 saturates the core: THD {low}");
        assert!(high < 0.01, "2 kHz at 1.5 does not: THD {high}");
    }

    #[test]
    fn standing_current_makes_the_saturation_asymmetric() {
        let rate = 48_000.0;
        let period = 600; // 80 Hz
        let second_harmonic = |bias: f32| {
            let mut stage = Stage::new(4.0 * rate);
            let n = period * 12;
            let out: Vec<f64> = (0..n)
                .map(|i| stage.tick((2.0 * PI * i as f64 / period as f64).sin() as f32 * 0.5, bias) as f64)
                .collect();
            let seg = &out[n - period..];
            let bin = |k: usize| {
                let (mut re, mut im) = (0.0, 0.0);
                for (i, y) in seg.iter().enumerate() {
                    let ph = 2.0 * PI * (k * i) as f64 / period as f64;
                    re += y * ph.cos();
                    im += y * ph.sin();
                }
                re.hypot(im)
            };
            bin(2) / bin(1)
        };
        assert!(second_harmonic(0.0) < 1.0e-3, "symmetric without it: {}", second_harmonic(0.0));
        assert!(second_harmonic(1.0) > 0.02, "even harmonics with it: {}", second_harmonic(1.0));
    }

    #[test]
    fn full_scale_noise_at_maximum_drive_stays_bounded() {
        let mut stage = Stage::new(192_000.0);
        let mut seed = 0x1234_5678_u32;
        let mut power = 0.0_f64;
        let n = 48_000;
        for i in 0..n {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let x = (seed as f32 / u32::MAX as f32 - 0.5) * 2.0 * 300.0;
            let y = stage.tick(x, 0.0);
            // The driver cannot swing past its rails; band-limiting a random
            // rail-to-rail wave (the decimation filters) overshoots them.
            assert!(y.abs() < 2.0 * RAIL as f32, "sample {i}: {y} is beyond the driver's rails");
            if i > n / 2 {
                power += (y as f64) * (y as f64);
            }
        }
        let rms = (power / (n / 2) as f64).sqrt();
        assert!(rms > 0.5, "the transformer keeps passing (clipped) signal: rms {rms}");
    }

    #[test]
    fn survives_hostile_input() {
        let mut stage = Stage::new(192_000.0);
        for (i, x) in [f32::NAN, f32::INFINITY, -1.0e9, 1.0e9, 0.0, -3.0].iter().cycle().take(6000).enumerate() {
            let y = stage.tick(*x, if i % 2 == 0 { 1.0 } else { -1.0 });
            assert!(y.is_finite() && y.abs() < 8.0, "{y}");
        }
    }

    #[test]
    fn idle_bypass_parks_a_magnetised_core_silently() {
        let rate = 48_000.0;
        let mut stage = Stage::new(4.0 * rate);
        for i in 0..4_800 {
            stage.tick((2.0 * (2.0 * PI * 50.0 * i as f64 / rate).sin()) as f32, 0.0);
        }
        let mut tail = 0.0_f32;
        for i in 0..(3 * rate as usize) {
            let y = stage.tick(0.0, 0.0);
            if i > rate as usize {
                tail = tail.max(y.abs());
            }
        }
        assert!(tail < 1.0e-6, "silent a second after the signal stops: {tail}");
        assert!(stage.state[..CIRCUIT_LEN].iter().all(|v| *v == 0.0), "parked demagnetised");
    }
}
