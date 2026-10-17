#!/usr/bin/env python3
"""Modal model + exciters vs the C++ Part (deterministic noise)."""
import os, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import elh

D = dict(det=1, det_d=32, space=0)
CASES = [
    ("mallet default", dict()),
    ("mallet note 24", dict(note=24)),
    ("mallet note 84", dict(note=84)),
    ("mallet note 100", dict(note=100)),
    ("sample meta 0", dict(strike_meta=0.0)),
    ("sample meta .2", dict(strike_meta=0.2, strike_timbre=0.8)),
    ("sample meta .38", dict(strike_meta=0.38, strike_timbre=0.2)),
    ("mallet meta .45 t.9", dict(strike_meta=0.45, strike_timbre=0.9)),
    ("plectrum meta .7", dict(strike_meta=0.7, signature=0.3)),
    ("plectrum meta .78", dict(strike_meta=0.78, strike_timbre=0.2)),
    ("strike 1.0 bleed", dict(strike=1.0)),
    ("bow .9 timbre .2", dict(bow=0.9, bow_timbre=0.2, strike=0)),
    ("bow+strike", dict(bow=0.6, strike=0.5)),
    ("blow .5", dict(blow=0.5, strike=0)),
    ("blow .5 meta .1 t .9", dict(blow=0.5, blow_meta=0.1, blow_timbre=0.9, strike=0)),
    ("blow .9 tube", dict(blow=0.9, strike=0)),
    ("blow 1 tube t .2", dict(blow=1.0, blow_timbre=0.2, strike=0, note=36)),
    ("contour 0", dict(contour=0.0, bow=0.5)),
    ("contour .3", dict(contour=0.3, bow=0.5, gate=0.3)),
    ("contour .5", dict(contour=0.5, bow=0.5, gate=0.5)),
    ("contour .8 gate .4", dict(contour=0.8, bow=0.5, gate=0.4)),
    ("retrig .25", dict(retrig=0.25, gate=0.12, strike_meta=0.2)),
    ("geometry 0", dict(geometry=0.0)),
    ("geometry .5", dict(geometry=0.5)),
    ("geometry .95", dict(geometry=0.95)),
    ("geometry 1", dict(geometry=1.0)),
    ("bright 0 damp .9", dict(brightness=0.0, damping=0.9)),
    ("bright 1 damp 0", dict(brightness=1.0, damping=0.0)),
    ("position 0", dict(position=0.0)),
    ("position .95", dict(position=0.95)),
    ("strength 0", dict(strength=0.0)),
    ("strength 1", dict(strength=1.0, bow=0.5)),
    ("sweep geometry", dict(geometry=0.0, geometry_end=1.0, seconds=2, contour=0.5, bow=0.5)),
    ("sweep position", dict(position=0.0, position_end=1.0, seconds=2, contour=0.5, bow=0.5)),
    ("sweep note", dict(note=30, note_end=90, seconds=2, contour=0.5, bow=0.5)),
    ("sweep damping", dict(damping=0.0, damping_end=1.0, seconds=2, contour=0.5, bow=0.5)),
]

if __name__ == "__main__":
    t = elh.Test()
    only = sys.argv[1:]
    for label, kw in CASES:
        if only and not any(o in label for o in only):
            continue
        args = dict(D, seconds=1)
        args.update(kw)
        t.compare(label, chans=("main", "aux", "center"), **args)
