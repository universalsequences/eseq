#!/usr/bin/env python3
"""Check the staged production instruments, including event timing and CPU."""
import argparse
import json
import platform

import numpy as np

from common import FACTORY, HERE, NAMES, digest, instrument
from engine import PARAMS
from runtime import stream, timer

STAGED = HERE/'output/staging/content/instruments/Physical Models'


def finite(y, state, label):
    assert np.isfinite(y).all() and np.isfinite(state).all(), label
    peak = float(abs(y).max())
    assert peak < 16, (label, peak)
    return peak


def signal_checks(inst, slug):
    result = []
    y, state = stream(inst, hits={137: .8})
    finite(y, state, 'onset')
    assert abs(y[:137]).max() == 0, 'Sound before trigger'
    assert abs(y[137:]).max() > .001, 'Silent strike'
    for block in [1, 17, 64, 127]:
        z, state = stream(inst, block=block, hits={137: .8})
        error = float(abs(y-z).max())
        assert error < 2e-5, ('block partition', block, error)
        result.append({'case': 'block '+str(block), 'max_error': error})
    z, state = stream(inst, hits={137: .8}, gate_only=True)
    assert abs(y-z).max() < 2e-5, 'Gate-only onset differs'
    for hits in [{}, {137: 0}]:
        z, state = stream(inst, hits=hits)
        assert abs(z).max() == 0, 'Idle/zero-velocity excitation'
    params = PARAMS + ([('openness', 'contact', 0, 0, 1, True)] if slug == 'hihat' else [])
    for name, group, default, lo, hi, _ in params:
        for value in [lo, hi]:
            key = group+'.'+name
            z, state = inst.render(1, vel=.8, params={key: value})
            result.append({'case': key+'='+str(value), 'peak': finite(z, state, key)})
    for character in np.linspace(0, 1, 25):
        z, state = inst.render(.6, vel=1, params={'voicing.character': float(character)})
        result.append({'case': 'voicing '+str(character), 'peak': finite(z, state, 'voicing')})
    events = {15011: {'voicing.character': .9, 'body.size': .6},
              24137: {'body.size': 1.8, 'contact.touch': .6}, 35127: {'contact.touch': 0}}
    z, state = stream(inst, hits={137: .8, 12003: .3, 24257: 1}, events=events)
    finite(z, state, 'automation')
    w, state = stream(inst, block=17, hits={137: .8, 12003: .3, 24257: 1}, events=events)
    assert abs(z-w).max() < 2e-5, 'Automation partition error'
    if slug == 'hihat':
        opened = {'contact.openness': 1}
        a, _ = stream(inst, seconds=2, params=opened, hits={137: .8})
        b, state = stream(inst, seconds=2, params=opened, hits={137: .8},
                          events={12003: {'contact.openness': 0}, 36003: {'contact.openness': 1}})
        finite(b, state, 'close/reopen')
        energy = lambda y: float(np.sum(y[40000:70000].astype(float)**2))
        ratio = energy(b)/max(energy(a), 1e-20)
        result.append({'case': 'close/reopen tail energy ratio', 'value': ratio})
        assert ratio < .25, ('Closing must dissipate the tail before reopening', ratio)
    return result


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--installed', action='store_true')
    parser.add_argument('--cpu', action='store_true')
    parser.add_argument('families', nargs='*')
    args = parser.parse_args()
    output = HERE/'output/verification'
    output.mkdir(parents=True, exist_ok=True)
    factory = FACTORY if args.installed else STAGED
    result = {'platform': platform.platform(), 'sample_rate': 48000,
              'frames_per_block': 128, 'instruments': []}
    for slug in args.families or NAMES:
        path = factory/NAMES[slug]/'dsp.lisp'
        inst = instrument(path)
        cases = signal_checks(inst, slug)
        presets = json.loads((factory/(NAMES[slug]+'.presets')).read_text())['presets']
        for name, value in presets[0]['params'].items():
            assert inst.params[name]['default'] == value, ('Default preset mismatch', name)
        for preset in presets:
            y, state = inst.render(1, vel=.8, params=preset['params'])
            cases.append({'case': 'preset '+preset['id'], 'peak': finite(y, state, preset['id'])})
        for sr in [44100, 96000]:
            other = instrument(path, sr=sr)
            y, state = other.render(1, vel=.8)
            cases.append({'case': 'sample rate '+str(sr), 'peak': finite(y, state, str(sr))})
        result['instruments'].append({'family': slug, 'source_sha256': digest(path),
                                     'compiler_sha256': inst.compiler_sha256, 'signal': cases})
        print(slug, len(cases), 'signal checks passed', flush=True)
    if args.cpu:
        measure = timer(output)
        baseline_names = ['PM Saron', 'PM Slenthem', 'PM Bonang', 'PM Slenthem Slendro', 'PM Kempyang', 'PM Kethuk']
        baselines = [instrument(FACTORY/name/'dsp.lisp') for name in baseline_names]
        kernels = [instrument(factory/NAMES[r['family']]/'dsp.lisp') for r in result['instruments']]
        values = [[] for _ in baselines+kernels]
        for inst in baselines+kernels:
            measure(inst, seconds=1)
        for repeat in range(7):
            indices = range(len(values)) if repeat % 2 == 0 else reversed(range(len(values)))
            for index in indices:
                values[index].append(measure((baselines+kernels)[index]))
        medians = [float(np.median(v)) for v in values]
        result['gamelan_us_per_block'] = dict(zip(baseline_names, medians[:len(baselines)]))
        result['gamelan_raw_us'] = values[:len(baselines)]
        ceiling = max(medians[:len(baselines)])
        for row, us, raw in zip(result['instruments'], medians[len(baselines):], values[len(baselines):]):
            row['cpu_us_per_block'] = us
            row['cpu_raw_us'] = raw
            row['percent_one_core'] = us*48000/128/10000
            assert us <= ceiling, (row['family'], 'CPU ceiling exceeded', us, ceiling)
            print(row['family'], round(us, 2), 'us/block;', round(row['percent_one_core'], 2), '% core', flush=True)
    (HERE/'verification.json').write_text(json.dumps(result, indent=2)+'\n')


if __name__ == '__main__':
    main()
