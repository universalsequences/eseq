#!/usr/bin/env python3
"""Verify cancellable unison onsets and complete-voice stereo detuning."""
import json
import hashlib
from pathlib import Path
import subprocess
import sys
import tempfile

import numpy as np
from check_filters import Instrument, ROOT
from check_voice import render, compile_source


def checked(path, source, sr):
    path.write_text(source)
    inst = Instrument(path, sample_rate=sr)
    subprocess.run([sys.executable, str(ROOT/'tools/audition/check_fusion.py'),
                    str(Path(inst.build_dir)/'patch.c')], check=True)
    return inst


def run():
    inputs = '\n'.join(f'(def {name} (in {i} @name {name}))' for i, name in enumerate(
        ('gate', 'pitch', 'velocity', 'trigger', 'note_on', 'legato', 'pressure'), 1))
    source = (ROOT/'content/defmacros/heat-unison-onset/macro.lisp').read_text()+'\n'+inputs+'''
(param delay_ms @default 0 @min 0 @max 300)
(param enabled @default 1 @min 0 @max 1)
(def (g n t l p v) (heat-unison-onset gate note_on trigger legato pitch velocity enabled delay_ms))
(out g 1)
(out n 2)
(out t 3)
(out l 4)
(out p 5)
'''
    cases = []
    with tempfile.TemporaryDirectory(prefix='heat-unison-') as folder:
        path = Path(folder)/'dsp.lisp'
        for sr in (44100, 48000, 96000):
            inst = checked(path, source, sr)
            for delay in (0, 7.5, 100, 300):
                y = render(inst, seconds=1, params={'delay_ms': delay},
                           notes=[(0, .8, 220, False), (.4, .8, 330, True)])
                onset = round(delay*sr/1000)
                assert np.array_equal(np.flatnonzero(y[:, 1]), [onset, round(.4*sr)])
                assert np.array_equal(np.flatnonzero(y[:, 2]), [onset])
                assert np.array_equal(np.flatnonzero(y[:, 3]), [round(.4*sr)])
                assert np.all(y[:onset, 0] == 0)
                assert np.all(y[onset:round(.8*sr), 0] == 1)
                assert np.all(y[round(.8*sr):, 0] == 0)
                cases.append(dict(sample_rate=sr, delay_ms=delay, onset=onset))
            cancelled = render(inst, seconds=.5, params={'delay_ms': 100},
                               notes=[(0, .05, 220, False)])
            assert np.count_nonzero(cancelled[:, :4]) == 0
            disabled = render(inst, seconds=.5, params={'enabled': 0})
            assert np.count_nonzero(disabled[:, :4]) == 0
            retrigger = render(inst, seconds=.5, params={'delay_ms': 100},
                               notes=[(0, .4, 220, False), (.05, .4, 440, False)])
            assert np.array_equal(np.flatnonzero(retrigger[:, 2]), [round(.15*sr)])
        # Isolate both sides of complete voices: with no detune/spread the
        # normalized sum equals one copy. Detune separates spectral peaks;
        # spread produces stereo difference without dropping either channel.
        inst = checked(path, compile_source()[0], 48000)
        common = {'unison_detune_cents': 0, 'unison_spread': 0,
                  'filter1_enabled': 0, 'filter2_enabled': 0, 'osc1_wave': 0}
        mono = render(inst, params=common)
        for count in (2, 3, 4):
            y = render(inst, params={**common, 'unison_voices': count})
            error = float(np.max(abs(y-mono)))
            assert error < 2e-7, (count, error)
            wide = render(inst, params={**common, 'unison_voices': count,
                                       'unison_detune_cents': 30, 'unison_spread': 1})
            stereo = float(np.sqrt(np.mean((wide[:, 0]-wide[:, 1])**2)))
            assert stereo > .003, (count, stereo)
            assert np.max(abs(wide[round(1.1*48000):])) < 1e-7
            cases.append(dict(voices=count, normalized_sum_error=error, stereo_difference_rms=stereo))
    (ROOT/'tools/heat/measurements/unison-development.json').write_text(json.dumps({'source_sha256': {'content/defmacros/heat-unison-onset/macro.lisp': hashlib.sha256((ROOT/'content/defmacros/heat-unison-onset/macro.lisp').read_bytes()).hexdigest()}, 'cases': cases}, indent=2)+'\n')
    print(f'{len(cases)} unison cases passed')


if __name__ == '__main__':
    run()
