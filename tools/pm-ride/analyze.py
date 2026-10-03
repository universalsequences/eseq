#!/usr/bin/env python3
"""Identify the ride cymbal of the "08. Forza G" reference sample.

Reads the exact library sample (stored by content hash; see LIBRARY): 10.1 s,
32.5 kHz stereo. The ride is panned hard left; whispered voice sits in the
right channel, so only the left channel is analysed. Writes analysis.json:
stroke onsets, the cymbal's shared ring poles and fast attack poles, every
stroke's complex residue on each pole, and each stroke's stick-contact noise
left after the modal fit. No PCM, recorded frame or envelope is written.
"""
import hashlib
import json
import sys
import time
from pathlib import Path

import numpy as np
import soundfile as sf
from scipy.optimize import nnls
from scipy.signal import butter, find_peaks, hilbert, sosfiltfilt

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
sys.path.insert(0, str(HERE))
import modal_fit as M

SAMPLE = ROOT/'.local/samples/2a9a7dbb2524bc928c0ddd2ad195df3c8d3f28690d14d84e17e1754f9cd2392d.wav'
LIBRARY = ROOT/'.local/samples.jsonl'
CHANNEL = 0                  # the ride is hard left; the whispers are right
HIGHPASS_HZ = 150            # below the ride's lowest mode (~180 Hz); removes 50 Hz bleed
STROKE_PROMINENCE_DB = 20    # stick contacts this far above their surroundings in 6-16 kHz
PRE_ONSET_S = 0.0005         # the stroke starts this long before its contact burst reaches 10%
BAND_STEP_HZ = 50            # ring-pole subbands: core width
BAND_REACH_HZ = 60           # poles this close to a band centre join its residue solve
SUBBAND_CUTOFF_HZ = 70
SUBBAND_RATE_HZ = 250
ESPRIT_ORDER = 20
ESPRIT_PENCIL = 40
RING_RATE = (0.3, 25.0)      # loss rates (1/s) of ring poles; faster decay is attack
MASK_S = (0.02, 0.03)        # subband samples this close before/after an onset are masked
ATTACK_POLES = 48
ATTACK_SECONDS = 0.08
ATTACK_FIT_FROM_HZ = 180
MAINS_HZ = 50
RING_MODES = 640             # ring poles the engine runs individually
ALLOCATION_ALPHA = 0.3       # mode budget per third octave ∝ count^(1-α) energy^α
THIRD_OCTAVES = 175*2**(np.arange(0, 6.7, 1/3))   # 175 Hz-17.6 kHz: mode budget and wash bands
WASH_SKIP_S = 0.002          # the stick click owns a stroke's first milliseconds
WASH_BUILDUP_S = (0.001, 0.004, 0.008, 0.016)
WASH_FAST_RATES = np.geomspace(12.0, 80.0, 7)
WASH_SLOW_RATES = np.geomspace(2.0, 15.0, 8)   # the kept ring modes span ~2-25/s; slower is the record's hiss


def library_reference():
    """Confirms the library entry still names this exact file."""
    sha = hashlib.sha256(SAMPLE.read_bytes()).hexdigest()
    assert sha == SAMPLE.stem, 'sample content no longer matches its hash name'
    rows = [json.loads(line) for line in LIBRARY.read_text().splitlines() if SAMPLE.stem in line]
    row = next(r for r in rows if r.get('hash') == SAMPLE.stem)
    return {'library': str(LIBRARY.relative_to(ROOT)), 'title': row['title'], 'tags': row['tags'],
            'sample': str(SAMPLE.relative_to(ROOT)), 'sha256': sha}


def detect_strokes(y, sr):
    """Stick contacts as 6-16 kHz bursts; returns (onset sample, prominence dB, peak dB)."""
    band = sosfiltfilt(butter(4, [6000, 0.48*sr], 'bandpass', fs=sr, output='sos'), y)
    win = int(0.001*sr)
    power = np.convolve(band**2, np.ones(win)/win, 'same')
    db = 10*np.log10(power + 1e-14)
    peaks, props = find_peaks(db, prominence=STROKE_PROMINENCE_DB, distance=int(0.05*sr))
    out = []
    for p, prom in zip(peaks, props['prominences']):
        a = p - int(0.005*sr)
        first = a + int(np.argmax(power[a:p + 1] >= 0.1*power[p]))
        out.append((first - int(round(PRE_ONSET_S*sr)), float(prom), float(db[p])))
    return out


def ring_poles(y, sr, onsets):
    """Shared ring poles of every subband (see modal_fit), and the subbands."""
    analytic = hilbert(y)
    bands, poles = [], []
    for centre in np.arange(175, 0.495*sr - BAND_STEP_HZ/2, BAND_STEP_HZ):
        b = M.Subband(analytic, sr, centre, SUBBAND_CUTOFF_HZ, SUBBAND_RATE_HZ)
        mask = b.t > onsets[0]
        for o in onsets:
            mask &= ~((b.t > o - MASK_S[0]) & (b.t < o + MASK_S[1]))
        z = M.esprit(b.z, M.free_segments(b.t, onsets, MASK_S[1], MASK_S[0]), ESPRIT_ORDER, ESPRIT_PENCIL)
        rate = -np.log(np.abs(z))*b.rate
        hz = np.angle(z)*b.rate/(2*np.pi) + centre
        core = (rate > RING_RATE[0]) & (rate < RING_RATE[1]) & (np.abs(hz - centre) <= BAND_STEP_HZ/2)
        core &= hz - centre < BAND_STEP_HZ/2
        # Near-stationary lines on the 50 Hz mains series are bleed from the
        # song (bass, hum), not cymbal modes.
        core &= ~((np.abs(hz - MAINS_HZ*np.round(hz/MAINS_HZ)) < 1.5) & (hz < 1000) & (rate < 2.0))
        poles += list(zip(hz[core], rate[core]))
        bands.append((centre, b, mask))
    poles = np.array(sorted(poles))
    return poles[:, 0], poles[:, 1], bands


def ring_residues(bands, onsets, hz, rate):
    """Every stroke's complex residue on a pole set, over the whole sample.

    Each band solves with every pole near it and keeps the residues of the
    poles in its own core, so neighbouring bands agree.
    """
    residues = np.zeros((len(hz), len(onsets)), complex)
    for centre, b, mask in bands:
        near = np.flatnonzero(np.abs(hz - centre) < BAND_REACH_HZ)
        if not len(near):
            continue
        c, _ = M.solve_complex(M.ring_basis(b.t[mask], onsets, hz[near] - centre, rate[near]), b.z[mask])
        c = c.reshape(len(onsets), -1)
        own = (hz[near] >= centre - BAND_STEP_HZ/2) & (hz[near] < centre + BAND_STEP_HZ/2)
        # Back from the band's demodulated frame to the signal's: e^{i2π fc t_s}.
        residues[near[own]] = (c[:, own]*np.exp(2j*np.pi*centre*onsets)[:, None]).T
    return residues


def select_ring(hz, rate, residues, budget=RING_MODES, edges=THIRD_OCTAVES):
    """The ring poles the engine runs individually.

    The budget is shared between third octaves in proportion to
    count^(1 - α) * energy^α, then each band keeps its most energetic poles.
    A pure energy ranking keeps 4 of the ~720 poles above 11 kHz and turns
    the sizzle into a few loud whistles; this keeps density across the
    spectrum for the same fit.
    """
    energy = (np.abs(residues)**2).sum(1)/(2*rate)
    bands = np.searchsorted(edges, hz)
    ids = sorted(set(bands))
    count = np.array([np.sum(bands == b) for b in ids])
    weight = count**(1 - ALLOCATION_ALPHA)*np.array([energy[bands == b].sum() for b in ids])**ALLOCATION_ALPHA
    quota = np.zeros(len(ids), int)
    while quota.sum() < budget:
        free = np.where(quota < count, weight, 0)
        share = free/free.sum()*(budget - quota.sum())
        add = np.minimum(np.floor(share).astype(int), count - quota)
        if not add.any():
            add[np.argmax(share)] = 1
        quota += add
    return np.sort(np.concatenate([np.flatnonzero(bands == b)[np.argsort(-energy[bands == b])[:q]]
                                   for b, q in zip(ids, quota)]))


def fit_wash(y, model, sr, onset_samples, edges=THIRD_OCTAVES):
    """Third-octave power the modes do not supply: the wash layer.

    The wash fills the record's power deficit, P_record - P_model, not
    the residual's power: where the modes have the right level but not the
    record's exact waveform (the chaotic first milliseconds), the residual
    holds the energy of both and adding it again would double it.

    Each band (rectangular, by FFT) is a stationary floor (the record's hiss,
    not reproduced) plus, per stroke, a fast and a slow power component. Both
    build up at a = 1/τ (the dense modes fill in after the hit as the plate's
    energy spreads); the fast one carries the early deficit, the slow one the
    dense floor between the partials that rings with the plate:
    P(t) = floor + Σ_s Σ_c p_sc a/(a - 2r_c) (e^{-2r_c(t - t_s)} - e^{-a(t - t_s)}).
    Rates and τ come from grids, p ≥ 0 by NNLS on relative error (rows
    weighted by the record's own band power), so the quiet late floor counts
    as much as the loud attack. A stroke's first WASH_SKIP_S belong to its
    click.
    """
    spectra = np.fft.rfft(y), np.fft.rfft(model)
    bins = np.fft.rfftfreq(len(y), 1/sr)
    hop = int(0.002*sr)
    smooth = int(0.006*sr)
    t = np.arange(0, len(y) - hop, hop)/sr
    onsets = onset_samples/sr
    live = t >= onsets[0]
    for o in onsets:
        live &= ~((t >= o - 0.003) & (t < o + WASH_SKIP_S))

    def columns(r, a):
        cols = []
        for o in onsets:
            u = np.maximum(t - o, 0)
            cols.append(np.where(t >= o, a/(a - 2*r)*(np.exp(-2*r*u) - np.exp(-a*u)), 0))
        return np.column_stack(cols)
    out = []
    for lo, hi in zip(edges, edges[1:]):
        record, modelled = [np.convolve(np.fft.irfft(np.where((bins >= lo) & (bins < hi), z, 0), len(y))**2,
                                        np.ones(smooth)/smooth, 'same')[(t*sr).astype(int)] for z in spectra]
        # Not clipped at zero: both powers beat, and clipping the negative
        # half would bias the deficit upward. NNLS keeps every power >= 0.
        deficit = record - modelled
        weight = 1/(record + 1e-3*record.max())
        fast_cache = {}
        best = None
        for tau in WASH_BUILDUP_S:
            for slow in WASH_SLOW_RATES:
                S = columns(slow, 1/tau)
                for fast in WASH_FAST_RATES:
                    key = (fast, tau)
                    if key not in fast_cache:
                        fast_cache[key] = columns(fast, 1/tau)
                    B = np.column_stack([np.ones(len(t)), fast_cache[key], S])
                    coef, err = nnls((B*weight[:, None])[live], (deficit*weight)[live])
                    if best is None or err < best[0]:
                        best = (err, fast, slow, tau, coef)
        _, fast, slow, tau, coef = best
        n = len(onsets)
        out.append({'band_hz': [round(float(lo), 2), round(float(hi), 2)], 'buildup_s': tau,
                    'fast_rate_per_s': round(float(fast), 4), 'slow_rate_per_s': round(float(slow), 4),
                    'floor_power': float(coef[0]), 'fast_power': [float(v) for v in coef[1:1 + n]],
                    'slow_power': [float(v) for v in coef[1 + n:]]})
    return out


def contact_noise(residual, onset, sr):
    """Stick contact left after the modal fit, in 2.5-16 kHz: click peak and decay."""
    band = sosfiltfilt(butter(4, [2500, 0.48*sr], 'bandpass', fs=sr, output='sos'), residual)
    win = int(0.0005*sr)
    power = np.convolve(band**2, np.ones(win)/win, 'same')
    seg = power[onset:onset + int(0.03*sr)]
    floor = float(np.median(power[onset + int(0.03*sr):onset + int(0.06*sr)]))
    seg = np.maximum(seg - floor, 1e-16)
    peak_at = int(np.argmax(seg[:int(0.004*sr)]))
    click = float(np.sqrt(seg[peak_at]))
    after = seg[peak_at:]
    t20 = int(np.argmax(after < after[0]*0.01)) or len(after)
    burst = residual[onset + max(0, peak_at - win):onset + peak_at + 2*win]
    N = 4096
    spec = np.abs(np.fft.rfft(burst*np.hanning(len(burst)), N))**2
    fr = np.fft.rfftfreq(N, 1/sr)
    sel = (fr > 2500) & (fr < 0.48*sr)
    centre = float(np.exp(np.sum(spec[sel]*np.log(fr[sel]))/np.sum(spec[sel])))
    return {'click_rms': click, 'click_decay_s': float(np.clip(t20/sr/(20/8.686), 0.0002, 0.006)),
            'centre_hz': centre, 'peak_ms': round(1000*peak_at/sr, 3)}


def as_sin_cos(c):
    """Re[c e^{iωt}] = Re(c) cos ωt - Im(c) sin ωt: the engine's (sin, cos) weights."""
    return [[round(float(-v.imag), 8), round(float(v.real), 8)] for v in c]


def main():
    ref = library_reference()
    x, sr = sf.read(SAMPLE)
    y = M.highpass(x[:, CHANNEL], sr, HIGHPASS_HZ)
    strokes = detect_strokes(y, sr)
    onset_samples = np.array([s for s, _, _ in strokes])
    onsets = onset_samples/sr
    print(len(strokes), 'strokes', np.round(onsets, 3), flush=True)

    t0 = time.time()
    hz, rate, bands = ring_poles(y, sr, onsets)
    every = ring_residues(bands, onsets, hz, rate)
    full_ring = M.ring_synthesis(hz, rate, every, onset_samples, len(y), sr)
    # The engine runs a subset; their residues are re-solved without the
    # others, so a kept pole never carries a dropped partner's cancelling
    # energy, and the wash is fitted to what the kept poles actually leave.
    keep = select_ring(hz, rate, every)
    energy = (np.abs(every)**2).sum(1)/(2*rate)
    kept_fraction = float(energy[keep].sum()/energy.sum())
    all_poles = len(hz)
    hz, rate = hz[keep], rate[keep]
    residues = ring_residues(bands, onsets, hz, rate)
    ring = M.ring_synthesis(hz, rate, residues, onset_samples, len(y), sr)
    print(f'{len(hz)} of {all_poles} ring poles in {time.time() - t0:.0f}s', flush=True)

    # Attack: what the rings leave in each stroke's first ATTACK_SECONDS, up
    # to the next stroke, fitted with shared fast poles.
    remaining = y - ring
    ends = list(onset_samples[1:] - int(0.0005*sr)) + [len(y)]
    windows = [remaining[o:min(o + int(ATTACK_SECONDS*sr), e)] for o, e in zip(onset_samples, ends)]
    t0 = time.time()
    seeds = M.seed_attack(windows, sr, ATTACK_POLES, ATTACK_FIT_FROM_HZ, 0.47*sr)
    af, ar, coefs, _ = M.fit_attack(windows, sr, seeds)
    print(f'{len(af)} attack poles in {time.time() - t0:.0f}s', flush=True)
    attack = np.zeros(len(y))
    for o, c in zip(onset_samples, coefs):
        n = min(len(y) - o, int(1.0*sr))   # the slowest attack pole is >100 dB down by then
        attack[o:o + n] += M.attack_basis(af, ar, n, sr)@c
    final = remaining - attack

    t0 = time.time()
    wash = fit_wash(y, ring + attack, sr, onset_samples)
    print(f'wash in {time.time() - t0:.0f}s', flush=True)

    def window_energy(sig, a, b):
        return sum(float(np.sum(sig[o + int(a*sr):o + int(b*sr)]**2)) for o in onset_samples)
    fit = {f'{int(a*1000)}-{int(b*1000)}ms': {
        'all_ring_poles_db': round(10*np.log10(window_energy(y - full_ring, a, b)/window_energy(y, a, b)), 2),
        'kept_ring_db': round(10*np.log10(window_energy(remaining, a, b)/window_energy(y, a, b)), 2),
        'kept_ring_attack_db': round(10*np.log10(window_energy(final, a, b)/window_energy(y, a, b)), 2)}
        for a, b in [(0, .01), (.01, .03), (.03, .06), (.06, .15), (.15, .3)]}
    print('residual energy', fit, flush=True)

    hits = []
    for s, (o, prom, peak) in enumerate(strokes):
        c = residues[:, s]
        hits.append({'index': s, 'onset_s': round(o/sr, 5), 'contact_prominence_db': round(prom, 2),
                     'contact_peak_db': round(peak, 2),
                     'ring_energy': float(np.sum(np.abs(c)**2/(2*rate))),
                     'contact': contact_noise(final, o, sr)})
    out = {'reference': ref, 'channel': CHANNEL, 'sample_rate': sr, 'highpass_hz': HIGHPASS_HZ,
           'residual_energy_db': fit, 'hits': hits, 'wash': wash,
           'ring': {'identified_poles': all_poles, 'kept_energy_fraction': round(kept_fraction, 5),
                    'hz': [round(float(v), 4) for v in hz], 'rate_per_s': [round(float(v), 5) for v in rate],
                    'sin_cos': [as_sin_cos(residues[:, s]) for s in range(len(strokes))]},
           'attack': {'hz': [round(float(v), 4) for v in af], 'rate_per_s': [round(float(v), 4) for v in ar],
                      'sin_cos': [[[round(float(c[q]), 8), round(float(c[len(af) + q]), 8)] for q in range(len(af))]
                                  for c in coefs]}}
    (HERE/'analysis.json').write_text(json.dumps(out, separators=(',', ':')) + '\n')
    np.save(ROOT/'.local/pm-ride/fit.npy', np.stack([y, ring, attack]))


if __name__ == '__main__':
    main()
