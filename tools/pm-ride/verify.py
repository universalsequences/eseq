#!/usr/bin/env python3
"""Implementation checks for the compiled PM Ride Kit DSP (not sonic claims).

- every key and preset renders finite audio and finite state at 44.1/48/96 kHz,
  as one cymbal voice with retriggers on other keys landing on the ring
- 128- and 512-frame blocks produce the same audio (process partition)
- every control at its minimum and maximum, and combined extremes, stay finite
  and bounded, including a burst of retriggers on one ringing plate
- key k+12n equals key k tuned 12n semitones; octaves beyond the range clamp
- Choke damps the plate after the key is released and does nothing while held
- Width keeps the plate's power
Writes verification.json.
"""
import hashlib
import json
from pathlib import Path

import numpy as np

import common as C
from build import DEST, FIRST_KEY, NAME, OCTAVE_RANGE

HERE = Path(__file__).resolve().parent
# Random stochastic layers (wash, click) differ between renders that start
# from different states; deterministic comparisons switch them off.
SILENT_NOISE = {'cymbal.wash': 0.0, 'stick.click': 0.0}


def finite(y, mem, label):
    assert np.isfinite(y).all(), f'non-finite audio: {label}'
    assert np.isfinite(mem).all(), f'non-finite state: {label}'
    peak = float(np.abs(y).max())
    assert peak < 4.0, f'unbounded output {peak}: {label}'
    return peak


def main():
    presets = json.loads((DEST.parent/(NAME + '.presets')).read_text())['presets']
    report = {'source_sha256': hashlib.sha256((DEST/'dsp.lisp').read_bytes()).hexdigest(), 'rates': {}}
    for sr in [44100, 48000, 96000]:
        inst = C.instrument(sr)
        big = C.instrument(sr, block=512)
        report['compiler_sha256'] = inst.compiler_sha256
        names = [n for n in inst.params if not n.startswith('__mod__')]
        peaks, partition = [], 0.0
        for row in range(12):
            for preset in presets:
                # The key, then two other strokes landing on its ring.
                events = [(0.0, FIRST_KEY + row, .9), (.013, FIRST_KEY + (row + 5) % 12, .7), (.25, FIRST_KEY + row, 1.0)]
                y, mem = C.play(inst, events, .6, preset['params'], release=.4)
                peaks.append(finite(y, mem, f'{sr} row {row} {preset["name"]}'))
            y128, _ = C.play(inst, [(0.0, FIRST_KEY + row, .7), (.1, FIRST_KEY + (row + 3) % 12, .5)], .4)
            y512, _ = C.play(big, [(0.0, FIRST_KEY + row, .7), (.1, FIRST_KEY + (row + 3) % 12, .5)], .4)
            partition = max(partition, float(np.abs(y128 - y512).max()))
        assert partition < 1e-5, f'block partition changes audio: {partition}'
        extremes = 0
        for name in names:
            spec = inst.params[name]
            for value in [spec['min'], spec['max']]:
                for row in [0, 6, 11]:
                    y, mem = C.play(inst, [(0.0, FIRST_KEY + row, 1.0), (.02, FIRST_KEY + row, 1.0)], .3, {name: value}, release=.2)
                    finite(y, mem, f'{sr} {name}={value} row {row}')
                    extremes += 1
        for corner in ['min', 'max']:
            params = {n: inst.params[n][corner] for n in names}
            for row in range(12):
                y, mem = C.play(inst, [(0.0, FIRST_KEY + row, 1.0), (.005, FIRST_KEY + row, 1.0)], .3, params)
                finite(y, mem, f'{sr} all-{corner} row {row}')
                extremes += 1
        # A drum roll on one plate: 32 accented hits in a second never blow up.
        roll = [(i/32, FIRST_KEY + [7, 10][i % 2], 1.0) for i in range(32)]
        y, mem = C.play(inst, roll, 1.5, {'cymbal.decay': 3.0, 'stick.hardness': 1.0})
        roll_peak = finite(y, mem, f'{sr} roll')
        # Every octave plays the same strokes, transposed by whole octaves.
        octave = 0.0
        for row in range(12):
            for shift in [-1, 1]:
                moved, _ = C.play(inst, [(0.0, FIRST_KEY + row + 12*shift, .8)], .2, SILENT_NOISE)
                tuned, _ = C.play(inst, [(0.0, FIRST_KEY + row, .8)], .2, SILENT_NOISE | {'cymbal.tune': 12.0*shift})
                octave = max(octave, float(np.abs(moved - tuned).max()))
        assert octave < 1e-5, octave
        top, _ = C.play(inst, [(0.0, FIRST_KEY + 12*(OCTAVE_RANGE[1] + 2), .8)], .2, SILENT_NOISE)
        edge, _ = C.play(inst, [(0.0, FIRST_KEY + 12*OCTAVE_RANGE[1], .8)], .2, SILENT_NOISE)
        octave = max(octave, float(np.abs(top - edge).max()))
        assert octave < 1e-5, octave
        # Choke: no effect while held, a fast decay once released.
        held, _ = C.play(inst, [(0.0, FIRST_KEY, 1.0)], 1.0, SILENT_NOISE | {'cymbal.choke': 1.0})
        free, _ = C.play(inst, [(0.0, FIRST_KEY, 1.0)], 1.0, SILENT_NOISE)
        choked, _ = C.play(inst, [(0.0, FIRST_KEY, 1.0)], 1.0, SILENT_NOISE | {'cymbal.choke': 1.0}, release=.2)
        assert np.abs(held - free).max() < 1e-6
        tail = slice(int(.5*sr), int(.6*sr))
        choke_db = float(10*np.log10(np.mean(choked[tail]**2)/np.mean(free[tail]**2)))
        assert choke_db < -40, choke_db
        # Width moves modes across the field without changing the plate's
        # power; it also guards the output against event-clock scheduling
        # (an event-held factor left the output alive on control ticks only).
        mono, _ = C.play(inst, [(0.0, FIRST_KEY, 1.0)], .5, SILENT_NOISE)
        wide, _ = C.play(inst, [(0.0, FIRST_KEY, 1.0)], .5, SILENT_NOISE | {'output.width': 1.0})
        width_db = float(10*np.log10(np.mean(wide**2)/np.mean(mono**2)))
        assert abs(width_db) < 3.0, width_db     # normalized on average over modes; one key deviates ~1.7 dB
        report['rates'][sr] = {'width_power_change_db': round(width_db, 2), 'renders': len(peaks), 'max_peak': max(peaks), 'block_partition_max_abs': partition,
                               'extreme_renders': extremes, 'roll_peak': roll_peak,
                               'octave_repeat_max_abs': octave, 'choke_db_300ms_after_release': round(choke_db, 1)}
        print(sr, report['rates'][sr], flush=True)
    (HERE/'verification.json').write_text(json.dumps(report, indent=1) + '\n')


if __name__ == '__main__':
    main()
