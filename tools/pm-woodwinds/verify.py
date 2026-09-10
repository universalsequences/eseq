#!/usr/bin/env python3
"""Render the factory woodwinds through the pinned compiler; see README.md."""
import argparse
import hashlib
import itertools
import json
from pathlib import Path
import platform
import subprocess
import sys

import numpy as np

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'tools/audition'))
from audition import Instrument, write_wav

FACTORY = ROOT / 'content/instruments/Physical Models'
BASELINE = Path(__file__).parent / 'baseline'


def prepared_source(source, output):
    # PM Flute has one imported macro. Resolve that exact dependency from the
    # same factory package used by the host; refuse any unhandled import.
    source = source.replace('(use-defmacro pitch-transpose)',
                            (ROOT / 'content/defmacros/pitch-transpose/macro.lisp').read_text())
    if '(use-defmacro ' in source:
        raise ValueError('New macro import: update woodwind source preparation')
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(source)
    return output


def rms(y):
    return float(np.sqrt(np.mean(np.asarray(y, dtype=np.float64) ** 2)))


def harmonic_db(y, sr, hz, start=.5, end=1.8):
    n = 8192
    window = np.hanning(n)
    frames = np.stack([y[i:i + n] * window
                       for i in range(int(start * sr), min(int(end * sr), len(y) - n), 2048)])
    power = abs(np.fft.rfft(frames)) ** 2
    freqs = np.fft.rfftfreq(n, 1 / sr)
    bands = np.array([np.mean(np.sum(power[:, abs(freqs - h * hz) < .13 * hz], axis=1))
                      for h in range(1, 13)])
    return 10 * np.log10(bands / max(np.sum(bands), 1e-30) + 1e-7)


def fundamental(y, sr, expected):
    # Search around the requested fundamental, even when H2 or H3 is louder.
    seg = y[int(.7 * sr):int(1.8 * sr)]
    n = 2 ** int(np.ceil(np.log2(len(seg) * 4)))
    spec = abs(np.fft.rfft(seg * np.hanning(len(seg)), n))
    freq = np.fft.rfftfreq(n, 1 / sr)
    bins = np.flatnonzero(abs(freq - expected) < .12 * expected)
    return float(freq[bins[np.argmax(spec[bins])]])


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--output', type=Path, default=Path(__file__).parent / 'output')
    ap.add_argument('--roundtrip', type=Path, help='ESEQ_PM_VERIFY_DIR from the Rust patch test')
    ap.add_argument('--roundtrip-only', action='store_true', help='Only check saved graph audio and host modulation')
    ap.add_argument('--flute-controls-only', action='store_true', help='Check the flute output/range fixes and retained default')
    args = ap.parse_args()
    if args.roundtrip_only and not args.roundtrip:
        ap.error('--roundtrip-only requires --roundtrip')
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    target = {('Darwin', 'arm64'): 'DGenLisp-macos-arm64',
              ('Linux', 'x86_64'): 'DGenLisp-linux-x86_64'}[(platform.system(), platform.machine())]
    compiler = ROOT / 'crates/sequencer/tools' / target
    stage = ROOT / 'crates/sequencer/tools/dgen-toolchain'
    builds = set()
    checks = []

    def instrument(path, sr=48000, block=128):
        inst = Instrument(str(path), compiler=str(compiler), toolchain_root=str(stage),
                          sample_rate=sr, max_frames=block)
        if inst.build_dir not in builds:
            subprocess.run([sys.executable, str(ROOT / 'tools/audition/check_fusion.py'),
                            str(Path(inst.build_dir) / 'patch.c')], check=True, capture_output=True)
            builds.add(inst.build_dir)
        return inst

    def render(inst, label, seconds=3.5, pitch=370, params=None, **kwargs):
        y, state = inst.render(seconds, pitch=pitch, params=params, **kwargs)
        assert np.all(np.isfinite(y)) and np.all(np.isfinite(state)), label
        assert np.max(abs(y)) < 4, (label, 'peak', np.max(abs(y)))
        checks.append({'case': label, 'peak': float(np.max(abs(y))), 'rms': rms(y)})
        return y

    flute_path = prepared_source((FACTORY / 'PM Flute/dsp.lisp').read_text(),
                                 output / 'source/flute/dsp.lisp')
    old_flute_path = prepared_source((BASELINE / 'flute.lisp').read_text(),
                                     output / 'source/original-flute/dsp.lisp')
    sax_path = FACTORY / 'PM Saxophone/dsp.lisp'
    flute, old_flute = instrument(flute_path), instrument(old_flute_path)
    sax, old_sax = instrument(sax_path), instrument(BASELINE / 'saxophone.lisp')

    if args.flute_controls_only:
        for sr in [44100, 48000, 96000]:
            current, original = instrument(flute_path, sr), instrument(old_flute_path, sr)
            p = current.params['embouchure']
            assert p['min'] <= p['default'] <= p['max']
            for hz in [55, 110, 185, 370, 740]:
                a = render(original, f'flute original {sr} {hz}', pitch=hz, gate_off=2)
                b = render(current, f'flute unity {sr} {hz}', pitch=hz, gate_off=2)
                delta = float(np.max(abs(a - b)))
                # Two float-evaluated oscillator scalings feed a recursive jet.
                # Require both a small instantaneous error and <-100 dBFS RMS
                # across rates; 96 kHz accumulates more phase-rounding error.
                assert delta < 1e-4 and rms(a - b) < 1e-5, (sr, hz, delta, rms(a - b))
                checks[-1]['baseline_max_error'] = delta
                checks[-1]['baseline_rms_error'] = rms(a - b)
            normal = render(current, f'flute default gain {sr}')
            half = render(current, f'flute half gain {sr}', params={'gain': .25})
            mute = render(current, f'flute muted {sr}', params={'gain': 0})
            assert rms(normal) > .01 and np.max(abs(mute)) == 0
            assert np.max(abs(half - normal * .5)) < 2e-5
            for value in [p['min'], p['default'], p['max']]:
                render(current, f'flute embouchure {sr} {value}', params={'embouchure': value}, gate_off=2)
        report = {'compiler_sha256': current.compiler_sha256,
                  'source_sha256': hashlib.sha256((FACTORY / 'PM Flute/dsp.lisp').read_bytes()).hexdigest(),
                  'checks': checks}
        (Path(__file__).parent / 'flute-ui-validation.json').write_text(json.dumps(report, indent=2) + '\n')
        print(f'PASS: {len(checks)} flute control/default renders')
        return

    if args.roundtrip_only:
        bank = json.loads((FACTORY / 'PM Saxophone.presets').read_text())['presets']
        for name, inst in [('PM Flute', flute), ('PM Saxophone', sax)]:
            saved = instrument(args.roundtrip.resolve() / name / 'dsp.lisp')
            variants = [{}] if name == 'PM Flute' else [{}, bank[1]['params'], bank[2]['params']]
            for params in variants:
                a = render(inst, name + ' source', params=params, gate_off=2)
                b = render(saved, name + ' patch save', params=params, gate_off=2)
                delta = float(np.max(abs(a - b)))
                assert delta < 3e-4, (name, 'patch save changes audio', delta)
                checks[-1]['roundtrip_max_error'] = delta
        for inst, parameter in [(flute, 'flutter.rate'), (sax, 'bore.acoustic')]:
            destinations = {d['name'] for d in inst.manifest['modDestinations']}
            assert parameter in destinations
            a = render(inst, parameter + ' modulation off')
            b = render(inst, parameter + ' modulation on', params={
                '__mod__' + parameter + '__active': 1,
                '__mod__' + parameter + '__depth__slot1': .5},
                ramps={'mod1': [(0, 1)]})
            assert rms(a - b) > 1e-3, (parameter, 'host modulator has no effect')
        (output / 'roundtrip-verification.json').write_text(json.dumps(checks, indent=2) + '\n')
        print(f'PASS: {len(checks)} saved-graph/modulation renders')
        return

    for name, old, new, tolerance in [('sax', old_sax, sax, 1e-6), ('flute', old_flute, flute, 2e-5)]:
        for hz in [55, 110, 185, 370, 740]:
            a = render(old, f'{name} original {hz}', pitch=hz, gate_off=2)
            b = render(new, f'{name} retained {hz}', pitch=hz, gate_off=2)
            delta = float(np.max(abs(a - b)))
            assert delta <= tolerance, (name, hz, delta)
            checks[-1]['baseline_max_error'] = delta

    # Zero depth disables both oscillators; floor cannot exceed depth.
    off = {'flutter.depth': 0}
    a = render(flute, 'flute motion off', params=off)
    b = render(flute, 'flute motion off alternate rates', params={**off,
               'flutter.rate': 11, 'flutter.drift_hz': 3})
    assert np.max(abs(a - b)) < 2e-5
    for name in ['rate', 'drift_hz', 'depth', 'floor']:
        key = 'flutter.' + name
        p = flute.params[key]
        lo = render(flute, key + ' min', params={key: p['min']})
        hi = render(flute, key + ' max', params={key: p['max']})
        assert rms(lo - hi) > 1e-4, (key, 'inaudible control')

    presets = json.loads((FACTORY / 'PM Saxophone.presets').read_text())['presets']
    by_id = {p['id']: p['params'] for p in presets}
    for preset in presets:
        for hz in [55, 110, 185, 370, 740]:
            y = render(sax, f"{preset['name']} {hz}", pitch=hz, params=preset['params'], gate_off=2)
            if preset['id'] != 'original' or hz <= 370:
                assert rms(y[int(.5 * 48000):int(1.8 * 48000)]) > .002, (preset['name'], hz, 'silent sustain')
            assert rms(y[-4800:]) < .001, (preset['name'], hz, 'release tail')
        hz = 183.44 if preset['id'] == 'alto-full' else (92.5 if 'bass' in preset['id'] or preset['id'] == 'original' else 369.53)
        y = render(sax, preset['name'] + ' preview', seconds=4, pitch=hz, params=preset['params'], gate_off=3)
        # Preview level only: a common peak ceiling, no change to factory gain.
        write_wav(str(output / (preset['id'] + '.wav')), y * (.8 / max(float(np.max(abs(y))), .01)), 48000)
        checks[-1]['harmonics_db'] = harmonic_db(y, 48000, hz).tolist()

    base = {**by_id['alto-soft'], 'reed.air': 0, 'expression.vib_cent': 0,
            'expression.vib_air': 0}
    for hz in [55, 185, 370, 740, 1400]:
        for name, p in sax.params.items():
            if name.startswith('__') or name.startswith('amp.'):
                continue
            for extreme in ['min', 'max']:
                y = render(sax, f'{hz} {name} {extreme}', pitch=hz,
                           params={**base, name: p[extreme]}, gate_off=2)
                assert rms(y[-4800:]) < .001, (hz, name, extreme, 'release tail')
        print(f'control extremes passed at {hz} Hz', flush=True)

    # Joint corners and moving controls test the feedback loop under p-locks,
    # including changes in delay length while the bore still holds energy.
    for pressure, aperture, position, loss in itertools.product([.05, 1.2], [.3, .9], [.04, .5], [.02, .85]):
        render(sax, 'joint corners', pitch=185, params={**base, 'reed.pressure': pressure,
               'reed.closure': aperture, 'bore.blow_pos': position, 'bore.damping': loss}, gate_off=2)
    for name, p in sax.params.items():
        if name.startswith('__') or name.startswith('amp.'):
            continue
        render(sax, name + ' automation', params=base, gate_off=2.5,
               ramps={name: [(0, p['min']), (.7, p['max']), (1.4, p['min']), (2, p['max'])]},
               retrig=[.8, 1.7])

    for sr in [44100, 48000, 96000]:
        inst = instrument(sax_path, sr=sr)
        for key in ['alto-soft', 'alto-full']:
            params = {**by_id[key], 'reed.air': 0, 'expression.vib_cent': 0,
                      'expression.vib_air': 0}
            for hz in [110, 185, 370, 740]:
                y = render(inst, f'{key} tuning {hz}/{sr}', pitch=hz, params=params, gate_off=2)
                f0 = fundamental(y, sr, hz)
                cents = 1200 * np.log2(f0 / hz)
                assert abs(cents) < 35, (key, sr, hz, cents)
                checks[-1]['pitch_error_cents'] = float(cents)

    for key in ['alto-soft', 'alto-full']:
        params = {**by_id[key], 'reed.air': 0, 'expression.vib_air': 0}
        quiet = render(sax, key + ' low velocity', params=params, vel=.25)
        loud = render(sax, key + ' full velocity', params=params, vel=1)
        silent = render(sax, key + ' zero velocity', params=params, vel=0)
        assert rms(quiet[24000:86400]) < .65 * rms(loud[24000:86400])
        assert np.max(abs(silent)) == 0

    # Noise off: block partitioning must not change the physical state update.
    params = {**base, 'expression.vib_cent': 12}
    a = render(sax, 'partition reference', params=params, gate_off=2)
    for block in [32, 64, 256]:
        inst = instrument(sax_path, block=block)
        b = render(inst, f'partition {block}', params=params, gate_off=2)
        assert np.max(abs(a - b)) < 1e-4, (block, np.max(abs(a - b)))

    if args.roundtrip:
        for name, inst in [('PM Flute', flute), ('PM Saxophone', sax)]:
            saved = instrument(args.roundtrip.resolve() / name / 'dsp.lisp')
            variants = [{}] if name == 'PM Flute' else [{}, base]
            for params in variants:
                a = render(inst, name + ' source', params=params, gate_off=2)
                b = render(saved, name + ' patch save', params=params, gate_off=2)
                assert np.max(abs(a - b)) < 3e-4, (name, 'patch save changes audio', np.max(abs(a - b)))

    report = {'compiler_sha256': sax.compiler_sha256,
              'source_sha256': {name: hashlib.sha256((FACTORY / name / 'dsp.lisp').read_bytes()).hexdigest()
                                for name in ['PM Flute', 'PM Saxophone']},
              'render_count': len(checks), 'builds': sorted(builds), 'checks': checks}
    (output / 'verification.json').write_text(json.dumps(report, indent=2) + '\n')
    print(f'PASS: {len(checks)} renders; report and WAV previews in {output}')


if __name__ == '__main__':
    main()
