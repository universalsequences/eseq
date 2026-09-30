#!/usr/bin/env python3
"""Identify the tabla's modal strokes from the "A5 - Ajanta" reference sample.

Reads the exact library sample (stored by content hash; see LIBRARY). It holds
two phrases: a fast dayan roll ringing at ~168 Hz, then a phrase of separated
strokes over a bayan whose pitch the player raises with the wrist (meend,
~96 -> 125 Hz). Writes analysis.json with, per reference stroke: contact
delays, modal frequencies, loss rates, bayan glide, per-contact per-channel
residues and the contact-noise envelope left after the modal fit. No PCM,
recorded phase or spectral frame is written.
"""
import hashlib
import json
import sys
import time
from pathlib import Path

import numpy as np
import soundfile as sf
from scipy.signal import resample_poly, find_peaks, butter, sosfiltfilt

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
sys.path.insert(0, str(HERE))
import modal_fit as M

SAMPLE = ROOT/'.local/samples/26575811ff66598482e96a818e6a884290d8d1a6a2c38f652d24be75e0ffd905.wav'
LIBRARY = ROOT/'.local/samples.jsonl'
HIGHPASS_HZ = 60
FIT_DECIMATION = 2          # 44.1 kHz source -> 22.05 kHz modal fit (modes above ~6 kHz become contact noise)
MODES_PER_HIT = 32
MAX_STRIKES = 3
MAX_SECONDS = 0.30
STRIKE_GAIN_DB = 0.8        # an extra contact must lower the residual at least this much
STROKE_PROMINENCE_DB = 20   # contact bursts this far above their surroundings start a stroke
CONTACT_PROMINENCE_DB = 10  # weaker bursts seed the extra-contact search
FLAM_SECONDS = 0.030        # strokes closer than this are one gesture
ROLL_END_S = 2.0            # strokes before this belong to the dayan roll
RING_WINDOW_S = (1.75, 2.30)  # free dayan ring after the roll dies away, before the second phrase
RING_MODES = 16
GLIDE_BELOW_HZ = 200.0      # modes allowed to follow the bayan's pitch in the second phrase
GLIDE_SEED_BELOW_HZ = 150.0  # modes that start out following it (beta 1)
TRACK_SPAN_S = (2.34, 3.24)  # the bayan is loud enough to track its pitch


def library_reference():
    """Confirms the library entry still names this exact file."""
    sha = hashlib.sha256(SAMPLE.read_bytes()).hexdigest()
    rows = [json.loads(line) for line in LIBRARY.read_text().splitlines() if SAMPLE.stem in line]
    row = next(r for r in rows if r.get('hash') == SAMPLE.stem)
    assert sha == SAMPLE.stem, 'sample content no longer matches its hash name'
    return {'library': str(LIBRARY.relative_to(ROOT)), 'title': row['title'], 'tags': row['tags'],
            'sample': str(SAMPLE.relative_to(ROOT)), 'sha256': sha}


def detect_contacts(mono, sr):
    """Hand contacts as bursts in the 2.5-12 kHz band, where the modes carry little.

    Returns (time, prominence dB). The time is where the burst's power first
    reaches a tenth of its peak: the ringing drum hides onsets in broadband
    level during the roll.
    """
    band = sosfiltfilt(butter(4, [2500, 12000], 'bandpass', fs=sr, output='sos'), mono)
    win = int(0.001*sr)
    power = np.convolve(band**2, np.ones(win)/win, 'same')
    db = 10*np.log10(power + 1e-14)
    peaks, props = find_peaks(db, prominence=CONTACT_PROMINENCE_DB, distance=int(0.02*sr))
    out = []
    for p, prom in zip(peaks, props['prominences']):
        a = p - int(0.005*sr)
        first = a + int(np.argmax(power[a:p + 1] >= 0.1*power[p]))
        out.append((first/sr, float(prom)))
    return out


def gestures(contacts):
    strong = [t for t, prom in contacts if prom >= STROKE_PROMINENCE_DB]
    groups = []
    for o in strong:
        if groups and o - groups[-1][-1] < FLAM_SECONDS:
            groups[-1].append(o)
        else:
            groups.append([o])
    return groups


def bayan_pitch(x, sr):
    """The second phrase's bayan pitch F(t), shared by every stroke.

    Short-time spectral peak (80 ms Hann, parabolic) of the 80-150 Hz band
    while the bayan is loud, fitted as a cubic in log2 frequency weighted by
    peak amplitude. Outside the span the pitch holds: before it the first
    stroke has not sounded; after it the bayan is >55 dB down, near the noise.
    Returns (log2 F function, summary).
    """
    mono = x.mean(1)
    W = int(0.08*sr)
    window = np.hanning(W)
    N = 1 << 15
    fr = np.fft.rfftfreq(N, 1/sr)
    band = np.flatnonzero((fr > 80) & (fr < 150))
    times, pitch, weight = [], [], []
    for t in np.arange(TRACK_SPAN_S[0], TRACK_SPAN_S[1] + 1e-9, 0.01):
        i0 = int(t*sr) - W//2
        spec = np.abs(np.fft.rfft(mono[i0:i0 + W]*window, N))
        i = band[np.argmax(spec[band])]
        a, b, c = np.log(spec[i - 1:i + 2])
        times.append(t)
        pitch.append(np.log2((i + 0.5*(a - c)/(a - 2*b + c))*sr/N))
        weight.append(spec[i])
    coef = np.polyfit(times, pitch, 3, w=np.array(weight))
    lo, hi = TRACK_SPAN_S

    def log2_track(t):
        return np.polyval(coef, np.clip(t, lo, hi))
    residual = 1200*np.sqrt(np.average((np.polyval(coef, times) - pitch)**2, weights=np.square(weight)))
    summary = {'span_s': list(TRACK_SPAN_S), 'log2_hz_cubic': [float(c) for c in coef],
               'start_hz': round(float(2**log2_track(lo)), 3), 'end_hz': round(float(2**log2_track(hi)), 3),
               'weighted_rms_cents': round(float(residual), 2)}
    return log2_track, summary


def window_glide(warp, hold):
    """Log-linear glide (cents/s) of the shared track over a stroke window: the engine's per-row meend."""
    t = np.linspace(0, hold, 64)
    return float(1200*np.polyfit(t, warp.ratio(t), 1)[0])


def fit_strikes(y, sr, known, freqs, fixed=None, warp=None):
    """Greedy contact search. Known contacts inside the gesture seed the search."""
    kw = dict(fixed=fixed, warp=warp, glide_mask=freqs < GLIDE_BELOW_HZ)
    betas0 = None
    if warp is not None:
        # Spectral seeds sit at a gliding mode's mean pitch; start them where
        # the shared track puts them at the window start.
        betas0 = (freqs < GLIDE_SEED_BELOW_HZ).astype(float)
        mid = warp.ratio(np.array([0.5*len(y)/sr]))[0]
        freqs = freqs*2**(-betas0*mid)
    f, r, g, C, res = M.fit_hit(y, sr, freqs, betas0=betas0, **kw)
    k = len(freqs)
    delays = []
    err = (res**2).sum()
    candidates = np.arange(0.003, min(0.08, len(y)/sr - 0.01), 0.0005)
    while len(delays) + 1 < MAX_STRIKES:
        best = None
        for d in list(candidates) + list(known):
            if d <= 0 or any(abs(d - e) < 0.004 for e in delays):
                continue
            e = (M.solve(f, r, y, sr, sorted(delays + [d]), g, warp)[2]**2).sum()
            if best is None or e < best[0]:
                best = (e, d)
        if best is None or 10*np.log10(err/best[0]) < STRIKE_GAIN_DB:
            break
        delays = sorted(delays + [best[1]])
        fx = None if fixed is None else (f[k:], r[k:], g[k:])
        f, r, g, C, res = M.fit_hit(y, sr, f[:k], r[:k], delays, betas0=g[:k], **(kw | {'fixed': fx}))
        err = (res**2).sum()
    return f, r, g, C, res, delays


def ringing(f, r, g, C, begin, end, sr, delays, warp):
    """A fitted stroke's modal response from its window start to `end` (source rate).

    Gliding modes keep following the shared bayan track past the window, as
    the real ring does under the next strokes.
    """
    n = end - begin
    out = np.zeros((n, C.shape[1]))
    for start in range(0, n, 16384):
        stop = min(n, start + 16384)
        out[start:stop] = M.basis(f, r, stop - start, sr, delays, g, warp, start=start)@C
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
    ref = library_reference()
    x, sr = sf.read(SAMPLE)
    x = M.highpass(x, sr, HIGHPASS_HZ)
    mono = x.mean(1)
    contacts = detect_contacts(mono, sr)
    groups = gestures(contacts)
    fs = sr/FIT_DECIMATION
    decimate = lambda seg: resample_poly(seg, 1, FIT_DECIMATION, axis=0)

    # The dayan roll hits every ~65 ms into a ring that lasts seconds; a 65 ms
    # window cannot resolve its long modes. They are identified once from the
    # free ring after the roll and shared, fixed, by every roll stroke.
    a, b = RING_WINDOW_S
    tail = decimate(x[int(a*sr):int(b*sr)])
    track, track_summary = bayan_pitch(x, sr)
    print('bayan pitch', track_summary, flush=True)
    rf, rr, rg, _, rres = M.fit_hit(tail, fs, M.seed_frequencies(tail, fs, RING_MODES, windows=((0.0, b - a),)),
                                    rate_bounds=(0.3, 300.0))
    ring = {'window_s': [a, b], 'hz': [round(float(v), 4) for v in rf], 'rate_per_s': [round(float(v), 4) for v in rr],
            'residual_db': round(float(10*np.log10((rres**2).sum()/(tail**2).sum())), 3)}
    print('dayan ring', ring['residual_db'], 'dB', np.round(rf, 1), flush=True)

    hits = []
    # Sequential deflation: the drums keep ringing into the next stroke's
    # window, so each fitted stroke's modal tail is subtracted before the
    # following stroke is identified.
    remaining = x.copy()
    for n, (g, nxt) in enumerate(zip(groups, groups[1:] + [None])):
        roll = g[0] < ROLL_END_S
        start = g[0] - 0.0015
        end = (nxt[0] - 0.001) if nxt else len(x)/sr
        end = min(end, start + MAX_SECONDS)
        seg = remaining[int(start*sr):int(end*sr)]
        y = decimate(seg)
        known = [t - start for t, _ in contacts if start + 0.003 < t < min(end, start + 0.08)]
        t0 = time.time()
        hold = end - start
        warp = None if roll or start >= TRACK_SPAN_S[1] else M.Warp(start, track)
        if roll:
            free = M.seed_frequencies(y, fs, MODES_PER_HIT - len(rf), avoid=rf)
            f, r, beta, C, res, delays = fit_strikes(y, fs, known, free, fixed=(rf, rr, np.zeros(len(rf))))
        else:
            free = M.seed_frequencies(y, fs, MODES_PER_HIT)
            f, r, beta, C, res, delays = fit_strikes(y, fs, known, free, warp=warp)
        remaining[int(start*sr):] -= ringing(f, r, beta, C, int(start*sr), len(x), sr, delays, warp)
        slope = window_glide(warp, hold) if warp else 0.0
        gl = beta*slope
        amps, _ = M.mode_table(f, r, C, 1 + len(delays))
        k = len(f)
        # Mono (sin, cos) excitation per contact: the resonator's initial phase.
        mc = C.mean(1)
        quadrature = [[mc[2*s*k:(2*s+1)*k], mc[(2*s+1)*k:(2*s+2)*k]] for s in range(1 + len(delays))]
        energy = lambda v: float((v**2).sum())
        early = int(0.025*fs)
        hit = {
            'index': n, 'phrase': 'roll' if roll else 'strokes',
            'onset_s': round(g[0], 5), 'window_s': [round(start, 5), round(end, 5)],
            'strike_delays_s': [0.0] + [round(float(d), 5) for d in delays],
            'glide_hold_s': round(hold, 5), 'bayan_glide_cents_per_s': round(slope, 3),
            'peak': round(float(np.abs(seg).max()), 5),
            'rms_db': round(float(10*np.log10(np.mean(seg**2))), 3),
            'residual_db': round(float(10*np.log10(energy(res)/energy(y))), 3),
            'residual_early_db': round(float(10*np.log10(energy(res[:early])/energy(y[:early]))), 3),
            'residual_late_db': round(float(10*np.log10(energy(res[early:])/max(energy(y[early:]), 1e-20))), 3),
            'modes': [{'hz': round(float(f[q]), 4), 'rate_per_s': round(float(r[q]), 4),
                       'beta': round(float(beta[q]), 4), 'glide_cents_per_s': round(float(gl[q]), 3),
                       'residue': [[round(float(v), 7) for v in amps[s, q]] for s in range(amps.shape[0])],
                       'sin_cos': [[round(float(quadrature[s][0][q]), 7), round(float(quadrature[s][1][q]), 7)]
                                   for s in range(amps.shape[0])]}
                      for q in np.argsort(f)],
            'detected_onsets_s': [round(o - g[0], 5) for o in g],
            'contact': [contact_click(mono, o, sr) for o in g],
        }
        hits.append(hit)
        low = [m for m in hit['modes'] if m['hz'] < 200 and abs(m['glide_cents_per_s']) > 1]
        lead = max(hit['modes'], key=lambda m: sum(v[0]**2 for v in m['residue']))
        print(f"{n:2d} {g[0]:6.3f} {hit['phrase']:7s} contacts={hit['strike_delays_s']} residual={hit['residual_db']:.1f} dB "
              f"(early {hit['residual_early_db']:.1f}, late {hit['residual_late_db']:.1f}) lead {lead['hz']:.0f} Hz "
              + ' '.join(f"glide {m['hz']:.0f}Hz {m['glide_cents_per_s']:+.0f}c/s" for m in low[:2])
              + f" {time.time()-t0:.0f}s", flush=True)
    out = {'reference': ref, 'sample_rate': sr, 'fit_sample_rate': fs, 'highpass_hz': HIGHPASS_HZ,
           'contacts': [[round(t, 5), round(p, 2)] for t, p in contacts], 'dayan_ring': ring,
           'bayan_pitch': track_summary, 'hits': hits}
    (HERE/'analysis.json').write_text(json.dumps(out, indent=1) + '\n')


if __name__ == '__main__':
    main()
