#!/usr/bin/env python3
"""Third-octave shape of the first pluck's buzz region (1-16 kHz, 60-470 ms): record vs model."""
import json
import sys
from pathlib import Path

import numpy as np
import scipy.signal as ss
import soundfile as sf

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(ROOT/'.local/pm-guitar'))
CENTRES = 1000*2**(np.arange(0, 13)/3)        # 1 kHz .. 16 kHz


def third_octaves(y, sr, a=0.06, b=0.47):
    out = []
    for fc in CENTRES:
        lo, hi = fc*2**(-1/6), min(fc*2**(1/6), 0.45*sr)
        f = ss.sosfiltfilt(ss.butter(4, [lo, hi], 'bandpass', fs=sr, output='sos'), y)
        out.append(10*np.log10(np.mean(f[int(a*sr):int(b*sr)]**2) + 1e-14))
    return np.array(out)


def reference(sr=48000):
    hero = json.loads((HERE/'hero.json').read_text())
    x, _ = sf.read(ROOT/'.local/pm-guitar/nascer-20s.wav')
    m = x.mean(1)
    return third_octaves(m[int(hero['onset_s']*sr):int((hero['onset_s'] + 0.48)*sr)], sr)


def model(params, sr=48000):
    from buzz_try import inst, t, j, c, shift
    p = {'pluck.take': j/max(c - 1, 1), 'pluck.vel_take': 0, 'pluck.humanize': 0} | params
    y = inst.render(seconds=0.48, pitch=float(t['ref_hz'][0]), vel=0.6, params=p)[0][:, 0].astype(float)
    return third_octaves(y*10**(-shift/20), sr)


if __name__ == '__main__':
    r = reference()
    m = model({})
    print('kHz  ' + ' '.join(f'{c/1000:5.1f}' for c in CENTRES))
    print('ref  ' + ' '.join(f'{v:5.0f}' for v in r))
    print('mod  ' + ' '.join(f'{v:5.0f}' for v in m))
    print('err  ' + ' '.join(f'{v:+5.1f}' for v in m - r))
