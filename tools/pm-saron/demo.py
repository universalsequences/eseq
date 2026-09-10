#!/usr/bin/env python3
"""Render an identified pelog phrase and all six presets, without samples."""
import json

import numpy as np
import soundfile as sf

from calibrate import KEYS
from common import HERE, SOURCE, instrument
from compare import rms


def main():
    inst = instrument()
    sr = int(inst.sample_rate)
    output = HERE/'output'
    output.mkdir(exist_ok=True)
    presets = json.loads((SOURCE.parent.parent/'PM Saron.presets').read_text())['presets']
    phrase = [0, 2, 1, 4, 3, 5, 6, 4, 5, 2, 1, 3, 0, 4, 2, 0]
    clips, levels = [], []
    for preset in presets:
        mix = np.zeros((int(8.5*sr), 2), dtype=np.float64)
        for i, bar in enumerate(phrase):
            start = int(i*.32*sr)
            vel = [.6, .8, .55, .7][i % 4]
            key = KEYS[bar]+preset['base_note_offset']
            y, state = inst.render(3.4, pitch=440*2**((key-69)/12), vel=vel,
                                   params=preset['params'], gate_off=.28)
            assert np.isfinite(y).all() and np.isfinite(state).all()
            mix[start:start+len(y)] += y
        assert abs(mix).max() < 1, (preset['id'], 'demo must not clip')
        sf.write(output/f'phrase-{preset["id"]}.wav', mix, sr, subtype='PCM_24')
        levels.append({'preset': preset['id'], 'peak': float(abs(mix).max()), 'rms': rms(mix)})
        clips.extend([mix, np.zeros((sr//2, 2))])
    sf.write(output/'six-voices.wav', np.concatenate(clips), sr, subtype='PCM_24')
    (HERE/'demo-levels.json').write_text(json.dumps(levels, indent=2)+'\n')
    print('Six synthesized phrases; peak', max(p['peak'] for p in levels), flush=True)


if __name__ == '__main__':
    main()
