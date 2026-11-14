"""Write the reference fixtures crates/sequencer/src/effects/triode.rs tests
assert against, and fit the memoryless display curve.

    python3 tools/saturator/make_fixtures.py

The fixtures are a stiff ODE solve (scipy Radau) of the circuit in
triode_ref.py: independent of the trapezoid + Newton algorithm the engine
runs, which is the point.
"""
import json
import os

import numpy as np
from scipy.optimize import fsolve, least_squares

from triode_ref import (CIRCUIT, TriodeStage, operating_point, solve_reference,
                        tube_currents)

RATE = 192000
HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(HERE, "../../crates/sequencer/src/effects/triode_fixtures.json")

# Dense broadband drive far past clipping: 48 sinusoids from 100 Hz to 20 kHz
# with random phases, 25 V RMS. A sum of sines so the ODE solve and the
# sampled solver see exactly the same band-limited waveform.
_rng = np.random.default_rng(7)
_NOISE_F = np.exp(_rng.uniform(np.log(100.0), np.log(20000.0), 48))
_NOISE_PH = _rng.uniform(0, 2 * np.pi, 48)


def dense(t):
    t = np.asarray(t, dtype=float)
    return 25.0 * np.sqrt(2.0 / 48) * np.sin(
        2 * np.pi * _NOISE_F * t[..., None] + _NOISE_PH).sum(axis=-1)


CASES = [
    # name, duration s, vbias, input volts as a function of time
    ("sine 1 kHz 2 V", 0.012, 0.0, lambda t: 2.0 * np.sin(2 * np.pi * 1000 * t)),
    ("sine 1 kHz 20 V (hard clip, grid conduction)", 0.012, 0.0,
     lambda t: 20.0 * np.sin(2 * np.pi * 1000 * t)),
    ("sine 80 Hz 5 V (cathode bypass)", 0.03, 0.0, lambda t: 5.0 * np.sin(2 * np.pi * 80 * t)),
    ("two-tone 1.9 + 2.3 kHz 3 V", 0.012, 0.0,
     lambda t: 3.0 * (np.sin(2 * np.pi * 1900 * t) + np.sin(2 * np.pi * 2300 * t))),
    ("burst then tail (bias recovery)", 0.03, 0.0,
     lambda t: np.where(t < 0.008, 15.0, 0.3) * np.sin(2 * np.pi * 700 * t)),
    ("cold bias switched in, sine 1 kHz 3 V", 0.012, -2.0, lambda t: 3.0 * np.sin(2 * np.pi * 1000 * t)),
    ("dense 100 Hz - 20 kHz noise, 25 V RMS", 0.012, 0.0, dense),
]


def static_curve(xs, c=CIRCUIT):
    """Memoryless midband transfer in the unit domain: caps held at their
    quiescent voltages (as AC shorts), no bias shift."""
    op = operating_point(0.0, c)
    vk = op["vk"]
    ra = c["Rs"]
    rgth = ra * c["Rg"] / (ra + c["Rg"]) + c["Rgs"]
    ka = c["Rg"] / (ra + c["Rg"])
    rpth = c["Rp"] * c["RL"] / (c["Rp"] + c["RL"])
    out = []
    guess = (op["vg"], op["vp"])
    vg_open = None
    for x in xs:
        def resid(u):
            vg, vp = u
            ik, ig = tube_currents(vg - vk, vp - vk)
            # grid: Thevenin of (e - vcin) and ground; plate: Rp to Vb, RL to (vp - vcout)
            f1 = ka * (x - op["vcin"]) - rgth * ig - vg
            f2 = (c["Vb"] - vp) / c["Rp"] - (ik - ig) - (vp - op["vcout"]) / c["RL"]
            return [f1, f2 * 1e3]
        vg, vp = fsolve(resid, guess, xtol=1e-13)
        guess = (vg, vp)
        out.append(vp - op["vcout"])
    out = np.array(out)
    i0 = np.argmin(np.abs(xs))
    gain = -(out[i0 + 1] - out[i0 - 1]) / (xs[i0 + 1] - xs[i0 - 1])
    return -(out - out[i0]) / gain


def fit_static():
    xs = np.linspace(-12, 12, 2401)
    ys = static_curve(xs)

    def model(p, x):
        lp, kp, ln, kn = p
        pos = lp * x / (np.abs(x) ** kp + lp ** kp) ** (1 / kp)
        neg = ln * x / (np.abs(x) ** kn + ln ** kn) ** (1 / kn)
        return np.where(x >= 0, pos, neg)

    fit = least_squares(lambda p: model(p, xs) - ys, [2.0, 2.0, 1.2, 2.0],
                        bounds=([0.2, 0.5, 0.2, 0.5], [8, 12, 8, 12]))
    err = model(fit.x, xs) - ys
    print("static curve: +limit %.4f +knee %.4f -limit %.4f -knee %.4f" % tuple(fit.x))
    print("static fit max error %.4f, rms %.4f (unit domain); curve range %.3f..%.3f"
          % (np.abs(err).max(), np.sqrt(np.mean(err ** 2)), ys.min(), ys.max()))
    return fit.x


def main():
    cases = []
    for name, dur, vbias, fn in CASES:
        t, ref, _ = solve_reference(fn, dur, RATE, vbias=vbias, rtol=1e-8, atol=1e-10, start_vbias=0.0)
        twin = TriodeStage(RATE, vbias=vbias, start_vbias=0.0).run(fn(t))
        db = 10 * np.log10(np.mean((twin - ref) ** 2) / np.mean(ref ** 2))
        print(f"{name}: twin vs ODE {db:.1f} dB, peak {np.abs(ref).max():.1f} V")
        # How closely the engine's solver must match. The hard-clipped cases
        # are limited by the time step, not the solver: a clipping edge lasts
        # about a microsecond, the step is 5.2, and the error (edge timing)
        # falls ~10 dB per doubling of the rate. The dense case is all edges.
        tolerance_db = -20.0 if name.startswith("dense") else -50.0
        cases.append(dict(name=name, vbias=vbias, tolerance_db=tolerance_db,
                          input=[float(np.float32(v)) for v in fn(t)],
                          output=[round(float(v), 4) for v in ref]))
    with open(OUT, "w") as f:
        json.dump(dict(rate=RATE, cases=cases), f, separators=(",", ":"))
    print("wrote", os.path.normpath(OUT), os.path.getsize(OUT) // 1024, "KB")
    fit_static()


if __name__ == "__main__":
    main()
