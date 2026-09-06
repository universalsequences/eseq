#!/usr/bin/env python3
"""Check compiled pitch-decay duration, polarity and note-on retrigger semantics.

This validates Heat's development law, not measured Analog sonic parity.
"""
import json
from pathlib import Path
import subprocess
import sys
import tempfile

import numpy as np
from check_filters import Instrument, ROOT
from check_voice import render


def run():
    macro = ROOT / 'content/defmacros/heat-pitch-envelope/macro.lisp'
    inputs = '\n'.join(f'(def {name} (in {i} @name {name}))' for i, name in enumerate(
        ('gate', 'pitch', 'velocity', 'trigger', 'note_on', 'legato', 'pressure'), 1))
    source = macro.read_text() + '\n' + inputs + '''
(param initial @default 0 @min -48 @max 48)
(param time_ms @default 500 @min 0 @max 15000)
(out (heat-pitch-envelope note_on initial time_ms) 1)
'''
    cases = []
    with tempfile.TemporaryDirectory(prefix='heat-pitch-') as folder:
        path = Path(folder) / 'dsp.lisp'
        path.write_text(source)
        for sr in (44100, 48000, 96000):
            inst = Instrument(path, sample_rate=sr)
            subprocess.run([sys.executable, str(ROOT / 'tools/audition/check_fusion.py'),
                            str(Path(inst.build_dir) / 'patch.c')], check=True)
            for ms in (0, 1, 37.5, 500, 15000):
                for initial in (-48, 0, 48):
                    # Second physical note-on is legato: trigger stays zero.
                    y = render(inst, seconds=1, params={'initial': initial, 'time_ms': ms},
                               notes=[(0, .25, 220, False), (.25, .75, 330, True)])[:, 0]
                    frames = np.arange(sr, dtype=np.float64)
                    frames[round(.25 * sr):] -= round(.25 * sr)
                    duration = round(ms * sr / 1000)
                    t = np.minimum(frames / max(1, duration), 1)
                    expected = initial * (np.exp(-5 * t) - np.exp(-5)) / (1 - np.exp(-5))
                    expected[frames >= duration] = 0
                    error = float(np.max(np.abs(y - expected)))
                    assert error < 2e-5, (sr, ms, initial, error)
                    assert np.all(y[frames >= duration] == 0), (sr, ms, initial)
                    cases.append({'sample_rate': sr, 'time_ms': ms, 'initial': initial, 'max_error': error})
    out = ROOT / 'tools/heat/measurements/pitch-envelope-development.json'
    out.write_text(json.dumps({'scope': 'Development law; Analog timing calibration pending', 'cases': cases}, indent=2) + '\n')
    print(f'{len(cases)} compiled pitch-envelope cases passed')


if __name__ == '__main__':
    run()
