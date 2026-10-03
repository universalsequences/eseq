#!/usr/bin/env python3
"""Identify six rides from the Donit pack, each as its own measured plate.

PM Ride release 1 (tools/pm-cymbals) interpolated fitted parameters between
recordings that, by their partials, are different physical cymbals (29 files,
centroids 0.3-7.6 kHz, shared partials at chance level). Release 2 keeps every
cymbal intact: Character chooses a cymbal, it never averages two.

Per reference, with the PM Ride Kit identification (tools/pm-ride): ring
poles by multi-segment ESPRIT, density-preserving third-octave selection and
re-solved residues of the kept poles, fast attack poles, the deficit-fitted
wash and the stick click. Writes analysis.json (coefficients only; no PCM,
recorded phase or envelopes).
"""
import hashlib
import importlib.util
import json
import sys
import time
from pathlib import Path

import numpy as np
import soundfile as sf

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
KIT = ROOT/'tools/pm-ride'
sys.path.insert(0, str(KIT))
import modal_fit as M  # noqa: E402

_spec = importlib.util.spec_from_file_location('ride_kit_analysis', KIT/'analyze.py')
K = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(K)

SOURCE = ROOT/'samples-to-analyze/Acoustic Cymbals Vol.1 by Donit'
# Character order, dark to bright (spectral centroid). Chosen from the 29
# rides: unclipped, a single clean strike, a ring of ~2 s to -30 dB; the
# clipped files (Ride_2/4/6/7/9/25) and the 300 Hz thud (Ride_5) are out.
CYMBALS = [
    ('Cymbal - Ride_18.wav', 'dark', 'Dark'),
    ('Cymbal - Ride_22.wav', 'warm', 'Warm'),
    ('Cymbal - Ride_11.wav', 'classic', 'Classic'),
    ('Cymbal - Ride_12.wav', 'dry', 'Dry'),
    ('Cymbal - Ride_10.wav', 'bright', 'Bright'),
    ('Cymbal - Ride.wav', 'crisp', 'Crisp'),
]
HIGHPASS_HZ = 100
RING_MODES = 448
ATTACK_POLES = 32
ATTACK_SECONDS = 0.08
LATE_SEGMENT_S = 0.37        # a second ESPRIT pass from here finds the poles that rule the late ring
SELECT_AFTER_S = 0.15        # poles are ranked by their energy after this, the audible tail
BANDS = 175*2**(np.arange(0, 7.1, 1/3))      # third octaves 175 Hz-22 kHz (44.1 kHz references)


def first_strike(y, sr):
    """The files start at the strike: where |x| first reaches 10% of its early peak, less 0.5 ms."""
    early = np.abs(y[:int(0.05*sr)])
    return max(0, int(np.argmax(early >= 0.1*early.max())) - int(0.0005*sr))


def ring_poles(y, sr, onsets):
    """Ring poles from the strike's free ring and, again, from its late ring.

    A single long hit's subspace (ESPRIT order 20 per 50 Hz band) is filled by
    the modes that are loud early; those that rule the late ring are found by
    a second pass from LATE_SEGMENT_S. Near-duplicates (within 0.4 Hz and
    35% in rate) are merged. Returns (hz, rate, subbands of the first pass).
    """
    hz, rate, bands = K.ring_poles(y, sr, onsets)
    late_hz, late_rate, _ = K.ring_poles(y, sr, onsets + LATE_SEGMENT_S)
    poles = np.array(sorted(list(zip(hz, rate)) + list(zip(late_hz, late_rate))))
    kept = [0]
    for i in range(1, len(poles)):
        j = kept[-1]
        if abs(poles[i, 0] - poles[j, 0]) < 0.4 and abs(np.log(poles[i, 1]/poles[j, 1])) < 0.3:
            continue
        kept.append(i)
    return poles[kept, 0], poles[kept, 1], bands


def identify(path, sr_expected=44100):
    x, sr = sf.read(path)
    assert sr == sr_expected and x.ndim == 1, (path, sr, x.shape)
    assert np.abs(x).max() < 0.99, f'{path} clips'
    y = M.highpass(x, sr, HIGHPASS_HZ)
    first = first_strike(y, sr)
    later = [s for s, _, _ in K.detect_strokes(y, sr) if s > first + int(0.08*sr)]
    onset_samples = np.array([first] + later)
    onsets = onset_samples/sr

    t0 = time.time()
    hz, rate, bands = ring_poles(y, sr, onsets)
    every = K.ring_residues(bands, onsets, hz, rate)
    keep = K.select_ring(hz, rate, every*np.exp(-rate*SELECT_AFTER_S)[:, None], RING_MODES, BANDS)
    energy = (np.abs(every)**2).sum(1)/(2*rate)
    kept_fraction = float(energy[keep].sum()/energy.sum())
    identified = len(hz)
    hz, rate = hz[keep], rate[keep]
    residues = K.ring_residues(bands, onsets, hz, rate)
    ring = M.ring_synthesis(hz, rate, residues, onset_samples, len(y), sr)

    remaining = y - ring
    ends = list(onset_samples[1:] - int(0.0005*sr)) + [len(y)]
    windows = [remaining[o:min(o + int(ATTACK_SECONDS*sr), e)] for o, e in zip(onset_samples, ends)]
    seeds = M.seed_attack(windows, sr, ATTACK_POLES, 180, 0.47*sr)
    af, ar, coefs, _ = M.fit_attack(windows, sr, seeds)
    attack = np.zeros(len(y))
    for o, c in zip(onset_samples, coefs):
        n = min(len(y) - o, int(1.0*sr))
        attack[o:o + n] += M.attack_basis(af, ar, n, sr)@c
    wash = K.fit_wash(y, ring + attack, sr, onset_samples, BANDS)
    final = remaining - attack

    def window_energy(sig, a, b):
        o = onset_samples[0]
        return float(np.sum(sig[o + int(a*sr):o + int(b*sr)]**2))
    fit = {f'{int(a*1000)}-{int(b*1000)}ms': round(10*np.log10(window_energy(final, a, b)/window_energy(y, a, b)), 2)
           for a, b in [(0, .01), (.01, .03), (.03, .1), (.1, .3), (.3, 1.0)]}
    print(f'{path.name}: {len(onsets)} strikes, {len(hz)} of {identified} ring poles, kept energy {kept_fraction:.3f}, '
          f'residual {fit} ({time.time() - t0:.0f}s)', flush=True)
    s = 0   # the file's first strike is the voice; later strikes only keep the fit honest
    return {'file': str(path.relative_to(ROOT)), 'sha256': hashlib.sha256(path.read_bytes()).hexdigest(),
            'sample_rate': sr, 'duration_s': round(len(x)/sr, 4), 'peak': round(float(np.abs(x).max()), 5),
            'onsets_s': [round(float(o), 5) for o in onsets], 'residual_energy_db': fit,
            'ring': {'identified_poles': identified, 'kept_energy_fraction': round(kept_fraction, 5),
                     'hz': [round(float(v), 4) for v in hz], 'rate_per_s': [round(float(v), 5) for v in rate],
                     'sin_cos': K.as_sin_cos(residues[:, s]),
                     'energy': float(np.sum(np.abs(residues[:, s])**2/(2*rate)))},
            'attack': {'hz': [round(float(v), 4) for v in af], 'rate_per_s': [round(float(v), 4) for v in ar],
                       'sin_cos': [[round(float(coefs[s][q]), 8), round(float(coefs[s][len(af) + q]), 8)] for q in range(len(af))]},
            'wash': [{k: (v[s] if isinstance(v, list) and k.endswith('_power') else v) for k, v in w.items()} for w in wash],
            'contact': K.contact_noise(final, int(onset_samples[s]), sr)}


def main():
    names = sys.argv[1:]
    path = HERE/'analysis.json'
    out = json.loads(path.read_text()) if path.exists() and names else {'references': {}}
    out['method'] = 'PM Ride Kit identification per reference cymbal (tools/pm-ride); first strike is the voice'
    for file, slug, name in CYMBALS:
        if names and slug not in names:
            continue
        out['references'][slug] = identify(SOURCE/file) | {'name': name}
    out['references'] = {slug: out['references'][slug] for _, slug, _ in CYMBALS if slug in out['references']}
    path.write_text(json.dumps(out, separators=(',', ':')) + '\n')


if __name__ == '__main__':
    main()
