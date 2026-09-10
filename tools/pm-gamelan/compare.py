#!/usr/bin/env python3
"""Compare the actual factory DSP to every reference, preserving raw levels."""
import argparse
import hashlib
import json

import numpy as np
import soundfile as sf

from analyze import read_reference
from common import instrument
from families import FAMILIES, HERE, ROOT

WINDOWS = [(0, .025), (.025, .12), (.12, .4), (.4, 1), (1, 2), (2, 3), (3, 5)]


def rms(y):
    return float(np.sqrt(np.mean(np.asarray(y, dtype=np.float64)**2)))


def band_power(y, sr, centers, start, end):
    x = y[int(start*sr):int(end*sr)]
    n = max(32768, 2**int(np.ceil(np.log2(len(x)))))
    window = np.hanning(len(x))
    a = np.mean(abs(np.fft.rfft(x*window[:, None], n, axis=0))**2, axis=1)
    a *= 2/(n*np.sum(window**2))
    f = np.fft.rfftfreq(n, 1/sr)
    edges = (centers[1:]+centers[:-1])/2
    power = []
    for i, center in enumerate(centers):
        width = max(15, 2/(end-start))
        lo = max(center-width, edges[i-1] if i else 0)
        hi = min(center+width, edges[i] if i < len(edges) else sr/2)
        power.append(a[(f >= lo) & (f < hi)].sum())
    return np.array(power), float(a.sum())


def compare(slug):
    family = FAMILIES[slug]
    data = json.loads((HERE/f'{slug}-analysis.json').read_text())
    inst = instrument(family.source)
    output = HERE/'output'/slug
    output.mkdir(parents=True, exist_ok=True)
    rows, medley = [], []
    for unit in data['units']:
        centers = np.array(unit['frequencies_hz'])
        for ref in unit['recordings']:
            path = ROOT/ref['file']
            assert hashlib.sha256(path.read_bytes()).hexdigest() == ref['sha256'], path
            y, sr, _ = read_reference(path)
            length = min(6, len(y)/sr)
            synth, state = inst.render(length, pitch=440*2**((unit['midi']-69)/12), vel=ref['velocity'])
            assert np.isfinite(synth).all() and np.isfinite(state).all()
            assert abs(synth).max() > 1e-5
            metrics = []
            for start, end in WINDOWS:
                if end > length:
                    continue
                a, total_a = band_power(y, sr, centers, start, end)
                b, total_b = band_power(synth, sr, centers, start, end)
                active = a > a.max()*.001
                error = 10*np.log10((b[active]/max(b.sum(), 1e-30)+1e-15)/(a[active]/max(a.sum(), 1e-30)+1e-15))
                # Weight secondary modes independently of the strongest peak,
                # which is not necessarily the pitch reference for these gongs.
                dominant = int(np.argmax(a))
                secondary = (np.arange(len(a)) != dominant) & (a > a.max()*.0001)
                relative = 10*np.log10((b[secondary]/max(b[dominant], 1e-30)+1e-15)
                                       /(a[secondary]/max(a[dominant], 1e-30)+1e-15))
                level_a, level_b = rms(y[int(start*sr):int(end*sr)]), rms(synth[int(start*sr):int(end*sr)])
                metrics.append({'window_seconds': [start, end],
                    'level_error_db': float(20*np.log10(max(level_b, 1e-15)/max(level_a, 1e-15))),
                    'rms_reference_model': [level_a, level_b],
                    'modal_error_db': float(np.average(abs(error), weights=a[active])),
                    'secondary_mode_error_db': float(np.average(abs(relative), weights=a[secondary])) if np.any(secondary) else None,
                    'modal_energy_fraction_reference_model': [float(a.sum()/max(total_a, 1e-30)), float(b.sum()/max(total_b, 1e-30))]})
            rows.append({'unit': unit['label'], 'midi': unit['midi'], 'strength': ref['strength'], 'windows': metrics,
                         'peak_reference_model': [float(abs(y[:len(synth)]).max()), float(abs(synth).max())]})
            frames = min(len(synth), sr*4)
            clip = np.concatenate([y[:frames], np.zeros((sr//2, 2)), synth[:frames]])
            sf.write(output/f'{unit["label"] or "pot"}-{ref["strength"]}-ab.wav', clip, sr, subtype='FLOAT')
            if ref['strength'] == 'medium':
                medley.append(clip)
            print(slug, unit['label'], ref['strength'], 'level dB',
                  [round(m['level_error_db'], 2) for m in metrics], flush=True)
    early = [w for r in rows for w in r['windows'][1:4]]
    def statistics(values):
        return {'median': float(np.median(values)), 'p90': float(np.percentile(values, 90)), 'maximum': float(max(values))}
    summary = {'early_absolute_level_error_db': statistics([abs(w['level_error_db']) for w in early]),
               'early_modal_error_db': statistics([w['modal_error_db'] for w in early]),
               'early_secondary_mode_error_db': statistics([w['secondary_mode_error_db'] for w in early if w['secondary_mode_error_db'] is not None])}
    result = {'source_sha256': hashlib.sha256(family.source.read_bytes()).hexdigest(),
              'compiler_sha256': inst.compiler_sha256, 'group': family.prefix,
              'metric_note': 'Raw levels, no gain matching. Spectral cell errors are not perceptual similarity. Secondary metric excludes the strongest peak, not necessarily the pitch mode. Each source recording is compared; no withheld independent recordings exist.',
              'summary': summary, 'comparisons': rows}
    (HERE/f'{slug}-comparison.json').write_text(json.dumps(result, indent=2)+'\n')
    sf.write(output/'reference-model-ab.wav', np.concatenate(medley), 48000, subtype='FLOAT')
    print(slug, json.dumps(summary), flush=True)
    return result


def check_report(slug, result):
    """Regression envelopes for this measured baseline, not similarity claims."""
    assert result['source_sha256'] == hashlib.sha256(FAMILIES[slug].source.read_bytes()).hexdigest()
    summary = result['summary']
    assert summary['early_absolute_level_error_db']['median'] < 1.5, (slug, 'median level')
    assert summary['early_absolute_level_error_db']['p90'] < 3, (slug, '90th percentile level')
    assert summary['early_absolute_level_error_db']['maximum'] < 6, (slug, 'maximum early level')
    assert summary['early_modal_error_db']['p90'] < 3, (slug, 'modal spectrum')
    assert summary['early_secondary_mode_error_db']['median'] < 3, (slug, 'secondary modes')
    assert summary['early_secondary_mode_error_db']['p90'] < 8, (slug, 'secondary modes p90')
    assert max(abs(r['windows'][0]['level_error_db']) for r in result['comparisons']) < 8, (slug, 'initial impact')
    print(slug, 'reference regression checks passed', flush=True)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('families', nargs='*', choices=list(FAMILIES))
    parser.add_argument('--check-saved', action='store_true', help='Check existing reports and DSP hashes without rerendering')
    args = parser.parse_args()
    for slug in args.families or FAMILIES:
        result = json.loads((HERE/f'{slug}-comparison.json').read_text()) if args.check_saved else compare(slug)
        check_report(slug, result)


if __name__ == '__main__':
    main()
