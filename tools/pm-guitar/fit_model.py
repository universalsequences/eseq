#!/usr/bin/env python3
"""Reduce analysis.json to a playable string model (model.json).

log|a_tk| = log L_t + log|sin(pi k beta_t)| - alpha_t log k - (f_tk/fc_t)^2 + h(f_tk)

L, beta (pluck position), alpha (finger contact tilt) and fc (fingertip/nail
roll-off) belong to the take; h(f) is one body/radiation EQ shared by every
note. Loss: sigma(f) = s0(pitch) + b*f^2. Body resonances are the fast
coupling poles found beside the low partials.
"""
import json
from pathlib import Path

import numpy as np
from scipy.optimize import least_squares

HERE = Path(__file__).resolve().parent
KNOTS = 40.0*2**(np.arange(0, 8.5, 0.5))      # 40 Hz .. 9 kHz, half-octave EQ knots
STRING_RATE_MAX = 90.0     # nylon partials above ~1.2 kHz die at 40-60/s; faster is attack transient
MIN_SNR = 3.0
NULL_FLOOR_DB = -45.0
DETAIL_PARTIALS = 32


def usable_pitches(d):
    """Pitches with >=3 takes plus single takes within 25 cents of equal temperament."""
    return [p for p in d['pitches'] if len(p['takes']) >= 2 or (abs(p['tuning_cents']) < 25 and p['name'] in ('D#4', 'F4', 'A#3'))]


def law_rate(f, s0, b2, cap):
    q = b2*f**2
    return s0 + q/(1 + q/cap)


def observations(pitches, law=None):
    """Partial amplitudes at the pluck. With `law` = (s0 per pitch, b2, cap), the
    amplitude seen at the start of the measurement window is carried back to
    the pluck at the law's rate instead of the partial's own fitted rate:
    fast upper-partial fits over-extrapolate the attack."""
    rows = []
    for pi, p in enumerate(pitches):
        settle = 1.6/p['bandwidth_hz']
        for q in p['partials']:
            if q['snr_db'] < MIN_SNR:
                continue
            rate = min(q['rate'])
            if rate > STRING_RATE_MAX:
                continue
            for ti, r in enumerate(q['residues']):
                if not r:
                    continue
                # two-pole low partials: the slow pole is the string; the fast
                # one is the body pulling it (extrapolating it back to the
                # pluck would inflate the level)
                j = int(np.argmin(q['rate'])) if len(q['rate']) == 2 else 0
                a = abs(complex(*r[j]))
                if law is not None:
                    a *= np.exp(-(q['rate'][j] - law_rate(q['nominal_hz'], law[0][pi], law[1], law[2]))*settle)
                if a <= 0:
                    continue
                rows.append((pi, ti, q['k'], q['nominal_hz'], np.log(a), q['snr_db'], rate))
    return rows


def eq_basis(f):
    """Linear interpolation weights of log2 frequency onto KNOTS."""
    x = np.clip(np.log2(np.asarray(f)/KNOTS[0])*2, 0, len(KNOTS) - 1 - 1e-9)
    i = np.floor(x).astype(int)
    w = x - i
    B = np.zeros((len(x), len(KNOTS)))
    B[np.arange(len(x)), i] = 1 - w
    B[np.arange(len(x)), i + 1] = w
    return B


def fit_excitation(pitches, rows):
    takes = sorted({(r[0], r[1]) for r in rows})
    tix = {t: i for i, t in enumerate(takes)}
    T = len(takes)
    ti = np.array([tix[(r[0], r[1])] for r in rows])
    k = np.array([r[2] for r in rows], float)
    f = np.array([r[3] for r in rows])
    y = np.array([r[4] for r in rows])
    w = np.sqrt(np.clip(np.array([r[5] for r in rows]), 0, 20))
    EQ = eq_basis(f)
    floor = 10**(NULL_FLOOR_DB/20)

    def model(x):
        L, beta, alpha, lfc = x[:T], x[T:2*T], x[2*T:3*T], x[3*T:4*T]
        h = x[4*T:]
        comb = np.log(np.abs(np.sin(np.pi*k*beta[ti])) + floor)
        return L[ti] + comb - alpha[ti]*np.log(k) - (f/np.exp(lfc[ti]))**2 + EQ@h

    def res(x):
        h = x[4*T:]
        smooth = 3.0*np.diff(h, 2)            # EQ curvature penalty
        anchor = [10*h.mean()]                # level lives in L, not h
        return np.r_[w*(model(x) - y), smooth, anchor]

    best = None
    for b0 in (0.12, 0.2, 0.3):
        x0 = np.r_[np.full(T, np.median(y)), np.full(T, b0), np.full(T, 0.8), np.full(T, np.log(3000.0)), np.zeros(len(KNOTS))]
        lo = np.r_[np.full(T, -30), np.full(T, 0.04), np.full(T, 0.0), np.full(T, np.log(800)), np.full(len(KNOTS), -6)]
        hi = np.r_[np.full(T, 5), np.full(T, 0.5), np.full(T, 3.0), np.full(T, np.log(12000)), np.full(len(KNOTS), 6)]
        sol = least_squares(res, x0, bounds=(lo, hi), x_scale='jac', max_nfev=400)
        if best is None or sol.cost < best.cost:
            best = sol
    x = best.x
    # refine beta per take on a grid (the comb is multimodal)
    for t in range(T):
        sel = ti == t
        if sel.sum() < 4:
            continue
        def cost(b):
            xx = x.copy(); xx[T + t] = b
            return np.sum((w[sel]*(model(xx)[sel] - y[sel]))**2)
        grid = np.linspace(0.04, 0.5, 185)
        x[T + t] = grid[np.argmin([cost(b) for b in grid])]
    sol = least_squares(res, x, bounds=(lo, hi), x_scale='jac', max_nfev=400)
    x = sol.x
    support = EQ.sum(0) > 1.0
    h = x[4*T:]
    idx = np.flatnonzero(support)
    for j in range(len(h)):
        if not support[j]:
            h[j] = h[idx[np.argmin(np.abs(idx - j))]]
    x[4*T:] = h
    err = model(x) - y
    snr = np.array([r[5] for r in rows])
    out = []
    for (pi, tk), t in tix.items():
        sel = ti == t
        corr = np.zeros(DETAIL_PARTIALS)
        for j in np.flatnonzero(sel):
            kk = int(k[j])
            if kk <= DETAIL_PARTIALS and snr[j] >= 6:
                # shrink toward the parametric pluck where the partial is noisy
                corr[kk - 1] = np.clip(-err[j], -1.4, 1.4)*min(1.0, (snr[j] - 3)/9)
        out.append({'pitch': pi, 'take': tk, 'level': float(x[t]), 'beta': float(x[T + t]), 'alpha': float(x[2*T + t]),
                    'fc_hz': float(np.exp(x[3*T + t])), 'rms_db': float(20/np.log(10)*np.sqrt(np.mean(err[sel]**2))),
                    'n': int(sel.sum()), 'detail_ln': corr.tolist()})
    return out, x[4*T:], err


def fit_loss(pitches):
    """sigma = s0(pitch) + b1 f + b2 f^2: s0 from each fundamental's string pole, b1/b2 shared."""
    s0 = []
    for p in pitches:
        q = p['partials'][0]
        s0.append(float(np.clip(min(q['rate']), 0.5, 8.0)))
    s0 = np.array(s0)
    data = []
    for pi, p in enumerate(pitches):
        for q in p['partials']:
            if q['snr_db'] < 6 or q['k'] <= 2:
                continue
            r = min(q['rate'])
            if r > STRING_RATE_MAX or r < 0.31:
                continue
            data.append((pi, q['nominal_hz'], r, q['snr_db']))
    pi = np.array([d[0] for d in data]); f = np.array([d[1] for d in data]); r = np.array([d[2] for d in data])
    w = np.array([d[3] for d in data])
    # sigma = s0 + b2 f^2 saturating at cap (the record's upper partials
    # flatten near 50/s rather than keep growing)
    def res(x):
        q = x[0]*f**2
        return np.sqrt(w)*(np.log(s0[pi] + q/(1 + q/x[1])) - np.log(r))
    sol = least_squares(res, [1e-5, 60.0], bounds=([0, 5], [1e-3, 300]), loss='soft_l1', f_scale=0.5)
    return s0, sol.x.tolist(), data


def coupling_poles(pitches):
    """Fast second poles beside partials 1-2: body resonances pulling the string."""
    out = []
    for p in pitches:
        for q in p['partials']:
            if q['k'] > 2 or len(q['rate']) < 2:
                continue
            j = int(np.argmax(q['rate']))
            if q['rate'][j] < 4 or q['snr_db'] < 8:
                continue
            res = [r for r in q['residues'] if r]
            ratio = np.median([abs(complex(*r[j]))/max(abs(complex(*r[1 - j])), 1e-9) for r in res])
            out.append({'pitch': p['name'], 'k': q['k'], 'hz': q['hz'][j], 'rate': q['rate'][j], 'string_hz': q['hz'][1 - j],
                        'string_rate': q['rate'][1 - j], 'amp_ratio': float(ratio)})
    return out


COUPLED_EXTRAPOLATION = 1.5    # max e-folds a fast pole may be carried back before its window


def coupled(pitches, takes):
    """Partials 1-2: the fast pole beside the string pole (body coupling).

    Per pitch: frequency ratio to the string pole and loss rate. Per take: its
    complex amplitude relative to the string pole's, with the back
    extrapolation from the window start capped (a fast pole carried back
    tens of ms would invent an attack the record does not have).
    """
    per_pitch = []
    for pi, p in enumerate(pitches):
        settle = 1.6/p['bandwidth_hz']
        ks = []
        for k in (1, 2):
            q = next((q for q in p['partials'] if q['k'] == k), None)
            if q is None or len(q['rate']) < 2 or q['snr_db'] < 6:
                ks.append(None)
                continue
            j = int(np.argmax(q['rate']))
            ks.append((q, j))
        per_pitch.append((ks, settle))
    for t in takes:
        ks, settle = per_pitch[t['pitch']]
        cpl = []
        for kk, item in enumerate(ks):
            if item is None:
                cpl.append({'ratio': 1.0, 'rate': 10.0, 're': 0.0, 'im': 0.0})
                continue
            q, j = item
            r = q['residues'][t['take']]
            if not r:
                cpl.append({'ratio': 1.0, 'rate': 10.0, 're': 0.0, 'im': 0.0})
                continue
            fast, slow = complex(*r[j]), complex(*r[1 - j])
            c = fast/slow if abs(slow) > 0 else 0j
            c *= np.exp(-max(0.0, q['rate'][j]*settle - COUPLED_EXTRAPOLATION))
            if abs(c) > 6:
                c *= 6/abs(c)
            cpl.append({'ratio': q['hz'][j]/q['hz'][1 - j], 'rate': q['rate'][j], 're': float(c.real), 'im': float(c.imag)})
        t['coupled'] = cpl


def main():
    import argparse
    ap = argparse.ArgumentParser()
    ap.add_argument('--cap', type=float, default=None, help='override the loss ceiling (1/s)')
    args = ap.parse_args()
    d = json.loads((HERE/'analysis.json').read_text())
    pitches = usable_pitches(d)
    s0, b, _ = fit_loss(pitches)
    if args.cap is not None:
        b[1] = args.cap
    rows = observations(pitches, (s0, b[0], b[1]))
    takes, h, err = fit_excitation(pitches, rows)
    coupled(pitches, takes)
    body = coupling_poles(pitches)
    print(f'excitation fit: {len(rows)} obs, rms {20/np.log(10)*np.sqrt(np.mean(err**2)):.1f} dB')
    print('EQ (dB) at', ' '.join(f'{k:.0f}' for k in KNOTS))
    print('       ', ' '.join(f'{20/np.log(10)*v:+.1f}' for v in h))
    print(f'loss b2 = {b[0]:.2e} /s/Hz^2 cap {b[1]:.1f}/s;', ' '.join(f"{p['name']} s0={v:.2f}" for p, v in zip(pitches, s0)))
    for t in takes:
        p = pitches[t['pitch']]
        print(f"  {p['name']:4s} take {t['take']:2d} @{p['takes'][t['take']]['onset_s']:6.2f}s L {20/np.log(10)*t['level']:6.1f}dB beta {t['beta']:.3f} alpha {t['alpha']:.2f} fc {t['fc_hz']:6.0f} rms {t['rms_db']:.1f}dB n {t['n']}")
    for c in body:
        print('  body', c)
    model = {'pitches': [{'name': p['name'], 'midi': p['midi'], 'f0_hz': p['f0_hz'], 'B': p['B'], 's0': float(v),
                          'takes': p['takes']} for p, v in zip(pitches, s0)],
             'loss_b2': b[0], 'loss_cap': b[1], 'eq_knots_hz': KNOTS.tolist(), 'eq_ln': h.tolist(), 'takes': takes, 'coupling': body}
    (HERE/'model.json').write_text(json.dumps(model, indent=1))


if __name__ == '__main__':
    main()
