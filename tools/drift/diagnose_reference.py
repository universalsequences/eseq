#!/usr/bin/env python3
"""Test component hypotheses against native captures, without changing synth DSP.

Clipping is fitted on the first ascending note only. All other windows are
holdouts. Pitch normalization comes from a separate solo-oscillator capture,
not a per-note fit to the loud output. This is a diagnostic, not a synth model.
"""
import argparse
import hashlib
import json
from pathlib import Path
import numpy as np
from scipy.optimize import minimize_scalar
from scipy.signal import czt
from analyze_reference import SR, capture_bytes, db, harmonics, load, window


def relative_error(predicted, measured):
    return float(np.linalg.norm(predicted-measured)/np.linalg.norm(measured))


def fit_ceiling(x, y, kind):
    def predict(c):
        return np.clip(x,-c,c) if kind=='clip' else c*np.tanh(x/c)
    fit = minimize_scalar(lambda c: np.mean((predict(c)-y)**2),
                          bounds=(1e-6,float(np.max(abs(x)))), method='bounded',
                          options={'xatol':1e-12})
    if not fit.success:
        raise ValueError(f'{kind} fit failed')
    return float(fit.x)


def harmonic_noise(x, frequency):
    """Median adjacent-bin amplitude, not a confidence interval or THD estimate.

    Six offsets are well outside the Hann main lobe in a one-second window
    and below half the minimum tested fundamental (65 Hz).
    """
    h = harmonics(x,frequency)
    w = np.hanning(len(x))
    wx = (x-x.mean())*w
    bins = []
    for offset in (-16,-12,-8,8,12,16):
        z = czt(wx,m=len(h)+1,w=np.exp(-2j*np.pi*frequency/SR),
                a=np.exp(2j*np.pi*offset/SR))
        bins.append(2*np.abs(z[1:])/w.sum())
    return h,np.median(bins,axis=0)


def diagnose(folder, reports):
    used = {}
    for batch,report in reports.items():
        ledger_path = folder/f'{batch}.json'
        if hashlib.sha256(ledger_path.read_bytes()).hexdigest() != report['ledger_sha256']:
            raise ValueError(f'{ledger_path}: ledger does not match analysis report')
        if hashlib.sha256(ledger_path.with_suffix('.als').read_bytes()).hexdigest() != report['set_sha256']:
            raise ValueError(f'{batch}: set changed after analysis')
    def capture(batch,name):
        path = folder/f'{batch} {name}.wav'
        raw = capture_bytes(path)
        digest = hashlib.sha256(raw).hexdigest()
        if digest != reports[batch]['files'][name]['sha256']:
            raise ValueError(f'{path}: capture does not match analysis report')
        used[f'{batch}/{name}'] = digest
        return load(path)

    ledger = json.loads((folder/'ordering.json').read_text())
    cases = {c['name']:c for c in ledger['cases']}
    source = reports['summing']['files']['solo2--6']['notes']
    normalization = {n['note']:n['rms']/source[0]['rms'] for n in source}
    scale = float(cases['ascending-p6']['parameters']['Mixer_OscillatorGain1']) / float(
        cases['ascending--6']['parameters']['Mixer_OscillatorGain1'])
    first = cases['ascending-p6']['notes'][0]
    x = window(capture('ordering','ascending--6'),first)[:,0]*scale
    y = window(capture('ordering','ascending-p6'),first)[:,0]
    ceiling = fit_ceiling(x,y,'clip')
    tanh_ceiling = fit_ceiling(x,y,'tanh')
    clipping = []
    for order in ('ascending','descending','low-repeat','high-repeat'):
        low = capture('ordering',order+'--6')
        high = capture('ordering',order+'-p6')
        for index,event in enumerate(cases[order+'-p6']['notes']):
            a = window(low,event)[:,0]*scale
            b = window(high,event)[:,0]
            c = ceiling*normalization[event['note']]
            clipping.append(dict(order=order,position=index,note=event['note'],
                training_window=(order=='ascending' and index==0),
                fixed_clip_relative_error=relative_error(np.clip(a,-ceiling,ceiling),b),
                normalized_clip_relative_error=relative_error(np.clip(a,-c,c),b),
                fixed_tanh_relative_error=relative_error(tanh_ceiling*np.tanh(a/tanh_ceiling),b),
                diagnostic_individual_ceiling=fit_ceiling(a,b,'clip')))

    # Compare -48 and -60 dB only where BOTH levels and BOTH paths have
    # strong coherent harmonics. Weak bins must not steer filter fitting.
    batch = 'linear-clean'
    quiet_ledger = json.loads((folder/f'{batch}.json').read_text())
    event = quiet_ledger['cases'][0]['notes'][0]
    frequency = reports[batch]['frequencies_hz'][0]
    bypass = {level:harmonic_noise(window(capture(batch,f'bypass-saw--{level}'),event)[:,0],frequency)
              for level in (48,60)}
    comparisons = []
    for typ in (1,2):
        for cutoff in (250,1000,4000):
            for res in (0,.5,.8,.95):
                results = []
                masks = []
                for level in (48,60):
                    name = f'linear-t{typ}-f{cutoff}-r{res:g}--{level}'
                    h,noise = harmonic_noise(window(capture(batch,name),event)[:,0],frequency)
                    bh,bnoise = bypass[level]
                    masks.append((db(h/np.maximum(noise,1e-15))>=30) &
                                 (db(bh/np.maximum(bnoise,1e-15))>=30))
                    results.append(db(h/bh))
                frequencies = frequency*np.arange(1,len(results[0])+1)
                valid = masks[0]&masks[1]&(frequencies<min(6*cutoff,14000))
                delta = results[0][valid]-results[1][valid]
                comparisons.append(dict(type=typ,cutoff=cutoff,resonance=res,
                    retained_harmonics=(np.flatnonzero(valid)+1).tolist(),
                    rms_difference_db=float(np.sqrt(np.mean(delta**2))) if len(delta) else None,
                    max_difference_db=float(np.max(abs(delta))) if len(delta) else None))

    noise_controls = []
    for name in ('bypass-saw--60','linear-t1-f1000-r0--60','linear-t2-f1000-r0--60'):
        a = window(capture('linear',name),event)[:,0]
        b = window(capture('linear-clean',name),event)[:,0]
        noise_controls.append(dict(case=name,triangular_setting_rms=float(np.sqrt(np.mean(a*a))),
            no_dither_setting_rms=float(np.sqrt(np.mean(b*b))),
            repeat_difference_rms=float(np.sqrt(np.mean((a-b)**2)))))
    return dict(methodology=__doc__,training_ceiling=ceiling,training_tanh_ceiling=tanh_ceiling,
        independent_pitch_normalization=normalization,clipping=clipping,
        quiet_comparisons=dict(note=event['note'],minimum_local_snr_db=30,cases=comparisons),
        dither_control=noise_controls,capture_sha256=used,
        limitations='No phase alignment, gain fit, or per-note clipping fit is used in holdout predictions. Individual ceilings are diagnostics only. Local spectral SNR is a heuristic. Clipping results do not establish internal gain placement, antialiasing, or a complete nonlinear filter model.')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('reference',type=Path)
    parser.add_argument('--measurements',type=Path,required=True)
    parser.add_argument('--out',type=Path,required=True)
    args = parser.parse_args()
    reports = {name:json.loads((args.measurements/f'{name}.json').read_text())
               for name in ('summing','linear','linear-clean','ordering')}
    result = diagnose(args.reference,reports)
    result['analysis_report_sha256'] = {
        name:hashlib.sha256((args.measurements/f'{name}.json').read_bytes()).hexdigest()
        for name in reports}
    args.out.write_text(json.dumps(result,indent=2,allow_nan=False)+'\n')
    print(f'Wrote {args.out}')
