#!/usr/bin/env python3
"""Signal, control, pitch and authored-graph checks of all factory models."""
import argparse
import hashlib
import json
from pathlib import Path
import time

import numpy as np

from analyze import peak_frequency
from build import outputs
from common import instrument
from compare import rms
from families import FACTORY, FAMILIES, HERE
from stream import stream


def verify(slug, roundtrip):
    family = FAMILIES[slug]
    data = json.loads((HERE/f'{slug}-analysis.json').read_text())
    inst = instrument(family.source)
    key = family.units[len(family.units)//2][1]
    checks = []

    def check(label, y, state, audible=False, peak_limit=16):
        assert np.isfinite(y).all() and np.isfinite(state).all(), label
        peak = float(abs(y).max())
        assert peak < peak_limit, (family.name, label, peak)
        if audible:
            assert rms(y) > 1e-6, (family.name, label, 'silent')
        checks.append({'case': label, 'peak': peak, 'rms': rms(y)})
        return y

    def render(label, seconds=.8, note=key, **kwargs):
        return check(label, *inst.render(seconds, pitch=440*2**((note-69)/12), **kwargs))

    for note in range(36, 109):
        y = render(f'key {note}', note=note, vel=.8, gate_off=.4)
        assert rms(y) > 1e-6 and abs(y).max() < 1, (slug, note)
    for unit in family.units:
        for velocity in [.01, .1, .3, .6, .7, .8, .9, 1]:
            render(f'velocity {unit[0]}/{velocity}', note=unit[1], vel=velocity)
    params = {n: p for n, p in inst.params.items() if not n.startswith('__')}
    for name, param in params.items():
        for value in [param['min'], param['max']]:
            render(f'extreme {name}/{value}', params={name: value}, gate_off=.4)
    rng = np.random.default_rng(51)
    for trial in range(16):
        setting = {n: float(rng.choice([p['min'], p['max']])) for n, p in params.items()}
        render(f'combined extremes {trial}', params=setting, gate_off=.5)
    assert abs(render('velocity zero', vel=0)).max() == 0
    held = render('held', seconds=2)
    lifted = render('key-up full lift', seconds=2, gate_off=.25, params={'damper.lift': 1})
    assert np.array_equal(held, lifted)
    fast = render('key-up fast', seconds=2, gate_off=.25, params={'damper.release_s': .02})
    slow = render('key-up slow', seconds=2, gate_off=.25, params={'damper.release_s': 4})
    touched = render('hand damp', seconds=2, params={'damper.touch': 1})
    assert np.array_equal(fast[:12000], slow[:12000])
    assert rms(fast[24000:]) < rms(slow[24000:])*.01 < rms(held[24000:])
    assert rms(touched[24000:]) < rms(held[24000:])*.01
    for name, low, high in [('body.decay', .25, 4), ('body.bloom', 0, 2), ('mallet.hardness', 0, 1)]:
        a = render(name+' low', params={name: low})
        b = render(name+' high', params={name: high})
        assert rms(a-b) > .001*rms(a), (slug, name, 'ineffective')
    # Strike timing must be independent of the coefficient clock phase.
    baseline, _ = stream(inst, frames=12000, onset=0, gate_end=12000, key=key, vel=1)
    onset_error = 0.
    for phase in range(1, 16):
        y, state = stream(inst, frames=12000+phase, onset=phase, gate_end=12000+phase, key=key, vel=1)
        check(f'onset phase {phase}', y, state, True)
        onset_error = max(onset_error, float(abs(baseline-y[phase:]).max()))
    assert onset_error < 2e-5, (slug, 'onset clock', onset_error)
    events = {8009: {'body.loss': 2}, 13007: {'tuning.tune': 30}, 18101: {'damper.lift': 1}}
    baseline, state = stream(inst, key=key, events=events)
    check('stream 128', baseline, state, True)
    partition_error = 0.
    for block in [1, 7, 31, 64, 127]:
        y, state = stream(inst, key=key, events=events, block=block)
        check(f'stream {block}', y, state, True)
        partition_error = max(partition_error, float(abs(y-baseline).max()))
    assert partition_error < 2e-5, (slug, 'process partition', partition_error)
    idle, state = stream(inst, idle=True, key=key)
    check('untriggered silence', idle, state)
    assert abs(idle).max() == 0
    gate, state = stream(inst, gate_only=True, key=key, events=events)
    check('gate-only onset', gate, state, True)
    assert np.array_equal(gate, baseline)
    assert {d['name'] for d in inst.manifest['modDestinations']} == set(params)-{'mallet.dynamics'}
    for name in ['body.decay', 'body.inharmonicity', 'damper.touch', 'output.gain']:
        render('host modulation '+name, params={f'__mod__{name}__active': 1,
            f'__mod__{name}__depth__slot1': .7}, ramps={'mod1': [(0, -1), (.3, 1), (.6, -.5)]})
    render('continuous automation', seconds=2, ramps={
        'body.inharmonicity': [(0, 0), (.7, 1.8), (1.3, 1)],
        'body.decay': [(0, .15), (.5, 4), (1, .3)], 'damper.touch': [(0, 0), (1.5, .8)]})
    bank = json.loads((FACTORY/(family.name+'.presets')).read_text())['presets']
    defaults = {n: p['default'] for n, p in params.items()}
    assert all(np.float32(bank[0]['params'][n]) == np.float32(v) for n, v in defaults.items())
    assert len({p['id'] for p in bank}) == len(bank)
    for preset in bank:
        assert set(preset['params']) == set(params)
        assert all(params[n]['min'] <= v <= params[n]['max'] for n, v in preset['params'].items())
        for note in [48, key, 96]:
            y = render('preset '+preset['id'], seconds=2, note=note, params=preset['params'], gate_off=.5)
            assert abs(y).max() < 1, (slug, preset['id'], note)
    pitch_checks = []
    for sr in [44100, 48000, 96000]:
        other = inst if sr == 48000 else instrument(family.source, sr=sr)
        for unit in data['units']:
            y, state = other.render(1.3, pitch=440*2**((unit['midi']-69)/12), vel=.8)
            check(f'pitch {sr}/{unit["label"]}', y, state, True)
            x = y[int(.1*sr):int(1.2*sr)]
            a = np.sqrt(np.mean(abs(np.fft.rfft(x*np.hanning(len(x))[:, None], 2**19, axis=0))**2, axis=1))
            f = np.fft.rfftfreq(2**19, 1/sr)
            indices = np.flatnonzero(abs(f-unit['fundamental_hz']) < 2)
            index = indices[np.argmax(a[indices])]
            hz = peak_frequency(f, a, index)
            cents = float(1200*np.log2(hz/unit['fundamental_hz']))
            assert abs(cents) < .3, (slug, unit['label'], sr, cents)
            pitch_checks.append({'sample_rate': sr, 'unit': unit['label'], 'cents_error': cents})
    saved = instrument(roundtrip/family.name/'dsp.lisp')
    a, _ = stream(inst, key=key, events=events, block=31)
    b, state = stream(saved, key=key, events=events, block=31)
    check('compiled editor writeback', b, state, True)
    roundtrip_error = float(abs(a-b).max())
    assert roundtrip_error < 2e-5, (slug, 'graph writeback', roundtrip_error)
    for path, expected in outputs(slug).items():
        assert path.read_text() == expected, f'Stale generated file: {path}'
    start = time.process_time()
    inst.render(6, pitch=440*2**((key-69)/12), retrig=[1, 2, 3, 4, 5])
    cpu = (time.process_time()-start)/6*100
    result = {'source_sha256': hashlib.sha256(family.source.read_bytes()).hexdigest(),
              'compiler_sha256': inst.compiler_sha256, 'onset_max_error': onset_error,
              'process_partition_max_error': partition_error, 'graph_writeback_max_error': roundtrip_error,
              'cpu_percent_one_voice': cpu, 'pitch_checks': pitch_checks, 'checks': checks}
    (HERE/f'{slug}-validation.json').write_text(json.dumps(result, indent=2)+'\n')
    print(slug, len(checks), 'signal checks;', len(pitch_checks), 'pitches;', round(cpu, 2), '% CPU', flush=True)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('families', nargs='*', choices=list(FAMILIES))
    parser.add_argument('--roundtrip', type=Path, required=True)
    args = parser.parse_args()
    for slug in args.families or FAMILIES:
        verify(slug, args.roundtrip)


if __name__ == '__main__':
    main()
