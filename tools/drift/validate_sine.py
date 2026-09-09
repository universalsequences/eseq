#!/usr/bin/env python3
"""Independent sine validation of the Type-I candidate from saw/HP captures.

No parameters, gain, delay, or phase are fitted to this batch. This validates
only the candidate's fundamental response; nonlinear harmonics are excluded.
"""
import argparse
import hashlib
import json
from pathlib import Path
import numpy as np
from analyze_reference import SR, load, window
from diagnose_reference import harmonic_noise
from fit_linear import response


def candidate(frequencies, cutoff, resonance):
    return (1.541*response(frequencies, 2*SR, cutoff, .423/(1-resonance))
            *response(frequencies, 2*SR, 20, 1.469, True))


def validate(ledger_path, report_path):
    ledger = json.loads(ledger_path.read_text())
    report = json.loads(report_path.read_text())
    for path, digest in [(ledger_path, report['ledger_sha256']),
                         (ledger_path.with_suffix('.als'), report['set_sha256'])]:
        if hashlib.sha256(path.read_bytes()).hexdigest() != digest:
            raise ValueError(f'{path}: provenance mismatch')
    cases = {c['name']: c for c in ledger['cases']}
    cache = {}

    def spectrum(name):
        if name not in cache:
            path = ledger_path.parent/f'{ledger_path.stem} {name}.wav'
            if hashlib.sha256(path.read_bytes()).hexdigest() != report['files'][name]['sha256']:
                raise ValueError(f'{path}: capture changed')
            y = load(path)
            values, snrs = [], []
            for event, note in zip(cases[name]['notes'], report['files'][name]['notes']):
                x = window(y, event)[:, 0]
                frequency = note['fundamental_hz']
                w = np.hanning(len(x))
                values.append(2*np.sum((x-x.mean())*w*np.exp(
                    -2j*np.pi*frequency*np.arange(len(x))/SR))/w.sum())
                amplitude, noise = harmonic_noise(x, frequency)
                snrs.append(float(20*np.log10(amplitude[0]/noise[0])))
            cache[name] = np.array(values), np.array(snrs)
        return cache[name]

    results = []
    for name, case in cases.items():
        if not name.startswith('sine-t1-'):
            continue
        source_name = report['spectral_ratios'][name]['bypass']
        source, source_snr = spectrum(source_name)
        output, output_snr = spectrum(name)
        frequencies = np.array([n['fundamental_hz'] for n in report['files'][name]['notes']])
        params = case['parameters']
        cutoff = float(params['Filter_Frequency'])
        resonance = float(params['Filter_Resonance'])
        ratio = candidate(frequencies, cutoff, resonance)/(output/source)
        valid = np.minimum(source_snr, output_snr) >= 30
        results.append(dict(case=name, cutoff=cutoff, resonance=resonance,
            source=source_name, notes=[dict(note=event['note'],
                frequency_hz=float(frequencies[i]), qualified=bool(valid[i]),
                minimum_local_snr_db=float(min(source_snr[i], output_snr[i])),
                magnitude_error_db=float(20*np.log10(abs(ratio[i]))),
                phase_error_radians=float(np.angle(ratio[i])))
                for i, event in enumerate(case['notes'])]))
    return dict(methodology=__doc__, analysis_report_sha256=hashlib.sha256(
        report_path.read_bytes()).hexdigest(), candidate_parameters=dict(
            internal_sample_rate=2*SR, gain=1.541, q_numerator=.423,
            hp_frequency=20, hp_q=1.469), cases=results)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('ledger', type=Path)
    parser.add_argument('--report', type=Path, required=True)
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    result = validate(args.ledger, args.report)
    args.out.write_text(json.dumps(result, indent=2, allow_nan=False)+'\n')
    print(f'Wrote {args.out}')
