//! One 12AX7 common-cathode gain stage, solved as a circuit.
//!
//! This is the `triode` shaper of the Roar engine and the default curve of
//! the Saturator builtin. Unlike the static shapers it is not a transfer
//! curve: it integrates the stage's four capacitors, so the distortion
//! depends on frequency and on what the signal did a moment ago.
//!
//! ```text
//!   e(t) --Rs--||Cin--+--Rgs-- G          Vb
//!                     |                    |
//!                     Rg                   Rp
//!                     |                    |
//!                   Vbias          P ------+--||Cout--+-- out
//!                                          |          RL
//!             K --+--Rk-- gnd              Cl         |
//!                 +--Ck-- gnd              gnd       gnd
//! ```
//!
//! * `Cin` with the grid-leak `Rg`: when the grid is driven positive it
//!   conducts, the current charges `Cin`, and the bias point slides negative
//!   and recovers over ~20 ms (blocking distortion, the "breathing").
//! * `Ck` across the cathode resistor: full gain at audio rates, with the
//!   cathode voltage riding up under sustained level (sag).
//! * `Cout` into the load: removes the plate DC and the DC the asymmetric
//!   clipping generates.
//! * `Cl`, what the plate has to drive (the next stage's Miller capacitance
//!   and strays): rounds the edges of a clipped wave, slower on the cutoff
//!   side where only `Rp` charges it. It is also what keeps those edges from
//!   aliasing.
//!
//! The tube is the Dempwolf-Zoelzer 12AX7 fit ("A physically-motivated triode
//! model for circuit simulations", DAFx-11), which includes grid current:
//!
//! ```text
//!   Ik = G  * (ln(1 + exp(C  * (Vak/mu + Vgk))) / C )^gamma
//!   Ig = Gg * (ln(1 + exp(Cg * Vgk)) / Cg)^xi + Ig0
//!   Ia = Ik - Ig
//! ```
//!
//! Discretization: trapezoidal companion models for the four capacitors
//! reduce the linear network to Thevenin sources at grid, cathode and plate,
//! leaving a 2x2 Newton solve on (Vgk, Vak) per step.
//!
//! The tube laws are read from cubic Hermite tables (`Tables`), and the
//! solve runs at 4x the host rate inside the shared halfband oversampler
//! (`oversample4.rs`).
//!
//! `tools/saturator/triode_ref.py` holds the same circuit twice: this exact
//! algorithm, and a stiff ODE integration it is checked against. The fixtures
//! the tests here assert against come from that ODE solve
//! (`tools/saturator/make_fixtures.py`). Keep the three in sync.

use super::oversample4;

// Tube (Dempwolf-Zoelzer 12AX7).
const TUBE_G: f64 = 2.242e-3;
const TUBE_C: f64 = 3.40;
const TUBE_MU: f64 = 103.2;
const TUBE_GAMMA: f64 = 1.26;
const TUBE_GG: f64 = 6.177e-4;
const TUBE_CG: f64 = 9.901;
const TUBE_XI: f64 = 1.314;
const TUBE_IG0: f64 = 8.025e-8;

// Circuit: the classic first preamp stage, driven from a previous stage's
// output impedance.
const VB: f64 = 300.0;
const RP: f64 = 100e3;
const RK: f64 = 1.5e3;
const CK: f64 = 22e-6;
const RS: f64 = 38e3;
const CIN: f64 = 22e-9;
const RG: f64 = 1e6;
const RGS: f64 = 68e3;
const COUT: f64 = 22e-9;
const RL: f64 = 1e6;
const CL: f64 = 100e-12;
const INV_RP: f64 = 1.0 / RP;
const INV_RK: f64 = 1.0 / RK;
const INV_RG: f64 = 1.0 / RG;
const INV_MU: f64 = 1.0 / TUBE_MU;

// Quiescent point at Vbias = 0 (from `triode_ref.operating_point`). State is
// stored as deviations from it, so an all-zero state is the stage at rest.
const OP_VGK: f64 = -1.517_794_393_080_631_2;
const OP_VAK: f64 = 203.103_450_572_826_46;
const OP_VK: f64 = 1.432_087_306_924_737_7;
const OP_VCIN: f64 = 0.080_250_080_670_312_2;
const OP_VCOUT: f64 = 204.535_537_879_751_2;

/// Midband small-signal voltage gain magnitude of the stage (it inverts).
pub const SMALL_SIGNAL_GAIN: f64 = 62.796_8;
/// Volts at the stage input for a unit-amplitude signal: where the stage
/// starts to compress audibly.
pub const INPUT_VOLTS: f64 = 1.0;
/// Volts of grid-leak return per unit of the bias control. Positive runs the
/// tube hotter (toward grid conduction), negative colder (toward cutoff).
pub const BIAS_VOLTS: f64 = 2.0;
/// The drive is unbounded upstream; past this the stage is fully clipped.
const MAX_INPUT_VOLTS: f64 = 60.0;

const MAX_NEWTON_ITERS: usize = 12;
// Newton step limiting. In cutoff the circuit is linear and a full step is
// exact, however large; the danger is only a step that lands deep in
// conduction, where the currents are exponential in the overshoot. So an
// iterate may not rise more than this above the conduction threshold (or
// above where it already is), and is otherwise free.
const VGK_RISE_LIMIT: f64 = 0.5;
// Roughly where the cathode current's knee sits at the plate voltages the
// stage visits (Vgk = -Vak/mu).
const GRID_KNEE_VGK: f64 = -3.0;
const VAK_MIN: f64 = 0.0;
const VAK_MAX: f64 = 2.0 * VB;
// The extrapolated starting point is only trusted for small moves.
const PREDICT_MAX_VGK: f64 = 0.5;
const PREDICT_MAX_VAK: f64 = 30.0;
// Newton converges quadratically: once a step is this small the next one
// would be around 1e-6 of it, so the solve stops there and applies that
// last step to the currents to first order instead of re-evaluating them.
const NEWTON_TOL_VGK: f64 = 1.0e-3;
const NEWTON_TOL_VAK: f64 = 5.0e-2;


/// Floats of circuit state per channel.
const CIRCUIT_LEN: usize = 12;
const ST_VGK: usize = 0; // deviation from OP_VGK
const ST_VAK: usize = 1; // deviation from OP_VAK
const ST_VCIN: usize = 2; // deviation from OP_VCIN
const ST_ICIN: usize = 3;
const ST_VCK: usize = 4; // deviation from OP_VK
const ST_ICK: usize = 5;
const ST_VCOUT: usize = 6; // deviation from OP_VCOUT
const ST_ICOUT: usize = 7;
const ST_DVGK: usize = 8; // last step's change, the next step's predictor
const ST_DVAK: usize = 9;
const ST_VCL: usize = 10; // plate voltage, deviation from the quiescent one
const ST_ICL: usize = 11;

// A second, identical stage for the twin-triode shaper sits after the first.
const STAGE_B: usize = CIRCUIT_LEN;
const OS: usize = 2 * CIRCUIT_LEN;
/// Floats of runtime state per channel ([`process`], [`process_twin`]).
pub const STATE_LEN: usize = OS + oversample4::STATE_LEN;
pub use oversample4::LATENCY_SAMPLES;

// Tube law tables: value and slope on a uniform grid, cubic Hermite between.
const TABLE_N: usize = 2048;
const CATHODE_S_MIN: f64 = -8.0;
const CATHODE_S_MAX: f64 = 12.0;
const GRID_V_MIN: f64 = -2.0;
const GRID_V_MAX: f64 = 6.0;

struct Tables {
    cathode: Vec<[f64; 2]>,
    grid: Vec<[f64; 2]>,
}

fn tables() -> &'static Tables {
    static TABLES: std::sync::OnceLock<Tables> = std::sync::OnceLock::new();
    TABLES.get_or_init(|| {
        let build = |min: f64, max: f64, f: &dyn Fn(f64) -> (f64, f64)| {
            (0..=TABLE_N)
                .map(|i| {
                    let (value, slope) = f(min + (max - min) * i as f64 / TABLE_N as f64);
                    [value, slope]
                })
                .collect::<Vec<_>>()
        };
        Tables {
            cathode: build(CATHODE_S_MIN, CATHODE_S_MAX, &cathode_law),
            grid: build(GRID_V_MIN, GRID_V_MAX, &grid_law),
        }
    })
}

/// Build the tube tables now, off the audio thread ([`TriodeCoeffs::new`]
/// otherwise builds them on first use).
pub fn warm_tables() {
    tables();
}

/// Cubic Hermite read of a value/slope table; returns (value, slope).
#[inline]
fn hermite(table: &[[f64; 2]], min: f64, max: f64, x: f64) -> (f64, f64) {
    // `min`/`max` are constants at both call sites, so these fold.
    let inv_h = TABLE_N as f64 / (max - min);
    let h = (max - min) / TABLE_N as f64;
    let t = (x - min) * inv_h;
    let i = (t as usize).min(TABLE_N - 1);
    let u = t - i as f64;
    let [f0, d0] = table[i];
    let [f1, d1] = table[i + 1];
    let (d0, d1) = (d0 * h, d1 * h);
    let u2 = u * u;
    let u3 = u2 * u;
    let value = (2.0 * u3 - 3.0 * u2 + 1.0) * f0
        + (u3 - 2.0 * u2 + u) * d0
        + (-2.0 * u3 + 3.0 * u2) * f1
        + (u3 - u2) * d1;
    let slope = ((6.0 * u2 - 6.0 * u) * (f0 - f1)
        + (3.0 * u2 - 4.0 * u + 1.0) * d0
        + (3.0 * u2 - 2.0 * u) * d1)
        * inv_h;
    (value, slope)
}

/// Sample-rate dependent constants of the discretized linear network.
#[derive(Clone, Copy)]
pub struct TriodeCoeffs {
    gcin: f64,
    gck: f64,
    gcout: f64,
    /// Weight of the signal arm and of the bias arm at the grid Thevenin node.
    ka: f64,
    kb: f64,
    rgth: f64,
    zk: f64,
    rpth: f64,
    gcl: f64,
    inv_gcin: f64,
    inv_gck: f64,
    inv_gcout: f64,
    /// Reciprocal of the load arm resistance (RL plus Cout's companion).
    inv_rl: f64,
    tables: &'static Tables,
    quiet_samples: f32,
}

impl TriodeCoeffs {
    /// For [`process`] at `host_rate` (the circuit is solved at 4x that).
    pub fn new(host_rate: f64) -> Self {
        Self::at_solver_rate(host_rate.max(1_000.0) * oversample4::FACTOR)
    }

    fn at_solver_rate(rate: f64) -> Self {
        let gcin = 2.0 * CIN * rate;
        let gck = 2.0 * CK * rate;
        let gcout = 2.0 * COUT * rate;
        let gcl = 2.0 * CL * rate;
        let ra = RS + 1.0 / gcin;
        let rb = ra * RG / (ra + RG);
        let rl = RL + 1.0 / gcout;
        Self {
            gcin,
            gck,
            gcout,
            ka: RG / (ra + RG),
            kb: ra / (ra + RG),
            rgth: rb + RGS,
            zk: 1.0 / (1.0 / RK + gck),
            rpth: 1.0 / (1.0 / RP + 1.0 / rl + gcl),
            gcl,
            inv_gcin: 1.0 / gcin,
            inv_gck: 1.0 / gck,
            inv_gcout: 1.0 / gcout,
            inv_rl: 1.0 / rl,
            tables: tables(),
            quiet_samples: oversample4::quiet_samples(rate / oversample4::FACTOR),
        }
    }
}

/// ln(1 + exp(x)) and its derivative (the logistic function), stable for
/// large |x|.
#[inline]
fn softplus(x: f64) -> (f64, f64) {
    if x > 36.0 {
        (x, 1.0)
    } else if x < -36.0 {
        let e = x.exp();
        (e, e)
    } else {
        let e = x.exp();
        (e.ln_1p(), e / (1.0 + e))
    }
}

/// Cathode current as a function of `s = Vak/mu + Vgk`, and its slope: the
/// derivative with respect to Vgk (with respect to Vak it is that over mu).
fn cathode_law(s: f64) -> (f64, f64) {
    let (sp, sig) = softplus(TUBE_C * s);
    let l = sp / TUBE_C;
    if l <= 0.0 {
        return (0.0, 0.0);
    }
    let p = l.powf(TUBE_GAMMA - 1.0);
    (TUBE_G * p * l, TUBE_G * TUBE_GAMMA * p * sig)
}

/// Grid current and its derivative with respect to Vgk.
fn grid_law(vgk: f64) -> (f64, f64) {
    let (sp, sig) = softplus(TUBE_CG * vgk);
    let l = sp / TUBE_CG;
    if l <= 0.0 {
        return (TUBE_IG0, 0.0);
    }
    let p = l.powf(TUBE_XI - 1.0);
    (TUBE_GG * p * l + TUBE_IG0, TUBE_GG * TUBE_XI * p * sig)
}

#[inline]
fn cathode_current(tables: &Tables, vgk: f64, vak: f64) -> (f64, f64) {
    let s = vak * INV_MU + vgk;
    if s <= CATHODE_S_MIN {
        // Cut off: under 1e-14 A.
        (0.0, 0.0)
    } else if s >= CATHODE_S_MAX {
        cathode_law(s)
    } else {
        hermite(&tables.cathode, CATHODE_S_MIN, CATHODE_S_MAX, s)
    }
}

#[inline]
fn grid_current(tables: &Tables, vgk: f64) -> (f64, f64) {
    if vgk <= GRID_V_MIN {
        // Not conducting: the leakage constant to within 1e-13 A.
        (TUBE_IG0, 0.0)
    } else if vgk >= GRID_V_MAX {
        grid_law(vgk)
    } else {
        hermite(&tables.grid, GRID_V_MIN, GRID_V_MAX, vgk)
    }
}

/// Advance the circuit one solver step with `e` volts at the input and
/// `vbias` volts on the grid-leak return. Returns the output voltage.
///
/// # Safety
/// `st` must point to [`STATE_LEN`] valid floats.
#[inline]
unsafe fn solve_step(c: &TriodeCoeffs, st: *mut f32, e: f64, vbias: f64) -> f64 {
    // Trapezoidal companion sources: v[n] = i[n]/g + (v[n-1] + i[n-1]/g).
    let hin = OP_VCIN + *st.add(ST_VCIN) as f64 + *st.add(ST_ICIN) as f64 * c.inv_gcin;
    let hk = OP_VK + *st.add(ST_VCK) as f64 + *st.add(ST_ICK) as f64 * c.inv_gck;
    let hout = OP_VCOUT + *st.add(ST_VCOUT) as f64 + *st.add(ST_ICOUT) as f64 * c.inv_gcout;
    let eg = c.ka * (e - hin) + c.kb * vbias;
    let vcl_prev = OP_VAK + OP_VK + *st.add(ST_VCL) as f64;
    let hcl = c.gcl * vcl_prev + *st.add(ST_ICL) as f64;
    let ep = c.rpth * (VB * INV_RP + hout * c.inv_rl + hcl);
    let vk0 = c.zk * c.gck * hk;

    // Start from a linear extrapolation of the last two solutions.
    let vgk_prev = OP_VGK + *st.add(ST_VGK) as f64;
    let vak_prev = OP_VAK + *st.add(ST_VAK) as f64;
    let mut vgk =
        vgk_prev + (*st.add(ST_DVGK) as f64).clamp(-PREDICT_MAX_VGK, PREDICT_MAX_VGK);
    let mut vak =
        vak_prev + (*st.add(ST_DVAK) as f64).clamp(-PREDICT_MAX_VAK, PREDICT_MAX_VAK);
    let mut ik = 0.0;
    let mut ig = 0.0;
    for _ in 0..MAX_NEWTON_ITERS {
        let (ik_n, dik) = cathode_current(c.tables, vgk, vak);
        let (ig_n, dig) = grid_current(c.tables, vgk);
        ik = ik_n;
        ig = ig_n;
        let dik_a = dik * INV_MU;
        let f1 = eg - c.rgth * ig - c.zk * ik - vk0 - vgk;
        let f2 = ep - c.rpth * (ik - ig) - c.zk * ik - vk0 - vak;
        let a11 = -c.rgth * dig - c.zk * dik - 1.0;
        let a12 = -c.zk * dik_a;
        let a21 = -c.rpth * (dik - dig) - c.zk * dik;
        let a22 = -c.rpth * dik_a - c.zk * dik_a - 1.0;
        let inv_det = 1.0 / (a11 * a22 - a12 * a21);
        let vgk_next = (vgk - (f1 * a22 - f2 * a12) * inv_det)
            .min(vgk.max(GRID_KNEE_VGK) + VGK_RISE_LIMIT);
        let vak_next = (vak - (a11 * f2 - a21 * f1) * inv_det).clamp(VAK_MIN, VAK_MAX);
        let dv1 = vgk - vgk_next;
        let dv2 = vak - vak_next;
        vgk = vgk_next;
        vak = vak_next;
        if dv1.abs() < NEWTON_TOL_VGK && dv2.abs() < NEWTON_TOL_VAK {
            ik -= dik * dv1 + dik_a * dv2;
            ig -= dig * dv1;
            break;
        }
    }
    if !(vgk.is_finite() && vak.is_finite()) {
        // Never observed; keep the audio thread safe regardless.
        std::ptr::write_bytes(st, 0, CIRCUIT_LEN);
        return 0.0;
    }

    let vk = vk0 + c.zk * ik;
    let vp = vak + vk;
    let vg = vgk + vk;
    let icin = (vg + ig * RGS - vbias) * INV_RG + ig;
    let ick = ik - vk * INV_RK;
    let icout = (vp - hout) * c.inv_rl;
    let vcout = hout + icout * c.inv_gcout;
    *st.add(ST_VCL) = (vp - OP_VAK - OP_VK) as f32;
    *st.add(ST_ICL) = (c.gcl * vp - hcl) as f32;
    *st.add(ST_VGK) = (vgk - OP_VGK) as f32;
    *st.add(ST_VAK) = (vak - OP_VAK) as f32;
    *st.add(ST_DVGK) = (vgk - vgk_prev) as f32;
    *st.add(ST_DVAK) = (vak - vak_prev) as f32;
    *st.add(ST_VCIN) = (hin + icin * c.inv_gcin - OP_VCIN) as f32;
    *st.add(ST_ICIN) = icin as f32;
    *st.add(ST_VCK) = (vk - OP_VK) as f32;
    *st.add(ST_ICK) = ick as f32;
    *st.add(ST_VCOUT) = (vcout - OP_VCOUT) as f32;
    *st.add(ST_ICOUT) = icout as f32;
    vp - vcout
}

/// One host-rate sample through the stage in the unit domain: `x` of
/// amplitude 1 is [`INPUT_VOLTS`] at the input, and the output is scaled and
/// re-inverted so the small-signal gain is +1. `bias` is the -1..1 bias
/// control. The output lags the input by [`LATENCY_SAMPLES`].
///
/// # Safety
/// `st` must point to [`STATE_LEN`] valid floats.
#[inline]
pub unsafe fn process(c: &TriodeCoeffs, st: *mut f32, x: f32, bias: f32) -> f32 {
    let limit = (MAX_INPUT_VOLTS / INPUT_VOLTS) as f32;
    let x = if x.is_finite() { x.clamp(-limit, limit) } else { 0.0 };
    let vbias = bias.clamp(-1.0, 1.0) as f64 * BIAS_VOLTS;
    let scale = -1.0 / (SMALL_SIGNAL_GAIN * INPUT_VOLTS);
    // Idle only at the nominal bias: any other setting rests elsewhere.
    if oversample4::idle(st, STATE_LEN, OS, x, oversample4::bias_at_rest(bias), c.quiet_samples) {
        return 0.0;
    }
    oversample4::process(st.add(OS), x, |v| {
        (solve_step(c, st, v as f64 * INPUT_VOLTS, vbias) * scale) as f32
    })
}

/// Loss between the two stages of the twin: what a passive tone stack or a
/// volume divider costs between gain stages of an amplifier.
pub const TWIN_INTERSTAGE: f64 = 0.1;
/// Volts at the twin's input for a unit-amplitude signal, chosen so the
/// second stage sees what the single stage does at the same setting
/// (`TWIN_INTERSTAGE * SMALL_SIGNAL_GAIN * TWIN_INPUT_VOLTS == INPUT_VOLTS`).
pub const TWIN_INPUT_VOLTS: f64 = INPUT_VOLTS / (TWIN_INTERSTAGE * SMALL_SIGNAL_GAIN);

/// Two of the stages in series, as in a high-gain preamp: the first one's
/// output, attenuated by [`TWIN_INTERSTAGE`], drives the second through its
/// own coupling capacitor and grid resistors. Once the first stage clips, the
/// second one's grid conducts on every peak, its input capacitor charges, and
/// the signal chokes and recovers (blocking distortion). The first stage is
/// treated as a stiff source: `Rs` already stands for its output impedance.
/// Same conventions as [`process`]; the two inversions cancel.
///
/// # Safety
/// `st` must point to [`STATE_LEN`] valid floats.
#[inline]
pub unsafe fn process_twin(c: &TriodeCoeffs, st: *mut f32, x: f32, bias: f32) -> f32 {
    let limit = (MAX_INPUT_VOLTS / TWIN_INPUT_VOLTS) as f32;
    let x = if x.is_finite() { x.clamp(-limit, limit) } else { 0.0 };
    let vbias = bias.clamp(-1.0, 1.0) as f64 * BIAS_VOLTS;
    let scale = 1.0 / (SMALL_SIGNAL_GAIN * SMALL_SIGNAL_GAIN * TWIN_INTERSTAGE * TWIN_INPUT_VOLTS);
    if oversample4::idle(st, STATE_LEN, OS, x, oversample4::bias_at_rest(bias), c.quiet_samples) {
        return 0.0;
    }
    oversample4::process(st.add(OS), x, |v| {
        let first = solve_step(c, st, v as f64 * TWIN_INPUT_VOLTS, vbias);
        (solve_step(c, st.add(STAGE_B), first * TWIN_INTERSTAGE, vbias) * scale) as f32
    })
}

/// Memoryless transfer of [`process_twin`], for the shaper display.
pub fn static_transfer_twin(x: f32) -> f32 {
    let k = (TWIN_INTERSTAGE * SMALL_SIGNAL_GAIN) as f32;
    -static_transfer(-k * static_transfer(x / k))
}

/// Memoryless midband transfer of the stage in the unit domain, for the
/// shaper display and the stateless `shaper_transfer`: the capacitors held at
/// their quiescent voltages, so no bias shift or sag. A rational fit of that
/// solve (`tools/saturator/make_fixtures.py` prints its error), dual
/// maintained with the `roar-shaper` fragment shaders.
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

// Fit constants, see `static_transfer`.
pub const STATIC_POS_LIMIT: f32 = 1.8300;
pub const STATIC_POS_KNEE: f32 = 2.8137;
pub const STATIC_NEG_LIMIT: f32 = 1.3855;
pub const STATIC_NEG_KNEE: f32 = 4.0145;

#[cfg(test)]
mod tests {
    use super::*;

    struct Stage {
        coeffs: TriodeCoeffs,
        state: [f32; STATE_LEN],
    }

    impl Stage {
        /// `rate` is the solver rate for `volts` (four times the host rate
        /// `tick` then runs at).
        fn new(rate: f64) -> Self {
            Self { coeffs: TriodeCoeffs::at_solver_rate(rate), state: [0.0; STATE_LEN] }
        }
        fn volts(&mut self, e: f64, vbias: f64) -> f64 {
            unsafe { solve_step(&self.coeffs, self.state.as_mut_ptr(), e, vbias) }
        }
        /// One host-rate sample through the oversampled stage.
        fn tick(&mut self, x: f32, bias: f32) -> f32 {
            unsafe { process(&self.coeffs, self.state.as_mut_ptr(), x, bias) }
        }
    }

    #[test]
    fn zero_state_is_the_quiescent_point() {
        // The hard-coded operating point is a DC solution of the circuit.
        let (ik, _) = cathode_law(OP_VAK / TUBE_MU + OP_VGK);
        let (ig, _) = grid_law(OP_VGK);
        let vg = OP_VGK + OP_VK;
        let vp = OP_VAK + OP_VK;
        assert!((ik - OP_VK / RK).abs() < 1e-10, "cathode KCL");
        assert!(((VB - vp) / RP - (ik - ig)).abs() < 1e-10, "plate KCL");
        assert!(((0.0 - vg) / (RG + RGS) - ig).abs() < 1e-12, "grid KCL");
        // So silence in is silence out, forever.
        let mut stage = Stage::new(192_000.0);
        for _ in 0..20_000 {
            assert!(stage.volts(0.0, 0.0).abs() < 2e-4);
        }
    }

    #[test]
    fn small_signals_pass_at_unity_gain() {
        let rate = 48_000.0;
        let mut stage = Stage::new(4.0 * rate);
        let n = (rate * 0.2) as usize;
        let mut peak = 0.0_f32;
        let mut dot = 0.0_f64;
        let signal =
            |i: usize| 1.0e-3 * (2.0 * std::f64::consts::PI * 1000.0 * i as f64 / rate).sin();
        for i in 0..n {
            let y = stage.tick(signal(i) as f32, 0.0);
            if i > n - 480 {
                peak = peak.max(y.abs());
                // Against the input LATENCY_SAMPLES ago: in phase, not inverted.
                dot += signal(i - LATENCY_SAMPLES) * y as f64;
            }
        }
        assert!(dot / 240.0 > 0.996e-6, "aligned at LATENCY_SAMPLES: {dot}");
        assert!(dot > 0.0, "the unit-domain stage does not invert");
        assert!((peak / 1.0e-3 - 1.0).abs() < 0.01, "small-signal gain {}", peak / 1.0e-3);
    }

    #[test]
    fn matches_the_ode_reference() {
        // Fixtures: tools/saturator/make_fixtures.py (scipy Radau solve of the
        // same circuit), output volts at 192 kHz.
        let fixtures: serde_json::Value =
            serde_json::from_str(include_str!("triode_fixtures.json")).unwrap();
        let rate = fixtures["rate"].as_f64().unwrap();
        for case in fixtures["cases"].as_array().unwrap() {
            let name = case["name"].as_str().unwrap();
            let input: Vec<f64> =
                case["input"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
            let reference: Vec<f64> =
                case["output"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
            let vbias = case["vbias"].as_f64().unwrap();
            let mut stage = Stage::new(rate);
            let mut err = 0.0;
            let mut power = 0.0;
            for (e, want) in input.iter().zip(&reference) {
                let got = stage.volts(*e, vbias);
                err += (got - want) * (got - want);
                power += want * want;
            }
            let db = 10.0 * (err / power).log10();
            // Per-case tolerance: see make_fixtures.py. Smooth cases match to
            // -60 dB or better; hard-clipped ones are limited by the time
            // step (edge timing), not by the solve.
            let tolerance = case["tolerance_db"].as_f64().unwrap();
            assert!(db < tolerance, "{name}: error {db:.1} dB relative to the reference");
        }
    }

    #[test]
    fn clipping_is_asymmetric_and_bounded() {
        let rate = 48_000.0;
        let mut stage = Stage::new(4.0 * rate);
        let (mut lo, mut hi) = (0.0_f32, 0.0_f32);
        for i in 0..(rate as usize / 5) {
            let x = 40.0 * (2.0 * std::f64::consts::PI * 220.0 * i as f64 / rate).sin();
            let y = stage.tick(x as f32, 0.0);
            assert!(y.is_finite());
            lo = lo.min(y);
            hi = hi.max(y);
        }
        assert!(hi < 3.0 && lo > -3.0, "bounded: {lo}..{hi}");
        assert!((hi + lo).abs() > 0.3, "grid conduction and cutoff clip differently: {lo}..{hi}");
    }

    #[test]
    fn grid_conduction_shifts_the_bias_and_it_recovers() {
        // Blocking distortion: a loud burst charges Cin through the
        // conducting grid, leaving the tube biased colder until Rg bleeds it.
        let rate = 48_000.0;
        let mut stage = Stage::new(4.0 * rate);
        for i in 0..(rate as usize / 10) {
            let x = 20.0 * (2.0 * std::f64::consts::PI * 440.0 * i as f64 / rate).sin();
            stage.tick(x as f32, 0.0);
        }
        let shifted = stage.state[ST_VCIN];
        assert!(shifted > 1.0, "Cin charged by grid current: {shifted} V");
        for _ in 0..(rate as usize / 2) {
            stage.tick(0.0, 0.0);
        }
        assert!(stage.state[ST_VCIN].abs() < 0.05 * shifted, "recovers through the grid leak");
    }

    #[test]
    fn full_scale_noise_at_maximum_drive_stays_physical() {
        // Dense broadband input far past clipping moves the grid tens of
        // volts per solver step; the solve must still converge every step
        // (it once fell behind, and the output collapsed with spikes).
        let mut stage = Stage::new(192_000.0);
        let mut seed = 0x1234_5678_u32;
        let mut power = 0.0_f64;
        let n = 48_000;
        for i in 0..n {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let x = (seed as f32 / u32::MAX as f32 - 0.5) * 2.0 * 60.0;
            let y = stage.tick(x, 0.0);
            // The plate can fall at most its quiescent 204 V (3.3 units);
            // the decimation filters overshoot a clipped edge a little.
            assert!(y.abs() < 4.0, "sample {i}: {y} is outside what the plate can swing");
            if i > n / 2 {
                power += (y as f64) * (y as f64);
            }
        }
        let rms = (power / (n / 2) as f64).sqrt();
        assert!(rms > 0.5, "the stage keeps passing (clipped) signal: rms {rms}");
    }

    fn twin(rate: f64, state: &mut [f32; STATE_LEN], x: f32, bias: f32) -> f32 {
        unsafe { process_twin(&TriodeCoeffs::new(rate), state.as_mut_ptr(), x, bias) }
    }

    #[test]
    fn twin_passes_small_signals_at_unity_gain_in_phase() {
        let rate = 48_000.0;
        let mut state = [0.0; STATE_LEN];
        let n = (rate * 0.2) as usize;
        let signal =
            |i: usize| 1.0e-3 * (2.0 * std::f64::consts::PI * 1000.0 * i as f64 / rate).sin();
        let (mut dot, mut power, mut reference) = (0.0_f64, 0.0_f64, 0.0_f64);
        for i in 0..n {
            let y = twin(rate, &mut state, signal(i) as f32, 0.0) as f64;
            if i >= n - 480 {
                let x = signal(i - LATENCY_SAMPLES);
                dot += x * y;
                power += y * y;
                reference += x * x;
            }
        }
        // Each stage's coupling network costs a fraction of a dB at 1 kHz.
        let gain = (power / reference).sqrt();
        assert!((gain - 1.0).abs() < 0.03, "small-signal gain {gain}");
        assert!(dot / (power * reference).sqrt() > 0.99, "in phase at LATENCY_SAMPLES");
    }

    #[test]
    fn twin_second_stage_blocks_once_the_first_one_clips() {
        // Blocking distortion: the clipped first stage swings the second
        // one's grid far positive every cycle, which charges the second
        // stage's input capacitor and biases it toward cutoff.
        let rate = 48_000.0;
        let shift = |level: f64| {
            let mut state = [0.0; STATE_LEN];
            for i in 0..(rate as usize / 10) {
                let x = level * (2.0 * std::f64::consts::PI * 440.0 * i as f64 / rate).sin();
                let y = twin(rate, &mut state, x as f32, 0.0);
                assert!(y.is_finite() && y.abs() < 4.0, "{y}");
            }
            let charged = state[STAGE_B + ST_VCIN];
            for _ in 0..(rate as usize / 2) {
                twin(rate, &mut state, 0.0, 0.0);
            }
            assert!(state[STAGE_B + ST_VCIN].abs() < 0.05 * charged.abs().max(1.0), "recovers");
            charged
        };
        // Unit level: neither grid conducts. Driven hard: several volts.
        assert!(shift(1.0) < 0.3, "{}", shift(1.0));
        assert!(shift(40.0) > 3.0, "{}", shift(40.0));
    }

    #[test]
    fn twin_static_curve_has_unit_slope_and_stays_bounded() {
        let slope = (static_transfer_twin(0.01) - static_transfer_twin(-0.01)) / 0.02;
        assert!((slope - 1.0).abs() < 0.02, "{slope}");
        // The second stage sees the first one's output inverted, so the two
        // clipping limits trade sides.
        assert!((static_transfer_twin(60.0) - STATIC_NEG_LIMIT).abs() < 0.05);
        assert!((static_transfer_twin(-60.0) + STATIC_POS_LIMIT).abs() < 0.05);
    }

    #[test]
    fn survives_hostile_input() {
        let mut stage = Stage::new(192_000.0);
        for (i, x) in [f32::NAN, f32::INFINITY, -1.0e9, 1.0e9, 0.0, -3.0].iter().cycle().take(6000).enumerate() {
            let y = stage.tick(*x, if i % 2 == 0 { 1.0 } else { -1.0 });
            assert!(y.is_finite() && y.abs() < 8.0, "{y}");
        }
    }
}
