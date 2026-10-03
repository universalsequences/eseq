#!/usr/bin/env python3
"""Calibrate the buzz partials' radiation EQ from the first pluck's buzz spectrum.

With the fret physics fixed (buzz.json), each pass moves the third-octave
readout gains (1-13 kHz) toward the recorded buzz spectrum over 60-470 ms.
The readout is linear, so this changes what is heard, not the string/fret
dynamics. Stores buzz_eq_ln in calibration.json; rebuild after each pass.
"""
import json
import subprocess
import sys
from pathlib import Path

import numpy as np

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))


def main(passes=4, step=0.8):
    from build import CALIBRATION, calibration
    for i in range(passes):
        subprocess.run([sys.executable, str(HERE/'build.py')], check=True, capture_output=True)
        out = subprocess.run([sys.executable, '-c', 'import sys; sys.path.insert(0, %r); sys.path.insert(0, %r);'
                              'import buzz_spectrum, json; r = buzz_spectrum.reference(); m = buzz_spectrum.model({});'
                              'print(json.dumps((m - r).tolist()))' % (str(HERE), str(HERE.parents[1]/'.local/pm-guitar'))],
                             check=True, capture_output=True, text=True)
        err = np.array(json.loads(out.stdout.strip().splitlines()[-1]))
        print(f'pass {i}: third-octave error', err.round(1))
        model = json.loads((HERE/'model.json').read_text())
        cal = calibration(len(model['eq_knots_hz']))
        g = np.array(cal.get('buzz_eq_ln', [0.0]*len(err)))
        upd = -step*np.clip(err, -30, 30)*np.log(10)/20
        upd[-1] = 0.0          # 16 kHz sits at the record's floor
        cal['buzz_eq_ln'] = np.clip(g + upd, -6, 4).round(4).tolist()
        CALIBRATION.write_text(json.dumps(cal, indent=1) + '\n')
    subprocess.run([sys.executable, str(HERE/'build.py')], check=True, capture_output=True)


if __name__ == '__main__':
    main()
