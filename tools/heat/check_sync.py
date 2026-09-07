#!/usr/bin/env python3
"""Compare compiled hard sync against a band-limited periodic reference.

Reference harmonics come from a dense master period, then discard everything
at/above Nyquist. Error includes passband droop as well as aliasing: no gain,
phase, or spectrum normalization is fitted to make a candidate pass.
"""
import json
import hashlib
from pathlib import Path
import subprocess
import sys
import tempfile

import numpy as np
from check_filters import Instrument, ROOT
from check_voice import render


def run():
    inputs = '\n'.join(f'(def {name} (in {i} @name {name}))' for i, name in enumerate(
        ('gate', 'pitch', 'velocity', 'trigger', 'note_on', 'legato', 'pressure'), 1))
    source = (ROOT/'content/defmacros/heat-sync/macro.lisp').read_text()+'\n'+inputs+'''
(param ratio @default 1 @min 1 @max 16)
(param wave @default 1 @min 0 @max 2)
(param duty @default 0.5 @min 0.01 @max 0.99)
(out (heat-sync pitch ratio wave duty) 1)
'''
    cases = []
    with tempfile.TemporaryDirectory(prefix='heat-sync-') as folder:
        path = Path(folder)/'dsp.lisp'
        path.write_text(source)
        for sr in (44100, 48000, 96000):
            inst = Instrument(path, sample_rate=sr)
            subprocess.run([sys.executable, str(ROOT/'tools/audition/check_fusion.py'),
                            str(Path(inst.build_dir)/'patch.c')], check=True)
            for period in (16, 64, 128):
                hz = sr/period
                for wave in (0, 1, 2):
                    for ratio in (1, 2.37, 7.2, 16):
                        actual_ratio = min(ratio, .45*period)
                        def waveform(phase):
                            phase = np.mod(phase*actual_ratio, 1)
                            return (np.sin(2*np.pi*phase) if wave == 0 else
                                    2*phase-1 if wave == 1 else np.where(phase < .37, 1., -1.))
                        dense = waveform(np.arange(131072)/131072)
                        coeff = np.fft.rfft(dense)/len(dense)
                        limited = coeff[:period//2+1].copy()
                        limited[-1] = 0
                        reference = np.fft.irfft(limited*period, n=period)
                        naive = waveform(np.arange(period)/period)
                        y = render(inst, seconds=period*8/sr,
                                   notes=[(0, period*8/sr, hz, False)],
                                   params={'wave': wave, 'ratio': ratio, 'duty': .37})[:, 0]
                        measured = y[-period:]
                        error = float(np.sqrt(np.mean((measured-reference)**2)))
                        naive_error = float(np.sqrt(np.mean((naive-reference)**2)))
                        assert np.max(abs(measured)) < 1.5, (sr, period, wave, ratio)
                        # Integer-ratio sine has no reset discontinuity. Other cases
                        # must reduce total band-limited error by at least 20%.
                        if naive_error > 1e-5:
                            assert error < .8*naive_error, (sr, period, wave, ratio, error, naive_error)
                        cases.append(dict(sample_rate=sr, period=period, wave=wave, ratio=ratio,
                                          rms_error=error, naive_rms_error=naive_error))
    out = ROOT/'tools/heat/measurements/sync-development.json'
    out.write_text(json.dumps({'source_sha256': {'content/defmacros/heat-sync/macro.lisp': hashlib.sha256((ROOT/'content/defmacros/heat-sync/macro.lisp').read_bytes()).hexdigest()}, 'cases': cases}, indent=2)+'\n')
    print(f'{len(cases)} hard-sync spectrum cases passed')


if __name__ == '__main__':
    run()
