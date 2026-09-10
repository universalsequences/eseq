#!/usr/bin/env python3
"""Level-preserving reference comparisons for every bar and strike strength."""
import hashlib
import json
import time

import numpy as np
import soundfile as sf

from analyze import read_reference
from calibrate import KEYS
from common import HERE, ROOT, SOURCE, instrument

WINDOWS = [(0, .025), (.025, .12), (.12, .4), (.4, 1), (1, 2), (2, 3)]


def rms(y):
    return float(np.sqrt(np.mean(np.asarray(y, dtype=np.float64)**2)))


def band_power(y, sr, centers, start, end):
    x = y[int(start*sr):int(end*sr)]
    n = max(32768, 2**int(np.ceil(np.log2(len(x)))))
    window = np.hanning(len(x))
    a = np.mean(abs(np.fft.rfft(x*window[:, None], n, axis=0))**2, axis=1)
    a *= 2/(n*np.sum(window**2))
    f = np.fft.rfftfreq(n, 1/sr)
    # Non-overlapping frequency cells with narrow maximum widths. Report the
    # uncaptured energy too, so a good modal score cannot hide an absent attack.
    edges = (centers[1:]+centers[:-1])/2
    power = []
    for i, center in enumerate(centers):
        width = max(25, 2/(end-start))
        lo = max(center-width, edges[i-1] if i else 0)
        hi = min(center+width, edges[i] if i < len(edges) else sr/2)
        power.append(a[(f >= lo) & (f < hi)].sum())
    return np.array(power), float(a.sum())


def main():
    data = json.loads((HERE/'reference-analysis.json').read_text())
    inst = instrument()
    output = HERE/'output'
    output.mkdir(exist_ok=True)
    rows = []
    medley = []
    for key, bar in zip(KEYS, data['bars']):
        centers = np.array(bar['frequencies_hz'])
        for ref in bar['recordings']:
            assert hashlib.sha256((ROOT/ref['file']).read_bytes()).hexdigest() == ref['sha256'], ref['file']
            y, sr, _ = read_reference(ROOT/ref['file'])
            length = min(4, len(y)/sr)
            started = time.process_time()
            synth, state = inst.render(length, pitch=440*2**((key-69)/12), vel=ref['velocity'])
            elapsed = time.process_time()-started
            assert np.isfinite(synth).all() and np.isfinite(state).all()
            assert abs(synth).max() > 1e-5, 'Silent model'
            errors, upper_errors, levels, capture = [], [], [], []
            for start, end in WINDOWS:
                a, total_a = band_power(y, sr, centers, start, end)
                b, total_b = band_power(synth, sr, centers, start, end)
                normalized_a, normalized_b = a/max(a.sum(), 1e-30), b/max(b.sum(), 1e-30)
                active = a > a.max()*.001
                error = 10*np.log10((normalized_b[active]+1e-15)/(normalized_a[active]+1e-15))
                errors.append(float(np.average(abs(error), weights=a[active])))
                upper = (np.arange(len(a)) > 0) & (a > a.max()*.0001)
                if np.any(upper):
                    # Removing the fundamental from the weights prevents a
                    # nearly sinusoidal sustain from hiding overtone errors.
                    relative = 10*np.log10((b[upper]/max(b[0], 1e-30)+1e-15)
                                           /(a[upper]/max(a[0], 1e-30)+1e-15))
                    upper_errors.append(float(np.average(abs(relative), weights=a[upper])))
                else:
                    upper_errors.append(None)
                levels.append([rms(y[int(start*sr):int(end*sr)]), rms(synth[int(start*sr):int(end*sr)])])
                capture.append([float(a.sum()/max(total_a, 1e-30)), float(b.sum()/max(total_b, 1e-30))])
            rows.append({'bar': bar['bar'], 'strength': ref['strength'], 'midi': key,
                         'weighted_modal_error_db': errors, 'rms_reference_model': levels,
                         'upper_modes_relative_to_fundamental_error_db': upper_errors,
                         'modal_energy_fraction_reference_model': capture,
                         'peak_reference_model': [float(abs(y[:len(synth)]).max()), float(abs(synth).max())],
                         'cpu_percent_one_voice': elapsed/length*100})
            secs = min(3, length)
            clip = np.concatenate([y[:int(secs*sr)], np.zeros((sr//2, 2)), synth[:int(secs*sr)]])
            sf.write(output/f'bar-{bar["bar"]}-{ref["strength"]}-ab.wav', clip, sr, subtype='FLOAT')
            if ref['strength'] == 'medium':
                medley.append(clip)
            level_db = [20*np.log10(max(b, 1e-15)/max(a, 1e-15)) for a, b in levels]
            print(bar['bar'], ref['strength'], 'spectrum dB', np.round(errors[:4], 2).tolist(),
                  'level dB', np.round(level_db[:4], 2).tolist(), flush=True)
    sf.write(output/'seven-bars-ab.wav', np.concatenate(medley), 48000, subtype='FLOAT')
    result = {'source_sha256': hashlib.sha256(SOURCE.read_bytes()).hexdigest(),
              'compiler_sha256': inst.compiler_sha256, 'windows_seconds': WINDOWS,
              'metric': 'Reference-energy-weighted absolute dB error of normalized modal-band power; often dominated by the fundamental. A separate upper-mode metric measures overtones relative to the fundamental, weighting only overtones with at least -40 dB of reference fundamental-band power (null if none). Neither is a perceptual similarity score. RMS and energy outside modal bands are reported independently. No gain matching or individual peak normalization.',
              'comparisons': rows}
    (HERE/'reference-comparison.json').write_text(json.dumps(result, indent=2)+'\n')
    # Independent level and upper-mode gates prevent a normalized spectrum
    # metric from accepting either a quiet model or a fundamental-only tone.
    for row in rows:
        label = (row['bar'], row['strength'])
        level_error = [abs(20*np.log10(b/a)) for a, b in row['rms_reference_model']]
        assert level_error[0] < 6.5, (label, 'initial attack level', level_error[0])
        assert max(level_error[1:4]) < 3, (label, 'early level', level_error[1:4])
        assert max(row['weighted_modal_error_db'][1:4]) < 3, (label, 'modal spectrum')
        upper = [v for v in row['upper_modes_relative_to_fundamental_error_db'][1:4] if v is not None]
        assert all(v < 8.5 for v in upper), (label, 'upper-mode spectrum', upper)
    print('All 35 references pass attack, level, modal and upper-mode comparison gates', flush=True)


if __name__ == '__main__':
    main()
