#!/usr/bin/env python3
"""Native single-voice CPU time of PM Bongos next to the gamelan models.

Uses the gamelan benchmark harness: 48 kHz, two strikes per second, the
production compiler/ABI, alternating repetitions. Writes performance.json.
"""
import json
import platform
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
sys.path.insert(0, str(ROOT/'tools/pm-gamelan'))
from common import instrument
from performance import timer

MODELS = {'PM Bongos': ROOT/'content/instruments/Drums/PM Bongos/dsp.lisp'}
for name in ['PM Kethuk', 'PM Kempyang', 'PM Bonang', 'PM Saron']:
    MODELS[name] = ROOT/'content/instruments/Physical Models'/name/'dsp.lisp'


def main():
    out = ROOT/'.local/pm-bongos/perf'
    out.mkdir(parents=True, exist_ok=True)
    measure = timer(out)
    report = {'platform': platform.platform(), 'sample_rate': 48000, 'unit': 'microseconds per block (one voice)', 'blocks': {}}
    for block in [128, 512]:
        insts = {name: instrument(path, block=block) for name, path in MODELS.items()}
        times = {name: [] for name in insts}
        for _ in range(7):
            for name, inst in insts.items():
                key = 62 if name == 'PM Bongos' else 72 if name in ('PM Bonang',) else 60
                times[name].append(measure(inst, key, .8, {}, 4.0))
        budget = block/48000*1e6
        report['blocks'][block] = {name: {'median_us': sorted(t)[3], 'percent_of_one_core': round(100*sorted(t)[3]/budget, 3)}
                                   for name, t in times.items()}
        for name, row in report['blocks'][block].items():
            print(f'{block:4d} {name:12s} {row["median_us"]:8.2f} us  {row["percent_of_one_core"]:6.2f}% core')
    (HERE/'performance.json').write_text(json.dumps(report, indent=1) + '\n')


if __name__ == '__main__':
    main()
