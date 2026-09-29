#!/usr/bin/env python3
"""Identify the bongo kit's modal strokes from the bongo-breaks reference loop.

Reads the exact sample the `bongo-breaks` project's sampler track plays
("A4 Bird Of Prey.flac", stored by content hash). Writes analysis.json with,
per reference hit: strike delays, modal frequencies, loss rates, per-strike
per-channel residues and the contact-noise envelope left after the modal fit.
No PCM, recorded phase or spectral frame is written.
"""
import hashlib
import json
import sys
import time
from pathlib import Path

import numpy as np
import soundfile as sf
from scipy.signal import resample_poly, stft, find_peaks, butter, sosfiltfilt

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
sys.path.insert(0, str(HERE))
import modal_fit as M

SAMPLE = ROOT/'.local/samples/610ba5adaf1687e2350062b641f38b9dbddc1685f3db2de201439871576d8d06.wav'
PROJECT = ROOT/'.local/projects/bongo-breaks.json'
FIT_DECIMATION = 2          # 32.5 kHz source -> 16.25 kHz modal fit (modes above ~6 kHz become contact noise)
MODES_PER_HIT = 32
MAX_STRIKES = 3
MAX_SECONDS = 0.32
STRIKE_GAIN_DB = 0.8        # an extra strike must lower the residual at least this much

# Reference hits in loop order. Onsets closer than 45 ms are one gesture
# (a flam across heads) and are identified together as extra strikes.
HITS = [
    ('low-open', 'Low Open'),
    ('slap-ghost', 'Slap + Ghost'),
    ('low-ghost', 'Low Ghost'),
    ('press-tone', 'Pressed Tone'),
    ('mid-open-flam', 'Mid Open Flam'),
    ('mid-open', 'Mid Open'),
    ('mid-open-short', 'Mid Open Short'),
    ('mid-ghost', 'Mid Ghost'),
    ('mid-open-2', 'Mid Open 2'),
    ('low-open-2', 'Low Open 2'),
    ('slap-low-flam', 'Slap Low Flam'),
    ('high-low-muted', 'High Low Muted'),
    ('high-slap-flam', 'High Slap Flam'),
]


def project_reference():
    """Confirms the bongo-breaks sampler track still plays this exact file."""
    data = json.loads(PROJECT.read_text())
    tracks = [t for t in data['tracks'] if t.get('sample_path', '').endswith(SAMPLE.name)]
    assert tracks, 'bongo-breaks no longer references the analysed sample'
    return {'project': str(PROJECT.relative_to(ROOT)), 'track': tracks[0]['name'],
            'sample': str(SAMPLE.relative_to(ROOT)),
            'sha256': hashlib.sha256(SAMPLE.read_bytes()).hexdigest()}


def detect_onsets(mono, sr):
    hop = 64
    f, t, Z = stft(mono, sr, nperseg=512, noverlap=512-hop)
    flux = np.maximum(0, np.diff(np.log(np.abs(Z) + 1e-6), axis=1)).sum(0)
    flux /= flux.max()
    peaks, _ = find_peaks(flux, height=0.15, distance=int(0.05*sr/hop))
    onsets = []
    for p in peaks:
        i0 = int((t[p+1] - 0.015)*sr)
        seg = np.abs(mono[i0:i0 + int(0.04*sr)])
        onsets.append((i0 + int(np.argmax(seg > 0.15*seg.max())))/sr)
    return onsets


def gestures(onsets):
    groups = []
    for o in onsets:
        if groups and o - groups[-1][-1] < 0.045:
            groups[-1].append(o)
        else:
            groups.append([o])
    return groups


def fit_strikes(y, sr, known):
    """Greedy strike search. Known onsets inside the gesture seed the search."""
    freqs = M.seed_frequencies(y, sr, MODES_PER_HIT)
    f, r, C, res = M.fit_hit(y, sr, freqs)
    delays = []
    err = (res**2).sum()
    candidates = np.arange(0.003, min(0.08, len(y)/sr - 0.01), 0.0005)
    while len(delays) + 1 < MAX_STRIKES:
        best = None
        for d in candidates:
            if any(abs(d - e) < 0.004 for e in delays):
                continue
            trial = sorted(delays + [d])
            e = (M.solve(f, r, y, sr, trial)[2]**2).sum()
            if best is None or e < best[0]:
                best = (e, d)
        for d in known:
            if all(abs(d - e) >= 0.004 for e in delays):
                e = (M.solve(f, r, y, sr, sorted(delays + [d]))[2]**2).sum()
                if e < best[0]:
                    best = (e, d)
        if best is None or 10*np.log10(err/best[0]) < STRIKE_GAIN_DB:
            break
        delays = sorted(delays + [best[1]])
        f, r, C, res = M.fit_hit(y, sr, f, r, delays)
        err = (res**2).sum()
    return f, r, C, res, delays


def ringing(f, r, C, begin, end, sr, delays):
    """A fitted hit's modal response from its window start to the end of the loop."""
    n = end - begin
    out = np.zeros((n, C.shape[1]))
    for start in range(0, n, 16384):
        stop = min(n, start + 16384)
        t = np.arange(start, stop)/sr
        cols = []
        for d in [0.0] + list(delays):
            tt = np.maximum(t - d, 0)
            e = np.exp(-np.outer(tt, r))*(t >= d)[:, None]
            cols += [e*np.sin(2*np.pi*np.outer(tt, f)), e*np.cos(2*np.pi*np.outer(tt, f))]
        out[start:stop] = np.hstack(cols)@C
    return out


def contact_click(mono, onset, sr):
    """Hand-on-skin contact measured in the record's 2.5-12 kHz band.

    The modes carry almost nothing up there: a click that falls ~20 dB within a
    few ms, then a quieter sizzle. Levels are RMS amplitudes; the pre-onset
    floor (hiss) is removed in power.
    """
    sos = butter(4, [2500, min(12000, 0.45*sr)], 'bandpass', fs=sr, output='sos')
    band = sosfiltfilt(sos, mono)
    win = int(0.0005*sr)
    power = np.convolve(band**2, np.ones(win)/win, 'same')
    o = int(onset*sr)
    floor = float(np.median(power[o - int(0.02*sr):o - int(0.003*sr)]))
    seg = np.maximum(power[o - int(0.001*sr):o + int(0.06*sr)] - floor, 1e-16)
    peak_at = int(np.argmax(seg[:int(0.006*sr)]))
    click = float(np.sqrt(np.mean(seg[max(0, peak_at - win):peak_at + win])))
    after = seg[peak_at:]
    t20 = int(np.argmax(after < after[0]*10**(-2.0)))
    click_decay = float(np.clip(t20/sr/(20/8.686), 0.0003, 0.004))
    ms = lambda a, b: slice(peak_at + int(a*sr), peak_at + int(b*sr))
    sizzle = float(np.sqrt(np.median(seg[ms(0.005, 0.02)])))
    tail = seg[ms(0.025, 0.045)]
    late = float(np.sqrt(np.median(tail))) if len(tail) else 0.0   # loop may end first
    sizzle_decay = float(np.clip(0.025/np.log(max(sizzle/max(late, 1e-9), 1.05)), 0.008, 0.06))
    edges = [1000, 2000, 4000, 8000, 16000]
    spectrum = []
    burst = mono[o - int(0.0005*sr) + peak_at:o + int(0.0015*sr) + peak_at]
    spec = np.abs(np.fft.rfft(burst*np.hanning(len(burst)), 4096))**2
    fr = np.fft.rfftfreq(4096, 1/sr)
    for lo, hi in zip(edges, edges[1:]):
        sel = (fr >= lo) & (fr < min(hi, 0.45*sr))
        spectrum.append(float(spec[sel].sum()/max(sel.sum(), 1)))
    return {'floor_rms': float(np.sqrt(floor)), 'click_rms': click, 'click_decay_s': click_decay,
            'sizzle_rms': sizzle, 'sizzle_decay_s': sizzle_decay, 'peak_ms': round(1000*(peak_at/sr - 0.001), 3),
            'band_edges_hz': edges, 'band_power_per_bin': spectrum}


def main():
    ref = project_reference()
    x, sr = sf.read(SAMPLE)
    x = M.highpass(x, sr)
    onsets = detect_onsets(x.mean(1), sr)
    groups = gestures(onsets)
    assert len(groups) == len(HITS), (len(groups), groups)
    fs = sr/FIT_DECIMATION
    hits = []
    # Sequential deflation: the heads keep ringing into the next hit's window,
    # so each fitted hit's modal tail (and the ring from before the loop
    # starts) is subtracted before the following hit is identified.
    remaining = x.copy()
    pre = resample_poly(x[:int((groups[0][0] - 0.002)*sr)], 1, FIT_DECIMATION, axis=0)
    pf, pr, pC, _ = M.fit_hit(pre, fs, M.seed_frequencies(pre, fs, 12, windows=((0.0, 0.15),)))
    remaining -= ringing(pf, pr, pC, 0, len(x), sr, [])
    for g, (slug, name), nxt in zip(groups, HITS, groups[1:] + [None]):
        start = g[0] - 0.0015
        end = (nxt[0] - 0.001) if nxt else len(x)/sr
        end = min(end, start + MAX_SECONDS)
        seg = remaining[int(start*sr):int(end*sr)]
        y = resample_poly(seg, 1, FIT_DECIMATION, axis=0)
        t0 = time.time()
        f, r, C, res, delays = fit_strikes(y, fs, [o - start for o in g[1:]])
        tail = ringing(f, r, C, int(start*sr), len(x), sr, delays)
        remaining[int(start*sr):] -= tail
        amps, _ = M.mode_table(f, r, C, 1 + len(delays))
        k = len(f)
        # Mono (sin, cos) excitation per strike: the resonator's initial phase.
        mono = C.mean(1)
        quadrature = [[mono[2*s*k:(2*s+1)*k], mono[(2*s+1)*k:(2*s+2)*k]] for s in range(1 + len(delays))]
        energy = lambda a: float((a**2).sum())
        early = int(0.025*fs)
        hit = {
            'slug': slug, 'name': name, 'onset_s': round(g[0], 5), 'window_s': [round(start, 5), round(end, 5)],
            'strike_delays_s': [0.0] + [round(float(d), 5) for d in delays],
            'peak': round(float(np.abs(seg).max()), 5),
            'rms_db': round(float(10*np.log10(np.mean(seg**2))), 3),
            'residual_db': round(float(10*np.log10(energy(res)/energy(y))), 3),
            'residual_early_db': round(float(10*np.log10(energy(res[:early])/energy(y[:early]))), 3),
            'residual_late_db': round(float(10*np.log10(energy(res[early:])/max(energy(y[early:]), 1e-20))), 3),
            'modes': [{'hz': round(float(f[q]), 4), 'rate_per_s': round(float(r[q]), 4),
                       'residue': [[round(float(a), 7) for a in amps[s, q]] for s in range(amps.shape[0])],
                       'sin_cos': [[round(float(quadrature[s][0][q]), 7), round(float(quadrature[s][1][q]), 7)]
                                   for s in range(amps.shape[0])]}
                      for q in np.argsort(f)],
            'detected_onsets_s': [round(o - g[0], 5) for o in g],
            'contact': [contact_click(x.mean(1), o, sr) for o in g],
        }
        hits.append(hit)
        print(f"{name:16s} strikes={hit['strike_delays_s']} residual={hit['residual_db']:.1f} dB "
              f"(early {hit['residual_early_db']:.1f}, late {hit['residual_late_db']:.1f}) {time.time()-t0:.0f}s", flush=True)
    out = {'reference': ref, 'sample_rate': sr, 'fit_sample_rate': fs, 'highpass_hz': 110,
           'onsets_s': [round(o, 5) for o in onsets], 'hits': hits}
    (HERE/'analysis.json').write_text(json.dumps(out, indent=1) + '\n')


if __name__ == '__main__':
    main()
