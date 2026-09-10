#!/usr/bin/env python3
"""Compiled-audio checks for PM Clarinet; run with --roundtrip after the patch test."""
import argparse
import hashlib
import itertools
import json
from pathlib import Path
import platform
import subprocess
import sys

import numpy as np

from analyze import ROOT, harmonics
sys.path.insert(0, str(ROOT / 'tools/audition'))
from audition import Instrument, write_wav

HERE = Path(__file__).resolve().parent
FACTORY = ROOT / 'content/instruments/Physical Models'
SOURCE = FACTORY / 'PM Clarinet/dsp.lisp'


def rms(y):
    return float(np.sqrt(np.mean(np.asarray(y, dtype=np.float64) ** 2)))


def fundamental(y, sr, expected, start=.7, end=1.7):
    segment = y[int(start * sr):int(end * sr)]
    n = 2 ** int(np.ceil(np.log2(len(segment) * 4)))
    magnitude = abs(np.fft.rfft(segment * np.hanning(len(segment)), n))
    frequency = np.fft.rfftfreq(n, 1 / sr)
    band = np.flatnonzero(abs(frequency - expected) < .12 * expected)
    i = band[np.argmax(magnitude[band])]
    log = np.log(magnitude[i - 1:i + 2] + 1e-30)
    offset = .5 * (log[0] - log[2]) / (log[0] - 2 * log[1] + log[2])
    return float((i + offset) * sr / n)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--roundtrip', type=Path, required=True,
                        help='ESEQ_PM_VERIFY_DIR from the factory patch test')
    args = parser.parse_args()
    output = HERE / 'output'
    output.mkdir(exist_ok=True)
    target = {('Darwin', 'arm64'): 'DGenLisp-macos-arm64',
              ('Linux', 'x86_64'): 'DGenLisp-linux-x86_64'}[(platform.system(), platform.machine())]
    compiler = ROOT / 'crates/sequencer/tools' / target
    stage = ROOT / 'crates/sequencer/tools/dgen-toolchain'
    builds, checks = set(), []

    def instrument(path=SOURCE, sr=48000, block=128):
        inst = Instrument(path, compiler=str(compiler), toolchain_root=str(stage),
                          sample_rate=sr, max_frames=block)
        if inst.build_dir not in builds:
            subprocess.run([sys.executable, str(ROOT / 'tools/audition/check_fusion.py'),
                            str(Path(inst.build_dir) / 'patch.c')], check=True, capture_output=True)
            builds.add(inst.build_dir)
        return inst

    def render(inst, label, seconds=2.8, pitch=295.608569, **kwargs):
        y, state = inst.render(seconds, pitch=pitch, **kwargs)
        assert np.all(np.isfinite(y)) and np.all(np.isfinite(state)), (label, 'non-finite state/audio')
        peak = float(np.max(abs(y)))
        assert peak < 4, (label, 'unbounded output', peak)
        checks.append({'case': label, 'peak': peak, 'rms': rms(y)})
        return y

    inst = instrument()
    params = {name: p for name, p in inst.params.items() if not name.startswith('__')}
    destinations = {d['name'] for d in inst.manifest['modDestinations']}
    expected_mods = set(params) - {'amp.attack', 'amp.decay', 'amp.sustain', 'amp.release',
                                  'reed.vel_blow', 'expression.vib_wait'}
    assert destinations == expected_mods, ('modulation contract', destinations ^ expected_mods)
    defaults = {name: p['default'] for name, p in params.items()}
    for name, p in params.items():
        assert p['min'] <= p['default'] <= p['max'], name
    presets = json.loads((FACTORY / 'PM Clarinet.presets').read_text())['presets']
    assert len({p['id'] for p in presets}) == len(presets)
    assert presets[0]['params'] == defaults, 'Reference Reed must recall the factory voicing'
    for preset in presets:
        assert set(preset['params']) == set(params), preset['name']
        for name, value in preset['params'].items():
            assert params[name]['min'] <= value <= params[name]['max'], (preset['name'], name)
        for hz in [55, 146.83, 295.608569, 587.33, 1174.66]:
            y = render(inst, f"{preset['id']} {hz} Hz", pitch=hz, params=preset['params'], gate_off=1.8)
            assert rms(y[24000:81600]) > .003, (preset['id'], hz, 'silent sustain')
            assert rms(y[-4800:]) < .001, (preset['id'], hz, 'release tail')
        hz = 110 if preset['id'] in {'reed-bass', 'chalumeau'} else 295.608569
        y = render(inst, preset['id'] + ' preview', seconds=4, pitch=hz,
                   params=preset['params'], gate_off=3)
        # Listening exports only: preserve preset-relative levels, cap any peak.
        write_wav(str(output / (preset['id'] + '.wav')), y * min(1, .85 / max(abs(y))), 48000)
    print('Preset registers and releases passed', flush=True)

    reference = json.loads((HERE / 'reference-analysis.json').read_text())
    y = render(inst, 'reference harmonic comparison', pitch=reference['fundamental_hz'])
    measured = harmonics(y, 48000, reference['fundamental_hz'], .7, 1.7)
    weights = np.array([3, 1, 3, 2, 2, 1, 1, .5, 1, .5, .5, .4, .3, .2, .2, .2])
    error = float(np.average((np.maximum(measured, -50) - np.maximum(reference['harmonics_db'], -50)) ** 2,
                             weights=weights))
    assert error < 20, ('reference spectral voicing', error)
    checks[-1].update(harmonics_db=measured.tolist(), weighted_spectral_mse=error)
    for sr in [44100, 48000, 96000]:
        current = instrument(sr=sr)
        for hz in [110, 220, reference['fundamental_hz'], 587.33, 1174.66, 1760]:
            y = render(current, f'natural tuning {sr}/{hz}', pitch=hz, gate_off=1.8)
            cents = float(1200 * np.log2(fundamental(y, sr, hz) / hz))
            assert abs(cents) < 15, (sr, hz, cents)
            checks[-1]['pitch_error_cents'] = cents
    print('Reference spectrum and tuning at 44.1/48/96 kHz passed', flush=True)

    for hz in [55, 295.608569, 1174.66]:
        for name, p in params.items():
            if name.startswith('amp.'):
                continue
            for extreme in ['min', 'max']:
                y = render(inst, f'{hz} {name} {extreme}', pitch=hz,
                           params={name: p[extreme]}, gate_off=1.8)
                assert rms(y[-4800:]) < .001, (hz, name, extreme, 'release tail')
        print(f'Individual control extremes passed at {hz} Hz', flush=True)

    corner_names = ['reed.pressure', 'reed.stiffness', 'reed.closure', 'reed.curve', 'bore.warp', 'bore.loss']
    for choices in itertools.product(['min', 'max'], repeat=len(corner_names)):
        values = {name: params[name][edge] for name, edge in zip(corner_names, choices)}
        y = render(inst, 'joint corners ' + '/'.join(choices), params=values, gate_off=1.8)
        assert rms(y[-4800:]) < .001, (values, 'release tail')
    for name, p in params.items():
        if name.startswith('amp.'):
            continue
        render(inst, name + ' moving/retrigger', gate_off=2,
               ramps={name: [(0, p['min']), (.5, p['max']), (1, p['min']), (1.5, p['max'])]},
               retrig=[.65, 1.25])
    render(inst, 'joint moving controls and pitch', gate_off=2,
           ramps={name: [(0, params[name]['min']), (1, params[name]['max']), (2, params[name]['min'])]
                  for name in corner_names} | {'pitch': [(0, 110), (.75, 440), (1.25, 220)]},
           retrig=[.75, 1.25])
    print('Joint extremes, automation and retriggers passed', flush=True)

    # Enable the relevant motion when checking dependent rate/delay controls.
    expressive = {'expression.vib_cent': 20, 'expression.vib_air': .05, 'expression.growl': .1}
    for name, p in params.items():
        lo = render(inst, name + ' response min', params={**expressive, name: p['min']}, gate_off=1.8)
        hi = render(inst, name + ' response max', params={**expressive, name: p['max']}, gate_off=1.8)
        # Velocity > breath requires a velocity below one to expose its effect.
        if name == 'reed.vel_blow':
            lo = render(inst, name + ' response min quiet', vel=.5, params={name: p['min']})
            hi = render(inst, name + ' response max quiet', vel=.5, params={name: p['max']})
        assert rms(lo - hi) > 1e-4, (name, 'ineffective control')
    normal = render(inst, 'normal gain')
    half = render(inst, 'half gain', params={'gain': defaults['gain'] * .5})
    mute = render(inst, 'zero gain', params={'gain': 0})
    assert np.max(abs(half - normal * .5)) < 2e-5 and np.max(abs(mute)) == 0
    quiet = render(inst, 'quarter velocity', vel=.25)
    silent = render(inst, 'zero velocity', vel=0)
    assert rms(quiet[24000:81600]) < .65 * rms(normal[24000:81600]) and np.max(abs(silent)) == 0
    off = render(inst, 'zero depth alternate motion rates',
                 params={'expression.vib_hz': 11, 'expression.growl_hz': 199})
    assert np.max(abs(off - normal)) < 2e-5, 'zero depth must disable motion'
    for parameter in ['reed.curve', 'bore.warp', 'gain']:
        y = render(inst, parameter + ' host modulation', params={
            '__mod__' + parameter + '__active': 1,
            '__mod__' + parameter + '__depth__slot1': .5}, ramps={'mod1': [(0, 1)]})
        assert rms(y - normal) > 1e-3, (parameter, 'host modulation has no effect')
    print('All controls, velocity, gain and host modulation passed', flush=True)

    # Noise included: reblocking must preserve the sample-by-sample state path.
    a = render(inst, 'partition reference', params=expressive, gate_off=1.8)
    for block in [32, 64, 256]:
        b = render(instrument(block=block), f'partition {block}', params=expressive, gate_off=1.8)
        delta = float(np.max(abs(a - b)))
        assert delta < 1e-4, (block, 'block partition', delta)
        checks[-1]['partition_max_error'] = delta
    saved = instrument(args.roundtrip.resolve() / 'PM Clarinet/dsp.lisp')
    for preset in presets:
        a = render(inst, preset['id'] + ' authored source', params=preset['params'], gate_off=1.8)
        b = render(saved, preset['id'] + ' graph save', params=preset['params'], gate_off=1.8)
        delta = float(np.max(abs(a - b)))
        assert delta < 3e-4, (preset['id'], 'graph save changes audio', delta)
        checks[-1]['roundtrip_max_error'] = delta
    report = {'platform': platform.platform(), 'compiler_sha256': inst.compiler_sha256,
              'source_sha256': hashlib.sha256(SOURCE.read_bytes()).hexdigest(),
              'presets_sha256': hashlib.sha256((FACTORY / 'PM Clarinet.presets').read_bytes()).hexdigest(),
              'reference_sha256': reference['sha256'], 'parameter_count': len(params),
              'modulation_destination_count': len(destinations), 'fusion_checked_builds': len(builds),
              'checks': checks}
    (HERE / 'validation.json').write_text(json.dumps(report, indent=2) + '\n')
    print(f'PASS: {len(checks)} audio/state renders', flush=True)


if __name__ == '__main__':
    main()
