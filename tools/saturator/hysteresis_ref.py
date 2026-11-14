"""Reference model of the Saturator "transformer" curve: an audio transformer
with a hysteretic iron core, driven by a line amplifier.

    x --[driver, rails]-- e(t) --Rs--+---------+------+-- out (1:1 secondary)
                                     |         |      |
                                  primary     Idc     RL
                                  (N turns    (bias)  |
                                   on core)    |      |
                                    gnd       gnd    gnd

The primary is a nonlinear inductor: v = N A dB/dt, B = mu0 (H + M), with
H = N i / l the field of the magnetising current and M the magnetisation of
the core, which follows the Jiles-Atherton model (Jiles & Atherton, "Theory
of ferromagnetic hysteresis", J. Magn. Magn. Mater. 61, 1986):

    He   = H + alpha M                          effective field
    Man  = Ms (coth(He/a) - a/He)               anhysteretic (Langevin)
    M    = Mirr + c (Man - Mirr)                reversible wall bowing
    dMirr/dH = (Man - Mirr) / (delta k - alpha (Man - Mirr)),  delta = sign(dH/dt)

which combine to

    dM/dH = [ (1-c)(Man - M) / ((1-c) delta k - alpha (Man - M)) + c Man' ]
            / (1 - c alpha Man'),               Man' = dMan/dHe

The irreversible term is dropped while delta (Man - M) <= 0 (just after a
reversal the raw equation gives a negative susceptibility, which no material
has), and its denominator is kept positive.

The core and geometry are CHOSEN to behave like a small nickel-iron line
output transformer. They are not measured from any particular part.

Two independent solvers of the same circuit live here:
  * `solve_reference`: stiff ODE integration in time (scipy Radau).
  * `Transformer`: the discrete algorithm the Rust engine implements
    (trapezoidal flux integration, the magnetisation law integrated along H
    with a theta rule, one bracketed Newton solve on H per step).

    python3 tools/saturator/hysteresis_ref.py            # fixtures + display fit
    python3 tools/saturator/hysteresis_ref.py measure    # THD / response tables
"""
import json
import os
import sys

import numpy as np
from scipy.integrate import solve_ivp

MU0 = 4e-7 * np.pi

# Core (Jiles-Atherton), chosen: a 50% nickel-iron lamination, 1.5 T.
CORE = dict(Ms=1.2e6, a=20.0, alpha=4.0e-6, k=8.0, c=0.25)
# Winding and circuit, chosen: 1600 turns on 0.42 cm^2 with an 8 cm path,
# driven from 600 ohm into a 10 k load (8.6 H unmagnetised).
CIRCUIT = dict(N=1600.0, A=0.42e-4, l=0.08, Rs=600.0, RL=10e3)

# Volts at the driver output per unit of input: a 60 Hz tone at 1 swings the
# core to 1.3 T, the onset of saturation (lower tones are past it, higher
# ones proportionally short of it).
INPUT_VOLTS = 33.0
RAIL = 3.0             # driver swing limit, in units
RAIL_KNEE = 4.0
BIAS_H = 120.0         # A/m of standing field per unit of the bias control


def langevin(q):
    """coth(q) - 1/q and its derivative."""
    q = float(q)
    if abs(q) < 1e-2:
        q2 = q * q
        return q * (1 / 3 - q2 / 45 + 2 * q2 * q2 / 945), 1 / 3 - q2 / 15 + 2 * q2 * q2 / 189
    e = np.exp(-2 * abs(q))
    coth = (1 + e) / (1 - e)
    val = np.sign(q) * (coth - 1 / abs(q))
    return val, 1 / (q * q) - 4 * e / ((1 - e) * (1 - e))


def irreversible(d, inv_d, p=CORE):
    """Irreversible dM/dH at distance d = |Man - M| from the anhysteretic
    curve, for a field moving toward it, and how much it changes per A/m of
    d. `inv_d` is 1 / (1 - c alpha Man')."""
    pin = (1 - p["c"]) * p["k"]
    den = pin - p["alpha"] * d
    if den > 0.1 * pin:
        return (1 - p["c"]) * d / den * inv_d, (1 - p["c"]) * pin / (den * den) * inv_d
    return (1 - p["c"]) * d / (0.1 * pin) * inv_d, (1 - p["c"]) / (0.1 * pin) * inv_d


def susceptibility_parts(h, m, delta, p=CORE):
    """dM/dH for a field moving in direction `delta` (+1, -1, or 0), split as
    (reversible part, irreversible part, rate, Man', Man - M). The
    irreversible part pulls M toward the anhysteretic curve; the rate is how
    much it changes per A/m of the distance |Man - M| still to go. It is
    dropped while the field moves away from the curve."""
    lv, ld = langevin((h + p["alpha"] * m) / p["a"])
    dman = p["Ms"] / p["a"] * ld
    diff = p["Ms"] * lv - m
    inv_d = 1.0 / (1 - p["c"] * p["alpha"] * dman)
    irr, rate = irreversible(abs(diff), inv_d) if delta * diff > 0.0 else (0.0, 0.0)
    return p["c"] * dman * inv_d, irr, rate, dman, diff


def susceptibility(h, m, delta, p=CORE):
    rev, irr, _, _, _ = susceptibility_parts(h, m, delta, p)
    return rev + irr


def rail(x):
    """The driver's swing limit: clean below the rails, firm at them."""
    x = np.asarray(x, dtype=float)
    return RAIL * x / (np.abs(x) ** RAIL_KNEE + RAIL ** RAIL_KNEE) ** (1 / RAIL_KNEE)


def constants(c=CIRCUIT, p=CORE):
    kdiv = c["RL"] / (c["Rs"] + c["RL"])
    rth = c["Rs"] * c["RL"] / (c["Rs"] + c["RL"])
    rh = rth * c["l"] / c["N"]          # volts per A/m of field
    na = c["N"] * c["A"]                # volt-seconds per tesla
    chi0 = p["c"] * p["Ms"] / (3 * p["a"]) / (1 - p["c"] * p["alpha"] * p["Ms"] / (3 * p["a"]))
    l0 = MU0 * (1 + chi0) * c["N"] ** 2 * c["A"] / c["l"]   # small-signal inductance
    return dict(kdiv=kdiv, rth=rth, rh=rh, na=na, chi0=chi0, l0=l0)


def small_signal_gain(freq=1000.0):
    """|v / e| for a vanishing signal at `freq` (demagnetised core)."""
    k = constants()
    w = 2 * np.pi * freq * k["l0"]
    return k["kdiv"] * w / np.hypot(k["rth"], w)


def solve_reference(x_fn, t_end, fs_out, bias=0.0, rtol=1e-9, atol=1e-9):
    """Radau solve in (H, M); x_fn(t) is the input in units. Returns unit out."""
    k = constants()
    hdc = bias * BIAS_H

    def rhs(t, y):
        h, m = y
        v = k["kdiv"] * INPUT_VOLTS * float(rail(x_fn(t))) - k["rh"] * (h - hdc)
        dbdt = v / k["na"]
        chi = susceptibility(h, m, np.sign(dbdt))
        dh = dbdt / (MU0 * (1 + chi))
        return [dh, chi * dh]

    n = int(round(t_end * fs_out))
    t_eval = np.arange(n) / fs_out
    sol = solve_ivp(rhs, (0.0, t_eval[-1]), [0.0, 0.0], method="Radau", t_eval=t_eval,
                    rtol=rtol, atol=atol, max_step=0.25 / fs_out)
    v = k["kdiv"] * INPUT_VOLTS * rail(x_fn(t_eval)) - k["rh"] * (sol.y[0] - hdc)
    return t_eval, v / (small_signal_gain() * INPUT_VOLTS), sol.y


class Transformer:
    """Discrete twin of crates/sequencer/src/effects/hysteresis.rs (keep in sync).

    Per step the unknown is the new field h1. The trapezoidal flux equation
    is linear, so it gives the new magnetisation directly, m(h1); what is
    solved is the magnetisation law integrated along H from (h0, m0) to
    (h1, m(h1)) with a theta rule,

        m(h1) - m0 = dh ((1 - theta) chi(h0, m0) + theta chi(h1, m(h1)))

    theta is 1/2 (trapezoidal, second order) while the step is short against
    the few A/m over which the irreversible pull toward the anhysteretic curve
    relaxes, and tends to 1 (backward Euler, which lands on that curve without
    overshooting it) as the step outgrows them: in saturation one step moves H
    by hundreds of A/m. The residual is monotonic in h1, so the iteration is
    bracketed; it is Newton's, with the step taken to the root of a parabola
    where the residual is one (see the loop).
    """

    MAX_ITERS = 12
    TOL_H = 1e-4          # A/m, plus 1e-5 relative
    CURVATURE_FLOOR = 1e-2  # per A/m

    def __init__(self, fs):
        k = constants()
        self.k = k
        self.c1 = 1.0 / (2.0 * fs * k["na"])    # T / (2 N A)
        self.slope_lin = MU0 + self.c1 * k["rh"]
        self.h = self.m = self.v = self.dh = 0.0
        self.scale = 1.0 / (small_signal_gain() * INPUT_VOLTS)
        self.iters = 0
        self.max_iters = 0

    def step(self, x, bias=0.0):
        k = self.k
        alpha = CORE["alpha"]
        vth = k["kdiv"] * INPUT_VOLTS * float(rail(x))
        hdc = bias * BIAS_H
        h0, m0 = self.h, self.m
        # Flux: mu0 (h1 + m1) = B0 + c1 (v1 + v0), v1 = vth - rh (h1 - hdc)
        #   =>  m1 = m_at0 - m_slope (h1 - h0)
        rhs = MU0 * (h0 + m0) + self.c1 * (vth + k["rh"] * hdc + self.v)
        m_slope = self.slope_lin / MU0
        m_at0 = rhs / MU0 - m_slope * h0
        # Start-of-step susceptibility: the irreversible part only acts for a
        # field moving toward the side the anhysteretic curve is on.
        rev0, _, _, dman0, diff0 = susceptibility_parts(h0, m0, 0.0)
        side = np.sign(diff0)
        irr0, rate0 = irreversible(abs(diff0), 1.0 / (1 - CORE["c"] * alpha * dman0))

        # Work in u = |h1 - h0| along the direction delta the field moves:
        #   psi(u) = delta (m(h1) - m0) - u mean(u),  decreasing and concave.
        gap = m_at0 - m0
        rate_at_curve = 1.0 / CORE["k"]
        h1 = h0 + self.dh
        lo, hi = -np.inf, np.inf
        for it in range(self.MAX_ITERS):
            dh = h1 - h0
            delta = np.sign(dh) if dh != 0.0 else np.sign(gap)
            if delta == 0.0:
                break
            u = abs(dh)
            m1 = m_at0 - m_slope * dh
            rev1, irr1, rate1, dman1, diff1 = susceptibility_parts(h1, m1, delta)
            chi1 = rev1 + irr1
            if delta == side:
                z = u * rate0
                theta = (1 + 2 * z) / (2 + 2 * z)
                dtheta = rate0 / (2 * (1 + z) * (1 + z))     # d theta / du
                chi0 = rev0 + irr0
            else:
                theta, dtheta, chi0 = 0.5, 0.0, rev0
            mean = (1 - theta) * chi0 + theta * chi1
            psi = delta * (m1 - m0) - u * mean
            if delta * psi > 0:
                lo = h1
            else:
                hi = h1
            # How fast the distance to the anhysteretic curve changes with u.
            gd = dman1 * (1 - alpha * m_slope) + m_slope
            flat = -m_slope - mean - u * dtheta * (chi1 - chi0)
            d1 = flat
            if irr1 > 0.0:
                # The irreversible pull grows linearly with u, so psi is close
                # to a parabola: step to its root, not the tangent's.
                d1 = flat - theta * u * rate1 * gd
                d2 = -2 * theta * rate1 * gd
                du = 2 * psi / (-d1 + np.sqrt(max(d1 * d1 - 2 * d2 * psi, 0.0)))
            else:
                # No pull at the trial point (the field moves away from the
                # curve, or the flux puts M past it): psi is a line here.
                du = -psi / flat
                d2 = 0.0
                to_curve = -delta * diff1 / gd      # >= 0
                if du > to_curve:
                    # ...up to where M meets the curve; continue from there on
                    # the parabola the pull makes beyond it.
                    psi_c = psi + flat * to_curve
                    d1 = flat - theta * (u + to_curve) * rate_at_curve * gd
                    d2 = -2 * theta * rate_at_curve * gd
                    du = to_curve + 2 * psi_c / (-d1 + np.sqrt(max(d1 * d1 - 2 * d2 * psi_c, 0.0)))
            nxt = h1 + delta * du
            bisected = nxt != h1 and not (lo < nxt < hi)
            if bisected:
                nxt = 0.5 * (lo + hi)
            step = nxt - h1
            h1 = nxt
            # Done when the step is negligible, or when what a step of this
            # size leaves behind is: Newton's error is the step squared times
            # the curvature over twice the slope (the parabola step leaves
            # less), with a floor for the curvature the model ignores.
            tol = self.TOL_H + 1e-5 * abs(h1)
            left = step * step * (abs(d2 / (2 * d1)) + self.CURVATURE_FLOOR)
            if abs(step) < tol or (left < tol and not bisected):
                break
        self.iters += it + 1
        self.max_iters = max(self.max_iters, it + 1)
        ms = CORE["Ms"]
        self.dh = h1 - h0
        self.h = h1
        self.m = min(max(m_at0 - m_slope * (h1 - h0), -ms), ms)
        self.v = vth - k["rh"] * (h1 - hdc)
        return self.v * self.scale

    def run(self, x, bias=0.0):
        return np.array([self.step(v, bias) for v in x])


# ---------------------------------------------------------------- fixtures

RATE = 192000
HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(HERE, "../../crates/sequencer/src/effects/hysteresis_fixtures.json")

_rng = np.random.default_rng(11)
_NOISE_F = np.exp(_rng.uniform(np.log(40.0), np.log(12000.0), 32))
_NOISE_PH = _rng.uniform(0, 2 * np.pi, 32)


def dense(t):
    t = np.asarray(t, dtype=float)
    return 6.0 * np.sqrt(2.0 / 32) * np.sin(
        2 * np.pi * _NOISE_F * t[..., None] + _NOISE_PH).sum(axis=-1)


CASES = [
    # name, seconds, bias, input in units as a function of time
    ("sine 60 Hz at 0.5 (below saturation)", 0.010, 0.0, lambda t: 0.5 * np.sin(2 * np.pi * 60 * t)),
    ("sine 60 Hz at 2 (core saturates)", 0.012, 0.0, lambda t: 2.0 * np.sin(2 * np.pi * 60 * t)),
    ("sine 200 Hz at 8 (rails, then the core)", 0.006, 0.0, lambda t: 8.0 * np.sin(2 * np.pi * 200 * t)),
    ("1 kHz riding on 50 Hz", 0.010, 0.0,
     lambda t: 1.5 * np.sin(2 * np.pi * 50 * t) + 0.3 * np.sin(2 * np.pi * 1000 * t)),
    ("standing current switched in, sine 80 Hz at 1", 0.008, 1.0, lambda t: 1.0 * np.sin(2 * np.pi * 80 * t)),
    ("dense 40 Hz - 12 kHz noise, 6 RMS", 0.006, 0.0, dense),
]


def error_db(got, want):
    return 10 * np.log10(np.mean((got - want) ** 2) / np.mean(want ** 2))


def make_fixtures():
    cases = []
    for name, dur, bias, fn in CASES:
        t, ref, _ = solve_reference(fn, dur, RATE, bias=bias)
        tw = Transformer(RATE)
        twin = tw.run(fn(t), bias)
        db = error_db(twin, ref)
        print(f"{name}: twin vs ODE {db:.1f} dB, peak {np.abs(ref).max():.2f}, "
              f"iters mean {tw.iters / len(t):.2f} max {tw.max_iters}")
        # How closely the engine's solver must match: 10 dB of margin on
        # what the twin achieves, no tighter than the fixtures' own rounding
        # and the engine's single-precision state allow.
        cases.append(dict(name=name, bias=bias, tolerance_db=float(np.clip(round(db + 10.0), -60.0, -40.0)),
                          input=[round(float(v), 5) for v in fn(t)],
                          output=[round(float(v), 4) for v in ref]))
    with open(OUT, "w") as f:
        json.dump(dict(rate=RATE, cases=cases), f, separators=(",", ":"))
    print("wrote", os.path.normpath(OUT), os.path.getsize(OUT) // 1024, "KB")


# ---------------------------------------------------------------- measures

def steady(freq, amp, bias=0.0, fs=RATE, cycles=6):
    """Last whole cycle of the twin's response to a sine."""
    per = int(round(fs / freq))
    t = np.arange(per * cycles) / fs
    y = Transformer(fs).run(amp * np.sin(2 * np.pi * (fs / per) * t), bias)
    return y[-per:]


def harmonic_amps(seg, count=12):
    spec = np.abs(np.fft.rfft(seg)) / (len(seg) / 2)
    return spec[1:count + 1]


def thd(seg):
    h = harmonic_amps(seg)
    return 100 * np.sqrt(np.sum(h[1:] ** 2)) / h[0]


def fit_static():
    """Display curve: level of a 60 Hz tone's fundamental out vs its level in."""
    from scipy.optimize import least_squares
    xs = np.concatenate([np.linspace(0.05, 2, 14), np.linspace(2.5, 12, 12)])
    ys = np.array([harmonic_amps(steady(60, x))[0] for x in xs])
    ys /= ys[0] / xs[0]     # unit slope at the origin

    def model(p, x):
        lim, knee = p
        return lim * x / (x ** knee + lim ** knee) ** (1 / knee)

    fit = least_squares(lambda p: model(p, xs) - ys, [1.5, 3.0], bounds=([0.2, 0.5], [8, 12]))
    err = model(fit.x, xs) - ys
    print("display curve (60 Hz fundamental vs input level): limit %.4f knee %.4f" % tuple(fit.x))
    print("  fit max error %.4f, curve tops out at %.3f" % (np.abs(err).max(), ys.max()))
    return fit.x


def measure():
    k = constants()
    print("small-signal: chi0 %.0f, L0 %.2f H, corner %.2f Hz, gain at 1 kHz %.5f"
          % (k["chi0"], k["l0"], k["rth"] / (2 * np.pi * k["l0"]), small_signal_gain()))
    print("THD % (twin)        x=0.25     1       4      16")
    for f in (50, 200, 1000):
        print(f"  {f:5d} Hz    " + "  ".join(f"{thd(steady(f, a)):6.2f}" for a in (0.25, 1, 4, 16)))
    print("H2 re fundamental, 80 Hz at x=1: bias 0 %.1f dB, bias 1 %.1f dB" % tuple(
        20 * np.log10(harmonic_amps(steady(80, 1.0, b))[1] / harmonic_amps(steady(80, 1.0, b))[0] + 1e-12)
        for b in (0.0, 1.0)))
    print("small-signal response (x = 1e-3), dB re 1 kHz")
    ref = harmonic_amps(steady(1000, 1e-3))[0]
    print("  " + "  ".join(f"{f}Hz {20 * np.log10(harmonic_amps(steady(f, 1e-3))[0] / ref):+.2f}"
                          for f in (20, 50, 100, 1000, 5000, 10000, 20000)))
    for amp in (16, 64, 300):
        seg = steady(100, amp)
        print(f"output range, 100 Hz at x={amp}: {seg.min():+.3f} .. {seg.max():+.3f}")


if __name__ == "__main__":
    if "measure" in sys.argv[1:]:
        measure()
    else:
        make_fixtures()
        fit_static()
