#!/usr/bin/env python3
"""Part: space (raw gain, spread, reverb, freeze) and the Ominous voice."""
import os, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import elh

D = dict(det=1, det_d=32)
CASES = [
    ("space 0", dict(space=0)),
    ("space .07", dict(space=0.07)),
    ("space .3", dict(space=0.3)),
    ("space .6", dict(space=0.6)),
    ("space .9", dict(space=0.9, contour=0.5, gate=0.3, blow=0.4)),
    ("space 1", dict(space=1.0, seconds=2)),
    ("freeze", dict(space=1.8, seconds=2)),
    ("space .9 diff/lp", dict(space=0.9, rev_diffusion=0.7, rev_lp=0.9)),
    ("omin default", dict(easter=1, space=0)),
    ("omin sustain", dict(easter=1, space=0, contour=0.8, gate=0.5, blow=0.8, strike=0.6)),
    ("omin fm", dict(easter=1, space=0, contour=0.5, blow=0.9, blow_meta=0.7, blow_timbre=0.8,
                     strike=0.7, strike_meta=0.3, strike_timbre=0.6, bow_timbre=0.7)),
    ("omin filter", dict(easter=1, space=0, contour=0.8, blow=0.8, geometry=0.8, brightness=0.3, damping=0.9)),
    ("omin detune pos", dict(easter=1, space=0, contour=0.8, blow=0.5, strike=0.5, bow=0.7, position=0.9)),
    ("omin strength", dict(easter=1, space=0, contour=0.2, blow=0.8, strength=0.9)),
    ("omin space .8", dict(easter=1, space=0.8, contour=0.8, blow=0.8)),
    ("omin sweep note", dict(easter=1, space=0, contour=0.8, blow=0.8, note=30, note_end=90, seconds=2)),
]

if __name__ == "__main__":
    t = elh.Test()
    only = sys.argv[1:]
    for label, kw in CASES:
        if only and not any(o in label for o in only):
            continue
        args = dict(D, seconds=1)
        args.update(kw)
        t.compare(label, chans=("main", "aux", "raw", "center", "sides"), **args)
