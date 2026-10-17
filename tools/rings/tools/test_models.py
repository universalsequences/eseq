#!/usr/bin/env python3
"""Per-model comparison against ref/rref with deterministic noise
(RREF_U / --const-u): settings across the controls' ranges, sweeps and
retriggers. usage: python3 tools/test_models.py [model ...]"""
import sys
import rh

CASES = [
    ("mid", dict(structure=0.5, brightness=0.5, damping=0.5, position=0.5)),
    ("low", dict(structure=0.0, brightness=0.0, damping=0.0, position=0.0, note=36)),
    ("high", dict(structure=0.9995, brightness=1.0, damping=0.9, position=1.0, note=72)),
    ("sustain", dict(structure=0.25, brightness=0.7, damping=1.0, position=0.2)),
    ("bright", dict(structure=0.8, brightness=0.95, damping=0.3, position=0.7, note=84)),
    ("deep", dict(structure=0.4, brightness=0.4, damping=0.7, position=0.4, note=16)),
    ("sweeps", dict(structure=0.1, structure_end=0.9, brightness=0.2, brightness_end=0.9,
                    damping=0.3, damping_end=0.8, position=0.9, position_end=0.1)),
    ("note sweep", dict(note=40, note_end=70, structure=0.6, brightness=0.6)),
    ("retrig", dict(retrig=0.25, structure=0.3, brightness=0.6, damping=0.6, position=0.3)),
]


def main():
    models = [int(a) for a in sys.argv[1:]] or list(range(12))
    t = rh.VoiceTest(const_u=0.73)
    worst = []
    for m in models:
        for label, kw in CASES:
            _, _, res = t.compare(f"m{m} {label}", model=m, seconds=1.0, **kw)
            worst.append((max(e0 for *_, e0 in res), max(e for *_, e, _e0 in res), m, label))
    worst.sort(reverse=True)
    print("worst e100:", [(round(a, 4), round(b, 4), m, l) for a, b, m, l in worst[:6]])


if __name__ == "__main__":
    main()
