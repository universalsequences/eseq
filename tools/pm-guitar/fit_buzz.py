#!/usr/bin/env python3
"""Fit the fret collision (gap, fret point, bounce, click/rattle) to the first pluck's buzz.

Target: the first hit's band levels in 10-60/60-150/150-300/300-470 ms, all
bands (the collision changes the string too). Above 1.5 kHz the hit is
essentially all buzz (hero.py residual). Writes buzz.json (instrument
defaults), then rebuild.
"""
import json
import sys
import time
from pathlib import Path

import numpy as np
from scipy.optimize import minimize

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(ROOT/'.local/pm-guitar'))
from buzz_try import run, R   # noqa: E402

NAMES = ['fret.buzz', 'fret.relief', 'fret.hardness', 'fret.alpha', 'fret.contact_loss', 'body.wood', 'fret.upper_decay']
LO = np.array([0.3, 0.0, 0.003, 1.0, 0.0, 2000, 0.5])
HI = np.array([0.995, 3.0, 1.0, 2.5, 1.0, 16000, 2.0])
WEIGHT = np.array([0.5, 0.5, 1.0, 1.0, 0.7])[:, None]     # bands: low, mid, 1.5-4k, 4-9k, 9-16k


LOG = {2, 5}     # hardness and zing decay span decades


def params(u):
    u = np.clip(u, 0, 1)
    v = LO + (HI - LO)*u
    for i in LOG:
        lo = max(LO[i], HI[i]*1e-3)
        v[i] = lo*(HI[i]/lo)**u[i]
    return dict(zip(NAMES, v))


from texture import metrics           # noqa: E402
from buzz_try import inst, t, j, c, shift  # noqa: E402
HERO = json.loads((HERE/'hero.json').read_text())
TEX = json.loads((HERE/'texture.json').read_text())['reference']


def texture_error(p):
    y = inst.render(seconds=0.48, pitch=float(t['ref_hz'][0]), vel=0.6,
                    params={'pluck.take': j/max(c - 1, 1), 'pluck.vel_take': 0, 'pluck.humanize': 0} | p)[0][:, 0].astype(float)
    m = metrics(y, 48000, HERO['f0_hz'], HERO['B'])
    # dB-like terms: spikiness, per-cycle depth, envelope roughness
    return np.array([m['buzz_crest_db'] - TEX['buzz_crest_db'],
                     10*np.log10(m['buzz_kurtosis']/TEX['buzz_kurtosis']),
                     m['buzz_cycle_depth_db'] - TEX['buzz_cycle_depth_db'],
                     np.mean(m['partial_roughness_db']) - np.mean(TEX['partial_roughness_db'])])


import buzz_spectrum   # noqa: E402
SPEC_REF = buzz_spectrum.reference()
SPEC_W = (buzz_spectrum.CENTRES < 13000).astype(float)   # 16 kHz band is at the record's floor


def spectrum_error(p):
    return np.clip(buzz_spectrum.model(p) - SPEC_REF, -30, 30)*SPEC_W


def cost(u):
    p = params(u)
    M = run(p)
    e = np.clip(M - R, -30, 30)
    tex = np.clip(texture_error(p), -30, 30)
    spec = spectrum_error(p)
    # band/time levels, texture and the buzz's third-octave shape (the ear's harshness)
    return float(np.sqrt((np.sum((WEIGHT*e)**2) + 4*np.sum(tex**2) + 2*np.sum(spec**2))
                         /(e.size + 4*len(tex) + 2*SPEC_W.sum())))


def main():
    rng = np.random.default_rng(1)
    t0 = time.time()
    best = None
    for i in range(160):
        u = rng.uniform(0, 1, len(NAMES))
        c = cost(u)
        if best is None or c < best[0]:
            best = (c, u)
            print(f'{i:3d} {c:6.2f}', {k: round(v, 3) for k, v in params(u).items()}, flush=True)
    sol = minimize(cost, best[1], method='Nelder-Mead', options={'maxfev': 220, 'xatol': 0.01, 'fatol': 0.02})
    p = params(sol.x)
    print('final', round(sol.fun, 2), {k: round(v, 3) for k, v in p.items()}, f'{time.time() - t0:.0f}s')
    print((run(p) - R).round(1))
    print('texture error (crest dB, kurtosis dB, cycle depth dB, roughness dB):', texture_error(p).round(1))
    print('third-octave buzz error 1-16 kHz:', spectrum_error(p).round(1))
    keys = {'fret.buzz': 'buzz_default', 'fret.relief': 'relief_default', 'fret.hardness': 'hardness_default',
            'fret.alpha': 'alpha_default', 'fret.contact_loss': 'contact_loss_default', 'body.wood': 'wood_default',
            'fret.upper_decay': 'upper_decay_default'}
    out = json.loads((HERE/'buzz.json').read_text()) if (HERE/'buzz.json').exists() else {}
    out.update({keys[k]: round(float(v), 4) for k, v in p.items()})
    out['fit_rms_db'] = round(float(sol.fun), 3)
    (HERE/'buzz.json').write_text(json.dumps(out, indent=1) + '\n')


if __name__ == '__main__':
    main()
