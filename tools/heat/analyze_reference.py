#!/usr/bin/env python3
"""Validate captured WAVs and measure deterministic gains and harmonic responses."""
import argparse
import hashlib
import json
from pathlib import Path
import numpy as np
from scipy.io import wavfile
from scipy.optimize import least_squares


def load(path):
    sr, y = wavfile.read(path)
    if sr != 48000 or y.dtype != np.float32 or y.shape != (960000, 2):
        raise ValueError(f"Invalid capture {path}: expected 20 s stereo float32 at 48 kHz; got {sr}, {y.dtype}, {y.shape}")
    if not np.isfinite(y).all():
        raise ValueError(f"Non-finite samples in {path}")
    return y


def harmonics(y, freq=32.68445, count=256):
    x = y[48000:96000, 0].astype(float)
    t = np.arange(len(x)) / 48000
    window = np.hanning(len(x))
    return np.array([2 * abs(np.sum(x * window * np.exp(-2j * np.pi * freq * h * t))) / window.sum()
                     for h in range(1, count + 1)])


def analyze(folder):
    report = {"sample_rate": 48000, "duration_seconds": 20, "files": {}, "filter_fits": {}}
    bypass = harmonics(load(folder / 'sources-v2 source-saw.wav'))
    frequencies = np.arange(1, len(bypass) + 1) * 32.68445
    for path in sorted(folder.glob('static *.wav')):
        y = load(path)
        name = path.stem.removeprefix('static ')
        rms = np.sqrt(np.mean(y[9*48000:10*48000].astype(float)**2, axis=0))
        report['files'][name] = dict(sha256=hashlib.sha256(path.read_bytes()).hexdigest(), rms=rms.tolist(), peak=float(np.max(abs(y))))
        if name.startswith('filter-'):
            response = harmonics(y) / np.maximum(bypass, 1e-12)
            report['files'][name]['response_hz'] = frequencies.tolist()
            report['files'][name]['response_db'] = (20*np.log10(np.maximum(response, 1e-12))).tolist()
            typ = int(name.split('-')[1])
            if typ in (0, 1, 2, 3, 4, 5, 6, 7):
                stages = 2 if typ % 2 else 1
                def predicted(p):
                    f0, q, gain = np.exp(p)
                    s = 1j * np.tan(np.pi * frequencies / 48000) / np.tan(np.pi * f0 / 48000)
                    denominator = s*s+s/q+1
                    numerator = (1 if typ<2 else s if typ<4 else (s*s+1) if typ<6 else s*s)
                    return abs(gain * (numerator / denominator)**stages)
                mask = (bypass > 1e-5) & (response > 1e-4)
                def residual(p):
                    return (20*np.log10(np.maximum(predicted(p)[mask],1e-12)) - 20*np.log10(response[mask]))
                result = least_squares(residual, np.log([682, .7, 1]), bounds=(np.log([20,.01,.001]), np.log([22000,100,100])))
                f0,q,gain=np.exp(result.x)
                report['filter_fits'][name] = dict(f0=f0, q=q, gain=gain, rms_error_db=float(np.sqrt(np.mean(result.fun**2))))
    return report


if __name__ == '__main__':
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('folder',type=Path)
    parser.add_argument('--out',type=Path,required=True)
    args=parser.parse_args()
    result=analyze(args.folder)
    args.out.write_text(json.dumps(result,indent=2)+'\n')
    print(json.dumps(result['filter_fits'],indent=2))
