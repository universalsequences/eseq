#!/usr/bin/env python3
"""Compare current physical-model sources and compilers using the native ABI.

Run serially after builds finish. Compilation, allocation, Python and file I/O
are outside timing. Both sides use identical sample-accurate retriggers.
"""
import argparse
import hashlib
import json
import os
import platform
import sys
from pathlib import Path

import numpy as np
import soundfile as sf

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT/'tools/pm-gamelan'))
from common import instrument
from performance import difference, timer


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser()
    for field in ['baseline', 'candidate', 'baseline-compiler', 'compiler', 'output']:
        parser.add_argument('--'+field, type=Path, required=True)
    parser.add_argument('--repeats', type=int, default=7)
    parser.add_argument('--blocks', type=int, nargs='+', default=[128, 512])
    parser.add_argument('names', nargs='*')
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    measure = timer(args.output)
    report = dict(platform=platform.platform(), sample_rate=48000, repeats=args.repeats,
        baseline_compiler_sha256=digest(args.baseline_compiler),
        compiler_sha256=digest(args.compiler), timing='Native process CPU time; alternating paired runs; two strikes per second; one voice.',
        audio_note='Phase-aligned waveform error is not a perceptual similarity percentage.', instruments=[])
    names = args.names or sorted(p.name for p in args.candidate.iterdir() if (p/'dsp.lisp').exists())
    # Compile everything before collecting CPU measurements.
    pairs = {}
    for name in names:
        for block in args.blocks:
            pair = []
            for folder, compiler in [(args.baseline, args.baseline_compiler), (args.candidate, args.compiler)]:
                os.environ['ESEQ_DGENLISP_TOOL'] = str(compiler.resolve())
                pair.append(instrument(folder/name/'dsp.lisp', block=block))
            pairs[name, block] = pair
        print('Compiled', name, flush=True)
    for name in names:
        row = dict(instrument=name, source_sha256=digest(args.candidate/name/'dsp.lisp'),
            baseline_source_sha256=digest(args.baseline/name/'dsp.lisp'), timings=[], audio=[])
        bank = json.loads((args.candidate/(name+'.presets')).read_text())['presets']
        for block in args.blocks:
            old, new = pairs[name, block]
            # Cover default, a quieter low note, and a wide/darker factory preset.
            settings = [(60, .8, {}), (48, .4, {}), (84, 1., bank[-1]['params'])]
            for note, velocity, params in settings:
                samples = [[], []]
                for repeat in range(args.repeats):
                    for side in ([0, 1] if repeat % 2 == 0 else [1, 0]):
                        samples[side].append(measure([old, new][side], note, velocity, params, 3))
                before, after = np.median(samples, axis=1)
                row['timings'].append(dict(block=block, note=note, velocity=velocity,
                    params=params, baseline_us=float(before), candidate_us=float(after),
                    speedup=float(before/after), samples_us=samples))
        old, new = pairs[name, args.blocks[0]]
        worst = -1
        for preset in bank:
            for note, velocity in [(48, .4), (60, .8), (84, 1.)]:
                kwargs = dict(pitch=440*2**((note-69)/12), vel=velocity, params=preset['params'], gate_off=2)
                a, state_a = old.render(3, **kwargs)
                b, state_b = new.render(3, **kwargs)
                assert all(np.isfinite(value).all() for value in [a, b, state_a, state_b]), (name, preset['id'], note)
                assert max(abs(a).max(), abs(b).max()) < 16, (name, preset['id'], note)
                metric = difference(a, b, 48000)
                row['audio'].append(dict(preset=preset['id'], note=note, velocity=velocity, **metric))
                if metric['normalized_rms_difference'] > worst:
                    worst = metric['normalized_rms_difference']
                    sf.write(args.output/(name+'-largest-difference-ab.wav'),
                        np.concatenate([a, np.zeros((12000, a.shape[1])), b]), 48000, subtype='FLOAT')
        report['instruments'].append(row)
        (args.output/'results.json').write_text(json.dumps(report, indent=2)+'\n')
        speedups = [r['speedup'] for r in row['timings']]
        print(name, 'speedup range', round(min(speedups), 2), round(max(speedups), 2),
            'worst preset waveform NRMSE', round(worst, 7), flush=True)


if __name__ == '__main__':
    main()
