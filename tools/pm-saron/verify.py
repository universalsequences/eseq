#!/usr/bin/env python3
"""Behavioral checks of the production-compiled saron and streaming ABI."""
import argparse
import ctypes
import hashlib
import json
from pathlib import Path
import platform
import time

import numpy as np

from calibrate import KEYS, START, END, tables
from common import HERE, ROOT, SOURCE, instrument
from compare import rms


def stream(inst, frames=24000, block=128, onset=137, idle=False, gate_only=False,
           events=None, params=None, key=77, vel=.8, gate_end=12003):
    state = inst.fresh_memory()
    for name, value in (params or {}).items():
        state[inst.params[name]['cellId']] = value
    inputs = [np.zeros(frames, dtype=np.float32) for _ in range(inst.n_in)]
    outputs = [np.zeros(frames, dtype=np.float32) for _ in range(inst.n_out)]
    inputs[inst.inputs['pitch']][:] = 440*2**((key-69)/12)
    if not idle:
        inputs[inst.inputs['gate']][onset:gate_end] = 1
        inputs[inst.inputs['velocity']][onset:gate_end] = vel
        if not gate_only:
            inputs[inst.inputs['trigger']][onset] = 1
    events = events or {}
    boundaries = sorted([*events, frames])
    offset = 0
    while offset < frames:
        for name, value in events.get(offset, {}).items():
            state[inst.params[name]['cellId']] = value
        count = min(block, next(b for b in boundaries if b > offset)-offset)
        def pointers(arrays):
            return (ctypes.POINTER(ctypes.c_float)*len(arrays))(*[
                a[offset:].ctypes.data_as(ctypes.POINTER(ctypes.c_float)) for a in arrays])
        inst.process_fn(pointers(inputs), pointers(outputs), count,
                        state.ctypes.data_as(ctypes.c_void_p), ctypes.byref(inst.context), None)
        offset += count
    return np.array(outputs).T, state


def onset_check(inst):
    baseline, _ = stream(inst, frames=16000, onset=0, gate_end=16000, vel=1)
    maximum = 0
    for phase in range(1, 16):
        y, _ = stream(inst, frames=16000+phase, onset=phase, gate_end=16000+phase, vel=1)
        error = float(abs(baseline-y[phase:]).max())
        maximum = max(maximum, error)
    print('Onset clock-phase maximum sample error:', maximum, flush=True)
    assert maximum < 2e-5, 'Note strength must not depend on its position in the control clock'
    return maximum


def pitch_checks():
    references = json.loads((HERE/'reference-analysis.json').read_text())['bars']
    rows = []
    for sr in [44100, 48000, 96000]:
        inst = instrument(sr=sr)
        for key, bar in zip(KEYS, references):
            y, _ = inst.render(1.2, pitch=440*2**((key-69)/12), vel=.6)
            segment = y[int(.05*sr):int(1.1*sr)]
            n = 2**19
            f = np.fft.rfftfreq(n, 1/sr)
            a = np.sqrt(np.mean(abs(np.fft.rfft(segment*np.hanning(len(segment))[:, None], n, axis=0))**2, axis=1))
            indices = np.flatnonzero(abs(f-bar['fundamental_hz']) < 5)
            k = indices[np.argmax(a[indices])]
            left, mid, right = np.log(a[k-1:k+2])
            measured = float((k+.5*(left-right)/(left-2*mid+right))*sr/n)
            cents = float(1200*np.log2(measured/bar['fundamental_hz']))
            assert abs(cents) < .15, (sr, key, cents)
            rows.append({'sample_rate': sr, 'midi': key, 'expected_hz': bar['fundamental_hz'],
                         'measured_hz': measured, 'cents_error': cents})
    (HERE/'pitch-validation.json').write_text(json.dumps({
        'source_sha256': hashlib.sha256(SOURCE.read_bytes()).hexdigest(),
        'compiler_sha256': inst.compiler_sha256, 'checks': rows}, indent=2)+'\n')
    print('21 reference-pitch checks passed; maximum cents error', max(abs(r['cents_error']) for r in rows), flush=True)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--onset-only', action='store_true')
    parser.add_argument('--pitches-only', action='store_true')
    parser.add_argument('--roundtrip', type=Path)
    args = parser.parse_args()
    if args.pitches_only:
        pitch_checks()
        return
    inst = instrument()
    onset_error = onset_check(inst)
    if args.onset_only:
        return
    checks = []
    def check(label, y, state, audible=False):
        assert np.isfinite(y).all() and np.isfinite(state).all(), label
        peak = float(abs(y).max())
        assert peak < 12, (label, peak)
        if audible:
            assert rms(y) > 1e-5, label
        checks.append({'case': label, 'peak': peak, 'rms': rms(y)})
        return y
    def render(label, seconds=1, key=77, **kwargs):
        return check(label, *inst.render(seconds, pitch=440*2**((key-69)/12), **kwargs))
    params = {n: p for n, p in inst.params.items() if not n.startswith('__')}
    defaults = {n: p['default'] for n, p in params.items()}
    for key in range(36, 109):
        y = render(f'key {key}', key=key, seconds=.7, vel=.8, gate_off=.4)
        assert rms(y) > 1e-5 and abs(y).max() < 1, (key, 'playable range')
    for key in KEYS:
        for vel in [.01, .1, .2, .3, .4, .5, .6, .7, .8, .9, 1]:
            render(f'velocity {key}/{vel}', key=key, vel=vel, seconds=.8)
    print('Keyboard and velocity sweep passed', flush=True)
    for name, p in params.items():
        for value in [p['min'], p['max']]:
            render(f'extreme {name}={value}', params={name: value}, gate_off=.4)
    rng = np.random.default_rng(20)
    for index in range(20):
        setting = {n: float(rng.choice([p['min'], p['max']])) for n, p in params.items()}
        render(f'combined extremes {index}', key=int(rng.choice(KEYS)), params=setting, seconds=1.5, gate_off=.35)
    for key in [48, 77, 96]:
        y = render(f'velocity zero {key}', key=key, vel=0)
        assert abs(y).max() == 0
        fast = render(f'damped {key}', key=key, seconds=2, gate_off=.25, params={'damper.release_s': .02})
        slow = render(f'released {key}', key=key, seconds=2, gate_off=.25, params={'damper.release_s': 4})
        free = render(f'free {key}', key=key, seconds=2, gate_off=.25, params={'damper.lift': 1})
        held = render(f'held {key}', key=key, seconds=2)
        assert np.array_equal(fast[:12000], slow[:12000]), 'Release must not change the held note'
        assert np.array_equal(free, held), 'Full lift must preserve natural ring after key-up'
        assert rms(fast[24000:]) < rms(slow[24000:])*.01
        assert rms(slow[24000:]) < rms(free[24000:])
        touched = render(f'hand damp {key}', key=key, params={'damper.touch': 1}, seconds=1)
        assert rms(touched[12000:]) < rms(held[12000:48000])*.01
    for name, changes in [('bar.decay', [.25, 4]), ('mallet.hardness', [0, 1]),
                          ('bar.bloom', [0, 2]), ('bar.inharmonicity', [0, 1.8])]:
        a = render(f'control effect {name} low', params={name: changes[0]})
        b = render(f'control effect {name} high', params={name: changes[1]})
        assert rms(a-b) > .01*rms(a), (name, 'ineffective control')
    event_map = {8009: {'bar.loss': 2}, 13007: {'tuning.tune': 30}, 18101: {'damper.lift': 1}}
    baseline, state = stream(inst, block=128, events=event_map)
    check('stream 128', baseline, state, True)
    partition_error = 0
    for block in [1, 7, 31, 64, 127]:
        y, state = stream(inst, block=block, events=event_map)
        check(f'stream {block}', y, state, True)
        error = float(abs(y-baseline).max())
        partition_error = max(partition_error, error)
        assert error < 2e-5, ('process partitioning', block, error)
    gate, state = stream(inst, gate_only=True, events=event_map)
    check('gate-only onset', gate, state, True)
    assert np.array_equal(gate, baseline)
    idle, state = stream(inst, idle=True)
    check('untriggered silence', idle, state)
    assert abs(idle).max() == 0
    for sr in [44100, 96000]:
        other = instrument(sr=sr)
        for key in [48, 77, 96]:
            check(f'sample rate {sr}/{key}', *other.render(1, pitch=440*2**((key-69)/12), gate_off=.3), True)
    bank = SOURCE.parent.parent/'PM Saron.presets'
    presets = json.loads(bank.read_text())['presets']
    assert all(np.float32(presets[0]['params'][n]) == np.float32(v) for n, v in defaults.items())
    assert len({p['id'] for p in presets}) == len(presets)
    for preset in presets:
        assert set(preset['params']) == set(params), preset['id']
        assert all(params[n]['min'] <= v <= params[n]['max'] for n, v in preset['params'].items())
        for key in [48, 77, 96]:
            y = render(f'preset {preset["id"]}/{key}', key=key, params=preset['params'], seconds=2, gate_off=.5)
            assert abs(y).max() < 1, preset['id']
    assert {d['name'] for d in inst.manifest['modDestinations']} == set(params)-{'mallet.dynamics'}
    render('live automation', seconds=2, ramps={
        'bar.inharmonicity': [(0, 0), (.7, 1.8), (1.3, 1)],
        'bar.decay': [(0, .15), (.5, 4), (1, .3)], 'damper.touch': [(0, 0), (1.5, .8)]})
    for name in ['bar.inharmonicity', 'bar.decay', 'damper.touch', 'output.gain']:
        render(f'host modulation {name}', params={f'__mod__{name}__active': 1,
               f'__mod__{name}__depth__slot1': .7}, ramps={'mod1': [(0, -1), (.3, 1), (.6, -.5), (1, 0)]})
    roundtrip_error = None
    if args.roundtrip:
        saved = instrument(args.roundtrip)
        a, _ = stream(inst, block=31, events=event_map)
        b, state = stream(saved, block=31, events=event_map)
        check('patch-editor writeback', b, state, True)
        roundtrip_error = float(abs(a-b).max())
        assert roundtrip_error < 2e-5, ('graph writeback', roundtrip_error)
    source = SOURCE.read_text()
    assert source[source.index(START):source.index(END)+len(END)] == tables()
    refs = json.loads((HERE/'reference-analysis.json').read_text())
    for bar in refs['bars']:
        for r in bar['recordings']:
            assert hashlib.sha256((ROOT/r['file']).read_bytes()).hexdigest() == r['sha256']
    start = time.process_time()
    inst.render(8, pitch=700, retrig=[1, 2, 3, 4, 5, 6, 7])
    cpu = (time.process_time()-start)/8*100
    result = {'platform': platform.platform(), 'source_sha256': hashlib.sha256(SOURCE.read_bytes()).hexdigest(),
              'compiler_sha256': inst.compiler_sha256, 'sample_rate': inst.sample_rate,
              'block': inst.max_frames, 'cpu_percent_one_voice': cpu,
              'onset_clock_phase_max_error': onset_error, 'process_partition_max_error': partition_error,
              'graph_writeback_max_error': roundtrip_error, 'checks': checks}
    (HERE/'validation.json').write_text(json.dumps(result, indent=2)+'\n')
    print(len(checks), 'checks passed; one-voice CPU', round(cpu, 2), '%', flush=True)


if __name__ == '__main__':
    main()
