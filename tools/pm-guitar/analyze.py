#!/usr/bin/env python3
"""Identify the plucked strings of the reference guitar intro (first 15 s).

Writes analysis.json: per pitch, tuning (f0, inharmonicity B), shared partial
poles (one or two per partial; two where the string couples to the body or
its polarizations split) and per-take complex residues at the pluck. Stores
coefficients only - no PCM, recorded frames or envelopes.
"""
import argparse
import hashlib
import json
import sys
from concurrent.futures import ProcessPoolExecutor
from pathlib import Path

import numpy as np
import scipy.signal as ss
import soundfile as sf

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
sys.path.insert(0, str(HERE))
from detect import onsets, refine_onset, new_notes, note_name, midi_to_hz   # noqa: E402
from partials import string_tuning, joint_partial                          # noqa: E402

SOURCE = Path.home()/'Downloads/Nascer.mp3'
WORK = ROOT/'.local/pm-guitar'
SPAN_S = 15.4
SR = 48000
MAX_PARTIALS = 40
FMAX = 9000.0
MIN_TAKE_S = 0.12


def load():
    wav = WORK/'nascer-20s.wav'
    if not wav.exists():
        import subprocess
        WORK.mkdir(parents=True, exist_ok=True)
        subprocess.run(['ffmpeg', '-loglevel', 'error', '-y', '-i', str(SOURCE), '-t', '20', '-ar', str(SR), str(wav)], check=True)
    x, sr = sf.read(wav)
    assert sr == SR
    m = ss.sosfiltfilt(ss.butter(4, 40, 'highpass', fs=sr, output='sos'), x.mean(1))
    return m, sr


def events(m, sr):
    t, strength = onsets(m, sr, SPAN_S)
    t = [refine_onset(m, sr, v) for v in t]
    out = []
    for i, o in enumerate(t):
        nxt = t[i + 1] if i + 1 < len(t) else SPAN_S + 0.5
        notes = new_notes(m, sr, o, nxt - o)
        out.append({'onset_s': float(o), 'next_s': float(nxt), 'prev_s': float(t[i - 1]) if i else max(0.0, o - 0.3),
                    'notes': [n for n, _ in notes], 'salience': [s for _, s in notes]})
    return out


def take_end(ev, i, f0):
    """A take lasts until the next pluck of any string (its partials overlap this
    one's harmonics in this tonal passage), at least MIN_TAKE_S, at most 1.6 s."""
    t0 = ev[i]['onset_s']
    for e in ev[i + 1:]:
        if e['onset_s'] - t0 >= MIN_TAKE_S:
            return min(e['onset_s'], t0 + 1.6)
    return min(t0 + 1.6, SPAN_S + 0.5)


def fit_pitch(args):
    midi, takes = args
    m, sr = load()
    f0, B, seen = string_tuning(m, sr, [(a, b) for a, b, _ in takes], midi)
    bw = min(0.45*f0, 60.0)
    partials = []
    for k in range(1, MAX_PARTIALS + 1):
        hz = k*f0*np.sqrt(1 + B*k*k)
        if hz > FMAX:
            break
        # Two poles only where the string couples to the body air/top modes
        # (k <= 2); above that one pole per partial, held near the measured
        # stiff-string series (free poles there chase neighbouring strings).
        r = joint_partial(m, sr, takes, hz, bw, n_new_max=2 if k <= 2 else 1,
                          dhz=min(8.0, 0.3*bw) if k <= 2 else 1.5)
        if r is None:
            continue
        r['k'] = k
        r['nominal_hz'] = float(hz)
        partials.append(r)
    return midi, {'f0_hz': f0, 'B': B, 'tuning_cents': float(1200*np.log2(f0/midi_to_hz(midi))),
                  'peaks_seen': seen, 'bandwidth_hz': bw, 'partials': partials}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--jobs', type=int, default=8)
    args = ap.parse_args()
    m, sr = load()
    ev = events(m, sr)
    groups = {}
    for i, e in enumerate(ev):
        if len(e['notes']) != 1:
            continue                  # chords/ambiguous onsets are not clean takes
        midi = e['notes'][0]
        groups.setdefault(midi, []).append(i)
    jobs = []
    take_index = {}
    for midi, idx in sorted(groups.items()):
        f0 = midi_to_hz(midi)
        takes = [(ev[i]['onset_s'], take_end(ev, i, f0), ev[i]['prev_s']) for i in idx]
        take_index[midi] = idx
        jobs.append((midi, takes))
    with ProcessPoolExecutor(args.jobs) as pool:
        fitted = dict(pool.map(fit_pitch, jobs))
    pitches = []
    for midi, takes in jobs:
        p = fitted[midi]
        p.update({'midi': midi, 'name': note_name(midi),
                  'takes': [{'event': i, 'onset_s': a, 'end_s': b, 'pre_s': c}
                            for i, (a, b, c) in zip(take_index[midi], takes)]})
        pitches.append(p)
    src = WORK/'nascer-20s.wav'
    out = {'source': SOURCE.name, 'decoded_sha256': hashlib.sha256(src.read_bytes()).hexdigest(),
           'span_s': SPAN_S, 'sample_rate': sr, 'events': ev, 'pitches': pitches}
    (HERE/'analysis.json').write_text(json.dumps(out, indent=1))
    for p in pitches:
        snr = np.median([q['snr_db'] for q in p['partials'][:8]])
        print(f"{p['name']:4s} takes {len(p['takes']):2d} f0 {p['f0_hz']:7.2f} ({p['tuning_cents']:+.0f}c) B {p['B']:.2e} partials {len(p['partials'])} median snr(1-8) {snr:.1f} dB")


if __name__ == '__main__':
    main()
