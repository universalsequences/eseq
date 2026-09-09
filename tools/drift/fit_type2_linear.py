#!/usr/bin/env python3
"""Fit a continuous Type-II small-signal candidate with unused resonance controls.

This identifies a transfer function, not the nonlinear circuit. Two second-order
sections have rational feedback laws with zero feedback at resonance zero.
Intermediate 0.05 settings and the -60 dB controls are excluded from fitting.
High-resonance, level-dependent observations remain diagnostics, not LTI truth.
"""
import argparse
import hashlib
import json
from pathlib import Path

import numpy as np
from scipy.optimize import least_squares

from analyze_reference import capture_bytes, load, window
from diagnose_reference import harmonic_noise
from fit_linear import response


def transfer(parameters, resonance, frequencies, cutoff=1000):
    fc1, fc2, gain, a, b, c, d, e, g = parameters
    k1 = (a*resonance+b*resonance**2)/(1+c*resonance)
    k2 = (d*resonance+e*resonance**2)/(1+g*resonance)
    s1 = 1j*np.tan(np.pi*frequencies/96000)/np.tan(np.pi*fc1*cutoff/1000/96000)
    s2 = 1j*np.tan(np.pi*frequencies/96000)/np.tan(np.pi*fc2*cutoff/1000/96000)
    return (gain/(1+(2-k1)*s1+s1*s1)/(1+(2-k2)*s2+s2*s2)
            * response(frequencies,96000,20,1.469,True)
            * (1j*frequencies/(1.6+1j*frequencies))**4)


def observations(folder, reports):
    result, provenance = [], {}
    for batch in ('character','resonance-law','sine-response'):
        ledger_path = folder/f'{batch}.json'
        report_path = reports/f'{batch}.json'
        report = json.loads(report_path.read_text())
        ledger = json.loads(ledger_path.read_text())
        for path, expected in [(ledger_path,report['ledger_sha256']),
                               (ledger_path.with_suffix('.als'),report['set_sha256'])]:
            if hashlib.sha256(path.read_bytes()).hexdigest() != expected:
                raise ValueError(f'{path}: provenance changed')
        entries = {case['name']:case for case in ledger['cases']}
        cache = {}

        def spectrum(name):
            if name not in cache:
                path = folder/f'{batch} {name}.wav'
                if hashlib.sha256(capture_bytes(path)).hexdigest() != report['files'][name]['sha256']:
                    raise ValueError(f'{path}: capture changed')
                signal = load(path)
                values, snr = [], []
                for event, record in zip(entries[name]['notes'],report['files'][name]['notes']):
                    x = window(signal,event)[:,0]
                    f = record['fundamental_hz']
                    weights = np.hanning(len(x))
                    h = 2*np.sum((x-x.mean())*weights*np.exp(-2j*np.pi*f*np.arange(len(x))/48000))/weights.sum()
                    _, noise = harmonic_noise(x,f)
                    values.append(h)
                    snr.append(20*np.log10(max(abs(h),1e-30)/max(noise[0],1e-30)))
                cache[name] = np.array(values), np.array(snr)
            return cache[name]

        for name, entry in entries.items():
            if not (name.startswith('law-t2-') or name.startswith('sine-t2-')):
                continue
            p = entry['parameters']
            res = float(p['Filter_Resonance'])
            source_gain = float(p['Mixer_OscillatorGain1'])
            base = report['spectral_ratios'][name]['bypass']
            output, output_snr = spectrum(name)
            source, source_snr = spectrum(base)
            snr = np.minimum(source_snr,output_snr)
            valid = snr >= 30
            if not np.any(valid):
                raise ValueError(f'{batch}/{name}: no qualified fundamentals')
            train = (batch != 'sine-response' and res <= .8
                     and abs(res*10-round(res*10)) < 1e-5 and source_gain > .002)
            if batch == 'character' and res >= .8:
                train = False
            f = np.array([n['fundamental_hz'] for n in report['files'][name]['notes']])
            result.append(dict(batch=batch,name=name,resonance=res,train=train,
                               cutoff=float(p['Filter_Frequency']),frequencies=f[valid],
                               measured=(output/source)[valid],qualified=int(valid.sum()),
                               minimum_snr_db=float(snr[valid].min())))
        provenance[batch] = hashlib.sha256(report_path.read_bytes()).hexdigest()
    return result, provenance


def fit(folder, reports):
    cases, provenance = observations(folder,reports)

    def residual(parameters):
        values = []
        for case in cases:
            if case['train']:
                ratio = transfer(parameters,case['resonance'],case['frequencies'],case['cutoff'])/case['measured']
                values.extend(np.log(abs(ratio)))
                values.extend(np.angle(ratio))
        return np.array(values)

    optimization = least_squares(residual,[1021,1021,1.3,3.5,0,1.5,8,0,3],
        bounds=([950,950,1,0,-10,0,0,-10,0],[1100,1100,2,20,10,20,20,10,20]),
        max_nfev=3000,xtol=1e-12,ftol=1e-12,gtol=1e-12)
    if not optimization.success:
        raise ValueError(f'Identification did not converge: {optimization.message}')
    rows = []
    for case in cases:
        ratio = transfer(optimization.x,case['resonance'],case['frequencies'],case['cutoff'])/case['measured']
        rows.append({k:v for k,v in case.items() if k not in ('frequencies','measured')} | dict(
            rms_magnitude_error_db=float(np.sqrt(np.mean((20*np.log10(abs(ratio)))**2))),
            rms_phase_error_radians=float(np.sqrt(np.mean(np.angle(ratio)**2)))))
    return dict(methodology=__doc__,report_sha256=provenance,minimum_local_snr_db=30,
                parameter_order=['fc1_at_1khz','fc2_at_1khz','gain','k1_r','k1_r2','k1_den_r',
                                 'k2_r','k2_r2','k2_den_r'],
                parameters=optimization.x.tolist(),cases=rows,
                production_accepted=False)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('reference',type=Path)
    parser.add_argument('--reports',type=Path,required=True)
    parser.add_argument('--out',type=Path,required=True)
    args = parser.parse_args()
    result = fit(args.reference,args.reports)
    args.out.write_text(json.dumps(result,indent=2,allow_nan=False)+'\n')
    print(f'Wrote {args.out}: {len(result["cases"])} cases')
