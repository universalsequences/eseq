#!/usr/bin/env python3
"""Compare piano source/compiler pairs, then time the native ABI.

Run after other builds/tests finish. No compilation, allocation or Python is
inside the timed region. This measures one voice, not whole-project CPU.
"""
import argparse
import hashlib
import json
import platform
import subprocess
import sys
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parents[2]
SOURCE = ROOT / 'content/instruments/Physical Models/PM Piano/dsp.lisp'
sys.path.insert(0, str(ROOT / 'tools/pm-gamelan'))
from performance import timer
sys.path.insert(0, str(ROOT / 'tools/audition'))
from audition import Instrument


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--baseline-compiler', type=Path, required=True)
    parser.add_argument('--baseline-source', type=Path, default=SOURCE,
                        help='Saved baseline DSP; defaults to the current piano source')
    parser.add_argument('--compiler', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--params', type=Path, help='Optional named parameter JSON for a project patch')
    parser.add_argument('--repeats', type=int, default=7)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    measure = timer(args.output)
    compilers = [args.baseline_compiler.resolve(), args.compiler.resolve()]
    sources = [args.baseline_source.resolve(), SOURCE]
    report = dict(source_sha256=digest(SOURCE), platform=platform.platform(),
                  baseline_source_sha256=digest(sources[0]),
                  compiler_sha256=[digest(p) for p in compilers], audio=[], timings=[])
    pairs = {}

    def pair(sr=48000, block=128):
        if (sr, block) not in pairs:
            result = []
            for source, compiler in zip(sources, compilers):
                inst = Instrument(source, compiler=str(compiler), sample_rate=sr,
                                  max_frames=block,
                                  toolchain_root=str(ROOT / 'crates/sequencer/tools/dgen-toolchain'))
                subprocess.run([sys.executable, str(ROOT / 'tools/audition/check_fusion.py'),
                                str(Path(inst.build_dir) / 'patch.c')], check=True,
                               stdout=subprocess.DEVNULL)
                result.append(inst)
            pairs[sr, block] = result
        return pairs[sr, block]

    def compare(label, seconds, sr=48000, block=128, **kwargs):
        rendered = [inst.render(seconds, **kwargs) for inst in pair(sr, block)]
        assert all(np.isfinite(v).all() for values in rendered for v in values), label
        a, b = (values[0].astype(np.float64) for values in rendered)
        peak_error = float(abs(a-b).max())
        nrmse = float(np.sqrt(np.sum((a-b)**2) / max(np.sum(a*a), 1e-30)))
        assert peak_error < 1e-4 and nrmse < .001, (label, peak_error, nrmse)
        report['audio'].append(dict(case=label, sample_rate=sr, block=block,
                                    peak_error=peak_error, normalized_rms_error=nrmse))

    presets = json.loads((SOURCE.parent.parent / 'PM Piano.presets').read_text())['presets']
    for preset in presets:
        for note in [21, 36, 60, 84, 108]:
            compare(f"{preset['id']}/{note}", 3.5, pitch=440*2**((note-69)/12),
                    params=preset['params'], gate_off=2.5, retrig=[.37])
        print('Compared preset', preset['id'], flush=True)
    for note in range(21, 109):
        compare(f'key/{note}', .6, pitch=440*2**((note-69)/12), gate_off=.3)
    print('Compared all 88 keys', flush=True)
    for sr in [44100, 48000, 96000]:
        for block in [32, 128, 512]:
            compare(f'automation/{sr}/{block}', 2, sr=sr, block=block, gate_off=.6,
                    retrig=[.35, 1.1], params={'swell.amount': .38, 'damper.release_s': 5},
                    ramps={'swell.amount': [(0, .38), (.4, 1), (1.3, 0)],
                           'string.damping': [(0, 1), (.8, 3)],
                           'tuning.unison': [(0, 1.5), (1, 12)]})
    compare('long bass swell and pedal', 32, pitch=27.5,
            params={'swell.amount': 1, 'swell.length_s': 8, 'swell.tail': 1,
                    'string.decay': 4, 'damper.pedal': 1}, gate_off=10)
    project_params = json.loads(args.params.read_text()) if args.params else None
    if project_params:
        for note in [38, 45, 50, 60]:
            compare(f'project/{note}', 8, pitch=440*2**((note-69)/12),
                    params=project_params, gate_off=1.8, retrig=[2, 4])
    # Finish every compile before measuring. Alternate paired repetitions.
    for block in [128, 512]:
        pair(block=block)
    cases = [('normal', {}), ('reverse', {'swell.amount': 1})]
    if project_params:
        cases.append(('project', project_params))
    for block in [128, 512]:
        for label, params in cases:
            for inst in pair(block=block):
                measure(inst, 50, .8, params, 1)
            samples = [[], []]
            for repeat in range(args.repeats):
                for side in ([0, 1] if repeat % 2 == 0 else [1, 0]):
                    samples[side].append(measure(pair(block=block)[side], 50, .8, params, 4))
            before, after = map(float, np.median(samples, axis=1))
            row = dict(case=label, frames=block, baseline_us=before, candidate_us=after,
                       cpu_reduction_percent=100*(1-after/before), raw_us=samples)
            report['timings'].append(row)
            print(label, block, round(before, 2), '->', round(after, 2), 'us', flush=True)
    report['artifacts'] = {f'{sr}/{block}': [str(i.build_dir) for i in items]
                           for (sr, block), items in pairs.items()}
    (args.output / 'results.json').write_text(json.dumps(report, indent=2)+'\n')
    print('Passed', len(report['audio']), 'audio comparisons', flush=True)


if __name__ == '__main__':
    main()
