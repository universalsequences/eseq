#!/usr/bin/env python3
"""Paired native-kernel timing and phase-aligned audio against a git baseline.

Compilation, Python, allocation and file I/O are outside the timed region.
The small C driver calls the unchanged production ABI with regular retriggers.
Results describe a single voice on this machine, not full-project CPU savings.
"""
import argparse
import ctypes as C
import hashlib
import json
import platform
import subprocess
from pathlib import Path

import numpy as np
import soundfile as sf

from common import instrument
from families import FAMILIES, FACTORY, HERE, ROOT


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def timer(output):
    library = output/'benchmark.dylib'
    subprocess.run(['cc', '-O2', '-dynamiclib' if platform.system() == 'Darwin' else '-shared',
                    '-fPIC', str(HERE/'benchmark.c'), '-o', str(library)], check=True)
    lib = C.CDLL(str(library))
    run = lib.run
    run.restype = C.c_double
    run.argtypes = [C.c_void_p]*3+[C.c_uint32]+[C.c_void_p]*3+[C.c_uint32]

    def measure(inst, key, velocity, params, seconds):
        frames = inst.max_frames
        inputs = np.zeros((inst.n_in, frames), np.float32)
        outputs = np.zeros((inst.n_out, frames), np.float32)
        state = inst.fresh_memory()
        for name, value in params.items():
            state[inst.params[name]['cellId']] = value
        for name, value in [('pitch', 440*2**((key-69)/12)), ('velocity', velocity), ('gate', 1)]:
            inputs[inst.inputs[name]] = value
        def pointers(arrays):
            return (C.c_void_p*len(arrays))(*[a.ctypes.data for a in arrays])
        blocks = round(seconds*inst.sample_rate/frames)
        elapsed = run(C.cast(inst.process_fn, C.c_void_p), pointers(inputs), pointers(outputs),
                      frames, state.ctypes.data, C.byref(inst.context),
                      inputs[inst.inputs['trigger']].ctypes.data, blocks)
        assert np.isfinite(state).all() and np.isfinite(outputs).all()
        return elapsed/blocks*1e6
    return measure


def difference(a, b, sr):
    energy = float(np.sum(a.astype(float)**2))
    error = float(np.sum((a.astype(float)-b)**2))
    windows = []
    for start, end in [(0, .025), (.025, .1), (.1, .4), (.4, 1), (1, 2), (2, 3)]:
        x, y = a[int(start*sr):int(end*sr)], b[int(start*sr):int(end*sr)]
        xr, yr = np.sqrt(np.mean(x.astype(float)**2)), np.sqrt(np.mean(y.astype(float)**2))
        windows.append({'seconds': [start, end], 'baseline_rms': float(xr),
                        'optimized_rms': float(yr), 'level_change_db': float(20*np.log10(max(yr, 1e-20)/max(xr, 1e-20)))})
    return {'normalized_rms_difference': float(np.sqrt(error/max(energy, 1e-30))),
            'correlation': float(np.sum(a.astype(float)*b)/np.sqrt(max(energy*np.sum(b.astype(float)**2), 1e-30))),
            'windows': windows}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--baseline', default='7e439b47')
    parser.add_argument('--repeats', type=int, default=7)
    parser.add_argument('families', nargs='*')
    args = parser.parse_args()
    commit = subprocess.check_output(['git', 'rev-parse', args.baseline], cwd=ROOT, text=True).strip()
    output = HERE/'output/performance'
    output.mkdir(parents=True, exist_ok=True)
    measure = timer(output)
    report = {'baseline_commit': commit, 'platform': platform.platform(), 'sample_rate': 48000,
              'block_frames': 128, 'repeats': args.repeats, 'timing': 'Native production ABI; paired alternating order; median microseconds per block; fresh state; periodic retriggers; no compilation or Python in timed region.',
              'audio_note': 'Phase-aligned waveform differences and raw window levels are not perceptual similarity percentages.', 'instruments': []}
    if args.families and (HERE/'performance.json').exists():
        previous = json.loads((HERE/'performance.json').read_text())
        for field in ['baseline_commit', 'platform', 'sample_rate', 'block_frames', 'repeats']:
            assert report[field] == previous[field], f'Cannot merge different benchmark {field}'
        selected_names = {'PM Saron' if slug == 'saron' else FAMILIES[slug].name for slug in args.families}
        report['instruments'] = [r for r in previous['instruments'] if r['instrument'] not in selected_names]
        for row in report['instruments']:
            assert row['source_sha256'] == digest(FACTORY/row['instrument']/'dsp.lisp'), 'Stale benchmark source'
    medley = []
    for slug in args.families or ['saron', *FAMILIES]:
        name = 'PM Saron' if slug == 'saron' else FAMILIES[slug].name
        keys = [74, 75, 77, 80, 81, 82, 84] if slug == 'saron' else [u[1] for u in FAMILIES[slug].units]
        velocities = [.2, .4, .6, .8, 1] if slug == 'saron' else [.6, .8, 1]
        source = FACTORY/name/'dsp.lisp'
        baseline = output/slug/'baseline.lisp'
        baseline.parent.mkdir(exist_ok=True)
        baseline.write_bytes(subprocess.check_output(['git', 'show', f'{commit}:{source.relative_to(ROOT)}'], cwd=ROOT))
        old, new = instrument(baseline), instrument(source)
        presets = json.loads((FACTORY/(name+'.presets')).read_text())['presets']
        cases = [('reference middle', keys[len(keys)//2], .8, {}),
                 ('quiet low', keys[0], velocities[0], {}), ('hard high', keys[-1], 1., {}),
                 ('open middle', keys[len(keys)//2], .8, presets[1]['params']),
                 ('harmonic middle', keys[len(keys)//2], .8, presets[4]['params'])]
        timings = []
        for label, key, velocity, params in cases:
            for inst in [old, new]:
                measure(inst, key, velocity, params, 1)
            values = [[], []]
            for repeat in range(args.repeats):
                for index in ([0, 1] if repeat % 2 == 0 else [1, 0]):
                    values[index].append(measure([old, new][index], key, velocity, params, 6))
            before, after = map(float, np.median(values, axis=1))
            timings.append({'case': label, 'baseline_us': before, 'optimized_us': after,
                            'speedup': before/after, 'cpu_reduction_percent': 100*(1-after/before),
                            'raw_us': values})
        audio = []
        rendered_cases = [(f'reference {key}/{velocity}', key, velocity, {}) for key in keys for velocity in velocities]
        rendered_cases += [(f'preset {p["id"]}/{key}', key, .8, p['params'])
                           for p in presets for key in sorted(set([keys[0], keys[len(keys)//2], keys[-1]]))]
        worst, pair = -1, None
        for label, key, velocity, params in rendered_cases:
            kw = dict(pitch=440*2**((key-69)/12), vel=velocity, params=params, gate_off=2)
            a, _ = old.render(3, **kw)
            b, _ = new.render(3, **kw)
            result = difference(a, b, 48000)
            audio.append({'case': label, **result})
            if label.startswith('reference') and result['normalized_rms_difference'] > worst:
                worst, pair = result['normalized_rms_difference'], (a, b)
        pause = np.zeros((24000, 2))
        ab = np.concatenate([pair[0], pause, pair[1], pause])
        sf.write(output/slug/'largest-reference-difference-ab.wav', ab, 48000, subtype='FLOAT')
        medley.append(ab)
        row = {'instrument': name, 'source_sha256': digest(source), 'baseline_source_sha256': digest(baseline),
               'compiler_sha256': new.compiler_sha256, 'timings': timings, 'audio': audio}
        report['instruments'].append(row)
        (HERE/'performance.json').write_text(json.dumps(report, indent=2)+'\n')
        print(slug, 'minimum reduction', round(min(t['cpu_reduction_percent'] for t in timings), 2),
              '%; worst reference waveform NRMSE', round(worst, 4), flush=True)
    order = {'PM Saron': 'saron'} | {f.name: slug for slug, f in FAMILIES.items()}
    report['instruments'].sort(key=lambda r: list(order).index(r['instrument']))
    (HERE/'performance.json').write_text(json.dumps(report, indent=2)+'\n')
    medley = [sf.read(output/order[r['instrument']]/'largest-reference-difference-ab.wav', dtype='float32')[0]
              for r in report['instruments']]
    sf.write(output/'all-six-ab.wav', np.concatenate(medley), 48000, subtype='FLOAT')


if __name__ == '__main__':
    main()
