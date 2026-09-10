#!/usr/bin/env python3
"""Extract stiff-string modes and decay from the bundled Salamander recordings.

This is an offline calibration tool. No PCM, spectral frames, or sample phases
are embedded in the instrument. The output describes passive resonant modes.
"""
import hashlib
import json
from pathlib import Path
import re

import numpy as np
from scipy.optimize import differential_evolution, least_squares
from scipy.signal import find_peaks, stft
import soundfile as sf

ROOT = Path(__file__).resolve().parents[2]
HERE = Path(__file__).resolve().parent
SAMPLES = ROOT / 'content/packages/universalsequences.factory-samples/samples/piano'
MODES = 96


def midi(path):
    note, octave = re.fullmatch(r'(A|C|Ds|Fs)(\d)v8', path.stem).groups()
    return 12 * (int(octave) + 1) + {'C': 0, 'Ds': 3, 'Fs': 6, 'A': 9}[note]


def hz(note):
    return 440 * 2 ** ((note - 69) / 12)


def read_reference(path):
    y, sr = sf.read(path, always_2d=True)
    # Locate the onset using stereo energy: averaging the two mics can cancel
    # a partial and understate the actual string energy.
    hop = 44
    energy = np.array([np.mean(y[i:i+hop] ** 2) for i in range(0, len(y), hop)])
    first = int(np.flatnonzero(energy > energy.max() * 1e-4)[0]) * hop
    return y[first:], sr, first / sr


def stiff_frequencies(f0, stiffness, count=MODES):
    n = np.arange(1, count + 1)
    # f0 denotes the first *sounding* partial, not the ideal flexible string.
    return f0 * n * np.sqrt((1 + stiffness * n*n) / (1 + stiffness))


def measure(path):
    note = midi(path)
    y, sr, onset = read_reference(path)
    expected = hz(note)
    a, b = int(.02 * sr), int(min(1.6, max(.24, 200/expected), len(y)/sr*.55) * sr)
    nfft = 2**19
    spectrum = np.sqrt(np.mean(abs(np.fft.rfft(y[a:b] * np.hanning(b-a)[:, None], nfft, axis=0)) ** 2, axis=1))
    freq = np.fft.rfftfreq(nfft, 1/sr)
    count = min(28, int(15000/expected))
    n = np.arange(1, count + 1)
    # Whitening by a local spectral floor prevents the loudest partial alone
    # from deciding the dispersion fit. The floor also rejects MP3 noise.
    from scipy.ndimage import maximum_filter1d
    local_peak = maximum_filter1d(spectrum, size=max(3, int(expected/(sr/nfft)*.8)))
    score = np.sqrt(spectrum / np.maximum(local_peak, spectrum.max()*1e-3))
    def objective(p):
        fs = stiff_frequencies(expected * 2**(p[0]/1200), 10**p[1], count)
        return -np.sum(np.interp(fs, freq, score) / np.sqrt(n))
    fit = differential_evolution(objective, [(-60, 150), (-6.5, -1.1)],
                                 seed=42, popsize=22, maxiter=180, tol=1e-8)
    f0, stiffness = expected * 2**(fit.x[0]/1200), 10**fit.x[1]
    peaks = find_peaks(spectrum)[0]
    # Refine with a quadratic interpolation of log magnitudes around peaks.
    picked, weights, numbers = [], [], []
    for h, target in enumerate(stiff_frequencies(f0, stiffness, count), 1):
        nearby = peaks[abs(freq[peaks] - target) < expected * .14]
        if len(nearby) == 0:
            continue
        k = nearby[np.argmax(spectrum[nearby])]
        if spectrum[k] < spectrum.max()*.008:
            continue
        left, mid, right = np.log(spectrum[k-1:k+2] + 1e-20)
        delta = .5*(left-right)/(left-2*mid+right)
        picked.append((k+delta)*sr/nfft)
        weights.append(np.sqrt(spectrum[k]/spectrum.max()))
        numbers.append(h)
    numbers = np.asarray(numbers)
    def residual(p):
        predicted = p[0] * numbers * np.sqrt((1 + p[1]*numbers**2)/(1+p[1]))
        return 1200*np.log2(predicted/picked) * weights
    refined = least_squares(residual, [f0, stiffness],
                            bounds=([expected*2**(-65/1200), 1e-7], [expected*2**(155/1200), .1]),
                            loss='soft_l1', f_scale=1)
    f0, stiffness = refined.x
    modes = stiff_frequencies(f0, stiffness)

    win = 2**int(np.ceil(np.log2(np.clip(sr/expected*8, 1024, 16384))))
    hop = max(512, win//4)
    f, t, z = stft(y.T, sr, nperseg=win, noverlap=win-hop, boundary=None)
    power = np.mean(abs(z)**2, axis=0)
    # Integrate energy around each mode across all unison-string peaks. Hann
    # ENBW is 1.5 bins; a real sine has half its energy at positive frequencies.
    amplitudes = []
    for i, center in enumerate(modes):
        half = min(expected*.32, max(sr/win*3, center*.008))
        band = abs(f-center) < half
        amplitudes.append(np.sqrt(4/1.5*np.sum(power[band], axis=0)))
    amplitudes = np.asarray(amplitudes)
    fit_rows = []
    errors = []
    for h, env in enumerate(amplitudes):
        if modes[h] > min(15000, sr*.42) or env.max() < amplitudes.max()*.0005:
            fit_rows.append([0, 1, 0, 2])
            errors.append(0)
            continue
        # Exclude the onset window and the terminal recording fade/noise floor.
        valid = (t < min(len(y)/sr*.78, 14)) & (env > max(env.max()*.004, .00003))
        ts, values = t[valid], env[valid]
        if len(ts) < 5:
            fit_rows.append([float(env.max()), 3, 0, 5])
            errors.append(0)
            continue
        # Positive fast + slow energy components represent the normal modes of
        # the bridge-coupled strings/polarizations. Fit amplitude in log space;
        # ordering r_fast = r_slow + delta guarantees a passive aftersound.
        peak = float(env.max())
        def curve(p):
            slow, delta = np.exp(p[2]), np.exp(p[3])
            return np.sqrt(np.exp(2*p[0]-2*(slow+delta)*ts) + np.exp(2*p[1]-2*slow*ts))
        def err(p):
            return (np.log(curve(p)+1e-12)-np.log(values+1e-12)) / np.sqrt(1+ts)
        slope = np.clip(-np.polyfit(ts, np.log(values+1e-12), 1)[0], .04, 35)
        candidates = [least_squares(err, np.log([peak, peak*mix, max(.0151, slope*slow), slope*fast]),
                          bounds=(np.log([peak*.0001, peak*.0001, .015, .001]),
                                  np.log([peak*5, peak*5, 80, 150])), max_nfev=180)
                      for mix, slow, fast in [(.4, .6, 1), (.06, .3, 4)]]
        p = min(candidates, key=lambda fit: np.sum(fit.fun**2))
        af, ass, rs, delta = np.exp(p.x)
        fit_rows.append([float(af), float(rs+delta), float(ass), float(rs)])
        errors.append(float(np.sqrt(np.mean(err(p.x)**2))*20/np.log(10)))
    return {'file': str(path.relative_to(ROOT)), 'sha256': hashlib.sha256(path.read_bytes()).hexdigest(),
            'midi': note, 'sample_rate': sr, 'onset_seconds': onset, 'seconds': len(y)/sr,
            'fundamental_hz': f0, 'tuning_cents': 1200*np.log2(f0/expected), 'stiffness_B': stiffness,
            'stiffness_identified': len(numbers) >= 2,
            'partial_peak_hz': picked, 'partial_numbers': numbers.tolist(),
            'mode_fit_columns': ['fast_amplitude', 'fast_rate_per_s', 'slow_amplitude', 'slow_rate_per_s'],
            'modes': fit_rows, 'mode_envelope_rmse_db': errors,
            'peak': float(abs(y).max()), 'rms_first_second': float(np.sqrt(np.mean(y[:sr]**2)))}


def main():
    records = []
    for path in sorted(SAMPLES.glob('*.mp3'), key=midi):
        r = measure(path)
        records.append(r)
        print(path.name, 'cents', round(r['tuning_cents'], 2), 'B', round(r['stiffness_B'], 7),
              'env dB', round(float(np.median(r['mode_envelope_rmse_db'][:12])), 2), flush=True)
    result = {'source': 'Alexander Holm, Salamander Grand Piano V3, CC BY 3.0; factory v8 MP3 subset',
              'license': 'https://creativecommons.org/licenses/by/3.0/', 'modes_per_note': MODES, 'notes': records}
    (HERE / 'reference-analysis.json').write_text(json.dumps(result, indent=2)+'\n')


if __name__ == '__main__':
    main()
