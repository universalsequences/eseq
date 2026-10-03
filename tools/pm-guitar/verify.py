#!/usr/bin/env python3
"""Stability/range checks for PM Nylon Guitar through the production compiler (writes verification.json)."""
import json
import sys
import time
from pathlib import Path

import numpy as np

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
from render import instrument, hz   # noqa: E402

RANGES = {'pluck.take': (0, 1), 'pluck.vel_take': (0, 1), 'pluck.humanize': (0, 1), 'pluck.position': (-0.25, 0.25),
          'pluck.finger': (-1, 1), 'pluck.vel_tone': (0, 1), 'pluck.detail': (0, 1), 'string.decay': (0.2, 4),
          'string.damping': (0, 3), 'string.mute': (0, 1), 'string.release': (0.03, 6), 'string.stiffness': (0, 4),
          'string.tune': (-100, 100), 'string.vibrato': (0, 60), 'string.vib_rate': (0.5, 9), 'body.body': (0, 2),
          'body.resonance': (0, 3), 'fret.buzz': (0, 1), 'fret.spread': (0, 1), 'fret.relief': (0, 3), 'fret.hardness': (0.001, 1), 'fret.alpha': (1, 2.5), 'fret.contact_loss': (0, 1), 'fret.level': (0, 2), 'fret.upper_decay': (0.25, 4), 'body.wood': (800, 16000), 'body.contact': (0, 4), 'output.tone_hz': (800, 20000), 'output.gain': (0, 4)}
NOTES = [28, 40, 52, 64, 76, 88, 100]


def check(y):
    return bool(np.isfinite(y).all()), float(np.abs(y).max())


def main():
    report = {'rates': {}}
    for sr in (44100, 48000, 96000):
        inst = instrument(sr)
        worst, cases, bad = 0.0, 0, []
        for note in NOTES:
            for vel in (0.05, 0.6, 1.0):
                ok, pk = check(inst.render(seconds=0.6, pitch=hz(note), vel=vel, retrig=[0.2, 0.21], gate_off=0.4)[0])
                cases += 1; worst = max(worst, pk)
                if not ok: bad.append((note, vel))
        for name, (lo, hi) in RANGES.items():
            for v in (lo, hi):
                for note in (40, 64, 88):
                    params = {name: v} | ({} if name == 'output.gain' else {'output.gain': 1.0})
                    ok, pk = check(inst.render(seconds=0.4, pitch=hz(note), vel=1.0, params=params, gate_off=0.3)[0])
                    cases += 1
                    if name != 'output.gain':
                        worst = max(worst, pk)
                    if not ok: bad.append((name, v, note))
        for corner in ('lo', 'hi'):
            params = {n: (r[0] if corner == 'lo' else r[1]) for n, r in RANGES.items() if n != 'output.gain'}
            for note in (40, 64, 88):
                ok, pk = check(inst.render(seconds=0.4, pitch=hz(note), vel=1.0, params=params)[0])
                cases += 1; worst = max(worst, pk)
                if not ok: bad.append((corner, note))
        # tails must die: long decay/release, every buzz setting, top notes
        tails = []
        for note in (70, 76, 82, 88, 94):
            for p in ({'string.decay': 4, 'string.release': 6, 'string.damping': 0.3},
                      {'string.decay': 4, 'string.release': 6, 'fret.buzz': 0.15, 'fret.spread': 1},
                      {'string.decay': 4, 'string.release': 6, 'fret.buzz': 1, 'fret.spread': 1, 'fret.hardness': 1, 'fret.alpha': 2.5},
                      {'string.decay': 2.2, 'string.release': 5.2, 'fret.buzz': 0.15, 'fret.level': 0.09, 'fret.relief': 0.39,
                       'fret.hardness': 0.26, 'fret.alpha': 2.26, 'fret.contact_loss': 0.18, 'fret.upper_decay': 0.8,
                       'string.damping': 0.48, 'string.vibrato': 8}):
                y = inst.render(seconds=6.0, pitch=hz(note), vel=0.8, params=p, gate_off=0.5)[0][:, 0]
                early = np.sqrt(np.mean(y[int(0.3*sr):int(0.5*sr)]**2)) + 1e-12
                late = np.sqrt(np.mean(y[int(5.5*sr):int(6.0*sr)]**2)) + 1e-12
                tails.append(float(20*np.log10(late/early)))
        a = inst.render(seconds=0.5, pitch=hz(52), vel=0.8, retrig=[0.123])[0]
        inst128 = a
        inst512 = instrument(sr, block=512).render(seconds=0.5, pitch=hz(52), vel=0.8, retrig=[0.123])[0]
        block_identical = bool(np.array_equal(inst128, inst512))
        report['rates'][sr] = {'cases': cases, 'non_finite': bad, 'max_peak_gain1': round(worst, 3),
                               'block_128_512_identical': block_identical,
                               'worst_tail_change_db': round(max(tails), 1)}
        print(sr, report['rates'][sr])
    # CPU: one voice, 48k/128, plucks every 0.5 s
    inst = instrument(48000)
    t0 = time.process_time()
    seconds = 20.0
    inst.render(seconds=seconds, pitch=hz(52), vel=0.8, retrig=list(np.arange(0.5, seconds, 0.5)))
    cpu = (time.process_time() - t0)/seconds*100
    report['cpu_percent_one_voice_48k_128'] = round(cpu, 2)
    print('cpu % of one core per voice', round(cpu, 2))
    (HERE/'verification.json').write_text(json.dumps(report, indent=1) + '\n')


if __name__ == '__main__':
    main()
