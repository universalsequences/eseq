#!/usr/bin/env python3
"""Analyze raw Drift captures without normalization or fitted response curves.

Harmonic ratios at high input levels describe nonlinear output spectra, NOT an
LTI transfer function. Mixed-oscillator cases report RMS and peaks only.
"""
import argparse
import gzip
import hashlib
import io
import json
from pathlib import Path
import numpy as np
from scipy.io import wavfile
from scipy.optimize import minimize_scalar
from scipy.signal import czt

SR = 48000


def db(x):
    return 20*np.log10(np.maximum(x, 1e-15))


def capture_bytes(path):
    if path.exists():
        return path.read_bytes()
    return gzip.decompress(path.with_suffix('.wav.gz').read_bytes())


def load(path):
    sr, y = wavfile.read(io.BytesIO(capture_bytes(path)))
    if sr != SR or y.dtype != np.float32 or y.shape != (20*SR, 2):
        raise ValueError(f'{path}: expected 20 s, stereo float32, 48 kHz; got {sr}, {y.dtype}, {y.shape}')
    if not np.isfinite(y).all():
        raise ValueError(f'{path}: nonfinite audio')
    if np.max(np.abs(y)) < 1e-8:
        raise ValueError(f'{path}: silent capture')
    return y


def window(y, event):
    # One steady second, 0.5 s after onset; within the 2.5-second held note.
    start = round((event['start_seconds'] + .5)*SR)
    return y[start:start+SR].astype(float)


def fundamental(x, nominal):
    t = np.arange(len(x))/SR
    w = np.hanning(len(x))
    wx = (x-x.mean())*w
    def objective(f):
        return -abs(np.sum(wx*np.exp(-2j*np.pi*f*t)))**2
    fit = minimize_scalar(objective, bounds=(nominal*.997, nominal*1.003),
                          method='bounded', options={'xatol': 1e-9})
    if not fit.success:
        raise ValueError('Could not estimate source fundamental')
    return float(fit.x)


def harmonics(x, frequency):
    count = min(256, int(20000/frequency))
    w = np.hanning(len(x))
    result = czt((x-x.mean())*w, m=count+1, w=np.exp(-2j*np.pi*frequency/SR))
    return 2*np.abs(result[1:])/w.sum()


def analyze(ledger_path):
    data = json.loads(ledger_path.read_text())
    if hashlib.sha256(ledger_path.with_suffix('.als').read_bytes()).hexdigest() != data['set_sha256']:
        raise ValueError('Capture set changed since ledger was generated')
    cases = {c['name']: c for c in data['cases']}
    folder, prefix = ledger_path.parent, ledger_path.stem
    paths = {name: folder/f'{prefix} {name}.wav' for name in cases}
    missing = [str(p) for p in paths.values()
               if not p.exists() and not p.with_suffix('.wav.gz').exists()]
    if missing:
        raise ValueError('Missing captures: '+', '.join(missing))
    control = load(paths['bypass-sine--24'])
    frequencies = [fundamental(window(control, n)[:,0], 440*2**((n['note']-69)/12))
                   for n in data['cases'][0]['notes']]
    frequency_by_note = dict(zip((n['note'] for n in data['cases'][0]['notes']), frequencies))
    for name,case in cases.items():
        p = case['parameters']
        if (p['Oscillator1_Type'] == '0' and p['Filter_OscillatorThrough1'] == 'false'
                and p['Mixer_OscillatorOn2'] == 'false' and p['Mixer_NoiseOn'] == 'false'):
            unmeasured = [n for n in case['notes'] if n['note'] not in frequency_by_note]
            if unmeasured:
                source = load(paths[name])
                for event in unmeasured:
                    frequency_by_note[event['note']] = fundamental(window(source,event)[:,0],
                        440*2**((event['note']-69)/12))
    files = {}
    for name, case in cases.items():
        path = paths[name]
        y = load(path)
        p = case['parameters']
        mixed = p['Mixer_OscillatorOn2'] == 'true'
        records = []
        for event in case['notes']:
            freq = frequency_by_note[event['note']]
            x = window(y, event)
            if np.max(np.abs(x)) < 1e-9:
                raise ValueError(f'{path}: silent steady window for note {event["note"]}')
            record = dict(note=event['note'], fundamental_hz=freq,
                rms=float(np.sqrt(np.mean(x*x))), peak=float(np.max(abs(x))),
                dc=x.mean(axis=0).tolist(), stereo_max_difference=float(np.max(abs(x[:,0]-x[:,1]))))
            if not mixed:
                h = harmonics(x[:,0], freq)
                record['harmonic_amplitudes'] = h.tolist()
                record['fundamental_dbfs'] = float(db(h[0]))
                if p['Oscillator1_Type'] == '0':
                    record['thd_db'] = float(db(np.linalg.norm(h[1:])/max(h[0],1e-15)))
            records.append(record)
        raw = capture_bytes(path)
        files[name] = dict(parameters=p, sha256=hashlib.sha256(raw).hexdigest(),
            bytes=len(raw), peak=float(np.max(abs(y))), notes=records)
    comparisons = {}
    for name, case in cases.items():
        p = case['parameters']
        if p['Mixer_OscillatorOn2'] == 'true':
            continue
        gain = float(p['Mixer_OscillatorGain1'])
        bypasses = [key for key, c in cases.items()
                    if c['parameters']['Oscillator1_Type'] == p['Oscillator1_Type']
                    and c['parameters']['Filter_OscillatorThrough1'] == 'false'
                    and c['parameters']['Mixer_OscillatorOn2'] == 'false'
                    and c['parameters']['Mixer_NoiseOn'] == 'false'
                    and abs(float(c['parameters']['Mixer_OscillatorGain1'])-gain)<1e-8
                    and c['notes']==case['notes']]
        if p['Filter_OscillatorThrough1']=='true' and bypasses:
            pairs = []
            for out, source in zip(files[name]['notes'], files[bypasses[0]]['notes']):
                a, b = np.array(out['harmonic_amplitudes']), np.array(source['harmonic_amplitudes'])
                # Suppress harmonics less than -80 dB relative to source fundamental.
                valid = b > max(b[0]*1e-4, 1e-10)
                pairs.append(dict(note=out['note'], harmonics=(np.flatnonzero(valid)+1).tolist(),
                    output_to_bypass_db=db(a[valid]/b[valid]).tolist()))
            comparisons[name] = dict(bypass=bypasses[0], notes=pairs)
    checks = {}
    pairs = []
    if data['batch']=='levels':
        pairs = [('repeat-bypass-sine','bypass-sine--24',1),('repeat-t1-saw','t1-saw-r0.8-p6',1)]
        for typ in (1,2):
            for vol in (.125,.5):
                pairs.append((f'volume-t{typ}-{vol:g}',f't{typ}-saw-r0.8-p6',vol/.25))
    elif data['batch']=='summing':
        for stem in ['sum--6','sum-p0','sum-p6','filtered-sum-t1','filtered-sum-t2']:
            for vol in (.125,.5):
                pairs.append((f'{stem}-v{vol:g}',f'{stem}-v0.25',vol/.25))
    for a,b,scale in pairs:
        ya,yb = load(paths[a]),load(paths[b])*scale
        checks[a] = dict(reference=b, expected_linear_scale=scale,
            relative_rms_error=float(np.linalg.norm(ya.astype(float)-yb)/np.linalg.norm(yb)),
            max_error=float(np.max(abs(ya-yb))))
    if data['batch']=='linear':
        reference = 'amp-placement-1'
        yb = load(paths[reference])
        for sustain in (.25,.5):
            name = f'amp-placement-{sustain:g}'
            ya = load(paths[name])
            notes = []
            for event in cases[name]['notes']:
                a,b = window(ya,event),window(yb,event)*sustain
                notes.append(dict(note=event['note'],
                    relative_rms_error=float(np.linalg.norm(a-b)/np.linalg.norm(b)),
                    rms_ratio=float(np.linalg.norm(a)/np.linalg.norm(b)*sustain)))
            checks[name] = dict(reference=reference, expected_linear_scale=sustain,
                scope='Steady sustain windows only; attack and decay are excluded.', notes=notes)
    return dict(live_version=data['live_version'], sample_rate=SR, duration_seconds=20,
        ledger_sha256=hashlib.sha256(ledger_path.read_bytes()).hexdigest(),
        set_sha256=data['set_sha256'], source_sha256=data['source_sha256'],
        methodology='Raw float samples. Steady window 0.5-1.5 s after onset. Hann-windowed harmonic demodulation at bypass-measured pitch. No gain/phase fit. High-level spectral ratios are not LTI responses.',
        frequencies_hz=frequencies, files=files, spectral_ratios=comparisons,
        repeatability_and_output_volume=checks)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('ledger',type=Path)
    parser.add_argument('--out',type=Path,required=True)
    args = parser.parse_args()
    result = analyze(args.ledger)
    args.out.parent.mkdir(parents=True,exist_ok=True)
    args.out.write_text(json.dumps(result,indent=2,allow_nan=False)+'\n')
    print(json.dumps(dict(files=len(result['files']), frequencies_hz=result['frequencies_hz'],
                         checks=result['repeatability_and_output_volume']),indent=2))
