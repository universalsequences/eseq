#!/usr/bin/env python3
"""Compare Digi FM audio to a saved source, then time the native process ABI.

Compilation, Python and allocation are outside the timed region. Timings are
single-voice CPU time, not the application's parallel callback wall time.
"""
import argparse
import ctypes as C
import hashlib
import json
import platform
import subprocess
from pathlib import Path

import numpy as np

from validate import COMPILER, DEST, ROOT, compile_source


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def pointers(arrays):
    ptr = C.POINTER(C.c_float)
    return (ptr * len(arrays))(*[a.ctypes.data_as(ptr) for a in arrays])


def native_timer(output):
    library = output / ('benchmark.dylib' if platform.system() == 'Darwin' else 'benchmark.so')
    subprocess.run(['cc', '-O2', '-dynamiclib' if platform.system() == 'Darwin' else '-shared',
                    '-fPIC', str(ROOT / 'tools/pm-gamelan/benchmark.c'), '-o', str(library)], check=True)
    lib = C.CDLL(str(library))
    run = lib.run
    run.restype = C.c_double
    run.argtypes = [C.c_void_p] * 3 + [C.c_uint32] + [C.c_void_p] * 3 + [C.c_uint32]

    def measure(inst, params, seconds=3):
        frames = inst.max_frames
        inputs = np.zeros((inst.n_in, frames), np.float32)
        outputs = np.zeros((inst.n_out, frames), np.float32)
        state = inst.fresh_memory()
        for name, value in params.items():
            state[inst.params[name]['cellId']] = value
        for name, value in [('pitch', 220), ('velocity', .8), ('gate', 1)]:
            inputs[inst.inputs[name]] = value
        blocks = round(seconds * inst.sample_rate / frames)
        elapsed = run(C.cast(inst.process_fn, C.c_void_p), pointers(inputs), pointers(outputs),
                      frames, state.ctypes.data, C.byref(inst.context),
                      inputs[inst.inputs['trigger']].ctypes.data, blocks)
        assert np.isfinite(state).all() and np.isfinite(outputs).all()
        return elapsed / blocks * 1e6

    return measure


def modulated_render(inst, chunks):
    """Drive the real modulation input with identical sample-timed transitions.

    Starts silent, crosses both harmonic assignments, returns to exact zero on
    retrigger, and changes during release. Irregular partitions force an enabled
    and disabled part of a gate into the same process call.
    """
    count = 8192
    inputs = np.zeros((inst.n_in, count), np.float32)
    inputs[inst.inputs['pitch']] = 220
    inputs[inst.inputs['velocity']] = .8
    inputs[inst.inputs['gate'], :6003] = 1
    inputs[inst.inputs['trigger'], [0, 3077, 4103]] = 1
    modulation = inputs[inst.inputs['mod1']]
    modulation[521:1539] = np.linspace(0, 1, 1018)
    modulation[1539:3077] = np.linspace(1, -1, 1538)
    modulation[3077:4103] = -.6
    modulation[6501:] = .75
    state = inst.fresh_memory()
    for name, value in [('__mod__harmonics__active', 1),
                        ('__mod__harmonics__depth__slot1', 6)]:
        state[inst.params[name]['cellId']] = value
    audio = np.zeros((count, inst.n_out), np.float32)
    position = 0
    call = 0
    while position < count:
        frames = min(chunks[call % len(chunks)], count - position)
        # The SIMD ABI uses padded channel buffers even for a short call.
        ins = np.zeros((inst.n_in, inst.max_frames), np.float32)
        ins[:, :frames] = inputs[:, position:position + frames]
        outs = np.zeros((inst.n_out, inst.max_frames), np.float32)
        inst.process_fn(pointers(ins), pointers(outs), frames,
                        state.ctypes.data_as(C.c_void_p), C.byref(inst.context), None)
        audio[position:position + frames] = outs[:, :frames].T
        position += frames
        call += 1
    assert np.isfinite(state).all() and np.isfinite(audio).all()
    return audio


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--baseline-source', type=Path, required=True)
    parser.add_argument('--candidate-source', type=Path, default=DEST / 'dsp.lisp')
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--repeats', type=int, default=7)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    baseline = args.baseline_source.read_text()
    candidate = args.candidate_source.read_text()
    report = dict(platform=platform.platform(), sample_rate=48000,
                  compiler_sha256=digest(Path(COMPILER)),
                  baseline_source_sha256=digest(args.baseline_source),
                  source_sha256=digest(args.candidate_source), audio=[], timings=[], artifacts={})
    pairs = {}
    for block in [32, 128, 512]:
        pairs[block] = [compile_source(args.output, f'{name}-{block}', source, block=block)
                        for name, source in [('baseline', baseline), ('candidate', candidate)]]
        report['artifacts'][block] = [i.build_dir for i in pairs[block]]
        print('Compiled block size', block, flush=True)

    def compare(label, before, after):
        assert np.isfinite(before).all() and np.isfinite(after).all(), label
        delta = before.astype(float) - after
        error = float(np.max(abs(delta)))
        nrmse = float(np.linalg.norm(delta) / max(np.linalg.norm(before), 1e-30))
        assert error < 1e-5 and nrmse < 1e-4, (label, error, nrmse)
        report['audio'].append(dict(case=label, peak_error=error, normalized_rms_error=nrmse))

    # The existing validator independently checks all routes and harmonic
    # anchors. These comparisons specifically protect the execution gate.
    for algorithm in range(1, 9):
        for harm in [-6, -2.5, 0, .07, 2.5, 6]:
            outputs = [inst.render(.12, pitch=220, gate_off=.081,
                                   params={'algorithm': algorithm, 'harmonics': harm})[0]
                       for inst in pairs[512]]
            compare(f'algorithm {algorithm}, harmonics {harm}', *outputs)
    presets = json.loads((DEST.parent / 'Digi FM.presets').read_text())['presets']
    for preset in presets:
        outputs = [inst.render(.4, pitch=220, gate_off=.213, retrig=[.103],
                               params=preset['params'])[0] for inst in pairs[512]]
        compare('preset ' + preset['name'], *outputs)
    reference = modulated_render(pairs[512][0], [512])
    for block, pair in pairs.items():
        for side, inst in enumerate(pair):
            for chunks in [[block], [1, 7, 16, block, 3, 29]]:
                compare(f'modulation {block}/{side}/{chunks}', reference,
                        modulated_render(inst, chunks))
    print('Passed audio comparisons:', len(report['audio']), flush=True)

    measure = native_timer(args.output)
    for block, pair in pairs.items():
        for harmonics in [0, .07, 5, -5]:
            params = {'harmonics': harmonics}
            for inst in pair:
                measure(inst, params, 1)
            samples = [[], []]
            for repeat in range(args.repeats):
                for side in [repeat % 2, 1 - repeat % 2]:
                    samples[side].append(measure(pair[side], params))
            before, after = map(float, np.median(samples, axis=1))
            row = dict(frames=block, harmonics=harmonics, baseline_us=before,
                       candidate_us=after, reduction_percent=100 * (1 - after / before),
                       raw_us=samples)
            report['timings'].append(row)
            print(json.dumps(row), flush=True)
    (args.output / 'results.json').write_text(json.dumps(report, indent=2) + '\n')


if __name__ == '__main__':
    main()
