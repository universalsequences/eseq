#!/usr/bin/env python3
"""Snapshot factory instruments and compare native renders across compilers.

Each instrument runs in a separate process. Snapshots include expanded factory
macros and instrument assets; candidate runs consume exactly the same snapshot.
Baseline waveforms are retained, not just peak/RMS summaries.
"""
import argparse
import ctypes as C
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys

import numpy as np

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'tools/audition'))
from audition import Instrument


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def modulation_changes(inst):
    """Identical sample-timed input, route changes and release for both builds.

    The host's LFO/sequencer is outside this harness; these are actual native
    modulation inputs and assignment cells, not block-rate parameter ramps.
    """
    count = 8192
    events = {0: 0, 1021: 1, 3077: 0, 5101: 1, 6143: 0}
    chunks = [1, 7, 64, 511, 128]
    state = inst.fresh_memory()
    active = [p['cellId'] for name, p in inst.params.items()
              if name.startswith('__mod__') and name.endswith('__active')]
    for name, p in inst.params.items():
        if name.startswith('__mod__') and name.endswith('__depth__slot1'):
            state[p['cellId']] = .02 * (p['max'] - p['min'])
    audio = np.zeros((count, inst.n_out), np.float32)
    ptr = C.POINTER(C.c_float)
    def pointers(arrays):
        return (ptr * len(arrays))(*[a.ctypes.data_as(ptr) for a in arrays])
    pos = call = 0
    while pos < count:
        if pos in events:
            state[active] = events[pos]
        stop = min(frame for frame in [*events, count] if frame > pos)
        frames = min(chunks[call % len(chunks)], stop - pos)
        ins = np.zeros((inst.n_in, inst.max_frames), np.float32)
        outs = np.zeros((inst.n_out, inst.max_frames), np.float32)
        indices = np.arange(pos, pos + frames)
        for name, values in [('pitch', 220), ('velocity', .8),
                             ('gate', indices < 6003),
                             ('trigger', np.isin(indices, [0, 2051])),
                             ('mod1', .25 * np.sin(indices * (2 * np.pi * 173 / 48000)))]:
            if name in inst.inputs:
                ins[inst.inputs[name], :frames] = values
        inst.process_fn(pointers(ins), pointers(outs), frames,
                        state.ctypes.data_as(C.c_void_p), C.byref(inst.context), None)
        audio[pos:pos + frames] = outs[:, :frames].T
        pos += frames
        call += 1
    return audio[:, 0] if inst.n_out == 1 else audio


def snapshot(out):
    catalog = out / 'catalog.json'
    if catalog.exists():
        return json.loads(catalog.read_text())
    entries = []
    for source in sorted((ROOT / 'content/instruments').rglob('dsp.lisp')):
        relative = source.parent.relative_to(ROOT / 'content/instruments')
        dest = out / 'sources' / relative
        shutil.copytree(source.parent, dest)
        imported = set()
        def expand(text):
            def resolve(match):
                name = match[1]
                if name in imported:
                    return ''
                imported.add(name)
                return expand((ROOT / 'content/defmacros' / name / 'macro.lisp').read_text())
            return re.sub(r'\(use-defmacro ([\w-]+)\)', resolve, text)
        (dest / 'dsp.lisp').write_text(expand(source.read_text()))
        bank = source.parent.with_suffix('.presets')
        presets = json.loads(bank.read_text())['presets'] if bank.exists() else []
        entries.append({'name': str(relative), 'source_sha256': sha(source),
                        'expanded_sha256': sha(dest / 'dsp.lisp'), 'presets': presets})
    catalog.write_text(json.dumps(entries, indent=2) + '\n')
    return entries


def worker(args, entry):
    out = args.output / args.phase / entry['name']
    out.mkdir(parents=True, exist_ok=True)
    source = args.output / 'sources' / entry['name'] / 'dsp.lisp'
    assert sha(source) == entry['expanded_sha256']
    inst = Instrument(source, compiler=args.compiler, sample_rate=48000, max_frames=512,
                      toolchain_root=str(ROOT / 'crates/sequencer/tools/dgen-toolchain'))
    check = subprocess.run([sys.executable, str(ROOT / 'tools/audition/check_fusion.py'),
                            str(Path(inst.build_dir) / 'patch.c')], capture_output=True, text=True)
    if check.returncode:
        raise RuntimeError(check.stdout + check.stderr)
    report = {'name': entry['name'], 'compiler_sha256': sha(args.compiler),
              'build': inst.build_dir, 'cases': [], 'failures': []}
    cases = [{'name': 'Default', 'params': {}}] + entry['presets']
    recordings = {}
    baseline = (np.load(args.output / 'baseline' / entry['name'] / 'audio.npz')
                if args.phase == 'candidate' else None)
    def record(key, audio, **metadata):
        row = {'key': key, **metadata,
               'peak': float(np.max(abs(audio))), 'finite': bool(np.isfinite(audio).all())}
        if not row['finite']:
            report['failures'].append(key + ': nonfinite audio')
        if baseline is None:
            recordings[key] = audio
        else:
            before = baseline[key]
            delta = audio.astype(float) - before
            row['exact'] = bool(np.array_equal(audio, before))
            row['peak_error'] = float(abs(delta).max())
            row['nrmse'] = float(np.linalg.norm(delta) / max(np.linalg.norm(before.astype(float)), 1e-30))
            if row['peak_error'] > 2e-5 or row['nrmse'] > 1e-4:
                report['failures'].append(key + ': waveform difference')
                recordings[key] = audio
        report['cases'].append(row)

    for preset_index, preset in enumerate(cases):
        stored = preset.get('params', {})
        params = {}
        used = set()
        # Match instrument_probe: canonical identity, then an unambiguous
        # legacy display name; clamp stored values to the declared range.
        for name, param in inst.params.items():
            alias = param.get('displayName', name)
            unique = sum(p.get('displayName', n) == alias for n, p in inst.params.items()) == 1
            key = name if name in stored else alias if unique and alias in stored else None
            if key is not None:
                params[name] = min(param['max'], max(param['min'], stored[key]))
                used.add(key)
        ignored = sorted(set(stored) - used)
        for note, velocity in [(48, .4), (60, .8), (84, 1.)]:
            effective_note = note + preset.get('base_note_offset', 0)
            audio, _ = inst.render(1.5, pitch=440 * 2 ** ((effective_note - 69) / 12),
                                   vel=velocity, params=params, gate_off=.903, retrig=[.417])
            key = f'p{preset_index}_n{note}'
            record(key, audio, preset=preset['name'], note=note, ignored_preset_fields=ignored)
    record('modulation_changes', modulation_changes(inst))
    if recordings:
        np.savez_compressed(out / 'audio.npz', **recordings)
    else:
        (out / 'audio.npz').unlink(missing_ok=True)
    (out / 'results.json').write_text(json.dumps(report, indent=2) + '\n')
    print(entry['name'], len(report['cases']), 'renders;', len(report['failures']), 'failures', flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--phase', choices=['baseline', 'candidate'], required=True)
    parser.add_argument('--compiler', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--worker', type=int)
    args = parser.parse_args()
    args.compiler = args.compiler.resolve()
    args.output = args.output.resolve()
    args.output.mkdir(parents=True, exist_ok=True)
    catalog = snapshot(args.output)
    if args.worker is not None:
        worker(args, catalog[args.worker])
        return
    summary = {'phase': args.phase, 'compiler_sha256': sha(args.compiler),
               'instruments': len(catalog), 'results': [], 'errors': [],
               'scope': 'Native instrument DSP; host effects and preset LFO programs are not rendered. '
                        'Modulation inputs and assignment changes are exercised separately.'}
    for index, entry in enumerate(catalog):
        dest = args.output / args.phase / entry['name']
        dest.mkdir(parents=True, exist_ok=True)
        command = [sys.executable, str(Path(__file__).resolve()), '--phase', args.phase,
                   '--compiler', str(args.compiler), '--output', str(args.output), '--worker', str(index)]
        try:
            with (dest / 'run.log').open('w') as log:
                result = subprocess.run(command, stdout=log, stderr=subprocess.STDOUT, timeout=180)
            if result.returncode:
                raise RuntimeError(f'exit {result.returncode}; see {dest / "run.log"}')
            row = json.loads((dest / 'results.json').read_text())
            summary['results'].append(row)
            print(entry['name'], len(row['cases']), 'renders;', len(row['failures']), 'failures', flush=True)
        except (RuntimeError, subprocess.TimeoutExpired) as error:
            summary['errors'].append({'name': entry['name'], 'error': str(error)})
            print('FAILED', entry['name'], error, flush=True)
        (args.output / f'{args.phase}.json').write_text(json.dumps(summary, indent=2) + '\n')
    if summary['errors'] or any(row['failures'] for row in summary['results']):
        raise SystemExit(1)


if __name__ == '__main__':
    main()
