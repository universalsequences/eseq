#!/usr/bin/env python3
"""Compare the compiled PM Ride Kit DSP with the reference ride.

The whole sample is re-played on one voice (the default one-cymbal voicing):
every reference stroke is a retrigger at its recorded onset, played by its own
key or by its substitute at a velocity matched to its ring energy. Each stroke
window is scored against the high-passed left channel by level and band
level. Also writes local listening material (never committed):
  .local/pm-ride/sample-ab.wav   reference ride (left channel), then the model's replay
  .local/pm-ride/model.wav       the model's replay alone (stereo, Width 0 = mono)
  .local/pm-ride/strokes.wav     every key played alone, in key order
  .local/pm-ride/groove.wav      a short groove on the twelve keys, Width 0.5
"""
import argparse
import json
from pathlib import Path

import numpy as np
import soundfile as sf
from scipy.optimize import nnls
from scipy.signal import butter, resample_poly, sosfiltfilt

import common as C
import modal_fit as M
from analyze import CHANNEL, HIGHPASS_HZ, SAMPLE, contact_noise
from build import FIRST_KEY, SUBSTITUTES, stroke_row, strokes

HERE = Path(__file__).resolve().parent
OUT = C.ROOT/'.local/pm-ride'
WINDOWS = [(0, .005), (.005, .015), (.015, .03), (.03, .06), (.06, .15), (.15, .3)]
BANDS = [175, 350, 700, 1400, 2800, 5600, 11200, 16000]
RENDER_RATE = 48000          # rendered as the app runs it, then resampled to the reference's 32.5 kHz


def play(inst, events, seconds, params=None, sr=32500):
    y, _ = C.play(inst, events, seconds, params)
    g = np.gcd(sr, RENDER_RATE)
    return resample_poly(y.astype(float), sr//g, RENDER_RATE//g, axis=0)


def replay_velocity(data, hit):
    """1 for installed strokes; a substitute is scaled to the stroke's ring energy (the engine is linear in velocity)."""
    key = data['hits'][SUBSTITUTES.get(hit['index'], hit['index'])]
    return float(np.clip(np.sqrt(hit['ring_energy']/key['ring_energy']), 0.05, 1.0))


def replay_events(data):
    return [(hit['onset_s'], FIRST_KEY + stroke_row(data, hit['index']), replay_velocity(data, hit)) for hit in data['hits']]


def band_levels(signal, sr, onset, a, b):
    out = []
    for lo, hi in zip(BANDS, BANDS[1:]):
        seg = sosfiltfilt(butter(4, [lo, min(hi, .48*sr)], 'bandpass', fs=sr, output='sos'),
                          signal[max(0, onset - int(.02*sr)):onset + int(.35*sr)])
        seg = seg[onset - max(0, onset - int(.02*sr)):]
        out.append(10*np.log10(np.mean(seg[int(a*sr):int(b*sr)]**2) + 1e-14))
    return np.array(out)


def metrics(ref, model, sr, onset, end):
    rows = []
    for a, b in WINDOWS:
        if onset + int(b*sr) > end:
            break
        r = 10*np.log10(np.mean(ref[onset + int(a*sr):onset + int(b*sr)]**2) + 1e-14)
        m = 10*np.log10(np.mean(model[onset + int(a*sr):onset + int(b*sr)]**2) + 1e-14)
        rb, mb = band_levels(ref, sr, onset, a, b), band_levels(model, sr, onset, a, b)
        audible = rb > rb.max() - 30
        rows.append({'ms': [int(a*1000), int(b*1000)], 'level_error_db': round(float(m - r), 2),
                     'band_error_db': [round(float(v), 2) for v in mb - rb],
                     'median_audible_band_error_db': round(float(np.median(np.abs(mb - rb)[audible])), 2)})
    return rows


def calibrate(inst, data, sr):
    """Scale the click (per key) and the wash (per band) to the record.

    The rendered click (click on minus off) must match each stroke's measured
    contact click. The rendered wash of the whole replay (wash on minus off;
    the noise is deterministic per render) must carry, per third-octave band,
    the power of the identified modes it stands in for.
    """
    from analyze import THIRD_OCTAVES
    from build import WASH_Q, coefficients
    path = HERE/'noise-calibration.json'
    old = json.loads(path.read_text()) if path.exists() else {}
    click = {}
    for row, hit in enumerate(strokes(data)):
        on = play(inst, [(0.0, FIRST_KEY + row, 1.0)], .1)
        off = play(inst, [(0.0, FIRST_KEY + row, 1.0)], .1, {'stick.click': 0.0})
        pad = np.concatenate([np.zeros(int(.03*sr)), (on - off)[:, 0], np.zeros(int(.1*sr))])
        measured = contact_noise(pad, int(.03*sr), sr)['click_rms']
        click[hit['slug']] = float(old.get('click', {}).get(hit['slug'], 1.0)*hit['contact']['click_rms']/max(measured, 1e-12))
    rate = RENDER_RATE
    events = replay_events(data)
    seconds = events[-1][0] + 1.0
    quiet = {'stick.click': 0.0}
    wash = C.play(inst, events, seconds, quiet)[0][:, 0] - C.play(inst, events, seconds, quiet | {'cymbal.wash': 0.0})[0][:, 0]
    # Target: the energy the engine's wash tables ask for over the same
    # replay; calibration corrects only the bank's normalization and leakage.
    tables, _ = coefficients(data)
    edges = THIRD_OCTAVES
    count = len(edges) - 1
    target = np.zeros(count)
    for onset, key, velocity in events:
        row = strokes(data)[key - FIRST_KEY]['index']
        target += np.array([w['fast_power'][row]/(2*w['fast_rate_per_s']) + w['slow_power'][row]/(2*w['slow_rate_per_s'])
                            for w in data['wash']])*velocity**2
    # Rectangular band energies (Parseval), as the leakage matrix assumes; a
    # band filter's own skirts would mix strong bands into the weak top ones.
    spectrum = np.abs(np.fft.rfft(wash))**2*2/len(wash)/rate
    bins = np.fft.rfftfreq(len(wash), 1/rate)
    measured = np.array([spectrum[(bins >= lo) & (bins < hi)].sum() for lo, hi in zip(edges, edges[1:])])
    # Every wash band leaks into its neighbours through its skirts, so the
    # band gains are solved together: measured = L @ emitted, with L the
    # bank's digital response power in each measurement band.
    f = np.linspace(1, .5*rate, 200000)
    centre = np.sqrt(edges[:-1]*edges[1:])
    leak = np.zeros((count, count))
    for j, f0 in enumerate(centre):
        x = np.tan(np.pi*f/rate)/np.tan(np.pi*f0/rate)
        h = 1/(1 + WASH_Q**2*(x - 1/x)**2)**2        # two unit-peak stages, power
        for i, (lo, hi) in enumerate(zip(edges, edges[1:])):
            leak[i, j] = h[(f >= lo) & (f < hi)].sum()/h.sum()
    old_factors = np.array(old.get('wash', [1.0]*count))
    emitted = nnls(leak, measured)[0]                 # emitted energy per band at the current factors
    wanted = nnls(leak, target)[0]
    factors = list(np.where(emitted > 1e-20, old_factors*wanted/np.maximum(emitted, 1e-20), old_factors).astype(float))
    path.write_text(json.dumps({'click': click, 'wash': factors}, indent=1) + '\n')
    print('Wrote noise-calibration.json; rerun build.py')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--calibrate-noise', action='store_true', help='write noise-calibration.json from the rendered click')
    args = parser.parse_args()
    data = json.loads((HERE/'analysis.json').read_text())
    stroke_row(data, 0)   # resolves SUBSTITUTES
    raw, sr = sf.read(SAMPLE)
    ref = M.highpass(raw[:, CHANNEL], sr, HIGHPASS_HZ)
    inst = C.instrument(RENDER_RATE)
    OUT.mkdir(parents=True, exist_ok=True)
    if args.calibrate_noise:
        calibrate(inst, data, sr)
        return
    events = replay_events(data)
    model = play(inst, events, len(ref)/sr)[:len(ref)]
    mono = M.highpass(model[:, 0].astype(float), sr, HIGHPASS_HZ)
    onsets = [int(round(h['onset_s']*sr)) for h in data['hits']]
    results = []
    for hit, onset, end in zip(data['hits'], onsets, onsets[1:] + [len(ref)]):
        rows = metrics(ref, mono, sr, onset, len(ref))
        results.append({'index': hit['index'], 'onset_s': hit['onset_s'], 'key': events[hit['index']][1],
                        'velocity': round(events[hit['index']][2], 3), 'substituted': hit['index'] in SUBSTITUTES,
                        'next_stroke_ms': round(1000*(end - onset)/sr, 1), 'windows': rows})
        print(f"{hit['index']:2d} {hit['onset_s']:6.3f} key {events[hit['index']][1]}{'*' if hit['index'] in SUBSTITUTES else ' '} "
              f"level dB " + ' '.join(f"{w['level_error_db']:+5.1f}" for w in rows)
              + ' | band med ' + ' '.join(f"{w['median_audible_band_error_db']:4.1f}" for w in rows))
    summary = {}
    for i, (a, b) in enumerate(WINDOWS):
        own = [r['windows'][i] for r in results if not r['substituted'] and len(r['windows']) > i]
        every = [r['windows'][i] for r in results if len(r['windows']) > i]
        summary[f'{int(a*1000)}-{int(b*1000)}ms'] = {
            'installed_mean_abs_level_error_db': round(float(np.mean([abs(w['level_error_db']) for w in own])), 2),
            'all_mean_abs_level_error_db': round(float(np.mean([abs(w['level_error_db']) for w in every])), 2),
            'all_mean_band_error_db': [round(float(v), 2) for v in np.mean([w['band_error_db'] for w in every], 0)]}
    for k, v in summary.items():
        print(k, v)
    gap = np.zeros((int(.4*sr), 2))
    solo = []
    for row in range(12):
        y = play(inst, [(0.0, FIRST_KEY + row, 1.0)], 1.6)
        solo += [y, gap]
    beat = 60/96
    groove = [(i*beat/2, FIRST_KEY + [0, 2, 1, 2, 0, 3, 1, 2][i % 8] + (10 if i % 16 == 8 else 0), .9) for i in range(32)]
    wide = play(inst, groove, 32*beat/2 + 2, {'output.width': .5})
    sf.write(OUT/'sample-ab.wav', np.vstack([np.repeat(ref[:, None], 2, 1), gap, gap, model]), sr)
    sf.write(OUT/'model.wav', model, sr)
    sf.write(OUT/'strokes.wav', np.vstack(solo), sr)
    sf.write(OUT/'groove.wav', wide, sr)
    (HERE/'comparison.json').write_text(json.dumps({
        'compiler_sha256': inst.compiler_sha256, 'render_rate': RENDER_RATE, 'sample_rate': sr,
        'method': 'one voice re-plays the whole sample as retriggers at the recorded onsets; each stroke window scored against the high-passed left channel',
        'summary': summary, 'hits': results}, indent=1) + '\n')


if __name__ == '__main__':
    main()
