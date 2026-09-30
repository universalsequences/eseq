#!/usr/bin/env python3
"""Implementation checks for the compiled PM Tabla DSP (not sonic claims).

- every key and preset renders finite audio and finite state at 44.1/48/96 kHz
- 128- and 512-frame blocks produce the same audio (process partition)
- every control at its minimum and maximum, and combined extremes, stay finite
  and bounded, including retriggers inside a flam
- key k+12n equals key k tuned 12n semitones; octaves beyond the range clamp
Writes verification.json.
"""
import hashlib
import json
import sys
from pathlib import Path

import numpy as np

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import compare as C
from build import DEST, FIRST_KEY, NAME, OCTAVE_RANGE, strokes


def finite(y, mem, label):
    assert np.isfinite(y).all(), f'non-finite audio: {label}'
    assert np.isfinite(mem).all(), f'non-finite state: {label}'
    peak = float(np.abs(y).max())
    assert peak < 4.0, f'unbounded output {peak}: {label}'
    return peak


def main():
    data = json.loads((HERE/'analysis.json').read_text())
    rows = len(strokes(data))
    presets = json.loads((DEST.parent/(NAME + '.presets')).read_text())['presets']
    report = {'source_sha256': hashlib.sha256((DEST/'dsp.lisp').read_bytes()).hexdigest(), 'rates': {}}
    for sr in [44100, 48000, 96000]:
        inst = C.instrument(sr)
        big = C.instrument(sr, block=512)
        report['compiler_sha256'] = inst.compiler_sha256
        peaks, partition = [], 0.0
        for row in range(rows):
            for preset in presets:
                params = {k: v for k, v in preset['params'].items()}
                y, mem = inst.render(seconds=.5, pitch=C.hz(FIRST_KEY + row), vel=.9, params=params,
                                     retrig=[.013, .25])
                peaks.append(finite(y, mem, f'{sr} row {row} {preset["name"]}'))
            y128, _ = inst.render(seconds=.4, pitch=C.hz(FIRST_KEY + row), vel=.7)
            y512, _ = big.render(seconds=.4, pitch=C.hz(FIRST_KEY + row), vel=.7)
            partition = max(partition, float(np.abs(y128 - y512).max()))
        assert partition < 1e-5, f'block partition changes audio: {partition}'
        extremes = 0
        names = [n for n in inst.params if not n.startswith('__mod__')]
        for name in names:
            spec = inst.params[name]
            for value in [spec['min'], spec['max']]:
                for row in [0, rows//2, rows - 1]:
                    y, mem = inst.render(seconds=.3, pitch=C.hz(FIRST_KEY + row), vel=1.0, params={name: value},
                                         retrig=[.02])
                    finite(y, mem, f'{sr} {name}={value} row {row}')
                    extremes += 1
        for corner in ['min', 'max']:
            params = {n: inst.params[n][corner] for n in names}
            for row in range(rows):
                y, mem = inst.render(seconds=.3, pitch=C.hz(FIRST_KEY + row), vel=1.0, params=params, retrig=[.005])
                finite(y, mem, f'{sr} all-{corner} row {row}')
                extremes += 1
        # Every octave plays the same twelve strokes, transposed by whole
        # octaves: key k+12n equals key k with Tune at 12n semitones.
        clamp = 0.0
        for row in range(rows):
            for shift in [-1, 1]:
                moved, _ = inst.render(seconds=.2, pitch=C.hz(FIRST_KEY + row + 12*shift), vel=.8, params={'contact.skin': 0.0})
                tuned, _ = inst.render(seconds=.2, pitch=C.hz(FIRST_KEY + row), vel=.8, params={'head.tune': 12.0*shift, 'contact.skin': 0.0})
                clamp = max(clamp, float(np.abs(moved - tuned).max()))
        assert clamp < 1e-6, clamp
        top, _ = inst.render(seconds=.2, pitch=C.hz(FIRST_KEY + 12*(OCTAVE_RANGE[1] + 2)), vel=.8)
        edge, _ = inst.render(seconds=.2, pitch=C.hz(FIRST_KEY + 12*OCTAVE_RANGE[1]), vel=.8)
        clamp = max(clamp, float(np.abs(top - edge).max()))
        assert clamp < 1e-6, clamp
        report['rates'][sr] = {'renders': len(peaks), 'max_peak': max(peaks), 'block_partition_max_abs': partition,
                               'extreme_renders': extremes, 'octave_repeat_max_abs': clamp}
        print(sr, report['rates'][sr], flush=True)
    (HERE/'verification.json').write_text(json.dumps(report, indent=1) + '\n')


if __name__ == '__main__':
    main()
