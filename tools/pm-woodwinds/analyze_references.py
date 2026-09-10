#!/usr/bin/env python3
"""Measure supplied sustained sax notes without redistributing the recordings.

Requires numpy, scipy and soundfile. The short gb2 clip is a moving articulation,
so it is reported as such rather than fitted as a steady note.
"""
import hashlib
import json
from pathlib import Path

import numpy as np
from scipy.signal import butter, periodogram, sosfiltfilt, stft
import soundfile as sf

from verify import ROOT, harmonic_db


def main():
    results = []
    for path in sorted((ROOT / 'samples-to-analyze').glob('*.wav')):
        y, sr = sf.read(path)
        if y.ndim > 1:
            y = y.mean(axis=1)
        entry = {'file': path.name, 'sha256': hashlib.sha256(path.read_bytes()).hexdigest(),
                 'sample_rate': sr, 'seconds': len(y) / sr, 'peak': float(np.max(abs(y)))}
        if len(y) < 3 * sr:
            entry['analysis'] = 'Short moving articulation; excluded from sustained-note fit.'
            results.append(entry)
            continue
        if 'gb3' in path.name:
            nominal, harmonic = 370, 2
        elif 'gb2' in path.name:
            nominal, harmonic = 183, 3
        else:
            entry['analysis'] = 'Unknown pitch; not part of the supplied Gb reference pair.'
            results.append(entry)
            continue
        freqs, times, z = stft(y, sr, nperseg=4096, noverlap=4096 - 128)
        mag = abs(z)
        band = np.flatnonzero(abs(freqs - nominal * harmonic) < nominal * .4)
        bins = band[np.argmax(mag[band], axis=0)]
        cols = np.arange(len(times))
        log = np.log(mag + 1e-12)
        denominator = log[bins - 1, cols] - 2 * log[bins, cols] + log[bins + 1, cols]
        delta = .5 * (log[bins - 1, cols] - log[bins + 1, cols]) / np.minimum(denominator, -1e-12)
        hz = (bins + delta) * sr / 4096 / harmonic
        use = (times > .4) & (times < 3.7)
        f0 = float(np.median(hz[use]))
        cents = 1200 * np.log2(hz[use] / f0)
        rate = sr / 128
        vibrato = sosfiltfilt(butter(3, [3, 9], fs=rate, btype='bandpass', output='sos'), cents)
        vf, vp = periodogram(vibrato, fs=rate, nfft=8192)
        valid = (vf > 3) & (vf < 9)
        entry.update({'fundamental_hz': f0,
                      'vibrato_rate_hz': float(vf[valid][np.argmax(vp[valid])]),
                      'vibrato_peak_depth_cents': float(np.sqrt(2) * np.std(vibrato)),
                      'harmonics_db': harmonic_db(y, sr, f0).tolist()})
        results.append(entry)
    output = Path(__file__).parent / 'reference-analysis.json'
    output.write_text(json.dumps(results, indent=2) + '\n')
    print(output)


if __name__ == '__main__':
    main()
