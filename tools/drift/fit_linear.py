#!/usr/bin/env python3
"""Identify candidate linear sections from noise-qualified complex responses.

These are per-case identification fits, not independent validation or native
topology claims. Parameters must not be copied into production without holdouts.
The HP controls distinguish a resonant HP section from a one-pole assumption.
"""
import argparse
import hashlib
import json
from pathlib import Path
import numpy as np
from scipy.optimize import least_squares
from scipy.signal import czt
from analyze_reference import load, window
from diagnose_reference import harmonic_noise


def response(frequencies, internal_rate, cutoff, q, highpass=False):
    s = 1j*np.tan(np.pi*frequencies/internal_rate)/np.tan(np.pi*cutoff/internal_rate)
    return (s*s if highpass else 1)/(1+s/q+s*s)


def fit(folder, report):
    ledger_path = folder/'response.json'
    if hashlib.sha256(ledger_path.read_bytes()).hexdigest() != report['ledger_sha256']:
        raise ValueError('Response ledger does not match report')
    ledger = json.loads(ledger_path.read_text())
    if hashlib.sha256(ledger_path.with_suffix('.als').read_bytes()).hexdigest() != report['set_sha256']:
        raise ValueError('Response set changed since capture analysis')
    event = ledger['cases'][0]['notes'][0]
    fundamental = report['frequencies_hz'][0]
    def spectrum(name):
        path = folder/f'response {name}.wav'
        if hashlib.sha256(path.read_bytes()).hexdigest() != report['files'][name]['sha256']:
            raise ValueError(f'{path}: capture changed since analysis')
        x = window(load(path),event)[:,0]
        amplitudes,noise = harmonic_noise(x,fundamental)
        w = np.hanning(len(x))
        h = czt((x-x.mean())*w,m=len(amplitudes)+1,
                w=np.exp(-2j*np.pi*fundamental/48000))[1:]*2/w.sum()
        return h,noise

    source,source_noise = spectrum('bypass-saw--24')
    cases = []
    settings = [(f'hp-{hp}',19999,0,hp) for hp in (100,1000,4000)]
    settings += [(f'response-t1-f{fc}-r{res:g}',fc,res,10)
                 for fc in (250,1000,4000) for res in (0,.5,.8)]
    for name,cutoff,resonance,hp in settings:
        output,noise = spectrum(name)
        f = fundamental*np.arange(1,len(output)+1)
        valid = ((abs(source)>source_noise*10**1.5) & (abs(output)>noise*10**1.5)
                 & (f<min(8*cutoff,16000)))
        frequencies,measured = f[valid],output[valid]/source[valid]
        if len(frequencies)<8:
            raise ValueError(f'{name}: too few reliable complex harmonics')
        fits = []
        for rate in (48000,96000):
            def model(v):
                fc,q,g,hpf,hpq = np.exp(v)
                return g*response(frequencies,rate,fc,q)*response(frequencies,rate,hpf,hpq,True)
            def residual(v):
                ratio = model(v)/measured
                return np.r_[np.log(abs(ratio)),np.angle(ratio)]
            result = least_squares(residual,np.log([cutoff,.42/(1-resonance),1.55,max(20,hp),1.4]),
                bounds=(np.log([cutoff*.5,.01,.1,1,.1]),
                        np.log([min(cutoff*2,rate*.49),100,10,15000,20])),max_nfev=1000)
            if not result.success:
                raise ValueError(f'{name}: optimizer did not converge')
            ratio = model(result.x)/measured
            fits.append(dict(internal_sample_rate=rate,
                parameters=dict(zip(('lp_frequency','lp_q','gain','hp_frequency','hp_q'),np.exp(result.x))),
                rms_magnitude_error_db=float(np.sqrt(np.mean((20*np.log10(abs(ratio)))**2))),
                rms_phase_error_radians=float(np.sqrt(np.mean(np.angle(ratio)**2)))))
        cases.append(dict(case=name,harmonics_compared=int(valid.sum()),
            capture_sha256=report['files'][name]['sha256'],fits=fits))
    return dict(methodology=__doc__,note=event['note'],minimum_local_snr_db=30,
        source_sha256=report['files']['bypass-saw--24']['sha256'],cases=cases)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('reference',type=Path)
    parser.add_argument('--report',type=Path,required=True)
    parser.add_argument('--out',type=Path,required=True)
    args = parser.parse_args()
    result = fit(args.reference,json.loads(args.report.read_text()))
    result['analysis_report_sha256'] = hashlib.sha256(args.report.read_bytes()).hexdigest()
    args.out.write_text(json.dumps(result,indent=2,allow_nan=False)+'\n')
    print(f'Wrote {args.out}')
