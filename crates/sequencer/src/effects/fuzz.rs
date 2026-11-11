//! A two-transistor germanium fuzz, solved as a circuit.
//!
//! This is the `fuzz` shaper of the Roar engine and of the Saturator builtin:
//! the Fuzz Face topology, drawn NPN with a positive supply. Like the triode
//! (`triode.rs`) it is not a transfer curve; it integrates the circuit's
//! capacitors, so what it does depends on what the signal did a moment ago.
//!
//! ```text
//!                                Vcc ----+-----------+
//!                                        |           |
//!                                       R1          Rt
//!                                        |           +--||Cout--+-- out
//!                                        |          Rc2         RL
//!              +----------Rf-----------------+       |          |
//!              |                         |   |       C2        Vcc
//! e --Rs--||Cin+-- B1   C1 --------------+-- B2
//!                                            E2 --+--Ra--+-- W
//!                  E1 = gnd                              +--Rb-- gnd
//!                                                        +--Ce-- gnd
//! ```
//!
//! (`Rf` runs from E2 back to B1; `Ccb1` sits across C1-B1, `Ccb2` across
//! C2-B2. The output is the voltage across `RL`, which returns to the supply
//! rail: the signal reference of the original positive-ground circuit, and
//! what keeps a move of the bias control from thumping straight into the
//! output.)
//!
//! * Q1 is a shunt-feedback stage: its base is a virtual earth, so the source
//!   resistance `Rs` sets the gain and keeps the stage clean right up to the
//!   point Q2 runs out of room. Then it clips hard, between cutoff and
//!   saturation.
//! * `Cin` and the base junction: a signal big enough to make Q1's base
//!   conduct charges `Cin`, the base sits negative, and Q1 only opens on the
//!   peaks. The wave turns into narrowing pulses, and when the signal stops
//!   the stage stays shut until `Rf` has recharged `Cin` (up to a few tenths
//!   of a second after a very hot signal).
//! * `Ce` bypasses the fuzz pot (`Ra` + `Rb`, fixed at 95 %): full gain above
//!   about 100 Hz, less below.
//! * `Ccb1`, `Ccb2`, the collector-base (Miller) capacitances: the bandwidth
//!   of the slow germanium devices, and what bounds the slope of a clipped
//!   edge.
//! * The bias control is the supply voltage, 9 V at centre: the dying
//!   battery. Starved, Q2 rests in saturation and nothing gets through until
//!   the signal is big enough to pull it out, so notes gate off as they decay.
//!
//! Transistors: Ebers-Moll with both junctions, so cutoff and saturation are
//! both the device's own:
//!
//! ```text
//!   If = Is * (exp(Vbe / Vt) - 1)       Ir = Is * (exp(Vbc / Vt) - 1)
//!   Ic = If - Ir * (1 + 1/Br)           Ib = If / Bf + Ir / Br
//! ```
//!
//! Component and device values are the commonly quoted ones for this
//! topology, written from memory, not taken from a schematic or a datasheet.
//! `Rs`, the fuzz setting and the Miller capacitances are choices.
//!
//! Discretization: trapezoidal companion models for the five capacitors, the
//! linear network inverted once per rate (`Coeffs`), leaving a 4x4 Newton
//! solve on the four junction voltages per step, at 4x the host rate inside
//! the shared oversampler (`oversample4.rs`).
//!
//! `tools/saturator/fuzz_ref.py` holds the same circuit twice: this exact
//! algorithm, and a stiff ODE integration it is checked against. The fixtures
//! the tests here assert against come from that ODE solve. Keep the three in
//! sync.

use super::oversample4;

const VT: f64 = 25.85e-3;
const INV_VT: f64 = 1.0 / VT;

// Transistors (germanium: high saturation current, modest gain).
const Q1_IS: f64 = 5.0e-7;
const Q1_BF: f64 = 70.0;
const Q1_BR: f64 = 10.0;
const Q2_IS: f64 = 5.0e-7;
const Q2_BF: f64 = 110.0;
const Q2_BR: f64 = 10.0;
const JUNCTION_IS: [f64; 4] = [Q1_IS, Q1_IS, Q2_IS, Q2_IS];

// Circuit.
const VCC: f64 = 9.0;
const R1: f64 = 33e3;
const RC2: f64 = 8.2e3;
const RT: f64 = 330.0;
const RF: f64 = 100e3;
// The 1 k fuzz pot at 95 %: Ra from the emitter to the wiper, Rb from the
// wiper to ground, Ce bypassing the wiper.
const RA: f64 = 50.0;
const RB: f64 = 950.0;
const CE: f64 = 20e-6;
const CIN: f64 = 2.2e-6;
const COUT: f64 = 10e-9;
const RL: f64 = 500e3;
// What drives it: about a guitar pickup's winding resistance with its volume
// up. The stage is current driven, so this sets both the gain and how hard
// the base junction is hit; a stiff source would be far harsher.
const RS: f64 = 10e3;
const CCB1: f64 = 22e-12;
const CCB2: f64 = 22e-12;
const INV_RA: f64 = 1.0 / RA;
const INV_RT: f64 = 1.0 / RT;
const INV_RC2: f64 = 1.0 / RC2;

// Quiescent point at 9 V (from `fuzz_ref.operating_point`). State is stored
// as deviations from it, so an all-zero state is the circuit at rest.
const OP_VB1: f64 = 0.160_367_973_038_430_26;
const OP_VC1: f64 = 0.686_869_807_343_097_2;
const OP_VC2: f64 = 4.676_342_000_112_153;
const OP_VE2: f64 = 0.507_953_782_202_034_4;
const OP_VW: f64 = 0.482_556_093_091_932_6;
const OP_VT: f64 = 8.832_730_698_714_773;
// Junction voltages there: Vbe1, Vbc1, Vbe2, Vbc2.
const OP_VN: [f64; 4] = [OP_VB1, OP_VB1 - OP_VC1, OP_VC1 - OP_VE2, OP_VC1 - OP_VC2];
// Capacitor voltages there: Cin (source side minus base), Ce, Cout, Ccb1, Ccb2.
const OP_VCIN: f64 = -OP_VB1;
const OP_VCE: f64 = OP_VW;
const OP_VCOUT: f64 = OP_VT - VCC;
const OP_VCB1: f64 = OP_VC1 - OP_VB1;
const OP_VCB2: f64 = OP_VC2 - OP_VC1;

/// Small-signal voltage gain of the circuit at 1 kHz (it does not invert).
pub const SMALL_SIGNAL_GAIN: f64 = 40.158_8;
/// Volts at the input for a unit-amplitude signal: where it starts to clip.
pub const INPUT_VOLTS: f64 = 8.0e-3;
/// The drive is unbounded upstream; this is already far hotter than any
/// instrument the circuit was built for.
const MAX_INPUT_VOLTS: f64 = 2.0;
/// Supply volts at the ends of the bias control (geometric either side of
/// the nominal 9 V): a flat battery, and twice the voltage for headroom.
pub const SUPPLY_MIN: f64 = 1.0;
pub const SUPPLY_MAX: f64 = 18.0;

const MAX_NEWTON_ITERS: usize = 16;
// Newton step limiting, see `limit_junction`. Upward steps are limited once
// they would land past the knee; downward ones while the junction is
// conducting at all.
const JUNCTION_KNEE: f64 = 0.20;
const JUNCTION_ON: f64 = 0.10;
const DOWN_MIN: f64 = -0.2;
const DOWN_FLOOR: f64 = 0.02;
// The extrapolated starting point is only trusted for small moves.
const PREDICT_MAX: f64 = 0.05;
// Newton converges quadratically: once a step is this small the solve stops
// and applies that last step to the currents to first order.
const NEWTON_TOL: f64 = 1.0e-4;
// Linear continuation of the exponential, never reached by a solution.
const EXP_LIMIT: f64 = 40.0;
// Below this (about -0.47 V) a junction is off: its current is its
// saturation current to within 1e-14 A, and it drops out of the solve.
const EXP_FLOOR: f64 = -18.0;

/// Floats of circuit state per channel.
const CIRCUIT_LEN: usize = 18;
const ST_VN: usize = 0; // 4 junction voltages, deviations from OP_VN
const ST_DVN: usize = 4; // last step's change, the next step's predictor
const ST_VCIN: usize = 8; // capacitor voltages, deviations from OP_*
const ST_ICIN: usize = 9;
const ST_VCE: usize = 10;
const ST_ICE: usize = 11;
const ST_VCOUT: usize = 12;
const ST_ICOUT: usize = 13;
const ST_VCB1: usize = 14;
const ST_ICB1: usize = 15;
const ST_VCB2: usize = 16;
const ST_ICB2: usize = 17;

const OS: usize = CIRCUIT_LEN;
/// Floats of runtime state per channel ([`process`]).
pub const STATE_LEN: usize = OS + oversample4::STATE_LEN;
pub use oversample4::LATENCY_SAMPLES;

// Nodes of the linear network, and the sources that drive it.
const NODES: usize = 6;
const B1: usize = 0;
const C1: usize = 1;
const C2: usize = 2;
const E2: usize = 3;
const W: usize = 4;
const T: usize = 5;
// Sources: e minus Cin's companion, the supply, Cout's companion voltage,
// and the companion currents of Ccb1, Ccb2, Ce.
const SOURCES: usize = 6;

/// Sample-rate dependent constants of the discretized linear network.
#[derive(Clone, Copy)]
pub struct Coeffs {
    /// Junction voltages from the sources, and from the junction currents.
    p: [[f64; SOURCES]; 4],
    k: [[f64; 4]; 4],
    /// Impedance to ground at the wiper and at the output tap.
    zw: f64,
    zt: f64,
    gce: f64,
    gcb1: f64,
    gcb2: f64,
    inv_gcin: f64,
    inv_gcout: f64,
    /// Conductance of the input arm (Rs plus Cin's companion) and of the
    /// load arm (RL plus Cout's companion).
    ga: f64,
    gl: f64,
    quiet_samples: f32,
}

/// Invert a small matrix in place (Gauss-Jordan, partial pivoting). The node
/// matrix is symmetric positive definite, so this cannot fail.
fn invert(a: &mut [[f64; NODES]; NODES]) {
    let mut inv = [[0.0; NODES]; NODES];
    for (i, row) in inv.iter_mut().enumerate() {
        row[i] = 1.0;
    }
    for col in 0..NODES {
        let mut pivot = col;
        for row in col + 1..NODES {
            if a[row][col].abs() > a[pivot][col].abs() {
                pivot = row;
            }
        }
        a.swap(col, pivot);
        inv.swap(col, pivot);
        let scale = 1.0 / a[col][col];
        for j in 0..NODES {
            a[col][j] *= scale;
            inv[col][j] *= scale;
        }
        for row in 0..NODES {
            if row != col {
                let factor = a[row][col];
                if factor != 0.0 {
                    for j in 0..NODES {
                        a[row][j] -= factor * a[col][j];
                        inv[row][j] -= factor * inv[col][j];
                    }
                }
            }
        }
    }
    *a = inv;
}

impl Coeffs {
    /// For [`process`] at `host_rate` (the circuit is solved at 4x that).
    pub fn new(host_rate: f64) -> Self {
        Self::at_solver_rate(host_rate.max(1_000.0) * oversample4::FACTOR)
    }

    fn at_solver_rate(rate: f64) -> Self {
        let gcin = 2.0 * CIN * rate;
        let gce = 2.0 * CE * rate;
        let gcout = 2.0 * COUT * rate;
        let gcb1 = 2.0 * CCB1 * rate;
        let gcb2 = 2.0 * CCB2 * rate;
        let ga = 1.0 / (RS + 1.0 / gcin);
        let gl = 1.0 / (RL + 1.0 / gcout);

        let mut g = [[0.0; NODES]; NODES];
        let mut stamp = |a: usize, b: Option<usize>, cond: f64| {
            g[a][a] += cond;
            if let Some(b) = b {
                g[b][b] += cond;
                g[a][b] -= cond;
                g[b][a] -= cond;
            }
        };
        stamp(B1, None, ga);
        stamp(B1, Some(E2), 1.0 / RF);
        stamp(B1, Some(C1), gcb1);
        stamp(C1, None, 1.0 / R1);
        stamp(C1, Some(C2), gcb2);
        stamp(C2, Some(T), 1.0 / RC2);
        stamp(E2, Some(W), 1.0 / RA);
        stamp(W, None, 1.0 / RB + gce);
        stamp(T, None, 1.0 / RT + gl);
        invert(&mut g);

        // Currents into the nodes from the sources...
        let mut b = [[0.0; SOURCES]; NODES];
        b[B1][0] = ga;
        b[C1][1] = 1.0 / R1;
        b[T][1] = 1.0 / RT + gl; // the load returns to the supply
        b[T][2] = gl;
        b[C1][3] = 1.0; // Ccb1 runs c1 -> b1
        b[B1][3] = -1.0;
        b[C2][4] = 1.0; // Ccb2 runs c2 -> c1
        b[C1][4] = -1.0;
        b[W][5] = 1.0;
        // ...and from the junction currents If1, Ir1, If2, Ir2.
        let mut m = [[0.0; 4]; NODES];
        m[B1][0] = -1.0 / Q1_BF;
        m[B1][1] = -1.0 / Q1_BR;
        m[C1][0] = -1.0;
        m[C1][1] = 1.0 + 1.0 / Q1_BR;
        m[C1][2] = -1.0 / Q2_BF;
        m[C1][3] = -1.0 / Q2_BR;
        m[C2][2] = -1.0;
        m[C2][3] = 1.0 + 1.0 / Q2_BR;
        m[E2][2] = 1.0 + 1.0 / Q2_BF;
        m[E2][3] = -1.0;

        // Node voltages from each.
        let mut node_u = [[0.0; SOURCES]; NODES];
        let mut node_f = [[0.0; 4]; NODES];
        for i in 0..NODES {
            for l in 0..NODES {
                for j in 0..SOURCES {
                    node_u[i][j] += g[i][l] * b[l][j];
                }
                for j in 0..4 {
                    node_f[i][j] += g[i][l] * m[l][j];
                }
            }
        }
        // Junction voltages: Vbe1 = b1, Vbc1 = b1 - c1, Vbe2 = c1 - e2,
        // Vbc2 = c1 - c2.
        let junctions: [(usize, Option<usize>); 4] =
            [(B1, None), (B1, Some(C1)), (C1, Some(E2)), (C1, Some(C2))];
        let mut p = [[0.0; SOURCES]; 4];
        let mut k = [[0.0; 4]; 4];
        for (row, (plus, minus)) in junctions.iter().enumerate() {
            for j in 0..SOURCES {
                p[row][j] = node_u[*plus][j] - minus.map_or(0.0, |n| node_u[n][j]);
            }
            for j in 0..4 {
                k[row][j] = node_f[*plus][j] - minus.map_or(0.0, |n| node_f[n][j]);
            }
        }
        Self {
            p,
            k,
            zw: 1.0 / (INV_RA + 1.0 / RB + gce),
            zt: 1.0 / (INV_RT + INV_RC2 + gl),
            gce,
            gcb1,
            gcb2,
            inv_gcin: 1.0 / gcin,
            inv_gcout: 1.0 / gcout,
            ga,
            gl,
            quiet_samples: oversample4::quiet_samples(rate / oversample4::FACTOR),
        }
    }
}

/// Supply volts for the -1..1 bias control.
#[inline]
fn supply_volts(bias: f32) -> f64 {
    let bias = bias.clamp(-1.0, 1.0) as f64;
    if bias == 0.0 {
        VCC
    } else {
        let end: f64 = if bias < 0.0 { SUPPLY_MIN } else { SUPPLY_MAX };
        VCC * (end / VCC).powf(bias.abs())
    }
}

/// A junction's transport current `Is * (exp(v / Vt) - 1)` and its slope.
#[inline]
fn junction(is: f64, v: f64) -> (f64, f64) {
    let x = v * INV_VT;
    if x < EXP_FLOOR {
        (-is, 0.0)
    } else if x > EXP_LIMIT {
        let e = EXP_LIMIT.exp();
        (is * (e * (1.0 + (x - EXP_LIMIT)) - 1.0), is * e * INV_VT)
    } else {
        let e = x.exp();
        (is * (e - 1.0), is * e * INV_VT)
    }
}

/// Solve the 4x4 system `a * x = b` in place; the solution is left in `b`.
/// Plain elimination first: the diagonal of the Newton matrix is at most -1
/// (a junction's own current always lowers its own voltage), so pivoting is
/// almost never needed, and without it the whole solve is straight-line code.
#[inline]
fn solve4(a: &mut [[f64; 4]; 4], b: &mut [f64; 4]) {
    let (a0, b0) = (*a, *b);
    let mut smallest = f64::INFINITY;
    for col in 0..4 {
        smallest = smallest.min(a[col][col].abs());
        let inv = 1.0 / a[col][col];
        a[col][col] = inv;
        for row in col + 1..4 {
            let factor = a[row][col] * inv;
            for j in col + 1..4 {
                a[row][j] -= factor * a[col][j];
            }
            b[row] -= factor * b[col];
        }
    }
    if !(smallest > 1.0e-3) {
        *a = a0;
        *b = b0;
        return solve4_pivoting(a, b);
    }
    for col in (0..4).rev() {
        let mut acc = b[col];
        for j in col + 1..4 {
            acc -= a[col][j] * b[j];
        }
        b[col] = acc * a[col][col];
    }
}

/// [`solve4`] with partial pivoting, for the rare ill-conditioned step.
#[cold]
fn solve4_pivoting(a: &mut [[f64; 4]; 4], b: &mut [f64; 4]) {
    for col in 0..4 {
        let mut pivot = col;
        for row in col + 1..4 {
            if a[row][col].abs() > a[pivot][col].abs() {
                pivot = row;
            }
        }
        a.swap(col, pivot);
        b.swap(col, pivot);
        let inv = 1.0 / a[col][col];
        a[col][col] = inv;
        for row in col + 1..4 {
            let factor = a[row][col] * inv;
            for j in col + 1..4 {
                a[row][j] -= factor * a[col][j];
            }
            b[row] -= factor * b[col];
        }
    }
    for col in (0..4).rev() {
        let mut acc = b[col];
        for j in col + 1..4 {
            acc -= a[col][j] * b[j];
        }
        b[col] = acc * a[col][col];
    }
}

/// Newton step limiting for a junction voltage (after SPICE's `pnjlim`). A
/// step is the linearization's prediction of a new current; where the
/// junction is exponential, go to the voltage at which it really carries
/// that current instead (`Vt * ln` of the predicted change in current).
/// Upward this stops the overshoot, downward it recovers from one in a step
/// instead of creeping back a `Vt` at a time.
#[inline]
fn limit_junction(next: f64, prev: f64) -> f64 {
    let z = (next - prev) * INV_VT;
    if z > 2.0 && next > JUNCTION_KNEE {
        prev.max(0.0) + VT * (1.0 + (next - prev.max(0.0)) * INV_VT).ln()
    } else if z < DOWN_MIN && z > -1.0 && prev > JUNCTION_ON {
        prev + VT * (1.0 + z).max(DOWN_FLOOR).ln()
    } else {
        next
    }
}

/// Advance the circuit one solver step with `e` volts at the input and a
/// supply of `vcc` volts. Returns the output voltage.
///
/// # Safety
/// `st` must point to [`CIRCUIT_LEN`] valid floats.
#[inline]
unsafe fn solve_step(c: &Coeffs, st: *mut f32, e: f64, vcc: f64) -> f64 {
    // Trapezoidal companions. Series capacitors (Cin, Cout) as a voltage
    // source `h` behind 1/g: v[n] = i[n]/g + (v[n-1] + i[n-1]/g). Shunt ones
    // as a current source `j` beside g: i[n] = g * v[n] - (g * v[n-1] + i[n-1]).
    let hin = OP_VCIN + *st.add(ST_VCIN) as f64 + *st.add(ST_ICIN) as f64 * c.inv_gcin;
    let hout = OP_VCOUT + *st.add(ST_VCOUT) as f64 + *st.add(ST_ICOUT) as f64 * c.inv_gcout;
    let jcb1 = c.gcb1 * (OP_VCB1 + *st.add(ST_VCB1) as f64) + *st.add(ST_ICB1) as f64;
    let jcb2 = c.gcb2 * (OP_VCB2 + *st.add(ST_VCB2) as f64) + *st.add(ST_ICB2) as f64;
    let jce = c.gce * (OP_VCE + *st.add(ST_VCE) as f64) + *st.add(ST_ICE) as f64;
    let u = [e - hin, vcc, hout, jcb1, jcb2, jce];

    let mut p = [0.0; 4];
    let mut vn_prev = [0.0; 4];
    let mut vn = [0.0; 4];
    for i in 0..4 {
        let mut acc = 0.0;
        for j in 0..SOURCES {
            acc += c.p[i][j] * u[j];
        }
        p[i] = acc;
        vn_prev[i] = OP_VN[i] + *st.add(ST_VN + i) as f64;
        // Start from a linear extrapolation of the last two solutions.
        vn[i] = vn_prev[i] + (*st.add(ST_DVN + i) as f64).clamp(-PREDICT_MAX, PREDICT_MAX);
    }

    let k = &c.k;
    let mut f = [0.0; 4];
    for _ in 0..MAX_NEWTON_ITERS {
        let mut df = [0.0; 4];
        for i in 0..4 {
            (f[i], df[i]) = junction(JUNCTION_IS[i], vn[i]);
        }
        // Residual r = p + K f(vn) - vn; Newton's step s solves
        // (K diag(f') - I) s = r.
        let mut r = [0.0; 4];
        for i in 0..4 {
            r[i] = p[i] - vn[i] + k[i][0] * f[0] + k[i][1] * f[1] + k[i][2] * f[2] + k[i][3] * f[3];
        }
        let mut s = [0.0; 4];
        if df[1] == 0.0 && df[3] == 0.0 {
            // Neither collector junction conducts (no transistor is
            // saturated): their columns vanish, leaving a 2x2 system in the
            // two base-emitter junctions and the other two steps explicit.
            let a00 = k[0][0] * df[0] - 1.0;
            let a02 = k[0][2] * df[2];
            let a20 = k[2][0] * df[0];
            let a22 = k[2][2] * df[2] - 1.0;
            let inv_det = 1.0 / (a00 * a22 - a02 * a20);
            s[0] = (r[0] * a22 - a02 * r[2]) * inv_det;
            s[2] = (a00 * r[2] - a20 * r[0]) * inv_det;
            let w0 = df[0] * s[0];
            let w2 = df[2] * s[2];
            s[1] = k[1][0] * w0 + k[1][2] * w2 - r[1];
            s[3] = k[3][0] * w0 + k[3][2] * w2 - r[3];
        } else {
            let mut jac = [[0.0; 4]; 4];
            for i in 0..4 {
                for j in 0..4 {
                    jac[i][j] = k[i][j] * df[j];
                }
                jac[i][i] -= 1.0;
            }
            s = r;
            solve4(&mut jac, &mut s);
        }
        let mut converged = true;
        for i in 0..4 {
            let next = limit_junction(vn[i] - s[i], vn[i]);
            s[i] = vn[i] - next;
            vn[i] = next;
            // A junction that was off and still is has no curvature: its
            // step was exact, however large.
            converged &= s[i].abs() < NEWTON_TOL || (df[i] == 0.0 && next * INV_VT < EXP_FLOOR);
        }
        if converged {
            for i in 0..4 {
                f[i] -= df[i] * s[i];
            }
            break;
        }
    }
    if !vn.iter().all(|v| v.is_finite()) {
        // Never observed; keep the audio thread safe regardless.
        std::ptr::write_bytes(st, 0, CIRCUIT_LEN);
        return 0.0;
    }

    // The junction voltages the linear network has for those currents (the
    // iterate, to within the convergence tolerance squared), then the nodes.
    for i in 0..4 {
        vn[i] = p[i] + k[i][0] * f[0] + k[i][1] * f[1] + k[i][2] * f[2] + k[i][3] * f[3];
    }
    let vb1 = vn[0];
    let vc1 = vb1 - vn[1];
    let ve2 = vc1 - vn[2];
    let vc2 = vc1 - vn[3];
    let vw = (ve2 * INV_RA + jce) * c.zw;
    let vt = (vcc * INV_RT + vc2 * INV_RC2 + (hout + vcc) * c.gl) * c.zt;

    for i in 0..4 {
        *st.add(ST_VN + i) = (vn[i] - OP_VN[i]) as f32;
        *st.add(ST_DVN + i) = (vn[i] - vn_prev[i]) as f32;
    }
    let icin = (e - hin - vb1) * c.ga;
    *st.add(ST_VCIN) = (hin + icin * c.inv_gcin - OP_VCIN) as f32;
    *st.add(ST_ICIN) = icin as f32;
    let icout = (vt - hout - vcc) * c.gl;
    *st.add(ST_VCOUT) = (hout + icout * c.inv_gcout - OP_VCOUT) as f32;
    *st.add(ST_ICOUT) = icout as f32;
    let vcb1 = vc1 - vb1;
    *st.add(ST_VCB1) = (vcb1 - OP_VCB1) as f32;
    *st.add(ST_ICB1) = (c.gcb1 * vcb1 - jcb1) as f32;
    let vcb2 = vc2 - vc1;
    *st.add(ST_VCB2) = (vcb2 - OP_VCB2) as f32;
    *st.add(ST_ICB2) = (c.gcb2 * vcb2 - jcb2) as f32;
    *st.add(ST_VCE) = (vw - OP_VCE) as f32;
    *st.add(ST_ICE) = (c.gce * vw - jce) as f32;
    icout * RL
}

/// One host-rate sample through the circuit in the unit domain: `x` of
/// amplitude 1 is [`INPUT_VOLTS`] at the input, and the output is scaled so
/// the small-signal gain at 1 kHz is +1. `bias` is the -1..1 bias control
/// (the supply voltage). The output lags the input by [`LATENCY_SAMPLES`].
///
/// # Safety
/// `st` must point to [`STATE_LEN`] valid floats.
#[inline]
pub unsafe fn process(c: &Coeffs, st: *mut f32, x: f32, bias: f32) -> f32 {
    let limit = (MAX_INPUT_VOLTS / INPUT_VOLTS) as f32;
    let x = if x.is_finite() { x.clamp(-limit, limit) } else { 0.0 };
    let bias = if bias.is_finite() { bias } else { 0.0 };
    let vcc = supply_volts(bias);
    // The clipped swing is proportional to the supply; make that up so the
    // bias control changes the character, not the level (up to 12 dB: below
    // about 2 V the stage barely passes signal and there is nothing to make up).
    let scale = (VCC / vcc).min(4.0) / (SMALL_SIGNAL_GAIN * INPUT_VOLTS);
    // Idle only at the nominal supply: at any other the circuit rests
    // elsewhere, which the solve has to find.
    if oversample4::idle(st, STATE_LEN, OS, x, oversample4::bias_at_rest(bias), c.quiet_samples) {
        return 0.0;
    }
    oversample4::process(st.add(OS), x, |v| {
        (solve_step(c, st, v as f64 * INPUT_VOLTS, vcc) * scale) as f32
    })
}

/// Memoryless midband transfer of the circuit in the unit domain, for the
/// shaper display and the stateless `shaper_transfer`: the three big
/// capacitors held at their quiescent voltages, so no input-capacitor
/// charging and no gating. A rational fit of that solve
/// (`tools/saturator/fuzz_ref.py` prints its error); the same form as
/// `triode::static_transfer`, so the `roar-shaper` shaders share one formula.
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
pub const STATIC_POS_LIMIT: f32 = 0.5183;
pub const STATIC_POS_KNEE: f32 = 8.8843;
pub const STATIC_NEG_LIMIT: f32 = 0.4932;
pub const STATIC_NEG_KNEE: f32 = 12.0;

#[cfg(test)]
mod tests {
    use super::*;

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
        fn volts(&mut self, e: f64, vcc: f64) -> f64 {
            unsafe { solve_step(&self.coeffs, self.state.as_mut_ptr(), e, vcc) }
        }
        /// One host-rate sample through the oversampled circuit.
        fn tick(&mut self, x: f32, bias: f32) -> f32 {
            unsafe { process(&self.coeffs, self.state.as_mut_ptr(), x, bias) }
        }
    }

    /// (Ic, Ib, Ie) of an Ebers-Moll transistor.
    fn bjt(is: f64, bf: f64, br: f64, vbe: f64, vbc: f64) -> (f64, f64, f64) {
        let (i_f, _) = junction(is, vbe);
        let (i_r, _) = junction(is, vbc);
        let ic = i_f - i_r * (1.0 + 1.0 / br);
        let ib = i_f / bf + i_r / br;
        (ic, ib, ic + ib)
    }

    #[test]
    fn zero_state_is_the_quiescent_point() {
        // The hard-coded operating point is a DC solution of the circuit.
        let (ic1, ib1, _) = bjt(Q1_IS, Q1_BF, Q1_BR, OP_VN[0], OP_VN[1]);
        let (ic2, ib2, ie2) = bjt(Q2_IS, Q2_BF, Q2_BR, OP_VN[2], OP_VN[3]);
        assert!(((OP_VE2 - OP_VB1) / RF - ib1).abs() < 1e-11, "Q1 base KCL");
        assert!(((VCC - OP_VC1) / R1 - ic1 - ib2).abs() < 1e-10, "Q1 collector KCL");
        assert!(((VCC - OP_VC2) / (RC2 + RT) - ic2).abs() < 1e-10, "Q2 collector KCL");
        assert!(
            (ie2 - (OP_VE2 - OP_VB1) / RF - OP_VE2 / (RA + RB)).abs() < 1e-10,
            "Q2 emitter KCL"
        );
        assert!((OP_VW - OP_VE2 * RB / (RA + RB)).abs() < 1e-12, "wiper");
        assert!((OP_VT - (VCC - (VCC - OP_VC2) * RT / (RC2 + RT))).abs() < 1e-12, "tap");
        // So silence in is silence out, forever.
        let mut stage = Stage::new(192_000.0);
        for _ in 0..20_000 {
            assert!(stage.volts(0.0, VCC).abs() < 1e-6);
        }
    }

    #[test]
    fn small_signals_pass_at_unity_gain() {
        let rate = 48_000.0;
        let mut stage = Stage::new(4.0 * rate);
        let n = (rate * 0.4) as usize;
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
        assert!(dot / 240.0 > 0.99e-6, "aligned at LATENCY_SAMPLES: {dot}");
        assert!((peak / 1.0e-3 - 1.0).abs() < 0.01, "small-signal gain {}", peak / 1.0e-3);
    }

    #[test]
    fn matches_the_ode_reference() {
        // Fixtures: tools/saturator/fuzz_ref.py (scipy Radau solve of the
        // same circuit), output volts at 192 kHz. Each case starts at rest
        // at 9 V and switches to its own supply at the first sample.
        let fixtures: serde_json::Value =
            serde_json::from_str(include_str!("fuzz_fixtures.json")).unwrap();
        let rate = fixtures["rate"].as_f64().unwrap();
        for case in fixtures["cases"].as_array().unwrap() {
            let name = case["name"].as_str().unwrap();
            let input: Vec<f64> =
                case["input"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
            let reference: Vec<f64> =
                case["output"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
            let vcc = case["vcc"].as_f64().unwrap();
            let mut stage = Stage::new(rate);
            let mut err = 0.0;
            let mut power = 0.0;
            for (e, want) in input.iter().zip(&reference) {
                let got = stage.volts(*e, vcc);
                err += (got - want) * (got - want);
                power += want * want;
            }
            let db = 10.0 * (err / power).log10();
            // Per-case tolerance: see fuzz_ref.py. Clipped cases are limited
            // by the time step (edge timing), not by the solve.
            let tolerance = case["tolerance_db"].as_f64().unwrap();
            assert!(db < tolerance, "{name}: error {db:.1} dB relative to the reference");
        }
    }

    #[test]
    fn clips_hard_between_cutoff_and_saturation() {
        let rate = 48_000.0;
        let mut stage = Stage::new(4.0 * rate);
        let (mut lo, mut hi) = (0.0_f32, 0.0_f32);
        let n = rate as usize / 5;
        let mut clean = 0.0_f32;
        for i in 0..n {
            let s = (2.0 * std::f64::consts::PI * 1000.0 * i as f64 / rate).sin();
            let y = stage.tick((4.0 * s) as f32, 0.0);
            assert!(y.is_finite());
            if i > n / 2 {
                lo = lo.min(y);
                hi = hi.max(y);
            }
        }
        let mut stage = Stage::new(4.0 * rate);
        for i in 0..n {
            let s = (2.0 * std::f64::consts::PI * 1000.0 * i as f64 / rate).sin();
            let y = stage.tick((0.1 * s) as f32, 0.0);
            if i > n / 2 {
                clean = clean.max(y.abs());
            }
        }
        // Below the clip point the shunt feedback keeps it linear...
        assert!((clean / 0.1 - 1.0).abs() < 0.08, "clean gain {}", clean / 0.1);
        // ...and 40x more input does not get 2x the swing Q2 has.
        assert!(hi < 1.0 && lo > -1.0 && hi - lo > 0.8, "clipped swing {lo}..{hi}");
    }

    #[test]
    fn a_starved_supply_gates_a_decaying_note() {
        // Sustain and tail of the same decaying note, at the nominal supply
        // and at the bottom of the bias control. Starved, Q2 rests saturated
        // and the tail is cut off instead of fading.
        let rate = 48_000.0;
        let tail_over_sustain = |bias: f32| {
            let mut stage = Stage::new(4.0 * rate);
            // Let the supply change settle first.
            for _ in 0..(rate as usize) {
                stage.tick(0.0, bias);
            }
            let window = (rate * 0.05) as usize;
            let (mut sustain, mut tail) = (0.0_f64, 0.0_f64);
            for i in 0..(rate * 0.3) as usize {
                let t = i as f64 / rate;
                let x = 12.0 * (-t / 0.06).exp() * (2.0 * std::f64::consts::PI * 220.0 * t).sin();
                let y = stage.tick(x as f32, bias) as f64;
                if i < window {
                    sustain += y * y;
                } else if i >= 4 * window && i < 5 * window {
                    // The input is 28 dB down here, still above the clip
                    // point of the healthy circuit's small-signal gain.
                    tail += y * y;
                }
            }
            (tail / sustain).sqrt()
        };
        let healthy = tail_over_sustain(0.0);
        let starved = tail_over_sustain(-1.0);
        assert!(healthy > 0.25, "at 9 V the tail is still there: {healthy}");
        assert!(starved < 0.03, "starved, the tail is gated off: {starved}");
    }

    #[test]
    fn full_scale_noise_at_maximum_drive_stays_physical() {
        // Dense broadband input far past clipping; the solve must still
        // converge every step.
        for bias in [0.0_f32, -1.0, 1.0] {
            let mut stage = Stage::new(192_000.0);
            let mut seed = 0x1234_5678_u32;
            let mut power = 0.0_f64;
            let n = 48_000;
            for i in 0..n {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let x = (seed as f32 / u32::MAX as f32 - 0.5) * 2.0 * 250.0;
                let y = stage.tick(x, bias);
                // The tap can move at most Rt times Q2's saturated current
                // (0.34 V at 9 V, 1.1 units; twice that at 18 V), plus what
                // the output capacitor's DC shift and the decimation
                // filters' overshoot add.
                assert!(y.abs() < 3.0, "bias {bias} sample {i}: {y} is outside what the tap can swing");
                if i > n / 2 {
                    power += (y as f64) * (y as f64);
                }
            }
            let rms = (power / (n / 2) as f64).sqrt();
            if bias >= 0.0 {
                assert!(rms > 0.1, "bias {bias}: keeps passing (clipped) signal: rms {rms}");
            }
        }
    }

    #[test]
    fn survives_hostile_input() {
        let mut stage = Stage::new(192_000.0);
        let inputs = [f32::NAN, f32::INFINITY, -1.0e9, 1.0e9, 0.0, -3.0];
        for (i, x) in inputs.iter().cycle().take(6000).enumerate() {
            let bias = match i % 4 {
                0 => 1.0,
                1 => -1.0,
                2 => f32::NAN,
                _ => 0.3,
            };
            let y = stage.tick(*x, bias);
            assert!(y.is_finite() && y.abs() < 8.0, "{y}");
        }
    }
}
