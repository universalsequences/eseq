#!/usr/bin/env python3
"""Model-only phrases; no reference audio, normalization, limiter or reverb."""
import hashlib
import json

import numpy as np
import soundfile as sf

from common import instrument
from families import FACTORY, FAMILIES, HERE


def main():
    sr = 48000
    result, medley = [], []
    for slug, family in FAMILIES.items():
        inst = instrument(family.source)
        keys = [unit[1] for unit in family.units]
        notes = [keys[i % len(keys)] for i in [0, 2, 1, 4, 3, 6, 4, 0]]
        if len(keys) == 1:
            notes = keys*8
        presets = json.loads((FACTORY/(family.name+'.presets')).read_text())['presets']
        for preset in presets:
            phrase = np.zeros((int(8*sr), 2), dtype=np.float64)
            for index, note in enumerate(notes):
                start = int(index*.55*sr)
                audio, _ = inst.render(3, pitch=440*2**((note-69)/12), vel=[.8, .6, .9, 1][index % 4],
                                       params=preset['params'], gate_off=.42)
                phrase[start:start+len(audio)] += audio
            peak = float(abs(phrase).max())
            assert np.isfinite(phrase).all() and 1e-5 < peak < 1, (slug, preset['id'], peak)
            path = HERE/'output'/slug/f'phrase-{preset["id"]}.wav'
            path.parent.mkdir(parents=True, exist_ok=True)
            sf.write(path, phrase, sr, subtype='FLOAT')
            result.append({'instrument': family.name, 'preset': preset['id'], 'peak': peak,
                           'source_sha256': hashlib.sha256(family.source.read_bytes()).hexdigest()})
            if preset['id'] == 'reference':
                medley.extend([phrase, np.zeros((sr//2, 2))])
        print(slug, 'six preset phrases rendered', flush=True)
    sf.write(HERE/'output/five-models.wav', np.concatenate(medley), sr, subtype='FLOAT')
    (HERE/'demo-levels.json').write_text(json.dumps(result, indent=2)+'\n')


if __name__ == '__main__':
    main()
