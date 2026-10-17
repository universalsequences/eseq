#!/usr/bin/env python3
"""Robustness: every model at extreme settings and pitches, at 44.1 and
96 kHz: no NaN/Inf, no blow-up, sound on every strum."""
import itertools, os, subprocess, sys
import numpy as np
import rh

EXTREMES = [dict(structure=s, brightness=b, damping=d, position=p)
            for s, b, d, p in itertools.product((0, 1), (0, 1), (0, 1), (0, 1))]
NOTES = (0, 24, 60, 96, 127)


def main():
    rates = [float(a) for a in sys.argv[1:]] or [44100.0, 96000.0]
    bad = 0
    for sr in rates:
        d = os.path.join(rh.WORK, "tests", f"robust_{int(sr)}")
        subprocess.run([sys.executable, os.path.join(rh.HERE, "assemble.py"), d], check=True, stdout=subprocess.DEVNULL)
        inst = rh.A.Instrument(d, max_frames=128, sample_rate=sr)
        worst_peak, quiet = 0.0, []
        for m in range(12):
            for i, ex in enumerate(EXTREMES):
                note = NOTES[i % len(NOTES)]
                hz = 440 * 2 ** ((note - 69) / 12)
                y, _ = inst.render(0.6, pitch=hz, params=dict(ex, model=m, level_db=0), retrig=[0.2, 0.21, 0.4])
                y = np.asarray(y)
                if not np.all(np.isfinite(y)):
                    print(f"NaN/Inf: sr {sr} model {m} note {note} {ex}")
                    bad += 1
                peak = float(np.abs(y).max())
                worst_peak = max(worst_peak, peak)
                for a, b in ((0, 0.2), (0.4, 0.6)):
                    seg = y[int(a * sr):int(b * sr)]
                    if float(np.abs(seg).max()) < 1e-5:
                        quiet.append((m, note, tuple(ex.values()), a))
        print(f"sr {sr:.0f}: worst peak {worst_peak:.3f}, silent strums {len(quiet)}")
        for q in quiet:
            print("   silent:", q)
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
