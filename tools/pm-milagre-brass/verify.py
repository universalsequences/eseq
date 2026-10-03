#!/usr/bin/env python3
"""Robustness and cost checks for PM Milagre Brass through the production compiler.

* the reference performance and every preset across the playable range
  (A1..A5, tongued and slurred) at 44.1/48/96 kHz: finite audio, peak < 4;
* every control at its min and max, plus all-min / all-max corners;
* 128- and 512-frame blocks bit-identical;
* CPU: one voice playing the phrase, 48 kHz, 128-frame blocks.
Writes verification.json.
"""
import json
import re
import sys
import time
from pathlib import Path

import numpy as np

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
from common import DEST, analysis, instrument  # noqa: E402
import performance as perf  # noqa: E402


def controls(inst):
    src = (DEST/'dsp.lisp').read_text()
    out = {}
    for m in re.finditer(r'\(param (\S+)([^)]*)\)', src):
        name, attrs = m.groups()
        g = re.search(r'@group (\S+)', attrs)
        full = f'{g.group(1)}.{name}' if g else name
        lo = float(re.search(r'@min (\S+)', attrs).group(1))
        hi = float(re.search(r'@max (\S+)', attrs).group(1))
        if full != 'voice_mode':
            out[full] = (lo, hi)
    return out


def melody(inst, params, seconds=2.4):
    """Tongued and slurred notes over the range, via ramps on the pitch input."""
    hz = [55, 110, 220, 330, 440, 660, 880]
    pts = []
    for i, h in enumerate(hz):
        t = i*seconds/len(hz)
        pts += [(t, h), (t + seconds/len(hz) - 1e-3, h)]
    y, _ = inst.render(seconds=seconds, pitch=hz[0], params=params, ramps={'pitch': pts},
                       retrig=[0.0, 0.7, 1.4], gate_off=seconds - 0.3)
    return y


def check(y, label, failures, peaks):
    peak = float(np.abs(y).max())
    peaks.append(peak)
    if not np.isfinite(y).all() or peak >= 4:
        failures.append(f'{label}: finite={np.isfinite(y).all()} peak={peak:.2f}')


def main():
    A = analysis()
    ph = json.loads((HERE/'phrase.json').read_text())
    presets = json.loads((DEST.parent/f'{DEST.name}.presets').read_text())['presets']
    report = dict(rates={}, blocks=None, cpu=None)
    for sr in (44100, 48000, 96000):
        inst = instrument(sr=sr)
        failures, peaks = [], []
        y = perf.render(inst, A['notes'], ph['breath'], tune_steps=ph['tune'])
        check(y, 'phrase', failures, peaks)
        for p in presets:
            check(melody(inst, p['params']), f"preset {p['name']}", failures, peaks)
        ranges = controls(inst)
        for name, (lo, hi) in ranges.items():
            for v in (lo, hi):
                check(melody(inst, {name: v}, 1.2), f'{name}={v}', failures, peaks)
        check(melody(inst, {k: lo for k, (lo, hi) in ranges.items()}, 1.2), 'all-min', failures, peaks)
        check(melody(inst, {k: hi for k, (lo, hi) in ranges.items()}, 1.2), 'all-max', failures, peaks)
        report['rates'][sr] = dict(renders=len(peaks), max_peak=round(max(peaks), 3), failures=failures)
        print(sr, report['rates'][sr])
    # constant controls (offline p-locks and ramps land on block boundaries)
    a, _ = instrument(block=128).render(seconds=1.5, pitch=110.0, retrig=[0.6], gate_off=1.2)
    b, _ = instrument(block=512).render(seconds=1.5, pitch=110.0, retrig=[0.6], gate_off=1.2)
    report['blocks'] = dict(identical=bool(np.array_equal(a, b)), max_diff=float(np.abs(a - b).max()))
    inst = instrument()
    t = time.perf_counter()
    reps = 3
    for _ in range(reps):
        perf.render(inst, A['notes'], ph['breath'], tune_steps=ph['tune'])
    dt = (time.perf_counter() - t)/reps
    report['cpu'] = dict(seconds_audio=perf.STEPS*perf.STEP, seconds_wall=round(dt, 4),
                         percent_core=round(100*dt/(perf.STEPS*perf.STEP), 2),
                         note='includes the Python host loop; an upper bound')
    print(report['blocks'], report['cpu'])
    (HERE/'verification.json').write_text(json.dumps(report, indent=1))
    if any(r['failures'] for r in report['rates'].values()):
        sys.exit('verification failed')


if __name__ == '__main__':
    main()
