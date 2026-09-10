#!/usr/bin/env python3
"""Compile a modal reduction into continuous register/excitation tables."""
import argparse
import hashlib
import json

import numpy as np
from scipy.optimize import linear_sum_assignment

from common import HERE, SOURCE

START = ';; BEGIN GENERATED CALIBRATION'
END = ';; END GENERATED CALIBRATION'
MODES = 24
KEYS = [74, 75, 77, 80, 81, 82, 84]


def coefficients():
    data = json.loads((HERE/'reference-analysis.json').read_text())
    assert [b['bar'] for b in data['bars']] == list(range(1, 8))
    arrays = {name: np.zeros((7, MODES)) for name in ['ratio', 'rate', 'rise', 'direct']}
    amplitude = np.zeros((7, 5, MODES))
    for row, bar in enumerate(data['bars']):
        ratio = np.array(bar['frequencies_hz'])/bar['fundamental_hz']
        amp = np.array([r['modal_amplitudes'] for r in bar['recordings']])
        energy = np.mean(amp**2, axis=0)/np.array(bar['rates_per_s'])
        if row == 0:
            modes = np.argsort(-energy)
            slots = np.arange(len(modes))
        else:
            # Continue modes by frequency and prominence, including through
            # weak/missing observations. This avoids changing mode identity
            # whenever one register has an extra weak partial in its spectrum.
            prev = arrays['ratio'][row-1]
            old_energy = np.mean(amplitude[row-1]**2, axis=0)/np.maximum(arrays['rate'][row-1], .15)
            cost = np.log(np.maximum(prev[:, None], .1)/ratio[None, :])**2
            cost += .015*np.log(np.maximum(old_energy[:, None], 1e-10)/np.maximum(energy[None, :], 1e-10))**2
            cost[prev == 0] = 2
            cost[0, 1:] = 1e6
            cost[1:, 0] = 1e6
            slots, modes = linear_sum_assignment(cost)
        for slot, mode in zip(slots, modes):
            arrays['ratio'][row, slot] = ratio[mode]
            arrays['rate'][row, slot] = bar['rates_per_s'][mode]
            arrays['rise'][row, slot] = bar['rise_seconds'][mode]
            arrays['direct'][row, slot] = bar['direct_fraction'][mode]
            amplitude[row, :, slot] = amp[:, mode]
    for slot in range(MODES):
        present = np.flatnonzero(arrays['ratio'][:, slot] > 0)
        if not len(present):
            for name, value in [('ratio', 1), ('rate', 1), ('rise', .001), ('direct', 1)]:
                arrays[name][:, slot] = value
        else:
            for name, a in arrays.items():
                a[:, slot] = np.interp(np.arange(7), present, a[present, slot])
    arrays['amplitude'] = amplitude.reshape(35, MODES)
    arrays['tuning'] = np.array([1200*np.log2(b['fundamental_hz']/(440*2**((key-69)/12)))
                                  for key, b in zip(KEYS, data['bars'])])
    arrays['mode_pan'] = np.sin(np.arange(MODES)*2.39996323)*.7
    arrays['mode_pan'][0] = 0
    return arrays


def tables():
    result = [START,
              ';; Latent Sonorities / memeshift: source CC BY-NC 4.0; see ATTRIBUTION.md.',
              ';; Seven bars, five ordinal strike strengths, one 24-mode passive system.',
              ';; No PCM, recorded phases, or per-frame spectral data.',
              f';; Analysis SHA256: {hashlib.sha256((HERE/"reference-analysis.json").read_bytes()).hexdigest()}']
    for name, a in coefficients().items():
        assert np.isfinite(a).all()
        shape = ' '.join(str(n) for n in a.shape)
        result.append(f'(def {name}_table (tensor @shape [{shape}] @data [')
        for row in a.reshape(-1, a.shape[-1]):
            result.append('  '+' '.join(f'{x:.9g}' for x in row))
        result.append(']))')
    return '\n'.join(result)+'\n'+END


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--check', action='store_true')
    args = parser.parse_args()
    source = SOURCE.read_text()
    a, b = source.index(START), source.index(END)+len(END)
    updated = source[:a]+tables()+source[b:]
    if args.check:
        assert updated == source, 'Regenerate the saron calibration tables'
    else:
        SOURCE.write_text(updated)


if __name__ == '__main__':
    main()
