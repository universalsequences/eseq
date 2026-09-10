#!/usr/bin/env python3
"""Measure the supplied clarinet note; keep the recording outside factory content."""
import hashlib
import json
from pathlib import Path

import numpy as np
from scipy.signal import butter, periodogram, sosfiltfilt, stft
import soundfile as sf

ROOT = Path(__file__).resolve().parents[2]
REFERENCE = ROOT / 'samples-to-analyze/249113__clarinet_pablo_proj__overall-quality-of-single-note-clarinet-d4.wav'


def harmonics(y, sr, f0, start=1, end=5.5, count=16):
    n = 8192
    window = np.hanning(n)
    frames = np.stack([y[i:i+n] * window for i in range(int(start*sr), min(int(end*sr), len(y)-n), 2048)])
    power = abs(np.fft.rfft(frames)) ** 2
    f = np.fft.rfftfreq(n, 1/sr)
    energy = np.array([np.mean(np.sum(power[:, abs(f-h*f0) < .13*f0], axis=1)) for h in range(1, count+1)])
    return 10*np.log10(energy / max(np.sum(energy), 1e-30) + 1e-8)


def main():
    y, sr = sf.read(REFERENCE)
    if y.ndim > 1:
        y = y.mean(axis=1)
    clean = sosfiltfilt(butter(3, 80, btype='highpass', fs=sr, output='sos'), y)
    f, t, z = stft(clean, sr, nperseg=8192, noverlap=8192-256)
    mag = abs(z)
    # The filename identifies D4; use its clear third harmonic for tracking,
    # then retain all 16 harmonics to check the odd/even balance independently.
    band = np.flatnonzero((f > 840) & (f < 940))
    bins = band[np.argmax(mag[band], axis=0)]
    cols = np.arange(len(t))
    log = np.log(mag + 1e-15)
    den = log[bins-1, cols] - 2*log[bins, cols] + log[bins+1, cols]
    shift = .5*(log[bins-1, cols]-log[bins+1, cols]) / np.minimum(den, -1e-12)
    hz = (bins+shift)*sr/8192/3
    use = (t > 1) & (t < 5.5)
    f0 = float(np.median(hz[use]))
    cents = 1200*np.log2(hz[use]/f0)
    fs = sr/256
    motion = sosfiltfilt(butter(3, [3, 9], fs=fs, btype='bandpass', output='sos'), cents)
    vf, vp = periodogram(motion, fs=fs, nfft=8192)
    valid = (vf > 3) & (vf < 9)
    window = max(1, int(.01*sr))
    rms = np.sqrt(np.convolve(clean**2, np.ones(window)/window, mode='same'))
    plateau = float(np.median(rms[int(sr):int(5.5*sr)]))
    onset = int(np.flatnonzero(rms > plateau*.1)[0])
    full = int(np.flatnonzero(rms > plateau*.9)[0])
    end = int(np.flatnonzero(rms > plateau*.1)[-1])
    hd = harmonics(clean, sr, f0)
    p = 10**(hd/10)
    report = {
        'file': REFERENCE.name, 'sha256': hashlib.sha256(REFERENCE.read_bytes()).hexdigest(),
        'sample_rate': sr, 'seconds': len(y)/sr, 'peak': float(np.max(abs(y))),
        'fundamental_hz': f0, 'midi_note': float(69+12*np.log2(f0/440)),
        'sustain_window_seconds': [1, 5.5], 'harmonics_db': hd.tolist(),
        'odd_to_even_db': float(10*np.log10(np.sum(p[::2])/np.sum(p[1::2]))),
        'onset_seconds': onset/sr, 'active_end_seconds': end/sr,
        'attack_10_90_ms': (full-onset)/sr*1000, 'sustain_rms': plateau,
        'pitch_motion_3_9_hz_peak_cents': float(np.sqrt(2)*np.std(motion)),
        'pitch_motion_dominant_hz': float(vf[valid][np.argmax(vp[valid])]),
        'pitch_motion_note': 'Band-limited pitch variation, not proof of intentional periodic vibrato.',
    }
    (Path(__file__).parent/'reference-analysis.json').write_text(json.dumps(report, indent=2)+'\n')
    print(json.dumps(report, indent=2))


if __name__ == '__main__':
    main()
