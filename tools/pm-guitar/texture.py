#!/usr/bin/env python3
"""Texture metrics that band levels miss, reference first pluck vs model.

buzz: 2-9 kHz crest factor and kurtosis (spikiness), per-cycle modulation
depth (dB between loudest and quietest of 12 phase bins of the note period).
ring: per-partial envelope roughness - RMS deviation (dB) of each partial's
envelope from its best straight-line (exponential) fit over 60-470 ms, i.e.
beating/fluctuation a single damped pole cannot make.
"""
import json
import sys
from pathlib import Path

import numpy as np
import scipy.signal as ss
import soundfile as sf

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
sys.path.insert(0, str(HERE))
from partials import baseband   # noqa: E402


def metrics(y, sr, f0, B, span=0.47):
    out = {}
    hf = ss.sosfiltfilt(ss.butter(4, [2000, 9000], 'bandpass', fs=sr, output='sos'), y)
    seg = hf[int(0.06*sr):int(span*sr)]
    out['buzz_crest_db'] = round(float(20*np.log10(np.abs(seg).max()/np.sqrt(np.mean(seg**2)))), 1)
    out['buzz_kurtosis'] = round(float(np.mean(seg**4)/np.mean(seg**2)**2), 1)
    env = ss.sosfiltfilt(ss.butter(2, 900, 'lowpass', fs=sr, output='sos'), np.abs(ss.hilbert(hf)))[int(0.06*sr):int(span*sr)]
    T = 1/f0
    ph = (np.arange(len(env))/sr % T)/T
    bins = [env[(ph >= i/12) & (ph < (i + 1)/12)].mean()**2 for i in range(12)]
    out['buzz_cycle_depth_db'] = round(float(10*np.log10(max(bins)/min(bins))), 1)
    rough = []
    for k in range(1, 11):
        hz = k*f0*np.sqrt(1 + B*k*k)
        z, dt = baseband(y, sr, hz, 0.5*f0, 0, int(span*sr))
        t = np.arange(len(z))*dt
        sel = (t > 0.06) & (t < span - 0.02)
        db = 20*np.log10(np.abs(z[sel]) + 1e-9)
        A = np.c_[np.ones(sel.sum()), t[sel]]
        fit = A@np.linalg.lstsq(A, db, rcond=None)[0]
        rough.append(round(float(np.sqrt(np.mean((db - fit)**2))), 2))
    out['partial_roughness_db'] = rough
    return out


def main():
    hero = json.loads((HERE/'hero.json').read_text())
    x, sr = sf.read(ROOT/'.local/pm-guitar/nascer-20s.wav')
    m = x.mean(1)
    ref = m[int(hero['onset_s']*sr):int((hero['onset_s'] + 0.48)*sr)]
    sys.path.insert(0, str(ROOT/'.local/pm-guitar'))
    from buzz_try import inst, t, j, c, shift
    params = {'pluck.take': j/max(c - 1, 1), 'pluck.vel_take': 0, 'pluck.humanize': 0}
    y = inst.render(seconds=0.48, pitch=float(t['ref_hz'][0]), vel=0.6, params=params)[0][:, 0].astype(float)
    y0 = inst.render(seconds=0.48, pitch=float(t['ref_hz'][0]), vel=0.6, params=params | {'fret.buzz': 0.0})[0][:, 0].astype(float)
    res = {}
    for name, sig in (('reference', ref), ('model', y), ('model_no_buzz', y0)):
        res[name] = metrics(sig, sr, hero['f0_hz'], hero['B'])
        print(name, res[name])
    (HERE/'texture.json').write_text(json.dumps(res, indent=1) + '\n')


if __name__ == '__main__':
    main()
