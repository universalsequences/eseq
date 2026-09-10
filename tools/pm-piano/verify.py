#!/usr/bin/env python3
"""Exercise the compiled piano's playable range, dampers and streaming ABI."""
import argparse
import ctypes
import hashlib
import itertools
import json
import platform
import time
from pathlib import Path

import numpy as np

from analyze import hz
from calibrate import tables, START, END
from common import HERE, ROOT, SOURCE, instrument

BANK = SOURCE.parent.parent / 'PM Piano.presets'


def rms(y):
    return float(np.sqrt(np.mean(np.asarray(y, dtype=np.float64)**2)))


def stream(inst, block, idle=False, gate_only=False, params=None):
    """Identical sample events with irregular process calls and parameter writes.

    Unlike the audition helper's block-rate ramps, parameter events here split
    a call at their exact sample. This isolates process partitioning from event
    quantization, including key-up velocity zero and off-grid control ticks.
    """
    n = 30001
    state = inst.fresh_memory()
    for name, value in (params or {}).items():
        state[inst.params[name]["cellId"]] = value
    inputs = [np.zeros(n, dtype=np.float32) for _ in range(inst.n_in)]
    outputs = [np.zeros(n, dtype=np.float32) for _ in range(inst.n_out)]
    inputs[inst.inputs['pitch']][:12013] = hz(60)
    inputs[inst.inputs['pitch']][12013:] = hz(67)
    if not idle:
        for start, end in [(137, 10003), (12013, 20011)]:
            inputs[inst.inputs['gate']][start:end] = 1
            inputs[inst.inputs['velocity']][start:end] = .8
            if not gate_only:
                inputs[inst.inputs['trigger']][start] = 1
    events = {8009: {'damper.release_s': 2}, 18101: {'string.damping': 2.1},
              24007: {'damper.pedal': 1}}
    boundaries = sorted([*events, n])
    offset = 0
    while offset < n:
        for name, value in events.get(offset, {}).items():
            state[inst.params[name]['cellId']] = value
        count = min(block, next(b for b in boundaries if b > offset)-offset)
        def pointers(arrays):
            return (ctypes.POINTER(ctypes.c_float) * len(arrays))(*[
                a[offset:].ctypes.data_as(ctypes.POINTER(ctypes.c_float)) for a in arrays])
        inst.process_fn(pointers(inputs), pointers(outputs), count,
                        state.ctypes.data_as(ctypes.c_void_p), ctypes.byref(inst.context), None)
        offset += count
    return np.array(outputs).T, state


def measured_peak(y, sr, expected):
    segment = np.mean(y[int(.025*sr):int(.85*sr)], axis=1)
    magnitude = abs(np.fft.rfft(segment*np.hanning(len(segment)), 2**19))
    frequencies = np.fft.rfftfreq(2**19, 1/sr)
    band = np.flatnonzero(abs(frequencies-expected) < expected*.025)
    index = band[np.argmax(magnitude[band])]
    a, b, c = np.log(np.maximum(magnitude[index-1:index+2], 1e-20))
    offset = .5*(a-c)/(a-2*b+c)
    return (index+offset)*sr/2**19


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--roundtrip', type=Path, required=True)
    args = parser.parse_args()
    checks, builds = [], set()

    def compiled(source=SOURCE, sr=48000, block=128):
        inst = instrument(source, sr, block)
        builds.add(str(inst.build_dir))
        return inst

    def audio(label, y, state):
        assert np.isfinite(y).all() and np.isfinite(state).all(), (label, 'non-finite')
        peak = float(abs(y).max())
        assert peak < 8, (label, 'excessive output', peak)
        checks.append({'case': label, 'peak': peak, 'rms': rms(y)})
        return y

    def render(inst, label, seconds=1.4, note=60, **kwargs):
        y, state = inst.render(seconds, pitch=hz(note), **kwargs)
        return audio(label, y, state)

    inst = compiled()
    params = {n: p for n, p in inst.params.items() if not n.startswith('__')}
    defaults = {n: p['default'] for n, p in params.items()}
    presets = json.loads(BANK.read_text())['presets']
    assert len({p['id'] for p in presets}) == len(presets)
    assert set(params) == set(presets[0]['params'])
    assert all(np.float32(presets[0]['params'][n]) == np.float32(v) for n, v in defaults.items())
    assert {d['name'] for d in inst.manifest['modDestinations']} == set(params)-{
        'hammer.velocity_tone', 'hammer.velocity_curve', 'damper.upper_free'}
    source = SOURCE.read_text()
    assert source[source.index(START):source.index(END)+len(END)] == tables()
    references = json.loads((HERE/'reference-analysis.json').read_text())['notes']
    for row in references:
        assert hashlib.sha256((ROOT/row['file']).read_bytes()).hexdigest() == row['sha256']
    for p in presets:
        assert set(p['params']) == set(params), p['id']
        for n, value in p['params'].items():
            assert np.float32(params[n]['min']) <= np.float32(value) <= np.float32(params[n]['max']), (p['id'], n)
        for note in [21, 36, 60, 84, 108]:
            swell = p['params']['swell.length_s'] if p['params']['swell.amount'] else 0
            y = render(inst, p['id']+' '+str(note), seconds=max(3, swell+.8), note=note,
                       params=p['params'], gate_off=max(.6, swell+.1))
            assert rms(y) > .0001 and abs(y).max() < 1, (p['id'], note, 'preset level')
    for note in range(21, 109):
        y = render(inst, 'chromatic '+str(note), seconds=.8, note=note, gate_off=.5)
        assert rms(y) > .0002 and abs(y).max() < 1, (note, 'keyboard continuity')
    print('Preset contracts, reference hashes and all 88 keys passed', flush=True)

    quiet_mechanism = {'hammer.knock': 0, 'damper.key_noise': 0}
    for note in [21, 60, 100]:
        tails = []
        prefix = None
        for release in [.02, .18, 1, 5, 20]:
            y = render(inst, f'release {note}/{release}', seconds=2, note=note, gate_off=.25,
                       params={**quiet_mechanism, 'damper.release_s': release})
            if prefix is None:
                prefix = y[:12000]
            assert np.array_equal(prefix, y[:12000]), 'release changed held attack'
            tails.append(rms(y[24000:36000]))
        assert all(a < b for a, b in zip(tails, tails[1:])), (note, tails)
        pedal = render(inst, f'pedal lift {note}', seconds=2, note=note, gate_off=.25,
                       params={**quiet_mechanism, 'damper.pedal': 1})
        held = render(inst, f'held string {note}', seconds=2, note=note, params=quiet_mechanism)
        assert np.array_equal(pedal, held), (note, 'pedal did not fully lift dampers')
    print('Independent release, long tails and full pedal lift passed across registers', flush=True)

    # Tune an audible mode in each region, including pitches between measured
    # rows. Bass fundamentals are weak; measuring their strongest mode avoids
    # mistaking a spectral leakage peak for fundamental tuning.
    midi = np.array([r['midi'] for r in references])
    stiffness = np.array([r['stiffness_B'] for r in references])
    for i, row in enumerate(references):
        if not row['stiffness_identified']:
            j = max(j for j in range(i) if references[j]['stiffness_identified'])
            stiffness[i] = stiffness[j]*2**((row['midi']-references[j]['midi'])/12)
    modes = np.array([r['modes'] for r in references])
    for sr in [44100, 48000, 96000]:
        current = compiled(sr=sr)
        for note in [21, 22, 36, 60, 61, 84, 107, 108]:
            y = render(current, f'tuning {sr}/{note}', seconds=1, note=note,
                       params={**quiet_mechanism, 'tuning.unison': 0})
            b = np.interp(note, midi, stiffness)
            f0 = hz(note)*2**(np.interp(note, midi, [r['tuning_cents'] for r in references])/1200)
            amplitudes = [np.interp(note, midi, modes[:, n, 0]+modes[:, n, 2]) for n in range(8)]
            mode = int(np.argmax(amplitudes))+1
            expected = f0*mode*np.sqrt((1+b*mode*mode)/(1+b))
            actual = measured_peak(y, sr, expected)
            cents = float(1200*np.log2(actual/expected))
            assert abs(cents) < 6, (sr, note, mode, cents)
            checks[-1]['mode_pitch_error_cents'] = cents
    print('Measured tuning at 44.1, 48 and 96 kHz passed', flush=True)

    expressive = {**defaults, 'body.resonance': .4, 'motion.pan': .5, 'motion.tremolo': .5,
                  'damper.release_s': .3, 'string.decay': 2}
    for name, p in params.items():
        note = 100 if name == 'damper.upper_free' else 60
        context = expressive
        if name.startswith('swell.'):
            context = {**expressive, 'swell.amount': 1, 'swell.length_s': .2, 'damper.release_s': 5}
        lo = render(inst, name+' min', note=note, vel=.6, gate_off=.3, params={**context, name: p['min']})
        hi = render(inst, name+' max', note=note, vel=.6, gate_off=.3, params={**context, name: p['max']})
        difference = rms(lo-hi)
        assert difference > 1e-7, (name, 'ineffective control', difference)
        checks[-1]['control_difference_rms'] = difference
        render(inst, name+' moving', vel=.7, gate_off=.7, params=context, retrig=[.55],
               ramps={name: [(0, p['min']), (.4, p['max']), (.8, p['min']), (1.2, p['max'])]})
    extremes = ['hammer.hardness', 'hammer.contact', 'string.stiffness', 'string.damping']
    for edges in itertools.product(['min', 'max'], repeat=len(extremes)):
        render(inst, 'joint '+str(edges), note=33, gate_off=.6, params={
            'string.decay': 4, **{n: params[n][edge] for n, edge in zip(extremes, edges)}})
    levels = [rms(render(inst, 'velocity '+str(v), vel=v, params=quiet_mechanism)) for v in [.25, .5, 1]]
    assert levels[0] < levels[1] < levels[2]
    for settings in [{'output.gain': 0}, {}]:
        y = render(inst, 'muted gain' if settings else 'zero velocity', params=settings, vel=1 if settings else 0)
        assert abs(y).max() == 0
    for name in ['hammer.hardness', 'string.stiffness', 'damper.release_s', 'output.gain']:
        y = render(inst, name+' modulated', params={**quiet_mechanism,
            '__mod__'+name+'__active': 1, '__mod__'+name+'__depth__slot1': .5},
            ramps={'mod1': [(0, 1)]}, gate_off=.3)
        normal = render(inst, name+' unmodulated', params=quiet_mechanism, gate_off=.3)
        assert rms(y-normal) > 1e-5, (name, 'host modulation')
    print('Every control, automation, joint extremes, velocity and modulation passed', flush=True)

    a = audio('streaming reference', *stream(inst, 128))
    for block in [7, 12, 28, 63]:
        b = audio('short calls '+str(block), *stream(inst, block))
        assert np.array_equal(a, b), (block, float(abs(a-b).max()))
    for block in [12, 28, 64, 256]:
        b = audio('compiled block '+str(block), *stream(compiled(block=block), block))
        assert np.array_equal(a, b), (block, float(abs(a-b).max()))
    idle = audio('idle voice', *stream(inst, 128, idle=True))
    gate = audio('gate-only voice', *stream(inst, 128, gate_only=True))
    assert abs(idle).max() == 0 and np.array_equal(a, gate), 'gate-only onset or key-up velocity behavior'
    roundtrip = compiled(args.roundtrip)
    assert set(roundtrip.params) == set(inst.params)
    b = audio('patch editor save', *stream(roundtrip, 128))
    assert np.max(abs(a-b)) < 1e-5, ('patch editor changed audio', float(abs(a-b).max()))
    print('Irregular streaming calls, gate-only triggers and graph-save audio passed', flush=True)

    timings = []
    for _ in range(3):
        start = time.process_time()
        render(inst, 'timing', seconds=4)
        timings.append((time.process_time()-start)/4)
    output = {'platform': platform.platform(), 'source_sha256': hashlib.sha256(SOURCE.read_bytes()).hexdigest(),
              'compiler_sha256': inst.compiler_sha256, 'parameter_count': len(params),
              'preset_count': len(presets), 'render_cpu_seconds_per_audio_second': timings,
              'fusion_audited_builds': sorted(builds), 'checks': checks}
    (HERE/'validation.json').write_text(json.dumps(output, indent=2)+'\n')
    print(len(checks), 'checks passed; median one-voice CPU/audio ratio', np.median(timings), flush=True)


if __name__ == '__main__':
    main()
