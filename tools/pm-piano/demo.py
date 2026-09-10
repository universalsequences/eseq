#!/usr/bin/env python3
"""Render polyphonic piano previews using only the synthesized instrument."""
import json

import numpy as np
import soundfile as sf

from analyze import hz
from common import HERE, SOURCE, instrument


def main():
    inst = instrument()
    sr = int(inst.sample_rate)
    output = HERE/'output'
    output.mkdir(exist_ok=True)
    presets = json.loads((SOURCE.parent.parent/'PM Piano.presets').read_text())['presets']
    voices = {p['id']: p['params'] for p in presets}
    levels = {}

    def save(name, y):
        peak = float(abs(y).max())
        assert np.isfinite(y).all() and peak < 1, (name, peak)
        sf.write(output/(name+'.wav'), y, sr, subtype='PCM_24')
        levels[name] = {'peak': peak, 'seconds': len(y)/sr}

    def note(mix, at, midi, duration, velocity, params, tail=2.4, gain=.48):
        y, state = inst.render(duration+tail, pitch=hz(midi), vel=velocity,
                               params=params, gate_off=duration)
        assert np.isfinite(state).all()
        start = int(at*sr)
        end = min(len(mix), start+len(y))
        mix[start:end] += y[:end-start]*gain

    # Four syncopated bars, with separate bass, chord and melody voices.
    beat = 60/98
    harmony = [(29, [56, 60, 63, 67]), (25, [56, 60, 65, 68]),
               (27, [55, 61, 65, 72]), (24, [55, 58, 62, 67])]
    melody = [[72, 75, 72, 70], [68, 72, 77, 75], [74, 72, 70, 67], [70, 67, 62, 60]]
    for key in ['reference-grand', 'soft-felt']:
        mix = np.zeros((int((16*beat+4)*sr), 2), dtype=np.float64)
        params = {**voices[key], 'damper.release_s': 1.4}
        for bar, ((bass, chord), tune) in enumerate(zip(harmony, melody)):
            start = bar*4*beat
            for step, pitch, vel in [(0, bass, .85), (1.5, bass+12, .63), (2.75, bass+7, .7)]:
                note(mix, start+step*beat, pitch, .5*beat, vel, params)
            for step, vel in [(.5, .67), (2, .75), (3.5, .57)]:
                for i, pitch in enumerate(chord):
                    note(mix, start+step*beat+i*.009, pitch, .34*beat, vel-i*.025, params)
            for step, pitch, vel in zip([.85, 1.6, 2.85, 3.6], tune, [.68, .55, .72, .6]):
                note(mix, start+step*beat, pitch, .34*beat, vel, params)
        save('groove-'+key, mix)

    reel = []
    for preset in presets:
        if preset['params']['swell.amount']:
            continue  # Swell presets have timed, held-note demos in demo_swell.py.
        clip = np.zeros((int(3.6*sr), 2), dtype=np.float64)
        for i, pitch in enumerate([36, 55, 60, 63, 67, 72]):
            note(clip, i*.12, pitch, .5, .8, preset['params'], gain=.4)
        reel.append(clip)
    save('seven-voices', np.concatenate(reel))

    # Identical notes: 50 ms, 500 ms and 5 s release, then fully lifted dampers.
    release = []
    for seconds, pedal in [(.05, 0), (.5, 0), (5, 0), (.05, 1)]:
        clip = np.zeros((int(3.5*sr), 2), dtype=np.float64)
        note(clip, 0, 60, .35, 1, {**voices['reference-grand'],
             'damper.release_s': seconds, 'damper.pedal': pedal}, tail=3.15, gain=.8)
        release.append(clip)
    save('release-comparison', np.concatenate(release))
    (HERE/'demo-levels.json').write_text(json.dumps(levels, indent=2)+'\n')
    print(json.dumps(levels, indent=2))


if __name__ == '__main__':
    main()
