#!/usr/bin/env python3
"""Fit a generated snare's knobs to the factory Membrane Snare.

A (1+1) evolution strategy over the knobs in FIT (log or linear, within
their ranges), minimising snare_bench's score plus a pitch term (semitones
between the strongest low peaks, x6). Prints each improvement and the best
knobs as --set flags for snare_bench.py.

Usage (repo root, same environment as snare_bench.py):
  python3 tools/modal/bench/snare_fit.py INSTRUMENT_DIR [--evals N] [--seed S]
      [--map batter=head --map snares=buzz ...] [--wires-off group.param=value]

--map renames a part of the knobs' names (FIT is written for
examples/snare.lisp's ids) to another patch's ids; a knob the instrument
lacks is left out.
"""

import argparse
import os
import sys

import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import snare_bench as sb  # noqa: E402

# knob, low, high, log scale
FIT = [
    ("batter.batter_tune", 0.3, 1.2, True),
    ("reso.reso_tune", 0.3, 1.5, True),
    ("batter.batter_loss", 0.5, 40, True),
    ("batter.batter_bright", 0.05, 8, True),
    ("reso.reso_loss", 0.5, 40, True),
    ("reso.reso_bright", 0.05, 8, True),
    ("stick.stick_mass", 0.003, 1, True),
    ("hit.hit_felt", 0.01, 5, True),
    ("hit.hit_hardness", 0.0, 1.0, False),
    ("air.air_loss", 5, 300, True),
    ("wires.wires_pitch", 300, 3500, True),
    ("wires.wires_loss", 0.5, 50, True),
    ("wires.wires_mass", 0.001, 2, True),
    ("snares.snares_gap", 0.0, 0.05, False),
    ("snares.snares_snap", 1, 200, True),
    ("snares.snares_stiff", 0.1, 200, True),
    ("snares.snares_drag", 0.0, 1.0, False),
    ("batter.batter_bend", 0.0, 4.0, False),
    ("mic2.mic2_gain", 0.05, 3, True),
]


def to_unit(v, lo, hi, log):
    return (np.log(v / lo) / np.log(hi / lo)) if log else (v - lo) / (hi - lo)


def from_unit(u, lo, hi, log):
    u = float(np.clip(u, 0, 1))
    return float(lo * (hi / lo) ** u) if log else float(lo + u * (hi - lo))


def strongest_low(y):
    seg = y[:int(sb.WINDOW * sb.SR)]
    spectrum = np.abs(np.fft.rfft(seg * np.hanning(len(seg)), 8 * len(seg)))
    f = np.fft.rfftfreq(8 * len(seg), 1 / sb.SR)
    keep = (f > 60) & (f < 600)
    return float(f[keep][np.argmax(spectrum[keep])])


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("candidate")
    ap.add_argument("--evals", type=int, default=150)
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--map", action="append", default=[])
    ap.add_argument("--wires-off", default="snares.snares_snap=0")
    args = ap.parse_args()
    rng = np.random.default_rng(args.seed)

    ref = sb.Subject(sb.REFERENCE, 440.0, {}, {"snares": 0.0}).measure()
    ref_f = strongest_low(ref["full"][1.0])
    subject = sb.Subject(args.candidate, 440.0, {}, sb.parse_sets([args.wires_off]))
    def default_of(name):
        return float(subject.inst.params[name]["default"])

    def renamed(knob):
        group, param = knob.split(".", 1)
        for pair in args.map:
            old, new = pair.split("=", 1)
            if group == old:
                group, param = new, new + param[len(old):]
        return f"{group}.{param}"

    fit = [(renamed(k), lo, hi, log) for k, lo, hi, log in FIT]
    fit = [entry for entry in fit if entry[0] in subject.inst.params]

    x = np.array([to_unit(min(max(default_of(k), lo), hi), lo, hi, log) for k, lo, hi, log in fit])

    def evaluate(u):
        subject.sets = {k: from_unit(ui, lo, hi, log) for ui, (k, lo, hi, log) in zip(u, fit)}
        m = subject.measure()
        if m["nan"] or m["peak"] > 50:
            return 1e9, m
        parts = sb.score(ref, m)
        pitch = 6 * abs(12 * np.log2(strongest_low(m["full"][1.0]) / ref_f))
        return sum(parts.values()) + pitch, m

    best, _ = evaluate(x)
    print(f"start score {best:.1f}", flush=True)
    step = 0.15
    for i in range(args.evals):
        trial = np.clip(x + step * rng.standard_normal(len(x)) * (rng.random(len(x)) < 0.4), 0, 1)
        s, _ = evaluate(trial)
        if s < best:
            best, x = s, trial
            step = min(step * 1.3, 0.3)
            print(f"[{i}] score {best:.1f}", flush=True)
        else:
            step = max(step * 0.93, 0.02)
    print("best", f"{best:.1f}")
    print(" ".join(f"--set {k}={from_unit(u, lo, hi, log):.5g}" for u, (k, lo, hi, log) in zip(x, fit)))


if __name__ == "__main__":
    main()
