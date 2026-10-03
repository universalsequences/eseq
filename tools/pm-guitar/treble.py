#!/usr/bin/env python3
"""Brightness across the neck: recorded plucks vs the model (writes treble.json).

For each recorded pitch: median fitted tilt alpha, roll-off fc and the
energy centroid of the first 12 partials in partial-number units. For the
model: centroid (Hz and partial units) of 10-150 ms, and the 1-3 kHz to
total energy ratio at 150-400 ms (long-ringing upper partials read as
metallic).
"""
import json
import sys
from pathlib import Path

import numpy as np
import scipy.signal as ss

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))


def model_brightness(y, sr, f0):
    out = {}
    for name, (a, b) in (('early', (0.01, 0.15)), ('late', (0.15, 0.4))):
        seg = y[int(a*sr):int(b*sr)]
        S = np.abs(np.fft.rfft(seg*np.hanning(len(seg))))**2
        fr = np.fft.rfftfreq(len(seg), 1/sr)
        out[name + '_centroid_hz'] = float(np.sum(fr*S)/np.sum(S))
        out[name + '_centroid_k'] = out[name + '_centroid_hz']/f0
        out[name + '_above_1k_db'] = float(10*np.log10(np.sum(S[fr > 1000])/np.sum(S)))
    return out


def main():
    model = json.loads((HERE/'model.json').read_text())
    rec = {}
    for pi, p in enumerate(model['pitches']):
        tk = [t for t in model['takes'] if t['pitch'] == pi]
        rec[p['name']] = {'f0': p['f0_hz'], 'takes': len(tk), 'alpha_median': float(np.median([t['alpha'] for t in tk])),
                          'fc_median': float(np.median([t['fc_hz'] for t in tk])),
                          'beta_median': float(np.median([t['beta'] for t in tk]))}
        print(f"{p['name']:4s} takes {len(tk):2d} alpha {rec[p['name']]['alpha_median']:.2f} fc {rec[p['name']]['fc_median']:6.0f} beta {rec[p['name']]['beta_median']:.3f}")
    from render import instrument, hz
    inst = instrument(48000)
    mod = {}
    for note in range(40, 89, 4):
        y = inst.render(seconds=0.4, pitch=hz(note), vel=0.7, params={'pluck.humanize': 0.0})[0][:, 0].astype(float)
        mod[note] = model_brightness(y, 48000, hz(note))
        b = mod[note]
        print(f"key {note}: early centroid {b['early_centroid_hz']:6.0f} Hz ({b['early_centroid_k']:.1f} x f0), >1k {b['early_above_1k_db']:5.1f} dB | late >1k {b['late_above_1k_db']:5.1f} dB")
    (HERE/'treble.json').write_text(json.dumps({'recorded': rec, 'model': mod}, indent=1) + '\n')


if __name__ == '__main__':
    main()
