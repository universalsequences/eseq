"""Reference model of the Saturator "op-amp clip" curve: a slew-limited op-amp
gain stage into a hard diode clipper (the topology of the ProCo RAT).

    python3 tools/saturator/opamp_clip_ref.py            # fixtures + static fit
    python3 tools/saturator/opamp_clip_ref.py measure    # THD / response tables
    python3 tools/saturator/opamp_clip_ref.py check      # twin against the stored fixtures

Circuit (values in CIRCUIT; all voltages relative to the 4.5 V reference):

    e --||Cin--+------(+)\\
               Rb        |  >--+-- vo --||Cc--Rs--+-- vd --Rt--+-- vt --||Co--+-- out
              ref   +-(-)/     |                  |            |              |
                    |          |               D+ v ^ D-       Ct             Ro
                    +--Rf||Cf--+                  |            |              |
                    |                            ref          ref            ref
                    +--R1--||C1-- ref
                    +--R2--||C2-- ref

Op-amp macromodel (LM308 with a 30 pF compensation capacitor): a differential
pair that saturates into the compensation integrator, and an output that
cannot pass the supply rails:

    dvo/dt = SR * tanh((v+ - v-) / Vlin),   Vlin = SR / (2 pi GBW),   |vo| <= Vmax

so the slew rate (SR) and the gain-bandwidth product (GBW) are both physical.
The diodes are a Shockley pair; `asym` (the bias control) puts up to two more
diodes in series with one of them.

Two independent solvers of the same circuit live here:
  * `solve_reference`: stiff ODE integration (scipy Radau, tight tolerances),
    with the diode node solved algebraically.
  * `OpampClip`: the discrete algorithm the Rust engine implements
    (crates/sequencer/src/effects/opamp_clip.rs; keep in sync).
`main` checks the second against the first, writes the fixtures the Rust tests
assert against, and fits the memoryless display curve.

Component and device values are from memory of the published schematic and of
common SPICE models, not checked against a unit or a datasheet.
"""
import json
import os
import sys

import numpy as np
from scipy.integrate import solve_ivp
from scipy.optimize import least_squares
from scipy.special import wrightomega

CIRCUIT = dict(
    Cin=22e-9, Rb=1e6,            # input coupling and bias resistor
    Rf=10e3, Cf=100e-12,          # feedback: the distortion pot at a fixed setting
    R1=47.0 + 470.0, C1=2.2e-6,   # gain leg 1 (47 ohm stock, plus a series "Ruetz" resistance)
    R2=560.0, C2=4.7e-6,          # gain leg 2
    SR=0.3e6, GBW=1.0e6,          # op-amp slew rate (V/s) and gain-bandwidth (Hz)
    Vmax=3.5,                     # output swing either side of the reference
    Cc=4.7e-6, Rs=1e3,            # coupling into the clipper
    Is=2.52e-9, nVt=1.752 * 25.85e-3,  # 1N914
    Rt=1.5e3, Ct=3.3e-9,          # filter control at minimum resistance
    Co=1e-6, Ro=100e3,            # output coupling into the volume pot
)
# Soft rail used by the ODE only (the discrete solver clamps): width in volts.
RAIL_SOFT = 0.02
ASYM_DIODES = 2.0  # extra series diodes at |asym| = 1


def vlin(c=CIRCUIT):
    return c["SR"] / (2 * np.pi * c["GBW"])


def diode_n(asym, c=CIRCUIT):
    """(n*Vt of the diode conducting for vd > 0, for vd < 0)."""
    pos = c["nVt"] * (1.0 + ASYM_DIODES * max(asym, 0.0))
    neg = c["nVt"] * (1.0 + ASYM_DIODES * max(-asym, 0.0))
    return pos, neg


def diode_current(vd, npos, nneg, c=CIRCUIT):
    return c["Is"] * (np.exp(vd / npos) - np.exp(-vd / nneg))


def solve_diode_node(u, rs, ht, yt, npos, nneg, c=CIRCUIT, guess=0.0):
    """vd with (u - vd)/rs = Id(vd) + (vd - ht)*yt. Monotonic: safeguarded Newton."""
    lo, hi = -4.0 * nneg / c["nVt"], 4.0 * npos / c["nVt"]
    vd = min(max(guess, lo), hi)
    for _ in range(200):
        ep = np.exp(vd / npos)
        en = np.exp(-vd / nneg)
        f = (u - vd) / rs - (vd - ht) * yt - c["Is"] * (ep - en)
        df = -1.0 / rs - yt - c["Is"] * (ep / npos + en / nneg)
        if f > 0:
            lo = vd
        else:
            hi = vd
        nxt = vd - f / df
        if not (lo < nxt < hi):
            nxt = 0.5 * (lo + hi)
        if abs(nxt - vd) < 1e-15:
            vd = nxt
            break
        vd = nxt
    return vd


def solve_reference(e_fn, t_end, fs_out, asym=0.0, c=CIRCUIT, rtol=1e-9, atol=1e-12):
    """States: vcin, vo, vf (across Cf), vc1, vc2, vcc, vct, vco. Starts at rest."""
    npos, nneg = diode_n(asym, c)
    vl = vlin(c)
    guess = [0.0]

    def rhs(t, y):
        vcin, vo, vf, vc1, vc2, vcc, vct, vco = y
        vp = e_fn(t) - vcin
        vm = vo - vf
        i1 = (vm - vc1) / c["R1"]
        i2 = (vm - vc2) / c["R2"]
        rail = c["SR"] * (np.logaddexp(0.0, (vo - c["Vmax"]) / RAIL_SOFT)
                          - np.logaddexp(0.0, (-vo - c["Vmax"]) / RAIL_SOFT))
        dvo = c["SR"] * np.tanh((vp - vm) / vl) - rail
        vd = solve_diode_node(vo - vcc, c["Rs"], vct, 1.0 / c["Rt"], npos, nneg, c, guess[0])
        guess[0] = vd
        i_s = (vo - vcc - vd) / c["Rs"]
        i_t = (vd - vct) / c["Rt"]
        return [vp / (c["Rb"] * c["Cin"]), dvo, (i1 + i2 - vf / c["Rf"]) / c["Cf"],
                i1 / c["C1"], i2 / c["C2"], i_s / c["Cc"], i_t / c["Ct"],
                (vct - vco) / (c["Ro"] * c["Co"])]

    n = int(round(t_end * fs_out))
    t_eval = np.arange(n) / fs_out
    sol = solve_ivp(rhs, (0.0, t_eval[-1]), np.zeros(8), method="Radau", t_eval=t_eval,
                    rtol=rtol, atol=atol, max_step=0.25 / fs_out)
    return t_eval, sol.y[6] - sol.y[7], sol.y


def small_signal_gain(f, c=CIRCUIT):
    """Complex gain e -> out of the linearized circuit."""
    s = 2j * np.pi * np.asarray(f, float)
    yg = 1 / (c["R1"] + 1 / (s * c["C1"])) + 1 / (c["R2"] + 1 / (s * c["C2"]))
    zf = c["Rf"] / (1 + s * c["Rf"] * c["Cf"])
    ideal = 1 + zf * yg
    a = 2 * np.pi * c["GBW"] / s
    g = a / (1 + a / ideal)
    # Clipper arm: Cc, Rs into Rt + Ct (the diodes are open at small signal).
    zt = c["Rt"] + 1 / (s * c["Ct"])
    arm = (1 / (s * c["Ct"])) / (1 / (s * c["Cc"]) + c["Rs"] + zt)
    hp_in = s * c["Rb"] * c["Cin"] / (1 + s * c["Rb"] * c["Cin"])
    hp_out = s * c["Ro"] * c["Co"] / (1 + s * c["Ro"] * c["Co"])
    return g * arm * hp_in * hp_out


class OpampClip:
    """Discrete twin of crates/sequencer/src/effects/opamp_clip.rs (keep in sync).

    Trapezoidal companions for every capacitor and for the op-amp integrator,
    except Cf, which is backward Euler: it sits between the op-amp output and
    the 43 ohm feedback node (a 4 ns time constant), and a trapezoid would
    ring on it. The op-amp is a scalar Newton solve on vo; its output is a
    stiff source, so the clipper node is solved after it, in closed form (the
    Wright omega function, a table in the engine) plus one correction step.
    """

    def __init__(self, fs, asym=0.0, c=CIRCUIT, iters=12):
        self.c, self.iters = c, iters
        self.npos, self.nneg = diode_n(asym, c)
        T = 1.0 / fs
        self.half_t = 0.5 * T
        g = lambda cap: 2 * cap / T
        self.gin, self.g1, self.g2 = g(c["Cin"]), g(c["C1"]), g(c["C2"])
        self.gc, self.gt, self.go = g(c["Cc"]), g(c["Ct"]), g(c["Co"])
        self.gf = c["Cf"] / T
        self.kin = c["Rb"] / (c["Rb"] + 1 / self.gin)
        self.y1 = 1 / (c["R1"] + 1 / self.g1)
        self.y2 = 1 / (c["R2"] + 1 / self.g2)
        d = 1 / c["Rf"] + self.gf + self.y1 + self.y2
        self.alpha = (1 / c["Rf"] + self.gf) / d
        self.inv_d = 1 / d
        self.rs = c["Rs"] + 1 / self.gc
        self.yt = 1 / (c["Rt"] + 1 / self.gt)
        self.ko = c["Ro"] / (c["Ro"] + 1 / self.go)
        self.vl = vlin(c)
        self.vcin = self.iin = 0.0
        self.vo = self.dvo = 0.0   # op-amp output and its last derivative
        self.vf = 0.0
        self.vc1 = self.i1 = self.vc2 = self.i2 = 0.0
        self.vcc = self.ic = 0.0
        self.vct = self.it = 0.0
        self.vco = self.io = 0.0

    def step(self, e):
        c = self.c
        # Input coupling: v+ = Rb * i.
        hin = self.vcin + self.iin / self.gin
        vp = (e - hin) * self.kin
        iin = vp / c["Rb"]
        self.vcin, self.iin = hin + iin / self.gin, iin
        # Feedback network: v- = alpha * vo + beta.
        h1 = self.vc1 + self.i1 / self.g1
        h2 = self.vc2 + self.i2 / self.g2
        beta = (self.y1 * h1 + self.y2 * h2 - self.gf * self.vf) * self.inv_d
        # Op-amp: vo = vo_prev + T/2 * (SR*tanh((v+ - v-)/Vlin) + previous derivative).
        base = self.vo + self.half_t * self.dvo
        k = self.half_t * c["SR"]
        vo = self.vo + 2.0 * self.half_t * self.dvo   # predictor
        th = 0.0
        fb = self.alpha / self.vl
        for _ in range(self.iters):
            th = np.tanh((vp - beta) / self.vl - fb * vo)
            dth = (1.0 - th * th) * fb
            step = (vo - base - k * th) / (1.0 + k * dth)
            vo -= step
            error = step * step * abs(th) * dth * fb * k   # Newton's error estimate
            th += dth * step   # the tanh at the new point, to first order
            if error < 1e-8:
                break
        dvo = c["SR"] * th
        if abs(vo) > c["Vmax"]:
            vo = np.copysign(c["Vmax"], vo)
            dvo = 0.0
        self.vo, self.dvo = vo, dvo
        vm = self.alpha * vo + beta
        self.vf = vo - vm
        i1 = (vm - h1) * self.y1
        i2 = (vm - h2) * self.y2
        self.vc1, self.i1 = h1 + i1 / self.g1, i1
        self.vc2, self.i2 = h2 + i2 / self.g2, i2
        # Clipper: (u - vd)/rs = Id(vd) + (vd - ht)*yt.
        hc = self.vcc + self.ic / self.gc
        ht = self.vct + self.it / self.gt
        u = vo - hc
        # With only the conducting diode, v + (Is/g)*exp(v/n) = |open| is
        # w + exp(w) = y in w = v/n + ln(Is/(g*n)). The other diode's reverse
        # current is then one Newton step, and only near zero.
        g_lin = 1.0 / self.rs + self.yt
        vopen = (u / self.rs + ht * self.yt) / g_lin
        n_on, n_other = (self.nneg, self.npos) if vopen < 0 else (self.npos, self.nneg)
        ln_c = np.log(c["Is"] / (g_lin * n_on))
        y = abs(vopen) / n_on + ln_c
        w = float(np.real(wrightomega(y)))   # wrightomega(y) = exp(w)
        w = y - w
        v = (w - ln_c) * n_on
        if v / n_other < 16.0:
            i_rev = c["Is"] * np.exp(-v / n_other)
            v += i_rev / (g_lin * (1.0 + (y - w)) + i_rev / n_other)
        vd = np.copysign(v, vopen)
        ic = (u - vd) / self.rs
        it = (vd - ht) * self.yt
        self.vcc, self.ic = hc + ic / self.gc, ic
        self.vct, self.it = ht + it / self.gt, it
        # Output coupling.
        ho = self.vco + self.io / self.go
        out = (self.vct - ho) * self.ko
        io = out / c["Ro"]
        self.vco, self.io = ho + io / self.go, io
        return out

    def run(self, e):
        return np.array([self.step(x) for x in e])


# ---------------------------------------------------------------------------
# Unit domain (what the engine exposes) and the fixtures.

INPUT_VOLTS = 0.03
GAIN_1K = float(abs(small_signal_gain(1000.0)))

RATE = 192000
HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(HERE, "../../crates/sequencer/src/effects/opamp_clip_fixtures.json")

_rng = np.random.default_rng(11)
_NOISE_F = np.exp(_rng.uniform(np.log(100.0), np.log(20000.0), 48))
_NOISE_PH = _rng.uniform(0, 2 * np.pi, 48)


def dense(t, rms):
    t = np.asarray(t, dtype=float)
    return rms * np.sqrt(2.0 / 48) * np.sin(
        2 * np.pi * _NOISE_F * t[..., None] + _NOISE_PH).sum(axis=-1)


def sine(f, a):
    return lambda t: a * np.sin(2 * np.pi * f * t)


CASES = [
    # name, duration s, asym, tolerance dB, input volts as a function of time
    ("sine 1 kHz 2 mV (linear)", 0.004, 0.0, -45.0, sine(1000, 0.002)),
    ("sine 1 kHz 20 mV (diode clipping)", 0.004, 0.0, -45.0, sine(1000, 0.02)),
    ("sine 1 kHz 0.5 V (rails)", 0.004, 0.0, -40.0, sine(1000, 0.5)),
    ("sine 6 kHz 0.5 V (slew limiting)", 0.003, 0.0, -22.0, sine(6000, 0.5)),
    ("sine 100 Hz 0.2 V (gain legs)", 0.012, 0.0, -40.0, sine(100, 0.2)),
    ("two-tone 1.9 + 2.3 kHz 30 mV", 0.004, 0.0, -45.0,
     lambda t: 0.03 * (np.sin(2 * np.pi * 1900 * t) + np.sin(2 * np.pi * 2300 * t))),
    ("asymmetric diodes, sine 700 Hz 60 mV", 0.004, 0.7, -45.0, sine(700, 0.06)),
    ("dense 100 Hz - 20 kHz noise, 0.3 V RMS", 0.004, 0.0, -25.0, lambda t: dense(t, 0.3)),
]


def error_db(a, b):
    return 10 * np.log10(np.mean((a - b) ** 2) / np.mean(b ** 2))


def static_curve(xs, c=CIRCUIT):
    """Memoryless midband transfer in the unit domain: an ideal amplifier of
    the 1 kHz gain with rails, into the diodes with the capacitors as shorts."""
    npos, nneg = diode_n(0.0, c)
    arm = float(abs(small_signal_gain(1000.0, c)) / abs(_amp_gain(1000.0, c)))
    out = []
    for x in xs:
        vo = np.clip(x * INPUT_VOLTS * GAIN_1K / arm, -c["Vmax"], c["Vmax"])
        out.append(solve_diode_node(vo, c["Rs"], 0.0, 0.0, npos, nneg, c) * arm)
    return np.array(out) / (INPUT_VOLTS * GAIN_1K)


def _amp_gain(f, c=CIRCUIT):
    s = 2j * np.pi * f
    yg = 1 / (c["R1"] + 1 / (s * c["C1"])) + 1 / (c["R2"] + 1 / (s * c["C2"]))
    ideal = 1 + c["Rf"] / (1 + s * c["Rf"] * c["Cf"]) * yg
    a = 2 * np.pi * c["GBW"] / s
    return a / (1 + a / ideal)


def fit_static():
    xs = np.linspace(-12, 12, 2401)
    ys = static_curve(xs)

    def model(p, x):
        lim, knee = p
        return lim * x / (np.abs(x) ** knee + lim ** knee) ** (1 / knee)

    fit = least_squares(lambda p: model(p, xs) - ys, [0.5, 3.0], bounds=([0.05, 0.5], [8, 16]))
    err = model(fit.x, xs) - ys
    print("static curve (symmetric): limit %.4f knee %.4f" % tuple(fit.x))
    print("static fit max error %.4f, rms %.4f (unit domain); curve range %.3f..%.3f"
          % (np.abs(err).max(), np.sqrt(np.mean(err ** 2)), ys.min(), ys.max()))
    return fit.x


def write_fixtures():
    cases = []
    for name, dur, asym, tolerance_db, fn in CASES:
        t = np.arange(int(round(dur * RATE))) / RATE
        # The stored input is rounded; integrate exactly what is stored at the
        # sample points by rounding the waveform's amplitude the same way.
        _, ref, _ = solve_reference(fn, dur, RATE, asym=asym)
        stored = [float("%.6g" % v) for v in fn(t)]
        twin = OpampClip(RATE, asym=asym).run(np.array(stored))
        print(f"{name}: twin vs ODE {error_db(twin, ref):.1f} dB, peak {np.abs(ref).max():.4f} V")
        cases.append(dict(name=name, asym=asym, tolerance_db=tolerance_db,
                          input=stored, output=[round(float(v), 6) for v in ref]))
    with open(OUT, "w") as f:
        json.dump(dict(rate=RATE, cases=cases), f, separators=(",", ":"))
    print("wrote", os.path.normpath(OUT), os.path.getsize(OUT) // 1024, "KB")


def measure():
    print("1 kHz gain %.4f (%.1f dB), phase %.1f deg" % (
        GAIN_1K, 20 * np.log10(GAIN_1K), np.degrees(np.angle(small_signal_gain(1000.0)))))
    fs = [20, 50, 100, 1000, 5000, 10000, 20000]
    g = np.abs(small_signal_gain(fs))
    print("small-signal response re 1 kHz: " + "  ".join(
        f"{f}Hz {20 * np.log10(v / GAIN_1K):+.2f}" for f, v in zip(fs, g)))
    n = RATE // 5
    t = np.arange(n) / RATE
    print("1 kHz, unit amplitude x: THD and output peaks (unit domain)")
    for x in (0.125, 0.25, 0.5, 1, 4, 16, 64, 256):
        y = OpampClip(RATE).run(x * INPUT_VOLTS * np.sin(2 * np.pi * 1000 * t)) / (INPUT_VOLTS * GAIN_1K)
        seg = y[-RATE // 10:]
        s = np.abs(np.fft.rfft(seg * np.hanning(len(seg))))
        h = [s[k * 100 - 2:k * 100 + 3].max() for k in range(1, 40)]
        thd = 100 * np.sqrt(sum(v ** 2 for v in h[1:])) / h[0]
        print(f"  x {x:7.3f}: THD {thd:6.2f}%  H2 {20*np.log10(h[1]/h[0]):6.1f} H3 {20*np.log10(h[2]/h[0]):6.1f}"
              f"  fund gain {20*np.log10(h[0]/(x*len(seg)/4)):+6.2f} dB  peak {seg.min():+.3f}..{seg.max():+.3f}")


def check():
    """The twin against the stored fixtures (no ODE solve)."""
    with open(OUT) as f:
        fixtures = json.load(f)
    for case in fixtures["cases"]:
        twin = OpampClip(fixtures["rate"], asym=case["asym"]).run(np.array(case["input"]))
        print(f"{case['name']}: twin vs ODE {error_db(twin, np.array(case['output'])):.1f} dB"
              f" (tolerance {case['tolerance_db']})")


if __name__ == "__main__":
    if "measure" in sys.argv[1:]:
        measure()
    elif "check" in sys.argv[1:]:
        check()
    else:
        write_fixtures()
        fit_static()
