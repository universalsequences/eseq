#!/usr/bin/env python3
"""String / Strings models vs the C++ Part (deterministic noise)."""
import os, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import elh

S1 = dict(det=1, det_d=48, det_str=32, space=0, model=1)
S5 = dict(det=1, det_d=112, det_str=32, space=0, model=2)
CASES = [
    ("string default", S1, dict()),
    ("string note 24", S1, dict(note=24)),
    ("string note 84", S1, dict(note=84)),
    ("string note 0 (src)", S1, dict(note=0)),
    ("string geo 0 (bridge)", S1, dict(geometry=0.0)),
    ("string geo .25", S1, dict(geometry=0.25)),
    ("string geo .6 (disp)", S1, dict(geometry=0.6)),
    ("string geo .95 (noise)", S1, dict(geometry=0.95)),
    ("string bright 0", S1, dict(brightness=0.0)),
    ("string bright 1", S1, dict(brightness=1.0)),
    ("string damp .97", S1, dict(damping=0.97)),
    ("string damp 0", S1, dict(damping=0.0)),
    ("string pos 0", S1, dict(position=0.0)),
    ("string pos .5", S1, dict(position=0.5, note=84)),
    ("string bow", S1, dict(bow=0.7, strike=0)),
    ("string blow", S1, dict(blow=0.6, strike=0)),
    ("string sweep note", S1, dict(note=30, note_end=80, seconds=2, contour=0.5, blow=0.5)),
    ("strings default", S5, dict()),
    ("strings chord 3", S5, dict(geometry=0.3)),
    ("strings chord 7", S5, dict(geometry=0.7, brightness=0.8)),
    ("strings chord 10", S5, dict(geometry=1.0)),
    ("strings sweep geo", S5, dict(geometry=0.0, geometry_end=1.0, seconds=2, contour=0.5, blow=0.5)),
    ("strings bow", S5, dict(bow=0.7, strike=0)),
]

if __name__ == "__main__":
    t = elh.Test()
    only = sys.argv[1:]
    for label, base, kw in CASES:
        if only and not any(o in label for o in only):
            continue
        args = dict(base, seconds=1)
        args.update(kw)
        t.compare(label, chans=("main", "aux", "raw", "center", "sides"), **args)
