#!/usr/bin/env python3
"""Validate and time Modal Snare changes against a saved production source.

Use .local/venvs/physical-models/bin/python. Native timing uses the existing
gamelan ABI driver; compilation, Python and allocation are outside timing.
"""
import argparse
import ctypes as C
import hashlib
import importlib.util
import json
import platform
import subprocess
import sys
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parents[2]
FACTORY = ROOT / 'content/instruments/Drums/Modal Snare'
sys.path.insert(0, str(ROOT / 'tools/pm-gamelan'))
gamelan_spec = importlib.util.spec_from_file_location(
    'gamelan_performance', ROOT / 'tools/pm-gamelan/performance.py')
gamelan_performance = importlib.util.module_from_spec(gamelan_spec)
gamelan_spec.loader.exec_module(gamelan_performance)
timer = gamelan_performance.timer
sys.path.insert(0, str(ROOT / 'tools/audition'))
from audition import Instrument

CONTROLS = {'stretch': (.4, 1.6), 'split': (0, 4), 'tilt': (0, 2.5),
            'visc': (0, 2), 'release': (20, 3000), 'release2': (20, 3000),
            'tip': (.03, .45), 'bright': (0, 2.5)}
COEFFICIENTS = ['bat-r', 'r2s', 'r3s', 'r-bat', 'r-res', 'spread',
                'spread1', 'spread2', 'spread3', 'bright-w', 'bright-norm',
                'rb1', 'rb2', 'rb3', 'rr1', 'rr2', 'rr3']
TENSORS = {'bat-r', 'r-bat', 'r-res', 'spread', 'bright-w'}
AUDIO_OUTPUT = '(out (* dcy level-v vel-gain) 1 @name audio)'


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def metrics(a, b, sr):
    a, b = a.astype(float), b.astype(float)
    norm = lambda x: max(float(np.linalg.norm(x)), 1e-30)
    level = lambda x, y: 20 * np.log10(norm(y) / norm(x))
    frequency = np.fft.rfftfreq(len(a), 1 / sr)
    def bands(x):
        power = abs(np.fft.rfft(x)) ** 2
        return np.array([power[(frequency >= lo) & (frequency < hi)].sum()
                         / max(power.sum(), 1e-30) for lo, hi in
                         [(0, 300), (300, 800), (800, 2000), (2000, 8000), (8000, sr)]])
    return dict(nrmse=norm(a-b) / norm(a), max_error=float(abs(a-b).max()),
                level_db=float(level(a, b)),
                early_level_db=float(level(a[:int(sr*.1)], b[:int(sr*.1)])),
                band_energy_fraction_delta=float(abs(bands(a)-bands(b)).max()))


def stream(inst, params, partitions, frames, modulation=None, sample_rates=None):
    """Exercise arbitrary call boundaries and audio-rate modulation inputs.

    Inputs depend on absolute sample positions, not partition boundaries.
    Fresh state includes every tensor and scalar default from the manifest.
    """
    memory = inst.fresh_memory()
    for name, value in params.items():
        memory[inst.params[name]['cellId']] = value
    result = np.zeros((frames, inst.n_out), np.float32)
    sample_rates = sample_rates or {0: inst.sample_rate}
    position = index = 0
    while position < frames:
        count = min(partitions[index % len(partitions)], frames-position)
        inst.context.sample_rate = sample_rates[max(n for n in sample_rates if n <= position)]
        count = min(count, min((n-position for n in sample_rates if n > position), default=count))
        assert 0 < count <= inst.max_frames
        absolute = np.arange(position, position+count)
        # Match the host and audition wrapper's fixed-capacity channel buffers.
        inputs = np.zeros((inst.n_in, inst.max_frames), np.float32)
        outputs = np.zeros((inst.n_out, inst.max_frames), np.float32)
        inputs[inst.inputs['pitch'], :count] = np.where(absolute < frames//2, 220, 440)
        inputs[inst.inputs['velocity'], :count] = .8
        inputs[inst.inputs['gate'], :count] = (absolute < frames*3//4).astype(np.float32)
        inputs[inst.inputs['trigger'], :count] = np.isin(absolute, [0, 17, frames//2, frames-29])
        if modulation is not None:
            inputs[inst.inputs['mod1'], :count] = modulation(absolute)
        pointers = lambda x: (C.c_void_p*len(x))(*[a.ctypes.data for a in x])
        # The audition wrapper declares float**; preserve that ABI type.
        float_pointers = C.POINTER(C.POINTER(C.c_float))
        inst.process_fn(C.cast(pointers(inputs), float_pointers),
                        C.cast(pointers(outputs), float_pointers), count,
                        memory.ctypes.data_as(C.c_void_p), C.byref(inst.context), None)
        result[position:position+count] = outputs[:, :count].T
        position += count
        index += 1
    inst.context.sample_rate = inst.sample_rate
    assert np.isfinite(result).all() and np.isfinite(memory).all()
    return result[:, 0] if inst.n_out == 1 else result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--baseline', type=Path, required=True)
    parser.add_argument('--candidate', type=Path, default=FACTORY / 'dsp.lisp')
    parser.add_argument('--compiler', type=Path, required=True)
    parser.add_argument('--params', type=Path)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--repeats', type=int, default=7)
    parser.add_argument('--no-timing', action='store_true')
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    sources = []
    for label, path in [('baseline', args.baseline), ('candidate', args.candidate)]:
        target = args.output / 'sources' / label / 'dsp.lisp'
        target.parent.mkdir(parents=True, exist_ok=True)
        if target.exists() and target.read_bytes() != path.read_bytes():
            raise ValueError(f'Refusing to overwrite a different saved source: {target}')
        target.write_bytes(path.read_bytes())
        sources.append(target)
    report = dict(platform=platform.platform(), compiler_sha256=digest(args.compiler),
                  source_sha256=[digest(p) for p in sources], coefficients=[],
                  audio=[], streaming=[], timings=[])
    pairs = {}
    def pair(sr=48000, block=128, diagnostic=False):
        key = sr, block, diagnostic
        if key not in pairs:
            items = []
            for source in sources:
                path = source
                if diagnostic:
                    path = source.with_name('coefficients.lisp')
                    text = source.read_text()
                    assert text.count(AUDIO_OUTPUT) == 1
                    outputs = []
                    for channel, name in enumerate(COEFFICIENTS, 1):
                        value = f'(sum {name})' if name in TENSORS else name
                        outputs.append(f'(out {value} {channel} @name coefficient{channel})')
                    path.write_text(text.replace(AUDIO_OUTPUT, '\n'.join(outputs)))
                inst = Instrument(path, compiler=str(args.compiler.resolve()),
                                  sample_rate=sr, max_frames=block,
                                  toolchain_root=str(ROOT / 'crates/sequencer/tools/dgen-toolchain'))
                subprocess.run([sys.executable, str(ROOT / 'tools/audition/check_fusion.py'),
                                str(Path(inst.build_dir) / 'patch.c')], check=True,
                               stdout=subprocess.DEVNULL)
                items.append(inst)
            pairs[key] = items
        return pairs[key]

    # Verify that each input invalidates every affected coefficient on the
    # same sample, including adjacent changes and returns to zero.
    for block in [1, 32]:
        for name, (lo, hi) in CONTROLS.items():
            ramps = {name: [(0, lo), (block/48000, hi), (2*block/48000, lo),
                            (3*block/48000, lo), (4*block/48000, hi)]}
            a, b = [i.render(8*block/48000, ramps=ramps)[0]
                    for i in pair(block=block, diagnostic=True)]
            error = float(np.max(abs(a-b) / np.maximum(abs(a), 1)))
            assert np.isfinite(b).all() and error < 1e-5, (name, block, error)
            report['coefficients'].append(dict(control=name, block=block, error=error))
    print('Coefficient changes match at single-sample resolution', flush=True)
    rates = {0: 48000, 1: 96000, 2: 44100, 9: 48000}
    a, b = [stream(i, {}, [1, 7], 16, sample_rates=rates)
            for i in pair(block=32, diagnostic=True)]
    error = float(np.max(abs(a-b) / np.maximum(abs(a), 1)))
    assert error < 1e-5, ('sample-rate changes', error)
    report['sample_rate_changes'] = dict(rates=rates, error=error)

    presets = json.loads(FACTORY.with_suffix('.presets').read_text())['presets']
    for preset in [dict(name='Default', params={}, base_note_offset=0), *presets]:
        for note in [29, 43, 57, 69, 89]:
            kwargs = dict(pitch=440*2**((note+preset['base_note_offset']-69)/12),
                          vel=.8, params=preset['params'])
            rendered = [i.render(2, **kwargs) for i in pair()]
            assert all(np.isfinite(a).all() for values in rendered for a in values)
            result = metrics(rendered[0][0], rendered[1][0], 48000)
            # Wire contact is chaotic: rounding can decorrelate its waveform.
            # Preserve level and spectral distribution, and report waveform
            # error separately rather than calling it perceptual similarity.
            assert abs(result['level_db']) < .25 and abs(result['early_level_db']) < .25, result
            assert result['band_energy_fraction_delta'] < .04, result
            report['audio'].append(dict(preset=preset['name'], note=note, **result))
        print('Compared', preset['name'], flush=True)

    project = (json.loads(args.params.read_text()) if args.params else
               next(p['params'] for p in presets if p['name'] == 'Jungle S'))
    for sr in [44100, 48000, 96000]:
        a, b = pair(sr=sr, block=512)
        for name in [None, *CONTROLS]:
            params = dict(project)
            modulation = None
            if name:
                lo, hi = CONTROLS[name]
                params[name] = (lo+hi)/2
                params[f'__mod__{name}__active'] = 1
                depth = f'__mod__{name}__depth__slot1'
                params[depth] = .15
                modulation = lambda n: np.sin(n*.031)
            partitions = [1, 7, 12, 63, 128, 3, 512]
            x = stream(a, params, partitions, 4096, modulation)
            y = stream(b, params, partitions, 4096, modulation)
            result = metrics(x, y, sr)
            assert abs(result['level_db']) < .25 and result['band_energy_fraction_delta'] < .04, (name, sr, result)
            # Partition invariance is checked on the candidate separately from
            # old/new numerical differences in the nonlinear wire model.
            regular = stream(b, params, [128], 4096, modulation)
            partition_error = metrics(regular, y, sr)
            assert partition_error['max_error'] < 1e-5, (name, sr, partition_error)
            report['streaming'].append(dict(sample_rate=sr, modulation=name,
                                            partition_max_error=partition_error['max_error'], **result))
        print('Checked irregular streaming and modulation at', sr, flush=True)

    # Finish compilation and validation before native paired measurements.
    for block in [128, 512]:
        pair(block=block)
    if not args.no_timing:
        measure = timer(args.output)
        for block in [128, 512]:
            for note in [29, 43, 69, 89]:
                items = pair(block=block)
                for inst in items:
                    measure(inst, note, 1, project, 1)
                samples = [[], []]
                for repeat in range(args.repeats):
                    for side in ([0, 1] if repeat % 2 == 0 else [1, 0]):
                        samples[side].append(measure(items[side], note, 1, project, 4))
                before, after = map(float, np.median(samples, axis=1))
                row = dict(block=block, note=note, baseline_us=before, candidate_us=after,
                           reduction_percent=100*(1-after/before), samples_us=samples)
                report['timings'].append(row)
                print('Timing', block, note, before, '->', after, flush=True)
    (args.output / 'results.json').write_text(json.dumps(report, indent=2)+'\n')


if __name__ == '__main__':
    main()
