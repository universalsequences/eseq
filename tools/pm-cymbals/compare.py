#!/usr/bin/env python3
"""Render reference/model auditions and measure narrow spectral concentration."""
import argparse
import json

import numpy as np
import soundfile as sf

from common import FACTORY, HERE, NAMES, ROOT, digest, instrument, read_reference


def concentration(y, sr=48000):
    mono = np.mean(y, axis=1)
    window = mono[round(.02*sr):round(.3*sr)]
    hz = np.fft.rfftfreq(len(window), 1/sr)
    power = abs(np.fft.rfft(window*np.hanning(len(window))))**2
    power = np.sort(power[(hz >= 800) & (hz <= 16000)])[::-1]
    power /= max(float(power.sum()), 1e-30)
    return {'top_10_bin_power_fraction': float(power[:10].sum()),
            'bins_for_90_percent_power': int(np.searchsorted(np.cumsum(power), .9)+1)}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--installed', action='store_true')
    args = parser.parse_args()
    factory = FACTORY if args.installed else HERE/'output/staging/content/instruments/Physical Models'
    output = HERE/'output/comparisons'
    output.mkdir(parents=True, exist_ok=True)
    result = []
    for slug, name in NAMES.items():
        calibration = json.loads((HERE/f'{slug}-calibration.json').read_text())
        records = calibration['references']
        voices = [('default', records[len(records)//2], {})]
        if slug == 'hihat':
            by_name = {r['source'].rsplit('/', 1)[-1]: r for r in records}
            voices = [('closed', by_name['Hihat - Close_8.wav'], {'contact.openness': 0}),
                      ('open', by_name['Hihat - Open_3.wav'], {'contact.openness': 1})]
        path = factory/name/'dsp.lisp'
        inst = instrument(path)
        for articulation, row, params in voices:
            reference, _, _ = read_reference(ROOT/row['source'])
            seconds = min(5.5, max(2., len(reference)/48000))
            model, _ = inst.render(seconds, vel=.8, params=params)
            reference = np.pad(reference[:len(model)], ((0, max(0, len(model)-len(reference))), (0, 0)))
            if reference.shape[1] == 1:
                reference = np.repeat(reference, 2, axis=1)
            key = slug+'-'+articulation
            sf.write(output/(key+'-model.wav'), model, 48000, subtype='FLOAT')
            # Match early RMS for audition only. Apply a shared final gain to
            # leave peak headroom without changing the A/B loudness relation.
            rms = lambda x: max(float(np.sqrt(np.mean(x[:24000]**2))), 1e-12)
            a, b = reference/rms(reference), model/rms(model)
            gain = min(.04, .9/max(float(abs(a).max()), float(abs(b).max())))
            gap = np.zeros((14400, 2))
            audition = np.concatenate([a*gain, gap, b*gain, gap, a*gain, gap, b*gain])
            sf.write(output/(key+'-reference-model.wav'), audition, 48000, subtype='PCM_24')
            result.append({'family': slug, 'articulation': articulation,
                           'reference': row['source'], 'reference_sha256': row['sha256'],
                           'source_sha256': digest(path), 'compiler_sha256': inst.compiler_sha256,
                           'reference_concentration': concentration(reference),
                           'model_concentration': concentration(model),
                           'model_peak': float(abs(model).max()),
                           'audition_order': 'reference, model, reference, model; early RMS matched'})
    (HERE/'comparison.json').write_text(json.dumps(result, indent=2)+'\n')
    print(json.dumps(result, indent=2))


if __name__ == '__main__':
    main()
