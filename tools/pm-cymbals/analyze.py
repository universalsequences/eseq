#!/usr/bin/env python3
"""Measure resonant lines, spectral shape and material loss from each example.

Numbered files have no velocity or object-identity metadata. They describe
voicing examples, not invented velocity layers or matched open/closed pairs.
Only physical coefficients leave this tool; analysis phases are discarded.
"""
import argparse
import json

import numpy as np
from scipy.ndimage import median_filter
from scipy.optimize import least_squares, nnls
from scipy.signal import find_peaks

from common import HERE, ROOT, SAMPLES, digest, read_reference

SR = 48000
BANDS = np.geomspace(100, 18500, 18)
WINDOWS = ((0, .02), (.02, .06), (.06, .15), (.15, .35), (.35, .7),
           (.7, 1.2), (1.2, 2), (2, 3.5), (3.5, 5.5))
MODES = 8


def spectrum(y, start, end, n=131072):
    x = y[int(start*SR):int(end*SR)]
    if len(x) < 16:
        return np.fft.rfftfreq(n, 1/SR), np.zeros(n//2+1)
    w = np.hanning(len(x))
    power = np.mean(abs(np.fft.rfft(x*w[:, None], n, axis=0))**2, axis=1)
    power *= 2/(n*np.sum(w*w))
    return np.fft.rfftfreq(n, 1/SR), power


def band_power(f, power):
    edges = np.r_[0, np.sqrt(BANDS[:-1]*BANDS[1:]), SR/2]
    return np.array([power[(f >= lo) & (f < hi)].sum() for lo, hi in zip(edges, edges[1:])])


def measure(path):
    y, onset, original_sr = read_reference(path)
    duration = len(y)/SR
    windows, levels = [], []
    for start, end in WINDOWS:
        end = min(end, duration)
        if end-start < .015:
            continue
        f, power = spectrum(y, start, end)
        windows.append([start, end])
        levels.append(band_power(f, power))
    levels = np.array(levels)
    times = np.mean(windows, axis=1)
    rates, band_amplitudes = [], []
    for b in range(len(BANDS)):
        active = (levels[:, b] > max(levels[:, b].max()*1e-4, 1e-12)) & (times > .04)
        t, e = times[active], levels[active, b]
        if len(t) < 2:
            rate = 20.
            amplitude = float(np.sqrt(levels[:, b].max()))
        else:
            fit = least_squares(lambda v: v[0]-2*np.exp(v[1])*t-np.log(e),
                                [np.log(e.max()), np.log(3.)], loss='soft_l1')
            rate = float(np.clip(np.exp(fit.x[1]), .1, 120))
            amplitude = float(np.exp(fit.x[0]/2))
        rates.append(rate)
        band_amplitudes.append(amplitude)
    # A passive reflection shelf approximates this monotone material-loss law.
    # Keep the per-band observations as evidence when the two-term law cannot
    # account for recording noise, damping gestures or nonlinear transients.
    b = BANDS**2/(BANDS**2+2800**2)
    weight = np.sqrt(np.maximum(np.max(levels, axis=0), 1e-12))
    weight /= weight.max()
    fit, _ = nnls(np.c_[np.ones(len(b)), b]*weight[:, None], np.array(rates)*weight)
    material = [float(np.clip(fit[0], .1, 60)), float(np.clip(fit[1], 0, 150))]

    f, power = spectrum(y, .04, min(duration, 1.1))
    floor = median_filter(power, size=151)
    candidates = find_peaks(power, distance=40)[0]
    candidates = [k for k in candidates if 90 < f[k] < 16000
                  and power[k] > 9*max(floor[k], 1e-15)]
    candidates.sort(key=lambda k: power[k], reverse=True)
    centers = []
    for k in candidates:
        if all(abs(f[k]-c) > max(12, c*.01) for c in centers):
            logp = np.log(np.maximum(power[k-1:k+2], 1e-30))
            offset = .5*(logp[0]-logp[2])/(logp[0]-2*logp[1]+logp[2])
            centers.append(float(f[k]+np.clip(offset, -.5, .5)*(f[1]-f[0])))
        if len(centers) == MODES:
            break
    centers.sort()
    mode_amplitude, mode_rate = [], []
    if centers:
        # Joint projection avoids counting overlapping nearby sinusoidal fits
        # more than once. Every channel contributes power; phases are discarded.
        hop, win = 2048, 4096
        t = np.arange(win)/SR
        design = np.column_stack([fn(2*np.pi*hz*t) for hz in centers for fn in (np.cos, np.sin)])
        weight_window = np.hanning(win)
        projection = np.linalg.pinv(design*weight_window[:, None], rcond=.001)*weight_window
        observed, moments = [], []
        for start in range(0, min(len(y)-win, int(2.5*SR)), hop):
            coef = projection@y[start:start+win]
            amplitude = np.sqrt(np.mean(coef.reshape(len(centers), 2, -1)**2, axis=2).sum(axis=1))
            observed.append(amplitude)
            moments.append((start+win/2)/SR)
        observed, moments = np.array(observed), np.array(moments)
        for i, hz in enumerate(centers):
            a = observed[:, i]
            active = (a > max(a.max()*.015, 1e-5)) & (moments > .04)
            if np.count_nonzero(active) < 2:
                mode_rate.append(20.)
                mode_amplitude.append(float(a.max()))
                continue
            fit = least_squares(lambda v: v[0]-np.exp(v[1])*moments[active]-np.log(a[active]),
                                [np.log(a.max()), np.log(3.)], loss='soft_l1')
            mode_rate.append(float(np.clip(np.exp(fit.x[1]), .1, 120)))
            mode_amplitude.append(float(np.exp(fit.x[0])))
    else:
        centers, mode_rate, mode_amplitude = [300.], [10.], [0.]
    f, total = spectrum(y, 0, min(duration, 1))
    centroid = float(np.sum(f*total)/max(total.sum(), 1e-30))
    return {'file': str(path.relative_to(ROOT)), 'sha256': digest(path),
            'source_sample_rate': original_sr, 'source_onset_samples': onset,
            'duration_seconds': duration, 'peak': float(abs(y).max()),
            'clipped_samples': int(np.count_nonzero(abs(y) >= .9999)),
            'centroid_hz': centroid, 'windows_seconds': windows,
            'band_power': levels.tolist(), 'band_amplitudes': band_amplitudes,
            'band_rates_per_s': rates, 'material_rates_per_s': material,
            'mode_frequencies_hz': centers, 'mode_rates_per_s': mode_rate,
            'mode_amplitudes': mode_amplitude}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('families', nargs='*')
    args = parser.parse_args()
    all_files = sorted(SAMPLES.glob('*.wav'))
    groups = {'crash': [p for p in all_files if 'Crash' in p.name],
              'ride': [p for p in all_files if 'Ride' in p.name],
              'hihat': [p for p in all_files if p.name.startswith('Hihat')]}
    for slug in args.families or groups:
        records = []
        for path in groups[slug]:
            record = measure(path)
            record['articulation'] = ('open' if 'Open' in path.name else
                'pedal' if 'Pedal' in path.name else 'muted' if 'Muted' in path.name else
                'closed' if 'Close' in path.name else 'strike')
            records.append(record)
            print(path.name, 'modes', len(record['mode_frequencies_hz']),
                  'material loss', np.round(record['material_rates_per_s'], 2), flush=True)
        records.sort(key=lambda r: r['centroid_hz'])
        report = {'family': slug, 'sample_rate': SR, 'band_centers_hz': BANDS.tolist(),
                  'method': 'Positive material losses, joint modal projection, phase discarded. '
                            'Numbered examples are voicings; no velocity or paired-hat identity is asserted.',
                  'references': records}
        (HERE/f'{slug}-analysis.json').write_text(json.dumps(report, indent=2)+'\n')


if __name__ == '__main__':
    main()
