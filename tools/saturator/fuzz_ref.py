"""Reference model of the Saturator "fuzz" curve: a two-transistor germanium
fuzz (the Fuzz Face topology), drawn NPN with a positive supply.

                                   Vcc ----+-----------+
                                           |           |
                                          R1          Rt
                                           |           +--||Cout--+-- out
                                           |          Rc2         RL
                 +----------Rf-----------------+       |          |
                 |                         |   |       C2        gnd
    e --Rs--||Cin+-- B1   C1 --------------+-- B2
                                               E2 --+--Ra--+-- W
                     E1 = gnd                              +--Rb-- gnd
                                                           +--Ce-- gnd
    (Rf runs from E2 back to B1; Ccb1 across C1-B1, Ccb2 across C2-B2. The
    output is the voltage across RL, which returns to the supply rail: the
    signal reference of the original positive-ground circuit, and what keeps a
    change of supply voltage from coupling straight into the output.)

Transistors: Ebers-Moll, both junctions (transport form):

    If = Is * (exp(Vbe / Vt) - 1)       Ir = Is * (exp(Vbc / Vt) - 1)
    Ic = If - Ir * (1 + 1/Br)           Ib = If / Bf + Ir / Br

Two independent solvers of the same circuit live here:
  * `solve_reference`: stiff ODE integration (scipy Radau) on the capacitor
    voltages, the memoryless part solved by its own 2-unknown Newton.
  * `FuzzStage`: the discrete algorithm crates/sequencer/src/effects/fuzz.rs
    implements (trapezoidal companions, nodal reduction to the four junction
    voltages, 4x4 Newton). Keep the two in sync.

    python3 tools/saturator/fuzz_ref.py            # write fixtures, fit curve
    python3 tools/saturator/fuzz_ref.py explore    # operating point, gains
    python3 tools/saturator/fuzz_ref.py check      # twin against the fixtures

Component and device values are the commonly quoted ones for this topology,
written from memory, not from a schematic or a datasheet.
"""
import json
import math
import os
import sys

import numpy as np
from scipy.integrate import solve_ivp
from scipy.optimize import fsolve, least_squares

VT = 25.85e-3

CIRCUIT = dict(
    Vcc=9.0, R1=33e3, Rc2=8.2e3, Rt=330.0, Rf=100e3,
    # The 1 k fuzz pot at 95 %: Ra from the emitter to the wiper, Rb from the
    # wiper to ground, Ce bypassing the wiper.
    Ra=50.0, Rb=950.0, Ce=20e-6,
    Cin=2.2e-6, Cout=10e-9, RL=500e3,
    # What drives it: a guitar pickup's winding resistance with the volume
    # up. The stage is current driven (its input is a virtual earth), so this
    # resistor sets both the gain and how hard the base junction is hit.
    Rs=10e3,
    # Collector-base capacitances (Miller): what limits the bandwidth and the
    # edge rate of the slow germanium devices.
    Ccb1=22e-12, Ccb2=22e-12,
)
Q1 = dict(Is=5e-7, Bf=70.0, Br=10.0)
Q2 = dict(Is=5e-7, Bf=110.0, Br=10.0)

# Unit domain (see fuzz.rs).
INPUT_VOLTS = 8.0e-3
MAX_INPUT_VOLTS = 2.0
# The bias control starves or feeds the supply, geometrically.
SUPPLY_MIN = 1.0    # volts at bias -1
SUPPLY_MAX = 18.0   # volts at bias +1

N_NODES = 6  # b1, c1, c2, e2, w, t
B1, C1, C2, E2, W, T = range(N_NODES)


def supply(bias, c=CIRCUIT):
    """Supply volts for the -1..1 bias control."""
    bias = min(max(bias, -1.0), 1.0)
    end = SUPPLY_MIN if bias < 0 else SUPPLY_MAX
    return c["Vcc"] * (end / c["Vcc"]) ** abs(bias)


def expm1_lim(x):
    """exp(x) - 1 with a linear continuation past x = 40 (never reached by a
    converged solution; keeps iterates finite)."""
    if x > 40.0:
        e = math.exp(40.0)
        return e * (1.0 + (x - 40.0)) - 1.0
    return math.expm1(x)


def bjt(vbe, vbc, q):
    """(Ic, Ib, Ie) into collector, into base, out of emitter."""
    i_f = q["Is"] * expm1_lim(vbe / VT)
    i_r = q["Is"] * expm1_lim(vbc / VT)
    ic = i_f - i_r * (1.0 + 1.0 / q["Br"])
    ib = i_f / q["Bf"] + i_r / q["Br"]
    return ic, ib, ic + ib


# ---------------------------------------------------------------------------
# Reference: ODE on the five capacitor voltages.
#   vcin = V(after Rs) - V(b1), vce = V(w), vcout = V(t) - V(out),
#   vcb1 = V(c1) - V(b1), vcb2 = V(c2) - V(c1)
# ---------------------------------------------------------------------------

def _memoryless(e, vcc, y, guess, c):
    vcin, vce, vcout, vcb1, vcb2 = y

    def resid(vb1, ve2):
        vc1 = vb1 + vcb1
        vc2 = vc1 + vcb2
        vt = (vcc / c["Rt"] + vc2 / c["Rc2"] + (vcout + vcc) / c["RL"]) / (
            1.0 / c["Rt"] + 1.0 / c["Rc2"] + 1.0 / c["RL"])
        ic1, ib1, _ = bjt(vb1, -vcb1, Q1)
        ic2, ib2, ie2 = bjt(vc1 - ve2, -vcb2, Q2)
        i_cb2 = (vt - vc2) / c["Rc2"] - ic2
        i_cb1 = (vcc - vc1) / c["R1"] + i_cb2 - ic1 - ib2
        i_cin = (e - vcin - vb1) / c["Rs"]
        r1 = i_cin + (ve2 - vb1) / c["Rf"] + i_cb1 - ib1
        r2 = ie2 - (ve2 - vb1) / c["Rf"] - (ve2 - vce) / c["Ra"]
        i_ce = (ve2 - vce) / c["Ra"] - vce / c["Rb"]
        i_cout = (vt - vcout - vcc) / c["RL"]
        return r1, r2, (i_cin, i_ce, i_cout, i_cb1, i_cb2), vt

    vb1, ve2 = guess
    for _ in range(200):
        r1, r2, _, _ = resid(vb1, ve2)
        h = 1e-7
        a1, a2, _, _ = resid(vb1 + h, ve2)
        b1, b2, _, _ = resid(vb1, ve2 + h)
        j11, j21 = (a1 - r1) / h, (a2 - r2) / h
        j12, j22 = (b1 - r1) / h, (b2 - r2) / h
        det = j11 * j22 - j12 * j21
        d1 = (r1 * j22 - r2 * j12) / det
        d2 = (j11 * r2 - j21 * r1) / det
        # junction-style damping
        d1 = max(min(d1, 0.1), -0.1)
        d2 = max(min(d2, 0.1), -0.1)
        vb1 -= d1
        ve2 -= d2
        if abs(d1) < 1e-13 and abs(d2) < 1e-13:
            break
    r1, r2, currents, vt = resid(vb1, ve2)
    return (vb1, ve2), currents, vt


def operating_point(vcc=None, c=CIRCUIT):
    """DC solution (caps open). Returns node and capacitor voltages."""
    vcc = c["Vcc"] if vcc is None else vcc

    def resid(u):
        vb1, vc1, vc2, ve2 = u
        ic1, ib1, _ = bjt(vb1, vb1 - vc1, Q1)
        ic2, ib2, ie2 = bjt(vc1 - ve2, vc1 - vc2, Q2)
        return [((ve2 - vb1) / c["Rf"] - ib1) * 1e6,
                ((vcc - vc1) / c["R1"] - ic1 - ib2) * 1e4,
                ((vcc - vc2) / (c["Rc2"] + c["Rt"]) - ic2) * 1e4,
                (ie2 - (ve2 - vb1) / c["Rf"] - ve2 / (c["Ra"] + c["Rb"])) * 1e4]

    best = None
    for guess in ([0.15, 0.7, 0.5 * vcc, 0.5], [0.15, 0.4, 0.2, 0.2], [0.1, 0.3, 0.1, 0.1]):
        sol, _, ok, _ = fsolve(resid, guess, xtol=1e-14, full_output=True)
        if ok == 1 and max(abs(r) for r in resid(sol)) < 1e-6:
            best = sol
            break
    assert best is not None, "no operating point"
    vb1, vc1, vc2, ve2 = best
    vw = ve2 * c["Rb"] / (c["Ra"] + c["Rb"])
    vt = vcc - (vcc - vc2) * c["Rt"] / (c["Rc2"] + c["Rt"])
    return dict(vb1=vb1, vc1=vc1, vc2=vc2, ve2=ve2, vw=vw, vt=vt,
                vcin=-vb1, vce=vw, vcout=vt - vcc, vcb1=vc1 - vb1, vcb2=vc2 - vc1)


def solve_reference(e_fn, t_end, fs_out, vcc=None, c=CIRCUIT, rtol=1e-8, atol=1e-11,
                    start_vcc=None):
    """`start_vcc`: supply the circuit is at rest with before t = 0 (default
    `vcc`). The engine starts at rest one sample before its first input, and
    a sampled solver's inputs are straight lines between samples; so the
    integration starts at t = -1/fs_out with e = 0 and `start_vcc`, and both
    ramp linearly to their t = 0 values."""
    vcc = c["Vcc"] if vcc is None else vcc
    vcc0 = vcc if start_vcc is None else start_vcc
    op = operating_point(vcc0, c)
    guess = [(op["vb1"], op["ve2"])]
    e0 = float(e_fn(0.0))

    def inputs(t):
        if t >= 0.0:
            return float(e_fn(t)), vcc
        ramp = 1.0 + t * fs_out
        return e0 * ramp, vcc0 + (vcc - vcc0) * ramp

    def rhs(t, y):
        e, v = inputs(t)
        guess[0], (i_cin, i_ce, i_cout, i_cb1, i_cb2), _ = _memoryless(e, v, y, guess[0], c)
        return [i_cin / c["Cin"], i_ce / c["Ce"], i_cout / c["Cout"],
                i_cb1 / c["Ccb1"], i_cb2 / c["Ccb2"]]

    n = int(round(t_end * fs_out))
    t_eval = np.arange(n) / fs_out
    y0 = [op["vcin"], op["vce"], op["vcout"], op["vcb1"], op["vcb2"]]
    lead = solve_ivp(rhs, (-1.0 / fs_out, 0.0), y0, method="Radau", rtol=rtol, atol=atol,
                     max_step=0.02 / fs_out)
    sol = solve_ivp(rhs, (0.0, t_eval[-1]), lead.y[:, -1], method="Radau", t_eval=t_eval,
                    rtol=rtol, atol=atol, max_step=0.25 / fs_out)
    out = np.empty(n)
    g = (op["vb1"], op["ve2"])
    for k in range(n):
        g, _, vt = _memoryless(float(e_fn(t_eval[k])), vcc, sol.y[:, k], g, c)
        out[k] = vt - sol.y[2, k] - vcc
    return t_eval, out, sol.y


# ---------------------------------------------------------------------------
# Discrete twin of fuzz.rs.
# ---------------------------------------------------------------------------

NEWTON_TOL = 1.0e-4
MAX_ITERS = 16
PREDICT_MAX = 0.05
EXP_LIMIT = 40.0     # linear continuation of the exponential past this
EXP_FLOOR = -18.0    # a junction below this (in Vt) is off and has no slope
JUNCTION_KNEE = 0.20
JUNCTION_ON = 0.10
DOWN_MIN = -0.2
DOWN_FLOOR = 0.02


def limit_junction(nxt, prev):
    """Newton step limiting in the current domain (after SPICE's pnjlim)."""
    z = (nxt - prev) / VT
    if z > 2.0 and nxt > JUNCTION_KNEE:
        base = max(prev, 0.0)
        return base + VT * math.log(1.0 + (nxt - base) / VT)
    if DOWN_MIN > z > -1.0 and prev > JUNCTION_ON:
        return prev + VT * math.log(max(1.0 + z, DOWN_FLOOR))
    return nxt


class FuzzStage:
    """Discrete twin of crates/sequencer/src/effects/fuzz.rs (keep in sync)."""

    def __init__(self, fs, vcc=None, c=CIRCUIT, start_vcc=None, iters=MAX_ITERS):
        self.c, self.iters = c, iters
        self.vcc = c["Vcc"] if vcc is None else vcc
        self.gcin = 2 * c["Cin"] * fs
        self.gce = 2 * c["Ce"] * fs
        self.gcout = 2 * c["Cout"] * fs
        self.gcb1 = 2 * c["Ccb1"] * fs
        self.gcb2 = 2 * c["Ccb2"] * fs
        self.ga = 1.0 / (c["Rs"] + 1.0 / self.gcin)
        self.gl = 1.0 / (c["RL"] + 1.0 / self.gcout)
        g = np.zeros((N_NODES, N_NODES))

        def stamp(a, b, cond):
            g[a, a] += cond
            if b is not None:
                g[b, b] += cond
                g[a, b] -= cond
                g[b, a] -= cond

        stamp(B1, None, self.ga)
        stamp(B1, E2, 1 / c["Rf"])
        stamp(B1, C1, self.gcb1)
        stamp(C1, None, 1 / c["R1"])
        stamp(C1, C2, self.gcb2)
        stamp(C2, T, 1 / c["Rc2"])
        stamp(E2, W, 1 / c["Ra"])
        stamp(W, None, 1 / c["Rb"] + self.gce)
        stamp(T, None, 1 / c["Rt"] + self.gl)
        ginv = np.linalg.inv(g)
        # sources u = [e - hin, Vcc, hout, Jcb1, Jcb2, Jce]
        b = np.zeros((N_NODES, 6))
        b[B1, 0] = self.ga
        b[C1, 1] = 1 / c["R1"]
        b[T, 1] = 1 / c["Rt"]
        b[T, 1] += self.gl                 # the load returns to the supply
        b[T, 2] = self.gl
        b[C1, 3], b[B1, 3] = 1.0, -1.0     # Ccb1: c1 -> b1
        b[C2, 4], b[C1, 4] = 1.0, -1.0     # Ccb2: c2 -> c1
        b[W, 5] = 1.0
        # junction currents f = [If1, Ir1, If2, Ir2] injected into the nodes
        m = np.zeros((N_NODES, 4))
        m[B1, 0], m[B1, 1] = -1 / Q1["Bf"], -1 / Q1["Br"]
        m[C1, 0], m[C1, 1] = -1.0, 1 + 1 / Q1["Br"]
        m[C1, 2], m[C1, 3] = -1 / Q2["Bf"], -1 / Q2["Br"]
        m[C2, 2], m[C2, 3] = -1.0, 1 + 1 / Q2["Br"]
        m[E2, 2], m[E2, 3] = 1 + 1 / Q2["Bf"], -1.0
        # junction voltages vn = n^T v: vbe1, vbc1, vbe2, vbc2
        nt = np.zeros((4, N_NODES))
        nt[0, B1] = 1.0
        nt[1, B1], nt[1, C1] = 1.0, -1.0
        nt[2, C1], nt[2, E2] = 1.0, -1.0
        nt[3, C1], nt[3, C2] = 1.0, -1.0
        self.vu = ginv @ b       # node voltages from the sources
        self.vf = ginv @ m       # node voltages from the junction currents
        self.p = nt @ self.vu    # junction voltages from the sources
        self.k = nt @ self.vf    # junction voltages from the junction currents
        self.isat = np.array([Q1["Is"], Q1["Is"], Q2["Is"], Q2["Is"]])
        op = operating_point(self.vcc if start_vcc is None else start_vcc, c)
        self.vn = np.array([op["vb1"], op["vb1"] - op["vc1"], op["vc1"] - op["ve2"],
                            op["vc1"] - op["vc2"]])
        self.dvn = np.zeros(4)
        self.vcin, self.icin = op["vcin"], 0.0
        self.vce, self.ice = op["vce"], 0.0
        self.vcout, self.icout = op["vcout"], 0.0
        self.vcb1, self.icb1 = op["vcb1"], 0.0
        self.vcb2, self.icb2 = op["vcb2"], 0.0
        self.iter_hist = np.zeros(iters + 1, dtype=int)

    @staticmethod
    def currents(vn, isat):
        x = vn / VT
        lim = x > EXP_LIMIT
        off = x < EXP_FLOOR
        ex = np.exp(np.minimum(x, EXP_LIMIT))
        f = isat * (np.where(lim, ex * (1.0 + (x - EXP_LIMIT)), ex) - 1.0)
        df = isat * ex / VT
        return np.where(off, -isat, f), np.where(off, 0.0, df)

    def step(self, e):
        hin = self.vcin + self.icin / self.gcin
        hout = self.vcout + self.icout / self.gcout
        jcb1 = self.gcb1 * self.vcb1 + self.icb1
        jcb2 = self.gcb2 * self.vcb2 + self.icb2
        jce = self.gce * self.vce + self.ice
        u = np.array([e - hin, self.vcc, hout, jcb1, jcb2, jce])
        p = self.p @ u
        vn_prev = self.vn
        vn = vn_prev + np.clip(self.dvn, -PREDICT_MAX, PREDICT_MAX)
        f = None
        for it in range(self.iters):
            f, df = self.currents(vn, self.isat)
            resid = p + self.k @ f - vn
            jac = self.k * df[None, :] - np.eye(4)
            step = np.linalg.solve(jac, resid)
            vn_next = np.array([limit_junction(a - s, a) for a, s in zip(vn, step)])
            dv = vn - vn_next
            vn = vn_next
            # A junction that was off and still is has no curvature: its
            # step was exact, however large.
            if np.all((np.abs(dv) < NEWTON_TOL) | ((df == 0.0) & (vn / VT < EXP_FLOOR))):
                f = f - df * dv
                break
        self.iter_hist[it + 1] += 1
        # The junction and node voltages the linear network has for those
        # currents.
        vn = p + self.k @ f
        v = self.vu @ u + self.vf @ f
        self.dvn = vn - vn_prev
        self.vn = vn
        icin = (e - hin - v[B1]) * self.ga
        self.vcin, self.icin = hin + icin / self.gcin, icin
        icout = (v[T] - hout - self.vcc) * self.gl
        self.vcout, self.icout = hout + icout / self.gcout, icout
        vcb1 = v[C1] - v[B1]
        self.vcb1, self.icb1 = vcb1, self.gcb1 * vcb1 - jcb1
        vcb2 = v[C2] - v[C1]
        self.vcb2, self.icb2 = vcb2, self.gcb2 * vcb2 - jcb2
        self.vce, self.ice = v[W], self.gce * v[W] - jce
        return icout * self.c["RL"]

    def run(self, e):
        return np.array([self.step(x) for x in e])


# ---------------------------------------------------------------------------
# Measurements, fixtures, display curve.
# ---------------------------------------------------------------------------

def small_signal_gain(freq, fs=192000, vcc=None, amp=1e-6, c=CIRCUIT):
    st = FuzzStage(fs, vcc=vcc, c=c)
    n = int(fs * max(0.25, 12.0 / freq))
    t = np.arange(n) / fs
    y = st.run(amp * np.sin(2 * np.pi * freq * t))
    m = int(fs * max(1, round(freq * 0.05)) / freq)  # whole cycles
    seg, ts = y[-m:], t[-m:]
    ref = np.exp(-2j * np.pi * freq * ts)
    return 2 * np.mean(seg * ref) / amp * 1j  # complex gain re sin


def static_curve(xs, c=CIRCUIT, input_volts=INPUT_VOLTS):
    """Memoryless midband transfer in the unit domain: Cin, Ce, Cout held at
    their quiescent voltages (AC shorts), the Miller capacitors open."""
    op = operating_point(None, c)
    vcc = c["Vcc"]
    out = []
    guess = [op["vb1"], op["vc1"], op["vc2"], op["ve2"]]
    order = np.argsort(np.abs(xs))  # continue outward from the origin
    res = np.empty(len(xs))
    for sign in (1, -1):
        guess = [op["vb1"], op["vc1"], op["vc2"], op["ve2"]]
        for i in order:
            x = xs[i]
            if (x >= 0) != (sign > 0):
                continue
            e = x * input_volts

            def resid(u):
                vb1, vc1, vc2, ve2 = u
                ic1, ib1, _ = bjt(vb1, vb1 - vc1, Q1)
                ic2, ib2, ie2 = bjt(vc1 - ve2, vc1 - vc2, Q2)
                vt = (vcc / c["Rt"] + vc2 / c["Rc2"] + (op["vcout"] + vcc) / c["RL"]) / (
                    1 / c["Rt"] + 1 / c["Rc2"] + 1 / c["RL"])
                return [((e - op["vcin"] - vb1) / c["Rs"] + (ve2 - vb1) / c["Rf"] - ib1) * 1e5,
                        ((vcc - vc1) / c["R1"] - ic1 - ib2) * 1e4,
                        ((vt - vc2) / c["Rc2"] - ic2) * 1e4,
                        (ie2 - (ve2 - vb1) / c["Rf"] - (ve2 - op["vce"]) / c["Ra"]) * 1e3]

            sol = fsolve(resid, guess, xtol=1e-14)
            guess = list(sol)
            vc2 = sol[2]
            vt = (vcc / c["Rt"] + vc2 / c["Rc2"] + (op["vcout"] + vcc) / c["RL"]) / (
                1 / c["Rt"] + 1 / c["Rc2"] + 1 / c["RL"])
            res[i] = vt - op["vcout"] - vcc
    i0 = np.argmin(np.abs(xs))
    gain = (res[i0 + 1] - res[i0 - 1]) / (xs[i0 + 1] - xs[i0 - 1])
    return (res - res[i0]) / gain


def explore():
    for vcc in (9.0, 18.0, 6.0, 4.0, 3.0, 2.0, 1.5):
        op = operating_point(vcc)
        print(f"Vcc {vcc:5.2f}: " + " ".join(f"{k}={op[k]:.4f}" for k in ("vb1", "vc1", "vc2", "ve2", "vt")))
    print("small-signal gain at the output")
    for f in (20, 50, 100, 300, 1000, 3000, 5000, 10000, 20000):
        g = small_signal_gain(f)
        print(f"  {f:6d} Hz: {abs(g):8.3f}  ({20 * np.log10(abs(g)):6.2f} dB, {np.degrees(np.angle(g)):7.1f} deg)")


RATE = 192000
HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(HERE, "../../crates/sequencer/src/effects/fuzz_fixtures.json")

# Dense broadband drive far past clipping: 32 sinusoids, random phases. A sum
# of sines so the ODE solve and the sampled solver see the same waveform.
_rng = np.random.default_rng(11)
_NOISE_F = np.exp(_rng.uniform(np.log(100.0), np.log(20000.0), 32))
_NOISE_PH = _rng.uniform(0, 2 * np.pi, 32)


def dense(t):
    t = np.asarray(t, dtype=float)
    return 0.1 * np.sqrt(2.0 / 32) * np.sin(
        2 * np.pi * _NOISE_F * t[..., None] + _NOISE_PH).sum(axis=-1)


def sine(freq, volts):
    return lambda t: volts * np.sin(2 * np.pi * freq * t)


CASES = [
    # name, duration s, supply volts (switched in from rest at 9 V), input,
    # tolerance dB. The loosely held cases are limited by the time step, not
    # by the solver: once Q1 is switched by a hot signal its collector edge
    # takes well under a microsecond, the step is 5.2, and the error (edge
    # timing) falls 5 to 20 dB per doubling of the rate.
    ("sine 1 kHz 4 mV (onset)", 0.004, 9.0, sine(1000, 4e-3), -60.0),
    ("sine 1 kHz 50 mV (hard clip)", 0.004, 9.0, sine(1000, 50e-3), -45.0),
    ("sine 150 Hz 20 mV (emitter bypass)", 0.009, 9.0, sine(150, 20e-3), -60.0),
    ("two-tone 1.9 + 2.3 kHz 10 mV", 0.005, 9.0,
     lambda t: 5e-3 * (np.sin(2 * np.pi * 1900 * t) + np.sin(2 * np.pi * 2300 * t)), -50.0),
    ("sine 1 kHz 1 V (base conduction charges Cin)", 0.004, 9.0, sine(1000, 1.0), -22.0),
    ("starved to 1.2 V, sine 1 kHz 30 mV", 0.005, 1.2, sine(1000, 30e-3), -40.0),
    ("dense 100 Hz - 20 kHz noise, 100 mV RMS", 0.004, 9.0, dense, -17.0),
]


def fit_static():
    xs = np.linspace(-8, 8, 1601)
    ys = static_curve(xs)

    def model(p, x):
        lp, kp, ln, kn = p
        pos = lp * x / (np.abs(x) ** kp + lp ** kp) ** (1 / kp)
        neg = ln * x / (np.abs(x) ** kn + ln ** kn) ** (1 / kn)
        return np.where(x >= 0, pos, neg)

    fit = least_squares(lambda p: model(p, xs) - ys, [0.5, 6.0, 0.5, 6.0],
                        bounds=([0.05, 0.5, 0.05, 0.5], [8, 12, 8, 12]))
    err = model(fit.x, xs) - ys
    print("static curve: +limit %.4f +knee %.4f -limit %.4f -knee %.4f" % tuple(fit.x))
    print("static fit max error %.4f, rms %.4f (unit domain); curve range %.3f..%.3f"
          % (np.abs(err).max(), np.sqrt(np.mean(err ** 2)), ys.min(), ys.max()))
    return fit.x


def main():
    op = operating_point()
    print("operating point:", {k: float(v) for k, v in op.items()})
    print("1 kHz small-signal gain: %.6f" % abs(small_signal_gain(1000)))
    cases = []
    for name, dur, vcc, fn, tolerance_db in CASES:
        n = int(round(dur * RATE))
        t = np.arange(n) / RATE
        e = np.array([float("%.7g" % v) for v in fn(t)])
        _, ref, _ = solve_reference(fn, dur, RATE, vcc=vcc, start_vcc=CIRCUIT["Vcc"])
        st = FuzzStage(RATE, vcc=vcc, start_vcc=CIRCUIT["Vcc"])
        twin = st.run(e)
        db = 10 * np.log10(np.mean((twin - ref) ** 2) / np.mean(ref ** 2))
        print(f"{name}: twin vs ODE {db:.1f} dB, peak {np.abs(ref).max():.4f} V, iters {st.iter_hist[1:9]}")
        cases.append(dict(name=name, vcc=vcc, tolerance_db=tolerance_db,
                          input=[float(v) for v in e],
                          output=[round(float(v), 6) for v in ref]))
    with open(OUT, "w") as f:
        json.dump(dict(rate=RATE, cases=cases), f, separators=(",", ":"))
    print("wrote", os.path.normpath(OUT), os.path.getsize(OUT) // 1024, "KB")
    fit_static()


def check():
    """Compare the discrete twin with the fixtures already on disk."""
    with open(OUT) as f:
        fixtures = json.load(f)
    for case in fixtures["cases"]:
        st = FuzzStage(fixtures["rate"], vcc=case["vcc"], start_vcc=CIRCUIT["Vcc"])
        twin = st.run(np.array(case["input"]))
        ref = np.array(case["output"])
        db = 10 * np.log10(np.mean((twin - ref) ** 2) / np.mean(ref ** 2))
        print(f"{case['name']}: twin vs ODE {db:.1f} dB (tolerance {case['tolerance_db']}), iters {st.iter_hist[1:]}")


if __name__ == "__main__":
    if sys.argv[1:] == ["explore"]:
        explore()
    elif sys.argv[1:] == ["check"]:
        check()
    else:
        main()
