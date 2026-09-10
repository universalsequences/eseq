#!/usr/bin/env python3
"""Refine physical coupling gains with both spectral and transient constraints.

Windowed spectra alone can ignore a short, overly coherent attack. The model's
actual crest and untapered window energy therefore constrain the same positive
coupling coefficients. There is no runtime limiter or per-hit normalization.
"""
import argparse
import json

import numpy as np
from scipy.optimize import least_squares
import soundfile as sf

from calibrate import SR, WINDOWS, moments, fit_gains
from common import HERE, ROOT, instrument, read_reference, digest
from engine import basis_source


def observations(y, windows):
    q = moments(y, windows)
    energy = []
    for a, b in windows:
        x = y[round(a*SR):round(b*SR)].astype(float)
        energy.append(x.T@x/len(x))
    return np.concatenate([q, energy])


def constrained_gains(q, target, basis, peak_limit, mode_caps=None):
    gains, _, _ = fit_gains(q, target, mode_caps=mode_caps)
    active = target > max(target.max()*1e-5, 1e-12)
    q, target = q[active], target[active]
    norm = np.maximum(np.sqrt(np.max(np.diagonal(q, axis1=1, axis2=2), axis=0)), 1e-10)
    q = q/norm[None, :, None]/norm[None, None, :]
    scaled = gains*norm
    upper = np.full(26, np.inf)
    if mode_caps is not None:
        upper[18:26] = np.maximum(np.array(mode_caps)*norm[18:], 1e-12)
    indices = np.array([], dtype=int)
    for iteration in range(8):
        audio = basis@(scaled/norm)
        new = np.argpartition(abs(audio), -32)[-32:]
        indices = np.unique(np.r_[indices, new])
        p = basis[indices]/norm[None, :]/peak_limit
        def residual(g):
            power = np.maximum(np.einsum('i,wij,j->w', g, q, g), 1e-20)
            return np.r_[10*np.log10(power/target), 60*np.maximum(abs(p@g)-1, 0)]
        def jacobian(g):
            qg = q@g
            power = np.maximum(qg@g, 1e-20)
            v = p@g
            return np.r_[(20/np.log(10))*qg/power[:, None],
                         60*p*(np.sign(v)*(abs(v) > 1))[:, None]]
        fit = least_squares(residual, np.maximum(scaled, 1e-14), jac=jacobian,
                            bounds=(0, upper), max_nfev=140, ftol=1e-7)
        scaled = fit.x
        if abs(basis@(scaled/norm)).max() <= peak_limit*1.01:
            break
    gains = scaled/norm
    audio = basis@gains
    power = np.maximum(np.einsum('i,wij,j->w', scaled, q, scaled), 1e-20)
    error = 10*np.log10(power/target)
    return gains, {'median_abs_db': float(np.median(abs(error))),
                   'rms_db': float(np.sqrt(np.mean(error**2))),
                   'p90_abs_db': float(np.percentile(abs(error), 90)),
                   'peak': float(abs(audio).max()), 'peak_limit': float(peak_limit),
                   'peak_limit_ratio': float(abs(audio).max()/peak_limit)}


def main():
    output = HERE/'output/calibration'
    parser = argparse.ArgumentParser()
    parser.add_argument('families', nargs='*')
    args = parser.parse_args()
    for slug in args.families or ['crash', 'ride', 'hihat']:
        basis_path = output/('hihat-basis.lisp' if slug == 'hihat' else 'basis.lisp')
        basis_path.write_text(basis_source(hat=slug == 'hihat'))
        inst = instrument(basis_path)
        path = HERE/f'{slug}-calibration.json'
        result = json.loads(path.read_text())
        measurements = {r['file']: r for r in json.loads((HERE/f'{slug}-analysis.json').read_text())['references']}
        for row in result['references']:
            reference, _, _ = read_reference(ROOT/row['source'])
            windows = [(a, min(b, len(reference)/SR)) for a, b in WINDOWS if min(b, len(reference)/SR)-a >= .015]
            params = {f'fit.r{i}': x for i, x in enumerate(row['region_rates'])}
            params |= {f'fit.hz{i}': x for i, x in enumerate(row['mode_frequencies'])}
            params |= {f'fit.rate{i}': x for i, x in enumerate(row['mode_rates'])}
            params |= {'fit.contact_s': row['contact_s'], 'fit.direct': row['direct']}
            if slug == 'hihat':
                params['contact.openness'] = 1 if row['articulation'] == 'open' else 0
            basis, _ = inst.render(windows[-1][1], vel=1, params=params)
            q = observations(basis, windows)
            target = observations(reference, windows)[:, 0, 0]
            mode_caps = np.pad(np.array(measurements[row['source']]['mode_amplitudes'])*1.15, (0, 8-len(measurements[row['source']]['mode_amplitudes'])))
            gains, quality = constrained_gains(q, target, basis, float(abs(reference).max())*1.25, mode_caps=mode_caps)
            row['band_gains'], row['mode_gains'] = gains[:18].tolist(), gains[18:].tolist()
            row['transient_fit'] = quality
            sf.write(output/(ROOT/row['source']).name, basis@gains*.65, SR, subtype='FLOAT')
            print(slug, (ROOT/row['source']).name, quality, flush=True)
        result['basis_source_sha256'] = digest(basis_path)
        result['compiler_sha256'] = inst.compiler_sha256
        result['engine_sha256'] = digest(HERE/'engine.py')
        result['template_sha256'] = digest(HERE/'engine.lisp.in')
        if slug == 'hihat':
            result['contact_template_sha256'] = digest(HERE/'contact.lisp.in')
        result['refinement'] = 'Positive physical couplings constrained by untapered window energy and a maximum crest 1.25 times the measured reference peak; fixed output headroom 0.65. No runtime limiting.'
        path.write_text(json.dumps(result, indent=2)+'\n')


if __name__ == '__main__':
    main()
