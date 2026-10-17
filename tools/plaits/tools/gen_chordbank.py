#!/usr/bin/env python3
"""Generate the constant tables of engines/chordbank.lisp (ChordBank ratios,
ComputeChordInversion voice map) and print them as inline @data lists."""
import re, os, numpy as np
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
U = open(os.path.join(os.environ["EURORACK"], "stmlib", "dsp", "units.cc")).read()
def arr(name):
    body = re.search(name + r"\[\]\s*=\s*\{(.*?)\};", U, re.S).group(1)
    return np.array([float(v) for v in re.findall(r"-?\d+\.?\d*(?:e[-+]?\d+)?", body)], dtype=np.float32)
hi, lo = arr("lut_pitch_ratio_high"), arr("lut_pitch_ratio_low")
def s2r(s):
    p = np.float32(s) + np.float32(128)
    i = int(np.floor(p)); f = np.float32(p - i)
    return np.float32(hi[i] * lo[int(f * np.float32(256))])
chords = [[0, .01, 11.99, 12], [0, 7, 7.01, 12], [0, 5, 7, 12], [0, 3, 7, 12], [0, 3, 7, 10],
          [0, 3, 10, 14], [0, 3, 10, 17], [0, 2, 9, 16], [0, 4, 11, 14], [0, 4, 7, 11], [0, 4, 7, 12]]
ratios = [float(s2r(np.float32(c))) for ch in chords for c in ch]
print("ratios", " ".join(repr(r) for r in ratios))
# ComputeChordInversion voice map per inversion_integral 0..20:
# per voice: note j, ratio multiplier, amp = 0.25 * (a + b * frac), aux-mask bit
inv = []
for ii in range(21):
    nr, rn = ii // 4, ii % 4
    v = {}
    mask = 0
    for i in range(4):
        tr = 0.25 * (1 << ((3 + ii - i) // 4))
        tv = (i - nr + 5) % 5; pv = (tv - 1 + 5) % 5
        if i == rn:
            v[tv] = (i, tr, 1, -1); v[pv] = (i, tr * 2, 0, 1)
        elif i < rn:
            v[pv] = (i, tr, 1, 0)
        else:
            v[tv] = (i, tr, 1, 0)
        if i == 0:
            if i >= rn: mask |= 1 << tv
            if i <= rn: mask |= 1 << pv
    assert sorted(v) == [0, 1, 2, 3, 4], (ii, v)
    for vo in range(5):
        j, m, a, b = v[vo]
        inv += [j, m, a, b, (mask >> vo) & 1]
print("inv", " ".join(f"{x:g}" for x in inv))
