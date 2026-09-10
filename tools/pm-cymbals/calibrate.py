#!/usr/bin/env python3
"""Fit radiation coefficients against the compiled physical impulse response.

The quadratic spectral forms retain correlations between overlapping radiation
filters. Independent band powers alone would double-count that energy. Nothing
from these analysis windows is played or scheduled by the instrument.
"""
import argparse
import json
from pathlib import Path

import numpy as np
from scipy.optimize import least_squares, nnls
import soundfile as sf

from common import HERE, ROOT, digest, instrument, read_reference
from engine import BANDS, basis_source

SR = 48000
WINDOWS = [(0, .02), (.02, .06), (.06, .15), (.15, .35), (.35, .7),
           (.7, 1.2), (1.2, 2), (2, 3.5), (3.5, 5.5)]
F = np.fft.rfftfreq(8192, 1/SR)
EDGES = np.r_[0, np.sqrt(BANDS[:-1]*BANDS[1:]), SR/2+1]
MASKS = [(F >= a) & (F < b) for a, b in zip(EDGES, EDGES[1:])]


def moments(y, windows):
    result = []
    for a, b in windows:
        x = y[round(a*SR):round(b*SR)]
        length = min(len(x), 4096)
        positions = np.unique(np.linspace(0, len(x)-length, 3).astype(int))
        w = np.hanning(length)
        transforms = [np.fft.rfft(x[p:p+length]*w[:, None], n=8192, axis=0)
                      * np.sqrt(2/(8192*np.sum(w*w)*len(positions))) for p in positions]
        for mask in MASKS:
            result.append(sum((z[mask].conj().T@z[mask]).real for z in transforms))
    return np.array(result)


def fit_gains(q, target, times=None, rates=None, mode_caps=None):
    # Ignore observations buried >50 dB below the recording's strongest band;
    # they mostly constrain recording hiss, not the free plate response.
    active = target > max(target.max()*1e-5, 1e-12)
    q, target = q[active], target[active]
    times = times[active] if times is not None else None
    norm = np.sqrt(np.max(np.diagonal(q, axis1=1, axis2=2), axis=0))
    norm = np.maximum(norm, 1e-10)
    q = q/norm[None, :, None]/norm[None, None, :]
    initial, _ = nnls(np.diagonal(q, axis1=1, axis2=2)/target[:, None], np.ones(len(target)), maxiter=1000)
    initial = np.sqrt(np.maximum(initial, 1e-12))
    groups = np.r_[np.repeat(np.arange(6), 3), np.full(8, 6)]
    def vectors(v):
        if rates is None:
            return np.broadcast_to(v, (len(q), 26)), np.ones((len(q), 26)), np.zeros(len(q))
        exponents = -times[:, None]*np.r_[v[26:], 0][groups][None, :]
        # Factor out the largest exponent before evaluating the quadratic
        # form. A proposal that lengthens a short decay can otherwise overflow
        # at late observation times. Restore that common scale in log power;
        # it cancels from both gain and rate derivatives.
        log_scale = np.max(exponents, axis=1)
        factors = np.exp(exponents-log_scale[:, None])
        return v[:26]*factors, factors, log_scale
    def residual(v):
        h, _, log_scale = vectors(v)
        power = np.maximum(np.einsum('wi,wij,wj->w', h, q, h), 1e-20)
        return 10*np.log10(power/target)+(20/np.log(10))*log_scale
    def jacobian(v):
        h, factors, _ = vectors(v)
        qg = np.einsum('wij,wj->wi', q, h)
        power = np.maximum(np.sum(qg*h, axis=1), 1e-20)
        jac = (20/np.log(10))*qg*factors/power[:, None]
        if rates is not None:
            dr = np.column_stack([np.sum(qg[:, groups == i]*h[:, groups == i], axis=1) for i in range(6)])
            jac = np.c_[jac, (-20/np.log(10))*times[:, None]*dr/power[:, None]]
        return jac
    if rates is None:
        lower, upper = np.zeros(26), np.full(26, np.inf)
    else:
        initial = np.r_[initial, np.zeros(6)]
        lower = np.r_[np.zeros(26), .1-rates]
        upper = np.r_[np.full(26, np.inf), np.maximum(3*rates, 8)]
    if mode_caps is not None:
        upper[18:26] = np.maximum(np.array(mode_caps)*norm[18:], 1e-12)
        initial = np.minimum(initial, upper*.999)
    fit = least_squares(residual, initial, jac=jacobian, bounds=(lower, upper),
                        loss='soft_l1', f_scale=3, max_nfev=100, ftol=1e-6)
    error = residual(fit.x)
    # Score balances typical detail with large individual spectral misses.
    score = float(np.mean(np.sqrt(9+error**2)-3))
    return fit.x[:26]/norm, (rates+fit.x[26:] if rates is not None else None), {'score': score, 'median_abs_db': float(np.median(abs(error))),
                       'rms_db': float(np.sqrt(np.mean(error**2))),
                       'p90_abs_db': float(np.percentile(abs(error), 90))}


def calibrate(inst, record, output):
    y, _, _ = read_reference(ROOT/record['file'])
    windows = [(a, min(b, len(y)/SR)) for a, b in WINDOWS if min(b, len(y)/SR)-a >= .015]
    target = moments(y, windows)[:, 0, 0]
    params = {'fit.hz'+str(i): float(hz) for i, hz in enumerate(record['mode_frequencies_hz'])}
    params |= {'fit.rate'+str(i): float(rate) for i, rate in enumerate(record['mode_rates_per_s'])}
    if 'contact.openness' in inst.params:
        params['contact.openness'] = 1 if record['articulation'] == 'open' else 0
    for i in range(len(record['mode_frequencies_hz']), 8):
        params['fit.hz'+str(i)] = 300*(i+1)
        params['fit.rate'+str(i)] = 30
    rates = np.array(record['band_rates_per_s']).reshape(6, 3)
    weights = np.max(record['band_power'], axis=0).reshape(6, 3)
    rates = np.sum(rates*weights, axis=1)/np.maximum(weights.sum(axis=1), 1e-20)
    mode_caps = np.pad(np.array(record['mode_amplitudes'])*1.15, (0, 8-len(record['mode_amplitudes'])))
    best = None
    times = np.repeat(np.mean(windows, axis=1), len(BANDS))
    for contact in [.0006, .0025, .008]:
        for direct in [0., .3, 1.]:
            current_rates = rates.copy()
            for iteration in range(2):
                values = params | {f'fit.r{i}': float(np.clip(r, .1, 120)) for i, r in enumerate(current_rates)}
                values |= {'fit.contact_s': contact, 'fit.direct': direct}
                basis, state = inst.render(windows[-1][1], vel=1, params=values)
                assert np.isfinite(basis).all() and np.isfinite(state).all()
                q = moments(basis, windows)
                gains, _, quality = fit_gains(q, target, mode_caps=mode_caps)
                if best is None or quality['score'] < best['fit']['score']:
                    best = {'source': record['file'], 'sha256': record['sha256'],
                            'articulation': record['articulation'], 'centroid_hz': record['centroid_hz'],
                            'region_rates': [values[f'fit.r{i}'] for i in range(6)],
                            'contact_s': contact, 'direct': direct,
                            'mode_frequencies': [values['fit.hz'+str(i)] for i in range(8)],
                            'mode_rates': [values['fit.rate'+str(i)] for i in range(8)],
                            'mode_gains': gains[18:].tolist(), 'band_gains': gains[:18].tolist(),
                            'fit': quality}
                    best_audio = basis@gains
                if iteration == 0:
                    # Analytic attenuation supplies a proposal only. Accept a
                    # fit after rerendering the actual lossy network above.
                    _, current_rates, _ = fit_gains(q, target, times, current_rates, mode_caps=mode_caps)
    sf.write(output/(Path(record['file']).stem+'.wav'), best_audio*.65, SR, subtype='FLOAT')
    return best


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('families', nargs='*')
    parser.add_argument('--limit', type=int)
    args = parser.parse_args()
    output = HERE/'output/calibration'
    output.mkdir(parents=True, exist_ok=True)
    for slug in args.families or ['crash', 'ride', 'hihat']:
        basis_path = output/('hihat-basis.lisp' if slug == 'hihat' else 'basis.lisp')
        basis_path.write_text(basis_source(hat=slug == 'hihat'))
        inst = instrument(basis_path)
        analysis = json.loads((HERE/f'{slug}-analysis.json').read_text())
        result = {'family': slug, 'engine_sha256': digest(HERE/'engine.py'),
                  'template_sha256': digest(HERE/'engine.lisp.in'),
                  'analysis_sha256': digest(HERE/f'{slug}-analysis.json'),
                  'compiler_sha256': inst.compiler_sha256, 'sample_rate': SR,
                  'headroom_gain': .65, 'references': []}
        for record in analysis['references'][:args.limit]:
            fit = calibrate(inst, record, output)
            result['references'].append(fit)
            print(slug, Path(record['file']).stem, fit['fit'], flush=True)
            (HERE/f'{slug}-calibration.json').write_text(json.dumps(result, indent=2)+'\n')


if __name__ == '__main__':
    main()
