#!/usr/bin/env python3
"""Compare modal piano renders with all 30 factory notes and make A/B clips."""
import json
import hashlib
import time

import numpy as np
from scipy.signal import resample_poly
import soundfile as sf

from analyze import read_reference, stiff_frequencies, hz
from common import HERE, ROOT, SOURCE, instrument


def energy_bands(y, sr, centers, start, end):
    segment = y[int(start*sr):int(end*sr)]
    n = 2**int(np.ceil(np.log2(max(len(segment), 16384))))
    z = np.fft.rfft(segment * np.hanning(len(segment))[:, None], n, axis=0)
    power = np.mean(abs(z)**2, axis=1)
    f = np.fft.rfftfreq(n, 1/sr)
    width = np.maximum(centers*.012, centers[0]*.32)
    return np.array([power[abs(f-c) < w].sum() for c, w in zip(centers, width)])


def rms(y):
    return float(np.sqrt(np.mean(y*y)))


def main():
    references = json.loads((HERE / 'reference-analysis.json').read_text())['notes']
    inst = instrument()
    output = HERE / 'output'
    output.mkdir(exist_ok=True)
    comparisons = []
    for r in references:
        reference, sr, _ = read_reference(ROOT / r['file'])
        length = min(8, len(reference)/sr-.01)
        started = time.process_time()
        synth, state = inst.render(length, pitch=hz(r['midi']))
        elapsed = time.process_time()-started
        assert np.isfinite(synth).all() and np.isfinite(state).all()
        centers = stiff_frequencies(r['fundamental_hz'], r['stiffness_B'])
        windows = [(.025, .12), (.12, .4), (.4, 1), (1, 2), (2, 3)]
        errors, levels = [], []
        for start, end in windows:
            a = energy_bands(reference, sr, centers, start, end)
            b = energy_bands(synth, inst.sample_rate, centers, start, end)
            a /= max(a.sum(), 1e-20)
            b /= max(b.sum(), 1e-20)
            active = (a > a.max()*.001) & (centers < 12000)
            error = 10*np.log10((b[active]+1e-12)/(a[active]+1e-12))
            errors.append(float(np.sum(abs(error)*a[active])/sum(a[active])))
            levels.append([rms(reference[int(start*sr):int(end*sr)]),
                           rms(synth[int(start*inst.sample_rate):int(end*inst.sample_rate)])])
        row = {'midi': r['midi'], 'weighted_partial_error_db': errors, 'rms_reference_synth': levels,
               'peak': float(abs(synth).max()), 'render_cpu_seconds': elapsed, 'audio_seconds': length}
        comparisons.append(row)
        print(r['midi'], 'dB', np.round(errors, 2), 'levels', np.round(levels[:2], 3).tolist(),
              'cpu', round(elapsed/length*100, 1), flush=True)
        if r['midi'] in [21, 36, 48, 60, 72, 84, 96, 108]:
            secs = min(3, length)
            a = resample_poly(reference[:int(secs*sr)], 160, 147)
            b = synth[:len(a)]
            # One shared gain, so A/B clips preserve the actual level difference.
            pair = np.concatenate([a, np.zeros((24000, 2)), b])
            sf.write(output / f'compare-{r["midi"]}.wav', pair, 48000, subtype='PCM_24')
    (HERE / 'reference-comparison.json').write_text(json.dumps({
        'compiler_sha256': inst.compiler_sha256,
        'source_sha256': hashlib.sha256(SOURCE.read_bytes()).hexdigest(),
        'windows_seconds': windows,
        'metric': 'Reference-energy-weighted absolute dB error of normalized partial-band power. This compares string-spectrum shape, not phase, overall level, body or recording noise. Late windows can measure the noise floor after the tone dies; read alongside the RMS levels.',
        'notes': comparisons}, indent=2)+'\n')


if __name__ == '__main__':
    main()
