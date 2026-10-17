#!/usr/bin/env python3
"""Chord bank tables for engines/chip.lisp (plaits/dsp/chords/chord_bank.cc).
Ratios use stmlib SemitonesToRatio (LUT product) in float32, like ChordBank::Reset."""
import os, re
import numpy as np
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
src = open(os.path.join(os.environ["EURORACK"], "stmlib/dsp/units.cc")).read()
def arr(n):
    m = re.search(n + r"\[\]\s*=\s*\{(.*?)\};", src, re.S)
    return np.array([float(v) for v in re.findall(r"-?\d+\.?\d*(?:e[-+]?\d+)?", m.group(1))], dtype=np.float32)
HI, LO = arr("lut_pitch_ratio_high"), arr("lut_pitch_ratio_low")
def s2r(semi):
    p = np.float32(semi) + np.float32(128); i = int(p); f = p - np.float32(i)
    return np.float32(HI[i] * LO[int(f * np.float32(256))])
CH = [[0.00, 0.01, 11.99, 12.00], [0.00, 7.00, 7.01, 12.00], [0.00, 5.00, 7.00, 12.00],
      [0.00, 3.00, 7.00, 12.00], [0.00, 3.00, 7.00, 10.00], [0.00, 3.00, 10.00, 14.00],
      [0.00, 3.00, 10.00, 17.00], [0.00, 2.00, 9.00, 16.00], [0.00, 4.00, 11.00, 14.00],
      [0.00, 4.00, 7.00, 11.00], [0.00, 4.00, 7.00, 12.00]]
ratios, sorted_r, counts = [], [], []
for c in CH:
    r = [s2r(v) for v in c]
    ratios += r
    counts.append(sum(1 for v in c if v not in (0.01, 7.01, 11.99, 12.00)))
    s = []
    for x in r:
        while x > 2: x = np.float32(x * np.float32(0.5))
        s.append(x)
    sorted_r += sorted(s)
fmt = lambda a: " ".join(repr(float(x)) for x in a)
print(f"(def chip_ratios (tensor @shape [44 1] @data [{fmt(ratios)}]))")
print(f"(def chip_sorted (tensor @shape [44 1] @data [{fmt(sorted_r)}]))")
print(f"(def chip_counts (tensor @shape [11 1] @data [{fmt(counts)}]))")
