#!/usr/bin/env python3
"""Compare gated Digi FM filters with an explicit, saved baseline source."""
import argparse
import ctypes as C
import hashlib
import json
from pathlib import Path

import numpy as np

from performance import modulated_render, native_timer, pointers
from validate import COMPILER, ROOT, TOOLCHAIN, Instrument


def switching(inst, chunks, retrigger, resonance):
    count = 24000
    events = {0: 0, 1031: 1, 7777: 0, 14003: 1, 19999: 0}
    state = inst.fresh_memory()
    for name, value in {'harmonics': 5, 'feedback': .4, 'resonance': resonance,
                        'amp_release_ms': 200}.items():
        state[inst.params[name]['cellId']] = value
    audio = np.zeros((count, inst.n_out), np.float32)
    pos = call = 0
    while pos < count:
        if pos in events:
            state[inst.params['filter_type']['cellId']] = events[pos]
        stop = min(frame for frame in [*events, count] if frame > pos)
        frames = min(chunks[call % len(chunks)], stop - pos)
        ins = np.zeros((inst.n_in, inst.max_frames), np.float32)
        outs = np.zeros((inst.n_out, inst.max_frames), np.float32)
        ins[inst.inputs['pitch']] = 220
        ins[inst.inputs['velocity']] = .8
        ins[inst.inputs['gate'], :frames] = np.arange(pos, pos + frames) < 21003
        if pos == 0 or (retrigger and pos in events):
            ins[inst.inputs['trigger'], 0] = 1
        inst.process_fn(pointers(ins), pointers(outs), frames,
                        state.ctypes.data_as(C.c_void_p), C.byref(inst.context), None)
        audio[pos:pos + frames] = outs[:, :frames].T
        pos += frames
        call += 1
    assert np.isfinite(state).all() and np.isfinite(audio).all()
    assert np.max(abs(audio)) < 2
    return audio


def error(before, after):
    delta = after.astype(float) - before
    return {'peak_error': float(np.max(abs(delta))),
            'nrmse': float(np.linalg.norm(delta) / max(np.linalg.norm(before), 1e-30))}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--baseline', type=Path, required=True)
    parser.add_argument('--candidate', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    instruments = [Instrument(path, compiler=COMPILER, toolchain_root=str(TOOLCHAIN),
                              sample_rate=48000, max_frames=512)
                   for path in [args.baseline, args.candidate]]
    report = {'compiler_sha256': hashlib.sha256(Path(COMPILER).read_bytes()).hexdigest(),
              'sources': [hashlib.sha256(path.read_bytes()).hexdigest()
                          for path in [args.baseline, args.candidate]],
              'builds': [i.build_dir for i in instruments], 'static': [],
              'transitions': [], 'timings': []}
    presets = json.loads((ROOT / 'content/instruments/Synths/Digi FM.presets').read_text())['presets']
    cases = [(f'algorithm {a} harmonics {h}', {'algorithm': a, 'harmonics': h})
             for a in range(1, 9) for h in [-5, 0, 5]]
    cases += [(p['name'], p['params']) for p in presets]
    for name, params in cases:
        for filter_type in [0, 1]:
            for pitch in [130.81278265, 261.6255653, 1046.5022612]:
                settings = {**params, 'filter_type': filter_type}
                a, b = [i.render(.5, pitch=pitch, vel=.8, params=settings,
                                 gate_off=.303, retrig=[.117])[0] for i in instruments]
                e = error(a, b)
                report['static'].append({'case': name, 'filter_type': filter_type,
                                         'pitch': pitch, **e})
                assert e['peak_error'] < 2e-6 and e['nrmse'] < 1e-5, report['static'][-1]
    print(len(report['static']), 'fixed-type comparisons passed', flush=True)
    a, b = [modulated_render(i, [17, 128, 511, 3, 64]) for i in instruments]
    report['audio_rate_modulation'] = error(a, b)
    assert report['audio_rate_modulation']['peak_error'] < 2e-6
    for i in instruments:
        y, _ = i.render(5, gate_off=.03, params={'resonance': 1, 'amp_release_ms': 100})
        assert np.isfinite(y).all() and np.max(abs(y[-4800:])) < 1e-6
    for chunks in [[512], [1, 17, 511, 64, 3]]:
        for retrigger in [False, True]:
            for resonance in [0, 1]:
                a, b = [switching(i, chunks, retrigger, resonance) for i in instruments]
                report['transitions'].append({'chunks': chunks, 'retrigger': retrigger,
                    'resonance': resonance, 'before_peak': float(np.max(abs(a))),
                    'after_peak': float(np.max(abs(b))), **error(a, b)})
    print('Modulation, release and filter switching checks passed', flush=True)
    timer = native_timer(args.output)
    for filter_type in [0, 1]:
        for algorithm in [2, 6]:
            params = {'filter_type': filter_type, 'algorithm': algorithm, 'harmonics': 5}
            samples = [[], []]
            for i in instruments:
                timer(i, params, 1)
            for repeat in range(7):
                for index in [repeat % 2, 1 - repeat % 2]:
                    samples[index].append(timer(instruments[index], params, 3))
            before, after = [float(np.median(s)) for s in samples]
            row = {**params, 'before_us': before, 'after_us': after,
                   'reduction_percent': 100 * (1 - after / before), 'samples_us': samples}
            report['timings'].append(row)
            print(row, flush=True)
    (args.output / 'results.json').write_text(json.dumps(report, indent=2) + '\n')


if __name__ == '__main__':
    main()
