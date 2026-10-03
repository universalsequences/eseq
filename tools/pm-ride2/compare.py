#!/usr/bin/env python3
"""Compare PM Ride release 2 with its six reference rides.

Each Character is struck once (key C4, velocity 1, Width 0), rendered at
48 kHz and resampled to the recordings' 44.1 kHz, divided by the build's
level gain, and scored against its own high-passed recording: level and band
level per window, and the tail's fine spectrum (flatness, lines per kHz, bins
holding 90% of the power: the measure in tools/pm-cymbals/comparison.json
where release 1 had about half the reference's spread).
Writes comparison.json and local listening material (never committed):
  .local/pm-ride2/ab.wav      each cymbal: recording, then model
  .local/pm-ride2/groove.wav  a ride pattern on every Character (Width 0.3)
"""
import argparse
import json
from pathlib import Path

import numpy as np
import soundfile as sf
from scipy.optimize import nnls
from scipy.signal import butter, find_peaks, medfilt, resample_poly, sosfiltfilt

import common as C
from analyze import HIGHPASS_HZ, K, M
from build import FIRST_KEY, WASH_Q, coefficients, level_gains, references

HERE = Path(__file__).resolve().parent
OUT = C.ROOT/'.local/pm-ride2'
RENDER_RATE = 48000
WINDOWS = [(0, .005), (.005, .015), (.015, .03), (.03, .06), (.06, .15), (.15, .3), (.3, .6), (.6, 1.0)]
BANDS = [175, 350, 700, 1400, 2800, 5600, 11200, 20000]
QUIET = {'output.width': 0.0}


def play(inst, character, seconds, params=None, sr=44100, velocity=1.0):
    y, _ = C.play(inst, [(0.0, FIRST_KEY, velocity)], seconds, QUIET | {'cymbal.character': float(character)} | (params or {}))
    g = np.gcd(sr, RENDER_RATE)
    return resample_poly(y[:, 0].astype(float), sr//g, RENDER_RATE//g)


def band_level(x, sr, lo, hi, a, b):
    seg = sosfiltfilt(butter(4, [lo, min(hi, .48*sr)], 'bandpass', fs=sr, output='sos'), x)[int(a*sr):int(b*sr)]
    return 10*np.log10(np.mean(seg**2) + 1e-14)


def texture(x, sr, start, lo, hi, n=8192):
    seg = x[int(start*sr):int(start*sr) + n]
    power = np.abs(np.fft.rfft(seg*np.hanning(len(seg))))**2
    fr = np.fft.rfftfreq(len(seg), 1/sr)
    p = power[(fr >= lo) & (fr < hi)]
    db = 10*np.log10(p + 1e-20)
    lines, _ = find_peaks(db - medfilt(db, 101), height=10)
    return {'flatness_db': round(float(10*np.log10(np.exp(np.mean(np.log(p + 1e-20)))/np.mean(p))), 2),
            'lines_per_khz': round(len(lines)/((hi - lo)/1000), 1)}


def concentration(x, sr):
    """pm-cymbals' fine-spectrum measure over the first second."""
    power = np.sort(np.abs(np.fft.rfft(x[:sr]))**2)[::-1]
    return {'top_10_bin_power_fraction': round(float(power[:10].sum()/power.sum()), 4),
            'bins_for_90_percent_power': int(np.searchsorted(np.cumsum(power)/power.sum(), .9) + 1)}


def reference(ref):
    x, sr = sf.read(C.ROOT/ref['file'])
    y = M.highpass(x, sr, HIGHPASS_HZ)
    onset = int(round(ref['onsets_s'][0]*sr))
    end = int(round(ref['onsets_s'][1]*sr)) if len(ref['onsets_s']) > 1 else len(y)
    return y[onset:end], sr


def calibrate(inst, data):
    """Click per cymbal and wash per cymbal per band, as in tools/pm-ride."""
    path = HERE/'noise-calibration.json'
    old = json.loads(path.read_text()) if path.exists() else {}
    gains = level_gains(data)
    edges = np.array([w['band_hz'][0] for w in references(data)[0][1]['wash']] + [references(data)[0][1]['wash'][-1]['band_hz'][1]])
    count = len(edges) - 1
    rate = RENDER_RATE
    f = np.linspace(1, .5*rate, 200000)
    leak = np.zeros((count, count))
    for j, f0 in enumerate(np.sqrt(edges[:-1]*edges[1:])):
        x = np.tan(np.pi*f/rate)/np.tan(np.pi*np.minimum(f0, .45*rate)/rate)
        h = 1/(1 + WASH_Q**2*(x - 1/x)**2)**2
        for i, (lo, hi) in enumerate(zip(edges, edges[1:])):
            leak[i, j] = h[(f >= lo) & (f < hi)].sum()/h.sum()
    click, wash, norms = {}, {}, {}
    for row, (slug, ref) in enumerate(references(data)):
        g = gains[row]
        params = {'cymbal.character': float(row), 'output.width': 0.0}
        on, _ = C.play(inst, [(0.0, FIRST_KEY, 1.0)], .1, params)
        off, _ = C.play(inst, [(0.0, FIRST_KEY, 1.0)], .1, params | {'stick.click': 0.0})
        r44 = lambda y: resample_poly(y.astype(float), 147, 160)
        pad = np.concatenate([np.zeros(1323), r44((on - off)[:, 0]), np.zeros(4410)])
        measured = K.contact_noise(pad, 1323, 44100)['click_rms']/g
        click[slug] = float(old.get('click', {}).get(slug, 1.0)*ref['contact']['click_rms']/max(measured, 1e-12))
        quiet = params | {'stick.click': 0.0}
        seconds = 3.0
        w = (C.play(inst, [(0.0, FIRST_KEY, 1.0)], seconds, quiet)[0][:, 0]
             - C.play(inst, [(0.0, FIRST_KEY, 1.0)], seconds, quiet | {'ring.wash': 0.0})[0][:, 0]).astype(float)
        spectrum = np.abs(np.fft.rfft(w))**2*2/len(w)/rate
        bins = np.fft.rfftfreq(len(w), 1/rate)
        measured = np.array([spectrum[(bins >= lo) & (bins < hi)].sum() for lo, hi in zip(edges, edges[1:])])
        target = np.array([(b['fast_power']/(2*b['fast_rate_per_s'])*(1 - np.exp(-2*b['fast_rate_per_s']*seconds))
                            + b['slow_power']/(2*b['slow_rate_per_s'])*(1 - np.exp(-2*b['slow_rate_per_s']*seconds)))*g**2
                           for b in ref['wash']])
        # Leakage is a property of the bank, so it is undone analytically: the
        # band energies to emit are a regularized NNLS deconvolution of the
        # target through the bank's response, pulled toward no correction
        # (one strike is too little data for a free deconvolution). The
        # render then corrects only the bank's normalization, by a bounded
        # factor, in bands that carry energy.
        mu = 0.05
        wanted = nnls(np.vstack([leak, np.sqrt(mu)*np.eye(count)]), np.concatenate([target, np.sqrt(mu)*target]))[0]
        shape = np.where(target > 0, wanted/np.maximum(target, 1e-30), 0.0)
        norm = np.array(old.get('wash_norm', {}).get(slug, [1.0]*count))
        predicted = leak@(wanted)
        significant = predicted > 1e-3*predicted.max()
        norm = np.where(significant, norm*np.clip(predicted/np.maximum(measured, 1e-30), 0.5, 2.0), norm)
        norms[slug] = list(norm.astype(float))
        wash[slug] = list((shape*norm).astype(float))
    path.write_text(json.dumps({'click': click, 'wash': wash, 'wash_norm': norms}, indent=1) + '\n')
    print('Wrote noise-calibration.json; rerun build.py')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--calibrate-noise', action='store_true')
    args = parser.parse_args()
    data = json.loads((HERE/'analysis.json').read_text())
    inst = C.instrument(RENDER_RATE)
    if args.calibrate_noise:
        calibrate(inst, data)
        return
    OUT.mkdir(parents=True, exist_ok=True)
    gains = level_gains(data)
    _, report = coefficients(data)
    results, ab = [], []
    for row, (slug, ref) in enumerate(references(data)):
        rec, sr = reference(ref)
        model = play(inst, row, len(rec)/sr + .05)[:len(rec)]/gains[row]
        windows = []
        for a, b in WINDOWS:
            if b*sr > len(rec):
                break
            rb = np.array([band_level(rec, sr, lo, hi, a, b) for lo, hi in zip(BANDS, BANDS[1:])])
            mb = np.array([band_level(model, sr, lo, hi, a, b) for lo, hi in zip(BANDS, BANDS[1:])])
            audible = rb > rb.max() - 30
            level = 10*np.log10(np.mean(model[int(a*sr):int(b*sr)]**2)/np.mean(rec[int(a*sr):int(b*sr)]**2))
            windows.append({'ms': [int(a*1000), int(b*1000)], 'level_error_db': round(float(level), 2),
                            'band_error_db': [round(float(v), 2) for v in mb - rb],
                            'median_audible_band_error_db': round(float(np.median(np.abs(mb - rb)[audible])), 2)})
        tex = {f'{int(s*1000)}ms {lo}-{hi}': {'recording': texture(rec, sr, s, lo, hi), 'model': texture(model, sr, s, lo, hi)}
               for s in (.05, .3) for lo, hi in ((1400, 5600), (5600, 11200), (11200, 18000))}
        conc = {'recording': concentration(rec, sr), 'model': concentration(model, sr)}
        results.append({'character': row, 'slug': slug, 'name': ref['name'], 'reference': ref['file'],
                        'windows': windows, 'texture': tex, 'concentration': conc})
        print(f"{row} {ref['name']:8s} level dB " + ' '.join(f"{w['level_error_db']:+5.1f}" for w in windows)
              + ' | band med ' + ' '.join(f"{w['median_audible_band_error_db']:4.1f}" for w in windows)
              + f" | 90% bins rec {conc['recording']['bins_for_90_percent_power']} model {conc['model']['bins_for_90_percent_power']}")
        for k, v in tex.items():
            print(f"     {k:18s} flatness/lines rec {v['recording']['flatness_db']:6.1f}/{v['recording']['lines_per_khz']:4.1f}"
                  f"  model {v['model']['flatness_db']:6.1f}/{v['model']['lines_per_khz']:4.1f}")
        gap = np.zeros(int(.4*sr))
        ab += [rec*gains[row], gap, model*gains[row], gap, gap]
    summary = {f'{int(a*1000)}-{int(b*1000)}ms': round(float(np.mean([abs(r['windows'][i]['level_error_db'])
                                                                       for r in results if len(r['windows']) > i])), 2)
               for i, (a, b) in enumerate(WINDOWS)}
    print('mean |level error| dB', summary)
    sf.write(OUT/'ab.wav', np.concatenate(ab), 44100)
    beat = 60/100
    groove = []
    for row in range(len(references(data))):
        # Straight eighths, accented on the beat, all on one ringing plate.
        hits = [(k*beat/2, FIRST_KEY, .95 if k % 2 == 0 else .55) for k in range(16)]
        y, _ = C.play(inst, hits, 8*beat + 1.5, {'cymbal.character': float(row), 'output.width': .3})
        groove.append(y)
    sf.write(OUT/'groove.wav', np.concatenate(groove), RENDER_RATE)
    (HERE/'comparison.json').write_text(json.dumps({
        'compiler_sha256': inst.compiler_sha256, 'render_rate': RENDER_RATE,
        'method': 'one strike per Character at C4, velocity 1, Width 0; rendered at 48 kHz, resampled to 44.1 kHz, divided by the level gain, scored against the high-passed recording',
        'summary_mean_abs_level_error_db': summary, 'cymbals': results}, indent=1) + '\n')


if __name__ == '__main__':
    main()
