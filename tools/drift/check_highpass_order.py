#!/usr/bin/env python3
"""Predict driven captures by changing only their final linear high-pass.

Use the captured 20 Hz HP output as the source, remove its known response,
and apply the independently identified target HP response. No fitting of
gain, phase, delay, or filter coefficients is performed.
"""
import argparse
import hashlib
import json
from pathlib import Path
import numpy as np
from scipy.signal import czt
from analyze_reference import SR, capture_bytes, load, window
from diagnose_reference import harmonic_noise
from fit_linear import response


def check(ledger_path, report_path):
    ledger = json.loads(ledger_path.read_text())
    report = json.loads(report_path.read_text())
    for path, digest in [(ledger_path, report['ledger_sha256']),
                         (ledger_path.with_suffix('.als'), report['set_sha256'])]:
        if hashlib.sha256(path.read_bytes()).hexdigest() != digest:
            raise ValueError(f'{path}: provenance mismatch')
    cases = {c['name']: c for c in ledger['cases']}
    cache = {}

    def spectra(name):
        if name not in cache:
            path = ledger_path.parent/f'{ledger_path.stem} {name}.wav'
            if hashlib.sha256(capture_bytes(path)).hexdigest() != report['files'][name]['sha256']:
                raise ValueError(f'{path}: capture changed')
            y, notes = load(path), []
            for event, record in zip(cases[name]['notes'], report['files'][name]['notes']):
                x = window(y,event)[:,0]
                f = record['fundamental_hz']
                _, noise = harmonic_noise(x,f)
                w = np.hanning(len(x))
                h = 2*czt((x-x.mean())*w, m=len(noise)+1,
                          w=np.exp(-2j*np.pi*f/SR))[1:]/w.sum()
                notes.append((f*np.arange(1,len(h)+1), h, noise))
            cache[name] = notes
        return cache[name]

    results = []
    for name, case in cases.items():
        if not name.startswith('hp-drive-'):
            continue
        hp = float(case['parameters']['Filter_HiPassFrequency'])
        if hp == 20:
            continue
        source_name = name.replace(f'-h{hp:g}-','-h20-')
        for event, source, target in zip(case['notes'], spectra(source_name), spectra(name)):
            frequencies, source_h, source_noise = source
            _, target_h, target_noise = target
            expected = source_h*(response(frequencies,2*SR,hp,1.47,True)
                                 /response(frequencies,2*SR,20,1.47,True))
            valid = ((abs(source_h)>source_noise*10**1.5)
                     & (abs(target_h)>target_noise*10**1.5))
            if not valid.any():
                results.append(dict(case=name,note=event['note'],harmonics_compared=0))
                continue
            ratio = expected[valid]/target_h[valid]
            results.append(dict(case=name,source=source_name,note=event['note'],
                harmonics_compared=int(valid.sum()),
                rms_magnitude_error_db=float(np.sqrt(np.mean((20*np.log10(abs(ratio)))**2))),
                rms_phase_error_radians=float(np.sqrt(np.mean(np.angle(ratio)**2)))))
    return dict(methodology=__doc__, minimum_local_snr_db=30,
        report_sha256=hashlib.sha256(report_path.read_bytes()).hexdigest(),
        hp_q=1.47, internal_sample_rate=2*SR, cases=results)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('ledger',type=Path)
    parser.add_argument('--report',type=Path,required=True)
    parser.add_argument('--out',type=Path,required=True)
    args = parser.parse_args()
    result = check(args.ledger,args.report)
    args.out.write_text(json.dumps(result,indent=2,allow_nan=False)+'\n')
    print(f'Wrote {args.out}')
