#!/usr/bin/env python3
"""Measure the opening brass phrase of "Bodas (Ao Vivo)" (Milton Nascimento,
Milagre Dos Peixes Ao Vivo, track 1-02) for PM Milagre Brass.

The first 7.9 s are one valveless lip-reed horn alone, playing partials of a
single bore: the pedal A2 (partial 1, 112.0 Hz, the recording sits ~31 cents
above A440), C#5 (partial 5), E5 (partial 6) and E4 (partial 3), slurring
between them. The band enters at 7.97 s.

Writes analysis.json: the hand-checked score (onsets, releases, slurs,
partial numbers), the measured pitch per note and per 10 ms frame the
amplitude (dB) and pitch of the sounding note's harmonics below 6 kHz, plus
the pre-roll noise floor per sixth-octave band. No PCM, phase or full
spectrum of the recording is stored.

Usage: analyze.py [path-to-mp3]   (defaults to the user's Music library path)
"""
import hashlib
import json
import subprocess
import sys
from pathlib import Path

import numpy as np

HERE = Path(__file__).resolve().parent
DEFAULT = Path.home()/'Music/Music/Media.localized/Music/Milton Nascimento/Milagre Dos Peixes (Ao Vivo)/1-02 Bodas (Ao Vivo).mp3'
SR = 48000
SECONDS = 8.0
HOP = 0.010
WIN = 2048
FMAX = 6000.0
HORN_HZ = 112.0      # bore partial spacing measured from partial 3 (336.0 Hz / 3)

# Hand-segmented from the spectrogram and a 5 ms band-limited level track.
# partial: which bore partial sounds; slur: pitch change without a new attack.
SCORE = [
    dict(name='A2 long', partial=1, on=0.045, off=2.075, slur=False),
    dict(name='A2 short', partial=1, on=2.230, off=2.440, slur=False),
    dict(name='A2 short', partial=1, on=2.640, off=2.865, slur=False),
    dict(name='C#5', partial=5, on=2.995, off=3.345, slur=False),
    dict(name='E5', partial=6, on=3.375, off=3.470, slur=False),
    dict(name='A2', partial=1, on=3.500, off=5.150, slur=False),
    dict(name='E4', partial=3, on=5.150, off=6.855, slur=True),
    dict(name='A2 swell', partial=1, on=6.855, off=7.790, slur=True),
]


def load(path):
    raw = subprocess.run(['ffmpeg', '-loglevel', 'error', '-i', str(path), '-t', str(SECONDS),
                          '-ac', '1', '-ar', str(SR), '-f', 'f32le', '-'],
                         capture_output=True, check=True).stdout
    return np.frombuffer(raw, dtype=np.float32).astype(np.float64)


def note_at(t):
    for i, n in enumerate(SCORE):
        if n['on'] <= t < n['off']:
            return i
    return None


def frame_spectrum(x, centre):
    s = int(round(centre*SR)) - WIN//2
    seg = np.zeros(WIN)
    lo, hi = max(s, 0), min(s + WIN, len(x))
    seg[lo - s:hi - s] = x[lo:hi]
    w = np.hanning(WIN)
    X = np.abs(np.fft.rfft(seg*w, 4*WIN))*2/w.sum()
    return X, np.fft.rfftfreq(4*WIN, 1/SR)


def peak_freq(X, fr, centre, rel=0.03):
    band = np.where((fr > centre*(1 - rel)) & (fr < centre*(1 + rel)))[0]
    i = band[X[band].argmax()]
    a, b, c = np.log(X[i - 1:i + 2] + 1e-12)
    return (i + 0.5*(a - c)/(a - 2*b + c))*(fr[1] - fr[0])


def harmonics(X, fr, f0):
    out = []
    half = min(0.45*f0, 40.0)
    for k in range(1, int(FMAX//f0) + 1):
        band = (fr > k*f0 - half) & (fr < k*f0 + half)
        out.append(20*np.log10(X[band].max() + 1e-9))
    return out


def main():
    path = Path(sys.argv[1]) if len(sys.argv) > 1 else DEFAULT
    x = load(path)
    # Per-note pitch: median of the strongest low harmonic's peak frequency.
    notes = []
    for n in SCORE:
        k = 3 if n['partial'] == 1 else 1        # the pedal's strongest harmonic is the 3rd
        nominal = HORN_HZ*n['partial']*k
        fs = [peak_freq(*frame_spectrum(x, t), nominal)/k
              for t in np.arange(n['on'] + 0.05, n['off'] - 0.03, 0.01)]
        notes.append(dict(n, hz=float(np.median(fs))))
    frames = []
    for t in np.arange(0.0, SECONDS - WIN/SR/2, HOP):
        X, fr = frame_spectrum(x, t)
        level = 20*np.log10(np.sqrt(np.mean(X**2)*len(X)/2) + 1e-9)
        i = note_at(t)
        # Between notes the previous note's ring (the hall) is what sounds.
        ref = i if i is not None else max([j for j, n in enumerate(SCORE) if n['off'] <= t] or [0])
        hz = None
        if i is not None:
            n = notes[i]
            k = 3 if n['partial'] == 1 else 1
            hz = round(float(peak_freq(X, fr, n['hz']*k, rel=0.02)/k), 3)
        frames.append(dict(t=round(float(t), 4), note=i, ref=ref, level=round(float(level), 2), hz=hz,
                           harm=[round(v, 2) for v in harmonics(X, fr, notes[ref]['hz'])]))
    floor = [f['level'] for f in frames if f['t'] < 0.03]
    # Pre-roll noise (hum, hiss, audience) per sixth-octave band: max over the
    # first 30 ms frames, the floor below which harmonics are not measurable.
    centres = np.geomspace(50, FMAX, 43)
    pre = [frame_spectrum(x, t) for t in (0.0, 0.01, 0.02)]
    fr = pre[0][1]
    noise = []
    for c in centres:
        band = (fr > c*2**(-1/12)) & (fr < c*2**(1/12))
        noise.append(round(float(max(20*np.log10(X[band].max() + 1e-9) for X, _ in pre)), 2))
    out = dict(source=dict(title='Bodas (Ao Vivo)', artist='Milton Nascimento',
                           album='Milagre Dos Peixes (Ao Vivo)', seconds=SECONDS,
                           sha256=hashlib.sha256(path.read_bytes()).hexdigest()),
               sample_rate=SR, hop=HOP, window=WIN, horn_hz=HORN_HZ, band_entry=7.97,
               noise_floor_db=round(float(np.mean(floor)), 2),
               noise_hz=[round(float(c), 1) for c in centres], noise_db=noise,
               notes=notes, frames=frames)
    (HERE/'analysis.json').write_text(json.dumps(out, indent=1))
    for n in notes:
        cents = 1200*np.log2(n['hz']/(440*2**((round(12*np.log2(n['hz']/440)) )/12)))
        print(f"{n['name']:9s} partial {n['partial']} {n['on']:.3f}-{n['off']:.3f}  {n['hz']:.2f} Hz ({cents:+.0f} c)")


if __name__ == '__main__':
    main()
