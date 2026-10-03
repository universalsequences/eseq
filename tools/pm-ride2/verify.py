#!/usr/bin/env python3
"""Implementation checks for the compiled PM Ride release 2 DSP (not sonic claims).

- every Character and preset renders finite audio and finite state at
  44.1/48/96 kHz, as one cymbal voice with retriggers landing on the ring
- 128- and 512-frame blocks produce the same audio (process partition)
- every control at its minimum and maximum, and combined extremes, stay finite
  and bounded, including a burst of retriggers on one ringing plate
- with Tracking 1, key C4+n equals C4 tuned n semitones; with Tracking 0 every key is C4
- Character switches on the next strike and never retunes a ringing plate
- Choke damps the plate after the key is released and does nothing while held
- Width keeps the plate's power
Writes verification.json.
"""
import hashlib
import json
from pathlib import Path

import numpy as np

import common as C
from build import DEST, FIRST_KEY, NAME

HERE = Path(__file__).resolve().parent
# Random stochastic layers (wash, click) differ between renders that start
# from different states; deterministic comparisons switch them off.
SILENT_NOISE = {'ring.wash': 0.0, 'stick.click': 0.0, 'output.width': 0.0}
CHARACTERS = 6


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
        for row in range(CHARACTERS):
            for preset in presets:
                # The cymbal, then two more strikes landing on its ring.
                params = preset['params'] | {'cymbal.character': float(row)}
                events = [(0.0, FIRST_KEY, .9), (.013, FIRST_KEY + 5, .7), (.25, FIRST_KEY, 1.0)]
                y, mem = C.play(inst, events, .6, params, release=.4)
                peaks.append(finite(y, mem, f'{sr} character {row} {preset["name"]}'))
            events = [(0.0, FIRST_KEY, .7), (.1, FIRST_KEY + 3, .5)]
            y128, _ = C.play(inst, events, .4, {'cymbal.character': float(row)})
            y512, _ = C.play(big, events, .4, {'cymbal.character': float(row)})
            partition = max(partition, float(np.abs(y128 - y512).max()))
        assert partition < 1e-5, f'block partition changes audio: {partition}'
        extremes = 0
        for name in names:
            spec = inst.params[name]
            for value in [spec['min'], spec['max']]:
                for row in [0, 2, 5]:
                    params = {name: value} if name == 'cymbal.character' else {name: value, 'cymbal.character': float(row)}
                    y, mem = C.play(inst, [(0.0, FIRST_KEY, 1.0), (.02, FIRST_KEY + 7, 1.0)], .3, params, release=.2)
                    finite(y, mem, f'{sr} {name}={value} row {row}')
                    extremes += 1
        for corner in ['min', 'max']:
            params = {n: inst.params[n][corner] for n in names}
            for key in [FIRST_KEY - 24, FIRST_KEY, FIRST_KEY + 24]:
                y, mem = C.play(inst, [(0.0, key, 1.0), (.005, key, 1.0)], .3, params)
                finite(y, mem, f'{sr} all-{corner} row {row}')
                extremes += 1
        # A drum roll on one plate: 32 accented hits in a second never blow up.
        roll = [(i/32, FIRST_KEY, 1.0) for i in range(32)]
        y, mem = C.play(inst, roll, 1.5, {'ring.decay': 3.0, 'stick.hardness': 1.0, 'cymbal.bell': 3.0})
        roll_peak = finite(y, mem, f'{sr} roll')
        # Tracking 1: key C4+n is C4 tuned n semitones (up to float32 rounding
        # of the pitch input's semitone conversion); Tracking 0: every key is C4.
        tracking, fixed_keys = 0.0, 0.0
        for n in [-12, -5, 7, 12]:
            moved, _ = C.play(inst, [(0.0, FIRST_KEY + n, .8)], .2, SILENT_NOISE | {'cymbal.tracking': 1.0})
            tuned, _ = C.play(inst, [(0.0, FIRST_KEY, .8)], .2, SILENT_NOISE | {'cymbal.tune': float(n)})
            tracking = max(tracking, float(np.abs(moved - tuned).max()/np.abs(tuned).max()))
            fixed, _ = C.play(inst, [(0.0, FIRST_KEY + n, .8)], .2, SILENT_NOISE)
            plain, _ = C.play(inst, [(0.0, FIRST_KEY, .8)], .2, SILENT_NOISE)
            fixed_keys = max(fixed_keys, float(np.abs(fixed - plain).max()))
        assert tracking < 2e-3 and fixed_keys == 0.0, (tracking, fixed_keys)
        # Character is read on the strike: changing it mid-ring leaves the ring alone.
        ring, _ = C.play(inst, [(0.0, FIRST_KEY, .8)], .3, SILENT_NOISE)
        mem = inst.fresh_memory()
        for k, v in SILENT_NOISE.items():
            mem[inst.params[k]['cellId']] = v
        first, mem = C.play(inst, [(0.0, FIRST_KEY, .8)], .15, mem=mem)
        mem[inst.params['cymbal.character']['cellId']] = 5.0
        rest, _ = C.play(inst, [], .15, mem=mem)
        switch = float(np.abs(np.concatenate([first, rest]) - ring).max())
        assert switch < 1e-4, switch
        # Choke: no effect while held, a fast decay once released.
        held, _ = C.play(inst, [(0.0, FIRST_KEY, 1.0)], 1.0, SILENT_NOISE | {'ring.choke': 1.0})
        free, _ = C.play(inst, [(0.0, FIRST_KEY, 1.0)], 1.0, SILENT_NOISE)
        choked, _ = C.play(inst, [(0.0, FIRST_KEY, 1.0)], 1.0, SILENT_NOISE | {'ring.choke': 1.0}, release=.2)
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
                               'tracking_vs_tune_max_relative': tracking, 'untracked_keys_max_abs': fixed_keys, 'character_switch_mid_ring_max_abs': switch, 'choke_db_300ms_after_release': round(choke_db, 1)}
        print(sr, report['rates'][sr], flush=True)
    (HERE/'verification.json').write_text(json.dumps(report, indent=1) + '\n')


if __name__ == '__main__':
    main()
