#!/usr/bin/env python3
"""Model switches while playing: every model -> every other model, at a few
host rates and switch times (mid-block and mid-process-call), with notes
before and after. Flags output that stops being audio: non-finite samples,
or a level stuck at one value (a poisoned feedback state shows up as the
output clipped to a constant). The models run behind gates that open on a
process call, mid-way through a 0.5 ms block, which the sample-level suites
(one model per render) never exercise.

    python3 tools/test_switch.py [INSTRUMENT_DIR]
"""
import os, sys
import numpy as np
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import elh

A = elh.A


def main():
    inst_dir = sys.argv[1] if len(sys.argv) > 1 else elh.INSTRUMENT
    flagged = 0
    for sr in (48000.0, 44100.0, 96000.0):
        inst = A.Instrument(inst_dir, sample_rate=sr, max_frames=128)
        checked = 0
        for at in (1.0101, 1.3333):
            for a in range(4):
                for b in range(4):
                    if a == b:
                        continue
                    y, _ = inst.render(2.5, pitch="c3", params={"model": a},
                                       ramps={"model": [(0, a), (at, a), (at + 1e-4, b), (2.5, b)]},
                                       retrig=[0.5, at + 0.2, at + 0.6, at + 0.9])
                    checked += 1
                    tail = y[int((at + 0.3) * sr):]
                    nonfinite = int((~np.isfinite(tail)).sum())
                    stuck = bool(np.all(tail.std(axis=0) < 1e-6) and np.abs(tail).max() > 1e-3)
                    if nonfinite or stuck:
                        flagged += 1
                        print(f"FLAG sr {sr:.0f} switch {a}->{b} at {at}s: nonfinite {nonfinite} "
                              f"stuck {stuck} level {np.abs(tail).max():.3f}", flush=True)
        print(f"sr {sr:.0f}: {checked} switches checked", flush=True)
    print("ok" if not flagged else f"{flagged} flagged")
    sys.exit(1 if flagged else 0)


if __name__ == "__main__":
    main()
