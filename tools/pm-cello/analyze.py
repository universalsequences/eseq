#!/usr/bin/env python3
"""Measure all supplied cello articulations without incorporating their PCM into the synth."""
import hashlib
import json
from pathlib import Path

import numpy as np
from scipy.signal import butter, find_peaks, sosfiltfilt, stft
import soundfile as sf

ROOT = Path(__file__).resolve().parents[2]
HERE = Path(__file__).resolve().parent
REFERENCES = {
    'crescendo': ('358373__mtg__cello-a2-bad-dynamics-crescendo.wav', 110, [(0.6, 1.3), (2.3, 3.1), (3.6, 4.3)]),
    'pizzicato': ('372760__sgossner__cello-section-tight-pizzicato-f2-pizzt_g1_v2_rr2.wav', 98, [(0.02, .18), (.18, .45), (.5, .9)]),
    'spiccato': ('372813__sgossner__cello-section-spiccato-f4-spic_g3_v2_rr2.wav', 390, [(.04, .16), (.16, .32), (.4, .65)]),
}


def mono(y):
    return y.mean(axis=1) if y.ndim > 1 else y


def rms_envelope(y, sr, hop=.02):
    y = mono(y)
    n = int(round(sr * hop))
    return np.array([np.sqrt(np.mean(y[i:i+n] ** 2)) for i in range(0, len(y), n)])


def harmonics(y, sr, f0, window, count=24):
    y = mono(y)
    start, end = [int(x * sr) for x in window]
    segment = y[start:end]
    n = 2 ** int(np.ceil(np.log2(max(8192, len(segment)))))
    power = abs(np.fft.rfft(segment * np.hanning(len(segment)), n)) ** 2
    frequencies = np.fft.rfftfreq(n, 1 / sr)
    bands = np.array([np.sum(power[abs(frequencies - f0 * h) < f0 * .18]) for h in range(1, count + 1)])
    return 10 * np.log10(bands / max(bands.sum(), 1e-30) + 1e-9)


def pitch_track(y, sr, expected):
    y = mono(y)
    n = 8192
    f, t, z = stft(y, sr, nperseg=n, noverlap=n - 256)
    magnitude = abs(z)
    band = np.flatnonzero(abs(f - expected) < expected * .12)
    bins = band[np.argmax(magnitude[band], axis=0)]
    cols = np.arange(len(t))
    log = np.log(magnitude + 1e-20)
    den = log[bins-1, cols] - 2 * log[bins, cols] + log[bins+1, cols]
    offset = .5 * (log[bins-1, cols] - log[bins+1, cols]) / np.minimum(den, -1e-12)
    return t, (bins + offset) * sr / n


def main():
    report = {}
    for key, (name, expected, windows) in REFERENCES.items():
        path = ROOT / 'samples-to-analyze' / name
        y, sr = sf.read(path)
        clean = sosfiltfilt(butter(2, 30, btype='highpass', fs=sr, output='sos'), y, axis=0)
        # Match the fit and A/B level measurements. Highpass only the pitch and
        # harmonic analysis, where recording rumble can mask the fundamental.
        env = rms_envelope(y, sr)
        peak_at = int(np.argmax(env))
        active = np.flatnonzero(env > max(env) * .02)
        t, hz = pitch_track(clean, sr, expected)
        use = (t > windows[0][0]) & (t < windows[1][1])
        f0 = float(np.median(hz[use]))
        a, b = int(windows[0][0] * sr), int(windows[1][1] * sr)
        segment = mono(clean)[a:b]
        spectrum = abs(np.fft.rfft(segment * np.hanning(len(segment)), 2**19))
        freq = np.fft.rfftfreq(2**19, 1 / sr)
        peaks = find_peaks(spectrum)[0]
        near = [i for i in peaks if abs(freq[i] - expected) < expected * .07]
        near = sorted(near, key=lambda i: spectrum[i], reverse=True)[:5]
        report[key] = {
            'file': name, 'sha256': hashlib.sha256(path.read_bytes()).hexdigest(),
            'sample_rate': sr, 'channels': 1 if y.ndim == 1 else y.shape[1], 'seconds': len(y) / sr,
            'fundamental_hz': f0, 'midi_note': float(69 + 12 * np.log2(f0 / 440)),
            'active_2_percent_seconds': [float(active[0] * .02), float(active[-1] * .02)],
            'peak_rms_seconds': float(peak_at * .02), 'peak_rms': float(max(env)),
            'envelope_signal': 'unfiltered mono average', 'spectral_highpass_hz': 30,
            'envelope_hop_seconds': .02, 'rms_envelope': env.tolist(),
            'analysis_windows_seconds': windows,
            'harmonics_db': [harmonics(clean, sr, f0, window).tolist() for window in windows],
            'near_fundamental_peaks': [[float(freq[i]), float(20*np.log10(spectrum[i] / max(spectrum)))] for i in near],
            'pitch_track_seconds': t[use][::8].tolist(), 'pitch_track_hz': hz[use][::8].tolist(),
        }
        print(key, 'f0', round(f0, 3), 'peak at', peak_at * .02,
              'H1-H8', np.round(report[key]['harmonics_db'][1][:8], 1).tolist(), flush=True)
    (HERE / 'reference-analysis.json').write_text(json.dumps(report, indent=2) + '\n')


if __name__ == '__main__':
    main()
