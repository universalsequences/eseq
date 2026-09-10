#!/usr/bin/env python3
"""Timing, envelope, host event, and off-state regressions for reverse piano."""
import argparse
import hashlib
import itertools
import json
import time
from pathlib import Path

import numpy as np
from scipy.signal import stft

from analyze import hz
from common import HERE, SOURCE, instrument
from verify import rms, stream


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--baseline', type=Path, required=True,
                        help='PM Piano dsp.lisp saved before adding the swell')
    parser.add_argument('--roundtrip', type=Path, required=True)
    args = parser.parse_args()
    inst = instrument()
    baseline = instrument(args.baseline)
    checks = []
    quiet = {'hammer.knock': 0, 'damper.key_noise': 0}

    def render(label, seconds=2, note=60, params=None, current=inst, **kw):
        y, state = current.render(seconds, pitch=hz(note), params=params, **kw)
        assert np.isfinite(y).all() and np.isfinite(state).all(), label
        peak = float(abs(y).max())
        assert peak < 2, (label, peak)
        checks.append({'case': label, 'peak': peak, 'rms': rms(y)})
        return y

    # Additive controls with amount=0 preserve old piano attacks, key-up tails,
    # nonlinear coloration and same-voice retriggers, not just one default note.
    differences = []
    for note in [21, 36, 60, 84, 108]:
        for params in [{}, {'body.resonance': .3, 'output.drive': .2, 'damper.release_s': 5}]:
            before = render(f'before {note}', note=note, params=params, current=baseline,
                            gate_off=.6, retrig=[.35])
            after = render(f'off {note}', note=note, params=params, gate_off=.6, retrig=[.35])
            delta = float(abs(before-after).max())
            assert delta < 5e-6, (note, delta)
            differences.append(delta)
    print('Original piano retained with Reverse blend = 0', flush=True)

    # A 40 ms energy window measures the crest without depending on carrier
    # phase. Very low fundamentals need a wider tolerance than treble.
    for sr in [44100, 48000, 96000]:
        current = instrument(sr=sr)
        for note in [21, 60, 100]:
            for length in [.05, .25, 1, 3, 8]:
                y = render(f'crest {sr}/{note}/{length}', current=current, note=note,
                           seconds=length+.2, vel=1, params={**quiet,
                           'swell.amount': 1, 'swell.length_s': length})
                energy = np.array([rms(b) for b in np.array_split(y, max(1, int(len(y)/(.04*sr))))])
                peak_time = (int(np.argmax(energy))+.5)*len(y)/len(energy)/sr
                assert abs(peak_time-length) < .075, (sr, note, length, peak_time)
                assert rms(y[int(max(0, length-.05)*sr):int((length+.02)*sr)]) > .001
                assert rms(y[int((length+.12)*sr):]) < .0001
                checks[-1]['crest_seconds'] = peak_time
    print('50 ms–8 s crests and endings passed across registers and sample rates', flush=True)

    # A two-minute held bass at 96 kHz exceeds a ten-million-sample clock.
    # Its tail must keep decaying instead of freezing when a sample counter
    # saturates or loses integer precision.
    long_sr = 96000
    long_tail = render('two-minute held swell', seconds=120, note=21,
                       current=instrument(sr=long_sr), params={**quiet,
                       'swell.amount': 1, 'swell.length_s': .05, 'swell.tail': 1,
                       'string.decay': 4, 'tuning.unison': 0})
    assert rms(long_tail[115*long_sr:]) < rms(long_tail[105*long_sr:110*long_sr])*.85

    base = {**quiet, 'swell.amount': 1, 'swell.length_s': 1, 'swell.tail': 1}
    held = render('held reverse', params=base)
    pedal = render('pedal reverse', params={**base, 'damper.pedal': 1}, gate_off=.2)
    assert np.array_equal(held, pedal), 'full pedal must keep the released swell alive'
    tails = []
    for release in [.02, .2, 2, 20]:
        y = render('released swell '+str(release), params={**base, 'damper.release_s': release}, gate_off=.2)
        tails.append(rms(y[43200:52800]))
    assert all(a < b for a, b in zip(tails, tails[1:])), tails
    assert tails[0] < 1e-8 and tails[-1] > .005
    stopped = render('pedal lowered mid-rise', params={**base, 'damper.pedal': 1}, gate_off=.1,
                     ramps={'damper.pedal': [(0, 1), (.4, 0)]})
    assert rms(stopped[43200:52800]) < rms(held[43200:52800])*.001
    cut = render('reverse-only ending', params={**base, 'swell.tail': 0})
    assert rms(cut[60000:72000]) < rms(held[60000:72000])*.001

    # A4's calibrated upper band decays faster than its low modes. Its relative
    # brightness should therefore rise on reversal. This is note-specific:
    # some register rows have strong fast-decaying fundamentals as well.
    spectral = render('A4 spectral rise', note=69, params=base)
    freq, t, spectrum = stft(spectral.mean(axis=1), fs=48000, nperseg=2048, noverlap=1024)
    power = abs(spectrum)**2
    band = lambda lo, hi: power[(freq > lo) & (freq < hi)].sum(axis=0)
    brightness = band(2000, 10000)/np.maximum(band(200, 10000), 1e-20)
    early = float(np.mean(brightness[(t > .2) & (t < .4)]))
    late = float(np.mean(brightness[(t > .88) & (t < .99)]))
    assert late > early*1.2, (early, late)

    # The landing time and shape remain fixed for an in-flight note. The next
    # onset picks up edits. Blend is intentionally live and tested separately.
    for name, changed in [('swell.length_s', .2), ('swell.curve', 4), ('swell.tail', 0)]:
        y = render(name+' latched', params=base, ramps={name: [(0, base.get(name, 1)), (.2, changed)]})
        assert np.array_equal(y, held), (name, 'changed in-flight note')
        y = render(name+' next trigger', params=base, retrig=[.5],
                   ramps={name: [(0, base.get(name, 1)), (.2, changed)]})
        unchanged = render(name+' unchanged retrigger', params=base, retrig=[.5])
        assert rms(y-unchanged) > 1e-4
    blend = render('live blend', params=base, ramps={'swell.amount': [(0, 1), (.5, 0)]})
    assert rms(blend-held) > .001
    for value in [0, .25, .5, 1]:
        y = render('swell velocity '+str(value), params=base, vel=value)
        if value == 0:
            assert abs(y).max() == 0
    muted = render('swell muted', params={**base, 'output.gain': 0})
    assert abs(muted).max() == 0
    print('Release, pedal, brightening, latched timing and live blending passed', flush=True)

    for length, curve, tail in itertools.product([.05, 8], [.25, 4], [0, 1]):
        render(f'swell corners {length}/{curve}/{tail}', seconds=length+.4, note=48,
               params={**base, 'swell.length_s': length, 'swell.curve': curve,
               'swell.tail': tail, 'string.decay': 4, 'string.damping': .2,
               'hammer.hardness': 1, 'hammer.contact': .35, 'tuning.unison': 25})
    for name in ['swell.amount', 'swell.length_s', 'swell.curve', 'swell.tail']:
        settings = {**base, 'swell.amount': .5, 'swell.length_s': .3, 'swell.tail': .4}
        normal = render(name+' modulation baseline', params=settings)
        modulated = render(name+' host modulation', params={**settings,
            '__mod__'+name+'__active': 1, '__mod__'+name+'__depth__slot1': .3},
            ramps={'mod1': [(0, 1)]})
        assert rms(normal-modulated) > 1e-5, name

    settings = {**base, 'swell.length_s': .123, 'swell.curve': .7, 'damper.release_s': 3}
    a, state = stream(inst, 128, params=settings)
    for block in [7, 12, 28, 63]:
        b, state = stream(inst, block, params=settings)
        assert np.array_equal(a, b), ('short calls', block, float(abs(a-b).max()))
    for block in [12, 28, 64, 256]:
        b, state = stream(instrument(block=block), block, params=settings)
        assert np.array_equal(a, b), ('compiled blocks', block, float(abs(a-b).max()))
    idle, _ = stream(inst, 128, idle=True, params=settings)
    gate, _ = stream(inst, 128, gate_only=True, params=settings)
    assert abs(idle).max() == 0 and np.array_equal(gate, a)
    saved = instrument(args.roundtrip)
    b, _ = stream(saved, 128, params=settings)
    save_delta = float(abs(a-b).max())
    assert save_delta < 1e-5, ('graph save', save_delta)
    print('Reverse streaming, retriggers, silence and graph-save audio passed', flush=True)

    timings = {}
    for amount in [0, 1]:
        runs = []
        for _ in range(3):
            start = time.process_time()
            render('CPU '+str(amount), seconds=4, params={**base, 'swell.amount': amount})
            runs.append((time.process_time()-start)/4)
        timings[str(amount)] = runs
    result = {'source_sha256': hashlib.sha256(SOURCE.read_bytes()).hexdigest(),
              'baseline_sha256': hashlib.sha256(args.baseline.read_bytes()).hexdigest(),
              'compiler_sha256': inst.compiler_sha256, 'checks': checks,
              'max_off_sample_delta': max(differences), 'graph_save_sample_delta': save_delta,
              'brightness_fraction_early_late': [early, late], 'cpu_seconds_per_audio_second': timings,
              'streaming_call_sizes': [7, 12, 28, 63, 128], 'compiled_block_sizes': [12, 28, 64, 128, 256]}
    (HERE/'swell-validation.json').write_text(json.dumps(result, indent=2)+'\n')
    print(len(checks), 'swell renders passed', flush=True)


if __name__ == '__main__':
    main()
