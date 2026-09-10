#!/usr/bin/env python3
"""Identify a shared passive modal response from every strike of each object.

An acoustic modal reduction identifies radiating poles, not geometry or mode
shapes. The estimation phases and frame envelopes never leave this program.
"""
import argparse
import hashlib
import json

import numpy as np
import soundfile as sf
from scipy.ndimage import median_filter
from scipy.optimize import least_squares
from scipy.signal import find_peaks

from families import FAMILIES, HERE, ROOT, references


def read_reference(path):
    y, sr = sf.read(path, always_2d=True)
    if sr != 48000 or y.shape[1] != 2 or not np.isfinite(y).all():
        raise ValueError(f'Unexpected reference format: {path}')
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
    return float(f[k] + delta*(f[1]-f[0]))


def measure(family, unit):
    label, key, approximate = unit
    records, signals, spectra, attack_spectra = [], [], [], []
    for layer, path in enumerate(references(family, label)):
        y, sr, onset = read_reference(path)
        records.append({'file': str(path.relative_to(ROOT)), 'sha256': hashlib.sha256(path.read_bytes()).hexdigest(),
                        'strength': family.strengths[layer], 'velocity': family.velocities[layer],
                        'sample_rate': sr, 'onset_samples': onset, 'seconds': len(y)/sr,
                        'peak': float(abs(y).max())})
        signals.append(y)
        f, a = spectrum(y, sr, .015, family.spectrum_seconds)
        spectra.append(a)
        if family.resolve_attack:
            _, early = spectrum(y, sr, .002, .06)
            attack_spectra.append(early)
    spectra = np.array(spectra)
    normalized = spectra / spectra.max(axis=1)[:, None]
    pooled = np.sqrt(np.mean(normalized**2, axis=0))
    pitch_index = np.argmax(pooled * ((f > approximate*.97) & (f < approximate*1.03)))
    fundamental = peak_frequency(f, pooled, pitch_index)
    local_floor = median_filter(pooled, size=301)
    candidates = find_peaks(pooled, distance=22, prominence=pooled.max()*.0006)[0]
    candidates = [k for k in candidates if f[k] >= fundamental*.85 and f[k] < 14000
                  and pooled[k] > 5*local_floor[k]
                  and np.count_nonzero(normalized[:, k] > .0005) >= 2]
    candidates = sorted(candidates, key=lambda k: pooled[k], reverse=True)
    added_attack_peaks = set()
    if family.resolve_attack:
        early = np.array(attack_spectra)
        early /= early.max(axis=1)[:, None]
        attack = np.sqrt(np.mean(early**2, axis=0))
        # Short-lived upper modes can be loud at impact yet vanish from the
        # sustained spectrum. Merge their resolved peaks before modal reduction.
        peaks = find_peaks(attack, distance=200, prominence=.003)[0]
        peaks = [k for k in peaks if 1000 < f[k] < 14000
                 and np.count_nonzero(early[:, k] > .002) >= 2]
        for k in peaks:
            if all(abs(f[k]-f[old]) > 40 for old in candidates):
                candidates.append(k)
                added_attack_peaks.add(k)
        candidates.sort(key=lambda k: pooled[k]+.25*attack[k], reverse=True)
    candidates = [pitch_index] + [k for k in candidates if abs(f[k]-fundamental) > 2]
    candidates = sorted(candidates[:family.modes])
    centers = np.array([peak_frequency(f, attack if k in added_attack_peaks else pooled, k) for k in candidates])
    pitch_mode = int(np.argmin(abs(centers-fundamental)))
    # Joint projection separates close modes. The window expands only when
    # required for conditioning; coefficients are not estimated from overlapping
    # independent FFT bands, which would count the same energy more than once.
    win = 4096 if fundamental < 300 else 2048
    while True:
        phase = 2*np.pi*np.arange(win)[:, None]*centers[None, :]/sr
        design = np.concatenate([np.cos(phase), np.sin(phase), np.ones((win, 1))], axis=1)
        window = np.sqrt(np.hanning(win))
        weighted = design*window[:, None]
        condition = float(np.linalg.cond(weighted))
        if condition < 30:
            break
        win *= 2
        if win > 16384:
            raise ValueError(f'{label}: unresolved modes, condition {condition}')
    projection = np.linalg.pinv(weighted)*window[None, :]
    envelopes, times = [], []
    hop = 512
    for y in signals:
        # The tail estimates the noise floor even when the fit uses only the
        # first few seconds. Do not learn recording-end fades as modal losses.
        frames = np.lib.stride_tricks.sliding_window_view(y, win, axis=0)[::hop]
        components = frames @ projection.T
        count = len(centers)
        env = np.sqrt(np.mean(components[:, :, :count]**2 + components[:, :, count:2*count]**2, axis=1)).T
        envelopes.append(env)
        times.append((np.arange(len(frames))*hop+win/2)/sr)
    attack_envelopes = None
    if family.resolve_attack:
        # A regularized short-window projection preserves resolved high modes
        # while rejecting nearly collinear low-mode combinations. Use it only
        # for isolated upper modes; their decay must not be inferred from a
        # window longer than the transient itself.
        short_win = 1024
        phase = 2*np.pi*np.arange(short_win)[:, None]*centers[None, :]/sr
        design = np.concatenate([np.cos(phase), np.sin(phase), np.ones((short_win, 1))], axis=1)
        window = np.sqrt(np.hanning(short_win))
        projection = np.linalg.pinv(design*window[:, None], rcond=.03)*window[None, :]
        attack_envelopes = []
        for y in signals:
            frames = np.lib.stride_tricks.sliding_window_view(y[:sr], short_win, axis=0)[::128]
            components = frames @ projection.T
            env = np.sqrt(np.mean(components[:, :, :count]**2+components[:, :, count:2*count]**2, axis=1)).T
            attack_envelopes.append((env, (np.arange(len(frames))*128+short_win/2)/sr))
    rates, rises, direct, amplitudes, errors = [], [], [], [], []
    for mode in range(len(centers)):
        valid_ts, valid_ys, peaks, slopes = [], [], [], []
        isolated_upper = centers[mode] > 1000 and all(abs(centers[mode]-c) > 80 for i, c in enumerate(centers) if i != mode)
        for layer, (env, ts, y) in enumerate(zip(envelopes, times, signals)):
            e = env[mode]
            noise = max(float(np.median(e[-max(3, len(e)//10):])), 1e-7)
            valid = (ts < min(len(y)/sr*.8, family.fit_seconds)) & (e > max(noise*3, e.max()*.015))
            if np.count_nonzero(valid) < 4:
                valid = ts < .4
            t, v = ts[valid], e[valid]
            if attack_envelopes is not None and isolated_upper:
                early_env, early_ts = attack_envelopes[layer]
                early_valid = (early_ts < win/sr) & (early_env[mode] > max(noise*3, e.max()*.015))
                later = t >= win/sr
                t = np.concatenate([early_ts[early_valid], t[later]])
                v = np.concatenate([early_env[mode, early_valid], v[later]])
            valid_ts.append(t)
            valid_ys.append(v)
            peaks.append(float(e.max()))
            slopes.append(float(np.clip(-np.polyfit(t, np.log(v+1e-15), 1)[0], .03, 80)))

        def residual(p):
            rate, rise = np.exp(p[:2])
            return np.concatenate([(np.log(np.exp(p[3+l]-rate*t)*(1-p[2]*np.exp(-t/rise))+1e-15)
                                    - np.log(v+1e-15))/np.sqrt(len(t)*(1+t))
                                   for l, (t, v) in enumerate(zip(valid_ts, valid_ys))])

        fit = least_squares(residual, [np.log(np.median(slopes)), np.log(.035), .5, *np.log(peaks)],
            bounds=([np.log(.03), np.log(.001), 0, *np.log(np.maximum(np.array(peaks)*.01, 1e-12))],
                    [np.log(150), np.log(.8), 1, *np.log(np.maximum(np.array(peaks)*20, 1e-10))]),
            loss='soft_l1', f_scale=.15, max_nfev=300)
        rates.append(float(np.exp(fit.x[0])))
        rises.append(float(np.exp(fit.x[1])))
        direct.append(float(1-fit.x[2]))
        amplitudes.append(np.exp(fit.x[3:]).tolist())
        errors.append(float(np.sqrt(np.mean(fit.fun**2))*np.sqrt(sum(map(len, valid_ts))/len(records))*20/np.log(10)))
        for record, slope in zip(records, slopes):
            record.setdefault('individual_rate_per_s', []).append(slope)
    for record, amplitudes_layer in zip(records, np.array(amplitudes).T):
        record['modal_amplitudes'] = amplitudes_layer.tolist()
    print(family.name, label or 'single', round(fundamental, 3), 'Hz', len(centers), 'modes',
          'median envelope error', round(float(np.median(errors)), 2), 'dB', flush=True)
    return {'label': label, 'midi': key, 'fundamental_hz': fundamental, 'pitch_mode': pitch_mode,
            'frequencies_hz': centers.tolist(), 'rates_per_s': rates, 'rise_seconds': rises,
            'direct_fraction': direct, 'envelope_rmse_db': errors,
            'projection_window_samples': win, 'projection_condition_number': condition,
            **({'attack_projection': {'window_samples': 1024, 'svd_relative_cutoff': .03,
                                     'minimum_frequency_hz': 1000, 'minimum_mode_spacing_hz': 80}}
               if family.resolve_attack else {}),
            'recordings': records}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('families', nargs='*', choices=list(FAMILIES))
    args = parser.parse_args()
    for slug in args.families or FAMILIES:
        family = FAMILIES[slug]
        data = {'name': family.name, 'group': family.prefix,
                'source_url': 'https://freesound.org/people/memeshift/packs/40343/',
                'source_license': 'https://creativecommons.org/licenses/by-nc/4.0/',
                'velocity_mapping': dict(zip(family.strengths, family.velocities)),
                'units': [measure(family, unit) for unit in family.units]}
        (HERE/f'{slug}-analysis.json').write_text(json.dumps(data, indent=2)+'\n')


if __name__ == '__main__':
    main()
