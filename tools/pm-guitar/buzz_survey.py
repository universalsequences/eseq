#!/usr/bin/env python3
"""How much does each recorded pluck buzz? (writes buzz-survey.json)

Buzz = 2-9 kHz energy that repeats at the note's own period (the string
hitting a fret once per cycle). Index: autocorrelation of the 2-9 kHz
envelope at the note period over 60-250 ms (or to the next pluck), and the
2-9 kHz level relative to the 70-1500 Hz string band.
"""
import json
import sys
from pathlib import Path

import numpy as np
import scipy.signal as ss
import soundfile as sf

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]


def main():
    model = json.loads((HERE/'model.json').read_text())
    x, sr = sf.read(ROOT/'.local/pm-guitar/nascer-20s.wav')
    m = x.mean(1)
    hf = ss.sosfiltfilt(ss.butter(4, [2000, 9000], 'bandpass', fs=sr, output='sos'), m)
    lo = ss.sosfiltfilt(ss.butter(4, [70, 1500], 'bandpass', fs=sr, output='sos'), m)
    env = ss.sosfiltfilt(ss.butter(2, 800, 'lowpass', fs=sr, output='sos'), np.abs(ss.hilbert(hf)))
    out = []
    for p in model['pitches']:
        T = 1/p['f0_hz']
        for i, t in enumerate(p['takes']):
            a = t['onset_s'] + 0.06
            b = min(t['onset_s'] + 0.25, t['end_s'] - 0.005)
            if b - a < 3*T:
                out.append({'pitch': p['name'], 'take': i, 'onset_s': t['onset_s'], 'periodicity': None})
                continue
            w = env[int(a*sr):int(b*sr)]
            w = w - w.mean()
            lag = int(round(T*sr))
            ac = float(np.dot(w[:-lag], w[lag:])/np.sqrt(np.dot(w[:-lag], w[:-lag])*np.dot(w[lag:], w[lag:])))
            rel = 10*np.log10(np.mean(hf[int(a*sr):int(b*sr)]**2)/np.mean(lo[int(a*sr):int(b*sr)]**2))
            out.append({'pitch': p['name'], 'take': i, 'onset_s': t['onset_s'], 'periodicity': round(ac, 3),
                        'hf_rel_db': round(float(rel), 1)})
    for o in out:
        print(o)
    (HERE/'buzz-survey.json').write_text(json.dumps(out, indent=1) + '\n')


if __name__ == '__main__':
    main()
