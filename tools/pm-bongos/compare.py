#!/usr/bin/env python3
"""Compare the compiled PM Bongos DSP with every reference hit.

Renders each stroke key through the production compiler/ABI at the source
rate, then reports level and spectral errors in time windows. Also writes
local listening material (never committed):
  .local/pm-bongos/ab.wav          each hit window: reference, then model loop
  .local/pm-bongos/strokes.wav     every key played alone, in key order
  .local/pm-bongos/loop-model.wav  the whole loop re-played by the model (two bars)
  .local/pm-bongos/loop-ab.wav     reference loop twice, then model loop twice
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
os.environ.setdefault('AUDITION_CACHE', str(ROOT/'.local/pm-bongos/cache'))
import modal_fit as M
from analyze import SAMPLE
from build import DEST, FIRST_KEY, stroke_row, strokes
from audition import Instrument

OUT = ROOT/'.local/pm-bongos'
WINDOWS = [(0, .01), (.01, .025), (.025, .05), (.05, .1), (.1, .2), (.2, .32)]
BANDS = [110, 220, 440, 880, 1760, 3520, 7040, 14080]


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


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--calibrate-noise', action='store_true', help='write noise-calibration.json from rendered skin noise')
    args = parser.parse_args()
    data = json.loads((HERE/'analysis.json').read_text())
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
    # Every key is rendered in isolation, then the loop is re-played twice by
    # summing those voices at the recorded onsets, so earlier strokes (and the
    # previous bar) ring into each window exactly as they do on the record.
    # Hits are scored on the second pass against the reference loop.
    n = len(x)
    rendered = {}
    renders = []
    for hit in data['hits']:
        row = stroke_row(data, hit['slug'])
        if row not in rendered:
            rendered[row] = render(inst, row, 1.2)
        renders.append(rendered[row])
    loop = np.zeros((3*n, 2))
    for bar in range(2):
        for hit, voice in zip(data['hits'], renders):
            start = bar*n + int(hit['window_s'][0]*sr)
            loop[start:start + len(voice)] += voice
    model_loop = loop[n:2*n]
    results, ab = [], []
    gap = np.zeros((int(.25*sr), 2))
    for hit in data['hits']:
        row = stroke_row(data, hit['slug'])
        a, b = hit['window_s']
        i, j = int(a*sr), int(b*sr)
        m = metrics(x[i:j], M.highpass(model_loop, sr)[i:j], sr)
        m.update(slug=hit['slug'], name=hit['name'], key=FIRST_KEY + row)
        results.append(m)
        ab += [raw[i:j], gap, model_loop[i:j], gap, gap]
        w = m['windows']
        print(f"{hit['name']:16s} level dB " + ' '.join(f"{v['level_error_db']:+5.1f}" for v in w)
              + ' | band med ' + ' '.join(f"{v['median_audible_band_error_db']:4.1f}" for v in w))
    solo = []
    for row in range(len(strokes(data))):
        solo += [rendered.get(row, render(inst, row, 1.2))[:int(.6*sr)], gap]
    sf.write(OUT/'ab.wav', np.vstack(ab), sr)
    sf.write(OUT/'strokes.wav', np.vstack(solo), sr)
    sf.write(OUT/'loop-model.wav', np.tile(model_loop, (2, 1)), sr)
    sf.write(OUT/'loop-ab.wav', np.vstack([raw, raw, gap, model_loop, model_loop]), sr)
    (HERE/'comparison.json').write_text(json.dumps({'compiler_sha256': inst.compiler_sha256, 'sample_rate': sr,
                                                    'method': 'second pass of the model re-playing the loop twice; each window scored against the highpassed reference',
                                                    'hits': results}, indent=1) + '\n')

if __name__ == '__main__':
    main()
