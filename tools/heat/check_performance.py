#!/usr/bin/env python3
"""Measure Heat performance controls through compiled DSP, in physical units."""
import json
from pathlib import Path
import subprocess
import sys
import tempfile

import numpy as np
from check_filters import Instrument, ROOT
from check_voice import compile_source, render


def compile_probe(path, source, sr):
    path.write_text(source)
    inst = Instrument(path, sample_rate=sr)
    subprocess.run([sys.executable, str(ROOT / 'tools/audition/check_fusion.py'),
                    str(Path(inst.build_dir) / 'patch.c')], check=True)
    return inst


def run():
    source, hashes = compile_source()
    cases = []
    with tempfile.TemporaryDirectory(prefix='heat-performance-') as folder:
        path = Path(folder) / 'dsp.lisp'
        # Observe the actual integrated oscillator-frequency path before audio
        # generation: pitch-envelope, key tracking, glide and expression remain
        # the authored graph, rather than a separate reimplementation.
        tuning_source = source[:source.index('; Equal-power pan and explicit master gain.')]
        a = tuning_source.index('  (tuple (+ (* lane1')
        b = tuning_source.index('\n(def copies ', a)
        tuning_source = tuning_source[:a] + '  (tuple osc1_hz played_octave))\n' + tuning_source[b:]
        tuning_source += '\n(out left0 1)\n(out right0 2)\n(out vibrato 3)\n'
        for sr in (44100, 48000, 96000):
            inst = compile_probe(path, tuning_source, sr)
            phrase = [(0, .8, 261.625565, False), (.1, .8, 1046.50226, True)]
            for mode, rate, duration in [(0, 0, 0), (1, 0, .2), (1, 1, .4), (2, 0, .2)]:
                y = render(inst, seconds=1, notes=phrase,
                           params={'glide_mode': mode, 'glide_time_ms': 200, 'glide_rate_mode': rate})
                elapsed = np.maximum(0, np.arange(sr) / sr - .1)
                expected = np.where(np.arange(sr) < round(.1*sr), 0,
                                    2 if duration == 0 else 2*np.minimum(1, elapsed/duration))
                error = float(np.max(abs(y[:, 1] - expected)))
                assert error < 3e-5, (sr, mode, rate, error)
                cases.append(dict(sample_rate=sr, glide_mode=mode, proportional=rate, error_octaves=error))
            # First note is never swept from zero; a gapped note in fingered
            # mode starts at its destination. Always mode retains prior pitch.
            gap = [(0, .05, 220, False), (.1, .6, 440, False)]
            fingered = render(inst, seconds=.7, notes=gap, params={'glide_mode': 2})
            assert abs(fingered[round(.1*sr), 0] - 440) < .002
            assert abs(fingered[0, 0] - 220) < .002
            base = [(0, .7, 440, False)]
            for controls, params, semitones in [
                ({'pitch_bend': 1}, {'bend_range_semitones': 12}, 12),
                ({'pitch_bend': -.5}, {'bend_range_semitones': 12}, -6),
                ({}, {'octave': 1, 'detune_cents': 50}, 12.5),
                ({}, {'stretch_cents': 100}, 9/12),
            ]:
                y = render(inst, seconds=.8, notes=base, controllers=controls, params=params)
                expected = 440*2**(semitones/12)
                error = float(np.max(abs(y[:, 0] - expected)))
                assert error < .01, (sr, controls, params, error)
                cases.append(dict(sample_rate=sr, controls=controls, params=params, error_hz=error))
            # Wheel depth is independent of the general LFOs. Delay is silent,
            # attack reaches its set depth, and release does not stop vibrato.
            y = render(inst, seconds=1, notes=base, controllers={'mod_wheel': .5}, params={
                'vibrato_wheel_cents': 100, 'vibrato_rate_hz': 5,
                'vibrato_delay_ms': 100, 'vibrato_attack_ms': 200})[:, 2]
            assert np.max(abs(y[:round(.1*sr)])) == 0
            depth = float(np.max(abs(y[round(.3*sr):])))
            assert 49.99 < depth < 50.01
            cases.append(dict(sample_rate=sr, vibrato_depth_cents=depth))
            y = render(inst, seconds=.8, notes=base, params={'tuning_error_cents': 50})[:, 0]
            cents = 1200*np.log2(y/440)
            assert np.max(abs(cents)) <= 50.01
            assert np.ptp(cents) < .001  # Held pitch error does not wander.
    out = ROOT / 'tools/heat/measurements/performance-development.json'
    out.write_text(json.dumps({'source_sha256': hashes, 'cases': cases}, indent=2)+'\n')
    print(f'{len(cases)} performance cases passed')


if __name__ == '__main__':
    run()
