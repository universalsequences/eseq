#!/usr/bin/env python3
"""Identify shared passive bar modes from five strikes of each pelog key.

Only frequencies, damping rates, and force-to-mode residues leave this tool.
Recording phases, waveform samples and spectral frames are never deployed.
"""
import hashlib
import json

import numpy as np
from scipy.ndimage import median_filter
from scipy.optimize import least_squares
from scipy.signal import find_peaks
import soundfile as sf

from common import HERE, ROOT, SAMPLES, STRENGTHS, VELOCITIES

MODE_COUNT = 24


def read_reference(path):
    y, sr = sf.read(path, always_2d=True)
    if sr != 48000 or y.shape[1] != 2 or not np.isfinite(y).all():
        raise ValueError(f'Unexpected reference format: {path}')
    # These edited files begin at the attack. An energy threshold locates only
    # the leading silence; it must not align each file to its later loudest peak.
    energy = np.mean(y*y, axis=1)
    onset = int(np.flatnonzero(energy > energy.max()*1e-4)[0])
    return y[onset:], sr, onset


def spectrum(y, sr, start, end):
    x = y[int(start*sr):int(end*sr)]
    n = 2**19
    a = np.sqrt(np.mean(abs(np.fft.rfft(x*np.hanning(len(x))[:, None], n, axis=0))**2, axis=1))
    return np.fft.rfftfreq(n, 1/sr), a


def peak_frequency(f, a, k):
    left, mid, right = np.log(np.maximum(a[k-1:k+2], 1e-30))
    delta = np.clip(.5*(left-right)/(left-2*mid+right), -.5, .5)
    return float(f[k]+delta*(f[1]-f[0]))


def measure_bar(bar):
    records, signals, spectra = [], [], []
    for layer, strength in enumerate(STRENGTHS):
        paths = list(SAMPLES.glob(f'*saron-pelog-saronmallet-{bar}-{strength}.wav'))
        if len(paths) != 1:
            raise ValueError(f'Expected one bar {bar} {strength} recording, got {paths}')
        path = paths[0]
        y, sr, onset = read_reference(path)
        records.append({'file': str(path.relative_to(ROOT)), 'sha256': hashlib.sha256(path.read_bytes()).hexdigest(),
                        'bar': bar, 'strength': strength, 'velocity': VELOCITIES[layer],
                        'sample_rate': sr, 'onset_samples': onset, 'seconds': len(y)/sr,
                        'peak': float(abs(y).max()), 'rms_first_second': float(np.sqrt(np.mean(y[:sr]**2)))})
        signals.append(y)
        f, a = spectrum(y, sr, .015, .5)
        spectra.append(a)
    spectra = np.array(spectra)
    normalized = spectra / spectra.max(axis=1)[:, None]
    pooled = np.sqrt(np.mean(normalized**2, axis=0))
    fundamental_index = np.argmax(pooled * ((f > 400) & (f < 1300)))
    fundamental = peak_frequency(f, pooled, fundamental_index)
    local_floor = median_filter(pooled, size=301)
    candidates = find_peaks(pooled, distance=60, prominence=pooled.max()*.0006)[0]
    # Reject room rumble, neighboring bars and broad noise. A retained mode is
    # a resolved spectral line in at least three independent strike strengths.
    candidates = [k for k in candidates if f[k] >= fundamental*.98 and f[k] < 15000
                  and pooled[k] > 5*local_floor[k]
                  and np.count_nonzero(normalized[:, k] > .00035) >= 3]
    candidates = sorted(sorted(candidates, key=lambda k: pooled[k], reverse=True)[:MODE_COUNT])
    centers = np.array([peak_frequency(f, pooled, k) for k in candidates])
    assert abs(centers[0]-fundamental) < 2, (bar, centers)
    # Resolve neighboring modes jointly. Integrating a separate FFT band for
    # each line double-counts energy when two modes are closer than the window
    # bandwidth. Weighted sinusoidal least squares separates their amplitudes;
    # the fitted phases are discarded immediately, never stored in calibration.
    win = 2048
    phase = 2*np.pi*np.arange(win)[:, None]*centers[None, :]/sr
    design = np.concatenate([np.cos(phase), np.sin(phase), np.ones((win, 1))], axis=1)
    window = np.sqrt(np.hanning(win))
    weighted = design*window[:, None]
    condition = float(np.linalg.cond(weighted))
    if condition > 1000:
        raise ValueError(f'Bar {bar}: unresolved modal fit (condition {condition})')
    projection = np.linalg.pinv(weighted)*window[None, :]
    envelopes, times = [], []
    for y in signals:
        frames = np.lib.stride_tricks.sliding_window_view(y, win, axis=0)[::256]
        components = frames @ projection.T
        count = len(centers)
        env = np.sqrt(np.mean(components[:, :, :count]**2+components[:, :, count:2*count]**2, axis=1)).T
        envelopes.append(env)
        times.append((np.arange(len(frames))*256+win/2)/sr)
    # Fit one loss/radiation response with independent excitation levels to
    # all five recordings. Separately measured noise-floor thresholds prevent
    # the quiet strikes' room noise from becoming an immortal resonant tail.
    rates, rises, direct, amplitudes, errors = [], [], [], [], []
    for m, center in enumerate(centers):
        valid_ts, valid_ys, peaks = [], [], []
        for env, ts, y in zip(envelopes, times, signals):
            e = env[m]
            noise = max(float(np.median(e[-max(3, len(e)//10):])), 1e-7)
            valid = (ts < min(len(y)/sr*.8, 5)) & (e > max(noise*3, e.max()*.015))
            if np.count_nonzero(valid) < 4:
                valid = (ts < .4)
            valid_ts.append(ts[valid])
            valid_ys.append(e[valid])
            peaks.append(e.max())
        slopes = [np.clip(-np.polyfit(t, np.log(v+1e-15), 1)[0], .15, 80)
                  for t, v in zip(valid_ts, valid_ys)]
        def residual(p):
            rate = np.exp(p[0])
            rise = np.exp(p[1])
            return np.concatenate([(np.log(np.exp(p[3+l]-rate*t)*(1-p[2]*np.exp(-t/rise))+1e-15)-np.log(v+1e-15))
                                   / np.sqrt(len(t)*(1+t)) for l, (t, v) in enumerate(zip(valid_ts, valid_ys))])
        fit = least_squares(residual, [np.log(np.median(slopes)), np.log(.035), .5, *np.log(peaks)],
                            bounds=([np.log(.15), np.log(.001), 0, *np.log(np.maximum(np.array(peaks)*.01, 1e-12))],
                                    [np.log(150), np.log(.3), 1, *np.log(np.maximum(np.array(peaks)*20, 1e-10))]),
                            loss='soft_l1', f_scale=.15, max_nfev=300)
        rates.append(float(np.exp(fit.x[0])))
        rises.append(float(np.exp(fit.x[1])))
        direct.append(float(1-fit.x[2]))
        amplitudes.append(np.exp(fit.x[3:]).tolist())
        errors.append(float(np.sqrt(np.mean(fit.fun**2))*np.sqrt(sum(map(len, valid_ts))/5)*20/np.log(10)))
        for l, r in enumerate(records):
            r.setdefault('individual_rate_per_s', []).append(float(slopes[l]))
    amps = np.array(amplitudes).T
    for layer, r in enumerate(records):
        r['modal_amplitudes'] = amps[layer].tolist()
    print('bar', bar, 'f0', round(fundamental, 3), 'modes', len(centers),
          'ratios', np.round(centers[:9]/fundamental, 3).tolist(),
          'losses', np.round(rates[:9], 3).tolist(), 'fit dB', np.round(errors[:9], 2).tolist(), flush=True)
    return {'bar': bar, 'fundamental_hz': fundamental, 'frequencies_hz': centers.tolist(),
            'modal_separation_condition_number': condition,
            'rates_per_s': rates, 'rise_seconds': rises, 'direct_fraction': direct,
            'envelope_rmse_db': errors, 'recordings': records}


def main():
    result = {'source': 'Latent Sonorities / memeshift / Bilawa Ade Respati, Rabih Beaini',
              'source_url': 'https://freesound.org/people/memeshift/packs/40343/',
              'source_license': 'https://creativecommons.org/licenses/by-nc/4.0/',
              'velocity_mapping': dict(zip(STRENGTHS, VELOCITIES)),
              'velocity_note': 'Ordinal strike labels, not measured MIDI or mallet velocity.',
              'model': 'One passive modal bar system; seven register calibrations, shared poles across five strikes.',
              'bars': [measure_bar(bar) for bar in range(1, 8)]}
    (HERE/'reference-analysis.json').write_text(json.dumps(result, indent=2)+'\n')


if __name__ == '__main__':
    main()
