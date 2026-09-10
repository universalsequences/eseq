#!/usr/bin/env python3
"""Reverse-piano timing and trip-hop phrase previews, entirely synthesized."""
import hashlib
import json

import numpy as np
import soundfile as sf

from analyze import hz
from common import HERE, SOURCE, instrument


def main():
    inst = instrument()
    sr = int(inst.sample_rate)
    voices = {p['id']: p['params'] for p in json.loads((SOURCE.parent.parent/'PM Piano.presets').read_text())['presets']}
    output = HERE/'output'
    output.mkdir(exist_ok=True)
    levels = {}

    def save(name, mix):
        peak = float(abs(mix).max())
        assert np.isfinite(mix).all() and peak < 1, (name, peak)
        sf.write(output/(name+'.wav'), mix, sr, subtype='PCM_24')
        levels[name] = {'peak': peak, 'seconds': len(mix)/sr}

    def note(mix, at, midi, duration, params, velocity=.8, gain=.42, tail=2):
        y, state = inst.render(duration+tail, pitch=hz(midi), vel=velocity,
                               params=params, gate_off=duration)
        assert np.isfinite(state).all()
        start = int(at*sr)
        count = min(len(y), len(mix)-start)
        mix[start:start+count] += y[:count]*gain

    # The same Cm9 voicing rises for 0.25, 1 and 3 seconds. Each ends promptly.
    reel = []
    for length in [.25, 1, 3]:
        mix = np.zeros((int((length+.65)*sr), 2))
        for midi in [48, 55, 58, 62, 67]:
            note(mix, .1, midi, length+.08, {**voices['reverse-dust'],
                 'swell.length_s': length, 'swell.curve': 1}, gain=.45, tail=.4)
        reel.append(mix)
    save('reverse-lengths', np.concatenate(reel))

    # Four 80 BPM bars. Reverse chords anticipate the same point in each bar;
    # dry bass and syncopated piano leave the rising texture easy to hear.
    beat = 60/80
    mix = np.zeros((int((16*beat+4)*sr), 2))
    chords = [(36, [51, 55, 58, 62]), (32, [51, 55, 58, 60]),
              (29, [51, 56, 60, 63]), (31, [50, 55, 57, 60])]
    for bar, (bass, chord) in enumerate(chords):
        start = bar*4*beat
        for j, midi in enumerate(chord):
            note(mix, start, midi, 2*beat+.12,
                 {**voices['reverse-dust'], 'swell.length_s': 2*beat, 'swell.tail': .5},
                 velocity=.86-j*.025, gain=.6, tail=1.6)
        note(mix, start+2*beat, bass, .75*beat, voices['soft-felt'], velocity=.88, gain=.65)
        for i, (step, midi) in enumerate(zip([2, 2.75, 3.5], [chord[2], chord[3], chord[1]])):
            note(mix, start+step*beat, midi+12, .3*beat, voices['soft-felt'],
                 velocity=.8-i*.07, gain=.6)
    save('reverse-trip-hop', mix)

    bloom = np.zeros((int(9*sr), 2))
    for midi in [36, 51, 55, 58, 62, 67]:
        note(bloom, 0, midi, 3.2, voices['blooming-felt'], velocity=.9, gain=.6, tail=5.8)
    save('reverse-felt-bloom', bloom)
    (HERE/'swell-demo-levels.json').write_text(json.dumps({
        'source_sha256': hashlib.sha256(SOURCE.read_bytes()).hexdigest(),
        'compiler_sha256': inst.compiler_sha256, 'clips': levels}, indent=2)+'\n')
    print(json.dumps(levels, indent=2))


if __name__ == '__main__':
    main()
