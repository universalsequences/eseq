#!/usr/bin/env python3
"""Compile the development Heat voice and verify routing/articulation invariants.

These are integration checks, not an Analog sonic-parity verdict. The separate
instrument_probe exercises production macro resolution and initialization.
"""
import argparse
import ctypes
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys
import tempfile

import numpy as np
from scipy.io import wavfile

from check_filters import Instrument, ROOT


def compile_source():
    paths = [ROOT / 'content/defmacros' / name / 'macro.lisp' for name in
             ('heat-envelope', 'heat-pitch-envelope', 'heat-lfo', 'heat-linear-filter', 'heat-soft-clip', 'heat-drive')]
    paths.append(ROOT / 'tools/heat/instrument/dsp.lisp')
    # Explicit dependency order keeps this check independent of host resolution.
    # A new dependency must be added here; unresolved imports hard-fail compile.
    text = '\n'.join(p.read_text() for p in paths)
    for name in ('heat-envelope', 'heat-pitch-envelope', 'heat-lfo', 'heat-linear-filter', 'heat-soft-clip', 'heat-drive'):
        text = text.replace(f'(use-defmacro {name})', '')
    return text, {str(p.relative_to(ROOT)): hashlib.sha256(p.read_bytes()).hexdigest() for p in paths}


def render(inst, seconds=1.5, params=None, notes=None, pressure=0):
    frames = round(seconds * inst.sample_rate)
    notes = notes or [(0, .75, 220, False)]
    signal = {name: np.zeros(frames, np.float32) for name in inst.inputs}
    signal['pressure'][:] = pressure
    signal['velocity'][:] = 1
    signal['pitch'][:] = notes[0][2]
    for start, end, hz, legato in notes:
        begin = round(start * inst.sample_rate)
        finish = round(end * inst.sample_rate)
        signal['gate'][begin:finish] = 1
        signal['pitch'][begin:] = hz
        signal['note_on'][begin] = 1
        signal['trigger'][begin] = not legato
        signal['legato'][begin] = legato
    memory = inst.fresh_memory()
    for name, value in (params or {}).items():
        memory[inst.params[name]['cellId']] = value
    inputs = [np.zeros(inst.max_frames, np.float32) for _ in range(inst.n_in)]
    outputs = [np.zeros(inst.max_frames, np.float32) for _ in range(inst.n_out)]
    ptr = ctypes.POINTER(ctypes.c_float)
    ip = (ptr * len(inputs))(*[x.ctypes.data_as(ptr) for x in inputs])
    op = (ptr * len(outputs))(*[x.ctypes.data_as(ptr) for x in outputs])
    result = np.zeros((frames, inst.n_out), np.float32)
    for offset in range(0, frames, inst.max_frames):
        n = min(inst.max_frames, frames - offset)
        for name, channel in inst.inputs.items():
            inputs[channel][:n] = signal[name][offset:offset+n]
        inst.process_fn(ip, op, n, memory.ctypes.data_as(ctypes.c_void_p),
                        ctypes.byref(inst.context), None)
        for channel in range(inst.n_out):
            result[offset:offset+n, channel] = outputs[channel][:n]
    if not np.isfinite(result).all() or not np.isfinite(memory).all():
        raise ValueError('Non-finite Heat output/state')
    return result


def run(out, demo):
    source, hashes = compile_source()
    report = {'scope': 'Development voice integration; not reference sonic parity',
              'source_sha256': hashes, 'cases': {}}
    def expect(name, passed, **metrics):
        report['cases'][name] = {'passed': bool(passed), **metrics}
        if not passed:
            raise ValueError(f'{name}: {metrics}')
        print(f'{name}: pass {metrics}')
    with tempfile.TemporaryDirectory(prefix='heat-voice-') as folder:
        path = Path(folder) / 'dsp.lisp'
        path.write_text(source)
        for sr in (44100, 48000, 96000):
            inst = Instrument(path, sample_rate=sr)
            report['compiler_sha256'] = inst.compiler_sha256
            subprocess.run([sys.executable, str(ROOT / 'tools/audition/check_fusion.py'),
                            str(Path(inst.build_dir) / 'patch.c')], check=True)
            y = render(inst)
            peak, rms = float(abs(y).max()), float(np.sqrt(np.mean(y*y)))
            expect(f'{sr}-default', peak > .01 and rms > .001, peak=peak, rms=rms)
            tail = float(abs(y[round(1.1*sr):]).max())
            expect(f'{sr}-released', tail < 1e-7, tail_peak=tail)
            # With filters bypassed and identical envelopes, the send has an
            # exact, audible channel-routing contract independent of DSP fit.
            common = {'filter1_enabled': 0, 'filter2_enabled': 0,
                      'amp1_pan': -1, 'amp2_pan': 1}
            direct = render(inst, params=common)
            serial = render(inst, params={**common, 'filter1_to_filter2': 1})
            half = render(inst, params={**common, 'filter1_to_filter2': .5})
            error = float(max(abs(direct[:, 0] - serial[:, 1]).max(),
                              abs(serial[:, 0]).max(), abs(direct[:, 1]).max(),
                              abs(half[:, 0] - .5*direct[:, 0]).max(),
                              abs(half[:, 1] - .5*direct[:, 0]).max()))
            expect(f'{sr}-serial-parallel-routing', error < 1e-6, max_error=error)
            muted = render(inst, params={'amp1_enabled': 0})
            expect(f'{sr}-amp-mute', abs(muted).max() == 0)
            pressured = render(inst, params={'pressure_amp_db': 6}, pressure=1)
            error = float(abs(pressured - y*10**(6/20)).max())
            expect(f'{sr}-pressure-level', error < 1e-6, max_error=error)
            articulation = {**common, 'osc1_wave': 0, 'osc2_wave': 0, 'osc2_enabled': 1,
                            'amp1_env_attack_ms': 1000, 'amp2_env_attack_ms': 1000,
                            'amp1_env_legato': 1, 'amp2_env_legato': 0}
            single = render(inst, params=articulation, notes=[(0,.8,220,False)])
            overlap = render(inst, params=articulation,
                             notes=[(0,.8,220,False),(.25,.8,220,True)])
            held_error = float(abs(single[:, 0] - overlap[:, 0]).max())
            retrigger_difference = float(abs(single[:, 1] - overlap[:, 1]).max())
            expect(f'{sr}-independent-legato-envelope-policy',
                   held_error < 1e-6 and retrigger_difference > .001,
                   held_max_error=held_error, retrigger_difference=retrigger_difference)
            # Every filter family at its high-Q/high-frequency limit, with
            # both lanes and all oscillator shapes exercised across the set.
            for mode in range(8):
                extreme = render(inst, seconds=.3, notes=[(0,.1,4000,False)], params={
                    'osc1_wave': mode % 4, 'osc2_enabled': 1, 'osc2_wave': (mode+1) % 4,
                    'filter1_mode': mode, 'filter2_mode': mode, 'filter1_q':100,
                    'filter2_q':100, 'filter1_cutoff_hz':22000, 'filter2_cutoff_hz':22000,
                    'filter1_to_filter2':.5, 'filter1_drive':6, 'filter2_drive':6,
                })
                expect(f'{sr}-filter{mode}-extremes', True, peak=float(abs(extreme).max()))
            if sr == 48000 and demo:
                phrases = [
                    ({'osc1_sub_level':.3, 'filter1_cutoff_hz':450, 'filter1_q':2,
                      'filter1_env_octaves':3.5, 'amp1_env_sustain':.45},
                     [(0,.3,110,False),(.4,.7,130.8128,False),(.8,1.1,164.8138,False),
                      (1.2,1.8,146.8324,False)]),
                    ({'osc2_enabled':1, 'osc2_cents':7, 'osc2_semitones':12,
                      'amp1_pan':-.6, 'amp2_pan':.6, 'amp1_env_attack_ms':150,
                      'amp2_env_attack_ms':150, 'amp1_env_release_ms':600,
                      'amp2_env_release_ms':600, 'filter1_cutoff_hz':1200,
                      'filter2_cutoff_hz':2400},
                     [(0,.8,220,False),(.6,1.4,261.6256,True),(1.2,1.8,329.6276,True)]),
                ]
                audio = np.concatenate([render(inst, seconds=2.5, params=p, notes=n) for p,n in phrases])
                demo.parent.mkdir(parents=True, exist_ok=True)
                wavfile.write(demo, sr, audio)
                report['demo'] = {'path': str(demo), 'peak': float(abs(audio).max()),
                                  'normalized': False}
            if sr == 48000:
                bank_path = ROOT / 'tools/heat/instrument.presets'
                report['preset_bank_sha256'] = hashlib.sha256(bank_path.read_bytes()).hexdigest()
                for preset in json.loads(bank_path.read_text())['presets']:
                    expected_names = {name for name, param in inst.params.items() if not param.get('hidden', False)}
                    expected_names.discard('base_note')
                    expect(f"preset-{preset['id']}-complete", set(preset['params']) == expected_names,
                           missing=sorted(expected_names - set(preset['params'])),
                           unknown=sorted(set(preset['params']) - expected_names))
                    audio = render(inst, params=preset['params'], pressure=.5)
                    peak = float(abs(audio).max())
                    expect(f"preset-{preset['id']}-audio", .003 < peak < 1, peak=peak)
    out.write_text(json.dumps(report, indent=2)+'\n')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--out', type=Path, required=True)
    parser.add_argument('--demo', type=Path)
    args = parser.parse_args()
    run(args.out, args.demo)
