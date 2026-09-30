#!/usr/bin/env python3
"""Compare the compiled PM Tabla DSP with every reference stroke.

Renders each stroke key through the production compiler/ABI at the source
rate, then reports level and spectral errors in time windows. Also writes
local listening material (never committed):
  .local/pm-tabla/ab.wav          each stroke window: reference, then model
  .local/pm-tabla/strokes.wav     every key played alone, in key order
  .local/pm-tabla/model.wav       the whole sample re-played by the model
  .local/pm-tabla/sample-ab.wav   reference sample, then the model's replay
"""
import argparse
import json
import os
import platform
import subprocess
import sys
from pathlib import Path

import numpy as np
import soundfile as sf
from scipy.signal import butter, sosfiltfilt

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(ROOT/'tools/audition'))
os.environ.setdefault('AUDITION_CACHE', str(ROOT/'.local/pm-tabla/cache'))
import modal_fit as M
from analyze import SAMPLE
from build import DEST, FIRST_KEY, SUBSTITUTES, stroke_row, strokes
from audition import Instrument

OUT = ROOT/'.local/pm-tabla'
WINDOWS = [(0, .01), (.01, .025), (.025, .05), (.05, .1), (.1, .2), (.2, .3)]
BANDS = [70, 140, 280, 560, 1120, 2240, 4480, 8960, 17920]


def instrument(sr, block=128, voices=1):
    target = {('Darwin', 'arm64'): 'DGenLisp-macos-arm64',
              ('Linux', 'x86_64'): 'DGenLisp-linux-x86_64'}[(platform.system(), platform.machine())]
    inst = Instrument(str(DEST), compiler=os.environ.get('ESEQ_DGENLISP_TOOL', str(ROOT/'crates/sequencer/tools'/target)),
                      toolchain_root=str(ROOT/'crates/sequencer/tools/dgen-toolchain'),
                      sample_rate=sr, max_frames=block, voices=voices)
    audit = subprocess.run([sys.executable, str(ROOT/'tools/audition/check_fusion.py'),
                            str(Path(inst.build_dir)/'patch.c')], capture_output=True, text=True)
    if audit.returncode:
        raise RuntimeError('Generated-C fusion audit failed:\n' + audit.stdout + audit.stderr)
    return inst


def hz(note):
    return 440*2**((note - 69)/12)


def render(inst, row, seconds, params=None, vel=1.0):
    y, _ = inst.render(seconds=seconds, pitch=hz(FIRST_KEY + row), vel=vel, params=params)
    assert np.isfinite(y).all()
    return y


def band_db(y, sr, lo, hi, a, b):
    sos = butter(4, [lo, min(hi, .45*sr)], 'bandpass', fs=sr, output='sos')
    seg = sosfiltfilt(sos, y.mean(1))[int(a*sr):int(b*sr)]
    return 10*np.log10(np.mean(seg**2) + 1e-14)


def metrics(ref, model, sr):
    n = min(len(ref), len(model))
    ref, model = ref[:n], model[:n]
    out = {'windows': []}
    for a, b in WINDOWS:
        if b*sr > n:
            break
        r = 10*np.log10(np.mean(ref[int(a*sr):int(b*sr)]**2) + 1e-14)
        m = 10*np.log10(np.mean(model[int(a*sr):int(b*sr)]**2) + 1e-14)
        bands = [band_db(model, sr, lo, hi, a, b) - band_db(ref, sr, lo, hi, a, b) for lo, hi in zip(BANDS, BANDS[1:])]
        weights = np.array([10**(band_db(ref, sr, lo, hi, a, b)/10) for lo, hi in zip(BANDS, BANDS[1:])])
        audible = weights > weights.max()*10**(-3)   # bands within 30 dB of the loudest
        out['windows'].append({'ms': [int(a*1000), int(b*1000)], 'level_error_db': round(float(m - r), 2),
                               'band_error_db': [round(float(e), 2) for e in bands],
                               'median_audible_band_error_db': round(float(np.median(np.abs(np.array(bands)[audible]))), 2)})
    return out


def early_energy(hit, seconds=0.025):
    """Modal energy of a stroke's first contact over its first `seconds` (fitted coefficients)."""
    return sum(sum(c**2 for c in m['residue'][0])*(1 - np.exp(-2*m['rate_per_s']*seconds))/(2*m['rate_per_s'])
               for m in hit['modes'])


def replay_velocity(data, hit):
    """Velocity at which a stroke's key replays it: 1 for installed strokes; a
    substitute is scaled to the reference stroke's early modal energy (the
    engine is linear in velocity apart from Vel > timbre)."""
    key = data['hits'][SUBSTITUTES.get(hit['index'], hit['index'])]
    return float(np.clip(np.sqrt(early_energy(hit)/early_energy(key)), 0.05, 1.0))


def deflated_reference(data, x, sr):
    """The reference with every earlier stroke's fitted ring removed, per stroke window.

    Rebuilds the analysis' sequential deflation from the stored coefficients
    (mono phasors; the reference is mono), so each installed key can be scored
    alone against its own stroke.
    """
    from analyze import ringing
    coef = data['bayan_pitch']['log2_hz_cubic']
    lo, hi = data['bayan_pitch']['span_s']
    track = lambda t: np.polyval(coef, np.clip(t, lo, hi))
    remaining = x.copy()
    own = {}
    for hit in data['hits']:
        a, b = hit['window_s']
        i, j = int(a*sr), int(b*sr)
        f = np.array([m['hz'] for m in hit['modes']])
        r = np.array([m['rate_per_s'] for m in hit['modes']])
        beta = np.array([m['beta'] for m in hit['modes']])
        s = len(hit['strike_delays_s'])
        C = np.concatenate([np.concatenate([[m['sin_cos'][q][0] for m in hit['modes']],
                                            [m['sin_cos'][q][1] for m in hit['modes']]]) for q in range(s)])
        C = np.repeat(C[:, None], x.shape[1], 1)
        warp = M.Warp(a, track) if np.any(beta) else None
        own[hit['index']] = remaining[i:j].copy()
        remaining[i:] -= ringing(f, r, beta, C, i, len(x), sr, hit['strike_delays_s'][1:], warp)
    return own


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--calibrate-noise', action='store_true', help='write noise-calibration.json from rendered skin noise')
    args = parser.parse_args()
    data = json.loads((HERE/'analysis.json').read_text())
    stroke_row(data, 0)   # resolves SUBSTITUTES
    raw, sr = sf.read(SAMPLE)
    x = M.highpass(raw, sr)
    inst = instrument(sr)
    OUT.mkdir(parents=True, exist_ok=True)
    if args.calibrate_noise:
        # Rendered skin noise (skin on minus skin off) must match the record's
        # 2.5-12 kHz click peak and 5-20 ms sizzle for every stroke.
        old = json.loads((HERE/'noise-calibration.json').read_text()) if (HERE/'noise-calibration.json').exists() else {}
        cal = {}
        sos = butter(4, [2500, min(12000, .45*sr)], 'bandpass', fs=sr, output='sos')
        win = int(.0005*sr)
        for row, hit in enumerate(strokes(data)):
            noise = (render(inst, row, .06) - render(inst, row, .06, {'contact.skin': 0.0})).mean(1)
            power = np.convolve(sosfiltfilt(sos, noise)**2, np.ones(win)/win, 'same')
            peak = int(np.argmax(power[:int(.006*sr)]))
            click = np.sqrt(np.mean(power[max(0, peak - win):peak + win]))
            sizzle = np.sqrt(np.median(power[peak + int(.005*sr):peak + int(.02*sr)]))
            target = hit['contact'][0]
            c0, s0 = old.get(hit['slug'], [1.0, 1.0])
            cal[hit['slug']] = [float(c0*target['click_rms']/max(click, 1e-12)),
                                float(s0*target['sizzle_rms']/max(sizzle, 1e-12))]
        (HERE/'noise-calibration.json').write_text(json.dumps(cal, indent=1) + '\n')
        print('Wrote noise-calibration.json; rerun build.py')
        return
    # Every key is rendered in isolation, then the sample is re-played by
    # summing those voices at the recorded onsets, so earlier strokes ring
    # into each window exactly as they do on the record. Strokes that are not
    # installed are played by their substitute key.
    n = len(x)
    rendered = {}
    loop = np.zeros((n + int(3*sr), 2))
    for hit in data['hits']:
        row = stroke_row(data, hit['index'])
        if row not in rendered:
            rendered[row] = render(inst, row, 3.0)
        vel = replay_velocity(data, hit)
        voice = rendered[row] if vel == 1.0 else render(inst, row, 3.0, vel=vel)
        start = int(hit['window_s'][0]*sr)
        loop[start:start + len(voice)] += voice
    model_loop = loop[:n]
    results, ab = [], []
    gap = np.zeros((int(.25*sr), 2))
    filtered = M.highpass(model_loop, sr)
    for hit in data['hits']:
        row = stroke_row(data, hit['index'])
        a, b = hit['window_s']
        i, j = int(a*sr), int(b*sr)
        m = metrics(x[i:j], filtered[i:j], sr)
        m.update(index=hit['index'], onset_s=hit['onset_s'], key=FIRST_KEY + row,
                 substituted=hit['index'] in SUBSTITUTES)
        results.append(m)
        ab += [raw[i:j], gap, model_loop[i:j], gap, gap]
        w = m['windows']
        print(f"{hit['index']:2d} {hit['onset_s']:6.3f} key {FIRST_KEY + row}{'*' if m['substituted'] else ' '} level dB "
              + ' '.join(f"{v['level_error_db']:+5.1f}" for v in w)
              + ' | band med ' + ' '.join(f"{v['median_audible_band_error_db']:4.1f}" for v in w))
    # Each installed key alone against its own stroke (earlier rings removed):
    # the engine's fidelity to the identified stroke, free of replay coupling.
    own = deflated_reference(data, x, sr)
    isolated = []
    for row, hit in enumerate(strokes(data)):
        ref = own[hit['index']]
        m = metrics(ref, M.highpass(rendered[row], sr)[:len(ref)], sr)
        m.update(index=hit['index'], name=hit['name'], key=FIRST_KEY + row)
        isolated.append(m)
        w = m['windows']
        print(f"isolated {hit['name']:12s} key {FIRST_KEY + row} level dB " + ' '.join(f"{v['level_error_db']:+5.1f}" for v in w)
              + ' | band med ' + ' '.join(f"{v['median_audible_band_error_db']:4.1f}" for v in w))
    solo = []
    for row in range(len(strokes(data))):
        solo += [rendered.get(row, render(inst, row, 3.0))[:int(1.0*sr)], gap]
    sf.write(OUT/'ab.wav', np.vstack(ab), sr)
    sf.write(OUT/'strokes.wav', np.vstack(solo), sr)
    sf.write(OUT/'model.wav', model_loop, sr)
    sf.write(OUT/'sample-ab.wav', np.vstack([raw, gap, gap, model_loop]), sr)
    (HERE/'comparison.json').write_text(json.dumps({'compiler_sha256': inst.compiler_sha256, 'sample_rate': sr,
                                                    'method': 'model re-plays the whole sample from isolated key renders; each stroke window scored against the highpassed reference',
                                                    'hits': results, 'isolated_keys': isolated}, indent=1) + '\n')

if __name__ == '__main__':
    main()
