#!/usr/bin/env python3
"""Compare each installed recorded pluck, played alone by the compiled model, with its window in the record.

The record is polyphonic: the reference band power of a window is the
power after the pluck minus the power that was already ringing in that band
just before it (a conservative "new energy" estimate). Restricted level/band
metrics; not a perceptual score. Writes comparison.json.
"""
import argparse
import json
import sys
from pathlib import Path

import numpy as np
import scipy.signal as ss
import soundfile as sf

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
sys.path.insert(0, str(HERE))
from render import instrument          # noqa: E402
from build import tables               # noqa: E402

WINDOWS = [(0.0, 0.03), (0.03, 0.1), (0.1, 0.25)]
BANDS = [70, 140, 280, 560, 1120, 2240, 4480, 8960]


def band_power(y, sr, a, b):
    out = []
    for lo, hi in zip(BANDS, BANDS[1:]):
        sos = ss.butter(4, [lo, hi], 'bandpass', fs=sr, output='sos')
        seg = ss.sosfiltfilt(sos, y)[max(0, int(a*sr)):int(b*sr)]
        out.append(np.mean(seg**2) if len(seg) else 0.0)
    return np.array(out)


def calibrate(rows, t, model, step=0.7):
    """One damped update of calibration.json from the band errors.

    EQ trim per knot from the 30-100 ms error of bands that hold a partial of
    the note; loss trim from how the error changes to 100-250 ms; body knock
    from the low bands of notes with no partial there; output trim from the
    median 30-100 ms level error.
    """
    from build import CALIBRATION, calibration, BODY
    knots = np.array(model['eq_knots_hz'])
    cal = calibration(len(knots))
    from build import GROUPS
    f0 = {g: hz for g, hz in zip(GROUPS, t['ref_hz'])}
    centres = np.sqrt(np.array(BANDS[:-1])*np.array(BANDS[1:]))
    e1 = [[] for _ in centres]; dd = [[] for _ in centres]; low = [[], []]
    for r in rows:
        a, b = r['windows'][1]['band_error_db'], r['windows'][2]['band_error_db']
        for i, (lo, hi) in enumerate(zip(BANDS, BANDS[1:])):
            if hi <= f0[r['group']]:
                if i < 2 and a[i] is not None:
                    low[i].append(a[i])
                continue
            if a[i] is not None:
                e1[i].append(a[i])
                if b[i] is not None:
                    dd[i].append(b[i] - a[i])
    lvl = np.median([r['windows'][1]['level_error_db'] for r in rows])
    band_e = np.array([np.median(v) - lvl if len(v) >= 3 else 0.0 for v in e1])
    band_d = np.array([np.median(v) if len(v) >= 3 else 0.0 for v in dd])
    # interpolate band corrections onto the half-octave knots (log frequency)
    lk = np.log2(knots)
    eq_step = -np.interp(lk, np.log2(centres), band_e)
    eq_step[knots > 3620] = 0.0
    rate_step = np.interp(lk, np.log2(centres), band_d)*np.log(10)/20/0.11
    rate_step[knots > 3620] = 0.0
    cal['eq_trim_db'] = np.clip(np.array(cal['eq_trim_db']) + step*eq_step, -18, 18).round(3).tolist()
    trim = np.clip(np.array(cal['loss_trim']) + step*rate_step, -20, 40)
    # Above 1.2 kHz the record's late energy is room/other strings: the
    # calibration may add nylon loss there, never take it away.
    trim[knots > 1200] = np.maximum(trim[knots > 1200], 0.0)
    cal['loss_trim'] = trim.round(3).tolist()
    cal['output_trim_db'] = round(float(cal['output_trim_db'] - step*lvl), 3)
    # knock: modes below 280 Hz, scaled until the treble notes' low bands match
    knock = np.array(cal['knock'], float)
    for i, (lo, hi) in enumerate(zip(BANDS[:2], BANDS[1:3])):
        if len(low[i]) < 3:
            continue
        need = -np.median(low[i]) - lvl
        for m, (hz, _, _) in enumerate(BODY):
            if lo <= hz < hi:
                knock[m] = knock[m]*10**(step*need/20) if knock[m] > 0 else 0.02
    cal['knock'] = np.clip(knock, 0, 50).round(5).tolist()
    # even keyboard: each recorded pitch (and a semitone either side) plays
    # the same 0-300 ms loudness at the neutral velocity
    from render import instrument
    inst = instrument(48000)
    loud = []
    for ref in t['ref_hz']:
        e = []
        for d in (-1, 0, 1):
            y = inst.render(seconds=0.3, pitch=float(ref*2**(d/12)), vel=0.6,
                            params={'pluck.humanize': 0.0, 'pluck.vel_take': 0.0, 'pluck.take': 0.5})[0][:, 0]
            e.append(np.mean(y.astype(float)**2))
        loud.append(10*np.log10(np.mean(e)))
    loud = np.array(loud)
    gt = np.array(cal.get('group_trim_db', [0.0]*len(loud)))
    cal['group_trim_db'] = np.clip(gt - step*(loud - np.median(loud)), -18, 18).round(3).tolist()
    print('group loudness dB', loud.round(1).tolist())
    CALIBRATION.write_text(json.dumps(cal, indent=1) + '\n')
    print('calibration', cal)


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('--string-only', action='store_true', help='no body bank or contact noise')
    ap.add_argument('--calibrate', action='store_true', help='print the output trim that zeroes the median 30-100 ms level error')
    args = ap.parse_args()
    model = json.loads((HERE/'model.json').read_text())
    t, _, info, _ = tables(model)
    x, sr = sf.read(ROOT/'.local/pm-guitar/nascer-20s.wav')
    m = ss.sosfiltfilt(ss.butter(4, 40, 'highpass', fs=sr, output='sos'), x.mean(1))
    inst = instrument(sr)
    rows = []
    starts, counts = t['start'].astype(int), t['count'].astype(int)
    for g, (s, c) in enumerate(zip(starts, counts)):
        ref_hz = t['ref_hz'][g]
        for j in range(c):
            r = info[s + j]
            onset = r['onset_s']
            params = {'pluck.take': j/max(c - 1, 1), 'pluck.vel_take': 0.0, 'pluck.humanize': 0.0}
            if args.string_only:
                params |= {'body.resonance': 0.0, 'body.contact': 0.0}
            y, _ = inst.render(seconds=0.3, pitch=float(ref_hz), vel=0.6, params=params)
            # undo the per-pitch loudness equalization: compare at the recorded level
            y = y[:, 0].astype(float)*10**(-r['level_shift_db']/20)
            seg = m[int(onset*sr):int((onset + 0.3)*sr)]
            lead = min(0.06, onset - 0.005)
            pre = band_power(m[int((onset - lead)*sr):int(onset*sr)], sr, 0, lead)
            res = {'group': r['group'], 'onset_s': onset, 'windows': []}
            for a, b in WINDOWS:
                ref = np.maximum(band_power(seg, sr, a, b) - pre, 1e-12)
                mod = band_power(y, sr, a, b) + 1e-12
                w = ref/ref.sum()
                audible = ref > ref.max()*1e-3
                band_err = 10*np.log10(mod/ref)
                res['windows'].append({'ms': [int(a*1000), int(b*1000)],
                                       'level_error_db': round(float(10*np.log10(mod.sum()/ref.sum())), 2),
                                       'band_error_db': [round(float(v), 1) if a_ else None for v, a_ in zip(band_err, audible)],
                                       'weighted_band_abs_db': round(float(np.sum(w*np.abs(band_err))), 2)})
            rows.append(res)
            print(f"{r['group']:4s} {onset:6.2f}s " + ' | '.join(
                f"{wd['ms'][0]}-{wd['ms'][1]}ms lvl {wd['level_error_db']:+5.1f} band {wd['weighted_band_abs_db']:4.1f}" for wd in res['windows']))
    lvl = np.array([[wd['level_error_db'] for wd in r['windows']] for r in rows])
    band = np.array([[wd['weighted_band_abs_db'] for wd in r['windows']] for r in rows])
    summary = {'median_level_error_db': np.median(lvl, 0).round(2).tolist(),
               'median_abs_level_error_db': np.median(np.abs(lvl - np.median(lvl, 0)), 0).round(2).tolist(),
               'median_weighted_band_abs_db': np.median(band, 0).round(2).tolist()}
    print('summary', summary)
    if args.calibrate:
        calibrate(rows, t, model)
    (HERE/'comparison.json').write_text(json.dumps({'summary': summary, 'rows': rows}, indent=1) + '\n')


if __name__ == '__main__':
    main()
