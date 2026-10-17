#!/usr/bin/env python3
"""Extreme settings at 44.1 and 96 kHz: no NaN/Inf, no silence, bounded peaks.
usage: robust.py INSTRUMENT_DIR"""
import os, sys, itertools
import numpy as np
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import elh

CASES = []
for model in (0, 1, 2, 3):
    for combo in [
        dict(),
        dict(bow=1, bow_timbre=1, blow=1, flow=1, blow_timbre=1, strike=1, mallet=1, strike_timbre=1, contour=1),
        dict(bow=1, bow_timbre=0, blow=1, flow=0, blow_timbre=0, strike=1, mallet=0, strike_timbre=0, contour=0),
        dict(geometry=0, brightness=0, damping=0, position=0, space=0),
        dict(geometry=1, brightness=1, damping=1, position=1, space=1, strength=1),
        dict(geometry=1, brightness=1, damping=1, bow=1, blow=1, strength=1, freeze=1),
        dict(mallet=0.95, strike_timbre=1, damping=1, brightness=1),
    ]:
        for note in ("c0", "c3", "c7"):
            CASES.append((model, note, combo))

if __name__ == "__main__":
    for sr in (44100.0, 96000.0):
        inst = elh.A.Instrument(sys.argv[1], sample_rate=sr)
        bad = 0
        peaks = []
        for model, note, combo in CASES:
            params = dict(combo, model=model, level_db=-12.0)
            y, _ = inst.render(1.5, pitch=note, params=params, gate_off=0.8)
            y = np.asarray(y)[:, :2]
            finite = np.isfinite(y).all()
            peak = float(np.abs(y).max())
            rms = float(np.sqrt(np.mean(y ** 2)))
            peaks.append(peak)
            if not finite or rms < 1e-6 or peak > 1.01:
                bad += 1
                print(f"  sr {sr:.0f} model {model} {note} {combo}: finite {finite} rms {rms:.2e} peak {peak:.3f}")
        print(f"sr {sr:.0f}: {len(CASES)} renders, {bad} flagged, max peak {max(peaks):.3f}, min peak {min(peaks):.2e}", flush=True)
