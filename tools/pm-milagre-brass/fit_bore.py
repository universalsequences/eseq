#!/usr/bin/env python3
"""Identify the horn: lip-valve constants, the modal bore and the body response.

Stage 1 of the PM Milagre Brass identification. A probe patch (the same lip
valve and modal bore as the instrument, with every constant a parameter, no
body EQ and no hall) is compiled with the production DGenLisp compiler. For
each sounding regime (partials 1, 3, 5, 6) it is blown with a slow breath ramp
and the steady harmonic spectrum is tabulated against breath. A fixed body
response G(f) (radiation, microphone and room colouring, shared by every note)
and a per-frame breath are then fitted jointly to the reference's harmonic
amplitudes, and the probe constants are searched to minimise what remains:

  1. random starts + Nelder-Mead over the ten lip/bore constants,
  2. per-mode frequency ratios (coordinate search in cents; ratios 3, 5 and 6
     are measured from the sounding notes and stay fixed),
  3. per-mode impedance peak heights (coordinate search in dB),

repeated for several rounds. Notes are weighted equally. Writes bore.json.
`--finalize` turns bore.json into fit.json: it fits an RBJ peaking cascade to
G(f) and seeds the performance stage with the per-frame breath.

Usage: fit_bore.py [--rounds N] [--resume] | --finalize
"""
import argparse
import json
import sys
from collections import Counter
from pathlib import Path

import numpy as np
from scipy.optimize import minimize

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
from common import FMAX, SR, WIN, analysis, fit_peaking, harmonics, instrument, peaking_response  # noqa: E402

K = 16
PROBE = HERE/'probe'
GNAMES = ['P', 'Lp', 'alpha', 'h0', 'beta', 'C0', 'Q0', 'qexp', 'fc', 'Zr', 'kappa']
LO = np.array([0.1, 0.05, 0.2, -1.5, -1.0, 0.05, 3, -0.5, 300, 0.0, 0.0])
HI = np.array([20, 8, 4, 2, 2.5, 40, 150, 1.4, 7000, 5, 3.0])
FIXED_MODES = (2, 4, 5)            # 0-based: ratios 3, 5, 6 are the sounding notes
BGRID = np.geomspace(0.01, 2.0, 160)     # breath before the register law
LOGF = np.log(np.geomspace(50, 6000, 36))
LAM = 50.0


def write_probe():
    L = ['(def mod1 (in 6 @name mod1 @modulator 1))', '(def mod2 (in 7 @name mod2 @modulator 2))',
         '(def mod3 (in 8 @name mod3 @modulator 3))', '(def mod4 (in 9 @name mod4 @modulator 4))']
    bounds = dict(P=(0, 20), Lp=(0, 8), alpha=(0, 4), h0=(-2, 2), beta=(-2, 3), C0=(0, 50), Q0=(2, 200),
                  qexp=(-1, 1.5), fc=(200, 8000), Zr=(0, 10), kappa=(0, 3), b=(0, 4), fb=(20, 2000))
    for name, (lo, hi) in bounds.items():
        L.append(f'(param {name} @default {lo} @min {lo} @max {hi})')
    for k in range(1, K + 1):
        L.append(f'(param r{k} @default {0.75 if k == 1 else k} @min 0.3 @max 30)')
        L.append(f'(param g{k} @default 0 @min -40 @max 40)')
    L += ['(def gate (in 1 @name gate))', '(def pitch (in 2 @name pitch))', '(def velocity (in 3 @name velocity))',
          '(def trigger (in 4 @name trigger))', '(def clock (in 5 @name clock))',
          '(make-history u1)', '(make-history u2)']
    hs, zs = [], []
    for k in range(1, K + 1):
        L += [f'(make-history y1_{k})', f'(make-history y2_{k})',
              f'(def f{k} (min (* fb r{k}) (* 0.45 samplerate)))',
              f'(def c{k} (* C0 (pow 10 (/ g{k} 20)) (exp (* -1 (/ f{k} fc) (/ f{k} fc)))))',
              f'(def R{k} (exp (/ (* -1 pi (/ f{k} (* Q0 (pow r{k} qexp)))) samplerate)))',
              f'(def a1_{k} (* 2 R{k} (cos (/ (* twopi f{k}) samplerate))))',
              f'(def a2_{k} (* R{k} R{k}))', f'(def b0_{k} (* 0.5 (- 1 a2_{k})))',
              f'(def h{k} (- (* a1_{k} (read-history y1_{k})) (* a2_{k} (read-history y2_{k})) (* b0_{k} (read-history u2))))']
        hs.append(f'(* c{k} h{k})')
        zs.append(f'(* c{k} b0_{k})')
    L += [f'(def phist (+ {" ".join(hs)}))', f'(def zd (+ Zr {" ".join(zs)}))',
          ';; register law: higher partials need proportionally less breath',
          '(def be (* b (pow (/ pitch 112) (- kappa))))',
          '(def opening (max 0 (+ h0 (* Lp (pow be alpha) (pow (/ pitch 112) (- beta)) (sin (* twopi (phasor pitch)))))))',
          '(def across (- (* P be) phist))', '(def zh (* zd opening))',
          '(def root (* 0.5 (- (sqrt (+ (* zh zh) (* 4 (abs across)))) zh)))',
          '(def flow (* (sign across) opening root))', '(def pm (+ phist (* zd flow)))',
          '(write-history u2 (read-history u1))', '(write-history u1 flow)']
    for k in range(1, K + 1):
        L += [f'(write-history y2_{k} (read-history y1_{k}))', f'(write-history y1_{k} (+ h{k} (* b0_{k} flow)))']
    L.append('(out pm 1 @name audio)')
    PROBE.mkdir(exist_ok=True)
    (PROBE/'dsp.lisp').write_text('\n'.join(L) + '\n')


class Problem:
    def __init__(self):
        write_probe()
        self.inst = instrument(PROBE)
        A = analysis()
        self.notes = A['notes']
        self.ref = []
        for f in A['frames']:
            if f['note'] is None:
                continue
            n = self.notes[f['note']]
            if n['on'] + 0.03 <= f['t'] <= n['off'] - 0.02:
                self.ref.append((f['note'], f['t'], np.array(f['harm'])))
        self.count = Counter(ni for ni, _, _ in self.ref)
        # one representative pitch per regime (the A2 notes differ by < 8 cents)
        self.regimes = {}
        for n in self.notes:
            self.regimes.setdefault(n['partial'], n['hz'])

    def tables(self, g, r, gm):
        prm = dict(zip(GNAMES, g))
        prm.update({f'r{k + 1}': r[k] for k in range(K)})
        prm.update({f'g{k + 1}': gm[k] for k in range(K)})
        tab = {}
        dur = 4.0
        centres = 0.3 + np.linspace(0, dur, len(BGRID))
        ramp = [(0, BGRID[0])] + [(float(c), float(b)) for c, b in zip(centres, BGRID)]
        w = np.hanning(WIN)
        fr = np.fft.rfftfreq(4*WIN, 1/SR)
        for part, hz in self.regimes.items():
            fb = hz if part == 1 else hz/r[part - 1]
            y, _ = self.inst.render(seconds=dur + 0.4, pitch=hz, params=dict(prm, fb=fb, b=BGRID[0]),
                                    ramps={'b': ramp})
            if not np.isfinite(y).all() or np.abs(y).max() > 1e3:
                return None
            y = y.astype(np.float64)
            rows = []
            for c in centres:
                s = int(c*SR) - WIN//2
                X = np.abs(np.fft.rfft(y[s:s + WIN]*w, 4*WIN))*2/w.sum()
                rows.append(harmonics(X, fr, hz))
            tab[part] = np.array(rows)
        return tab

    def fit_G_b(self, tab, iters=5):
        rows = []
        for ni, t, T in self.ref:
            n = self.notes[ni]
            S = tab[n['partial']]
            k = min(S.shape[1], len(T))
            rows.append((T[:k], S[:, :k], n['hz']*np.arange(1, k + 1), T[:k].max() - 40, 1.0/self.count[ni]))
        Gk = np.zeros(len(LOGF))
        nK = len(LOGF)
        D = np.diff(np.eye(nK), 2, axis=0)
        for _ in range(iters):
            AtA = np.zeros((nK, nK))
            Atb = np.zeros(nK)
            tot = cnt = 0.0
            picks = []
            for T, S, f, floor, wt in rows:
                G = np.interp(np.log(f), LOGF, Gk)
                err = ((np.maximum(S + G, floor) - np.maximum(T, floor))**2).sum(1)
                j = int(err.argmin())
                picks.append(j)
                tot += wt*err[j]
                cnt += wt*len(T)
                mask = T > floor
                lf = np.log(f)[mask]
                res = (T - S[j])[mask]
                idx = np.clip(np.searchsorted(LOGF, lf) - 1, 0, nK - 2)
                a = np.clip((lf - LOGF[idx])/(LOGF[idx + 1] - LOGF[idx]), 0, 1)
                for i, aa, v in zip(idx, a, res):
                    AtA[i, i] += wt*(1 - aa)**2
                    AtA[i + 1, i + 1] += wt*aa**2
                    AtA[i, i + 1] += wt*aa*(1 - aa)
                    AtA[i + 1, i] += wt*aa*(1 - aa)
                    Atb[i] += wt*(1 - aa)*v
                    Atb[i + 1] += wt*aa*v
            Gk = np.linalg.solve(AtA + LAM*D.T@D + 1e-3*np.eye(nK), Atb)
        return Gk, float(np.sqrt(tot/cnt)), picks

    def loss(self, g, r, gm):
        tab = self.tables(g, r, gm)
        return 99.0 if tab is None else self.fit_G_b(tab)[1]


def search(rounds, resume):
    P = Problem()
    out = HERE/'bore.json'
    if resume and out.exists():
        s = json.loads(out.read_text())
        g = np.array([s['g'].get(k, d) for k, d in zip(GNAMES, [4, 2, 2, 0.3, 1.0, 4, 30, 0.5, 1500, 0.5, 1.16])])
        r, gm = np.array(s['r']), np.array(s['gm'])
    else:
        g = np.array([4, 2, 2, 0.3, 1.0, 4, 30, 0.5, 1500, 0.5, 1.16])
        r = np.array([0.75] + [float(k) for k in range(2, K + 1)])
        gm = np.zeros(K)
    for part in (3, 5, 6):                          # measured from the sounding notes
        r[part - 1] = part*P.regimes[part]/(part*P.regimes[1])
    best = [P.loss(g, r, gm)]
    print('start', round(best[0], 3), flush=True)

    def save():
        out.write_text(json.dumps(dict(g=dict(zip(GNAMES, map(float, g))) | {'_order': GNAMES},
                                       r=list(map(float, r)), gm=list(map(float, gm)), loss=best[0]), indent=1))

    def wrap(u):
        return LO + (HI - LO)/(1 + np.exp(-u))

    def unwrap(th):
        p = np.clip((th - LO)/(HI - LO), 1e-4, 1 - 1e-4)
        return np.log(p/(1 - p))

    def f(u):
        e = P.loss(wrap(u), r, gm)
        if e < best[0] - 1e-4:
            best[0] = e
            g[:] = wrap(u)
            print(f'globals {e:.3f}', flush=True)
            save()
        return e

    rng = np.random.default_rng(0)
    for rnd in range(rounds):
        if rnd == 0 and not resume:
            for _ in range(16):
                f(unwrap(LO + (HI - LO)*rng.random(len(LO))))
        minimize(f, unwrap(g.copy()), method='Nelder-Mead', options=dict(maxfev=250, xatol=1e-3, fatol=1e-3))
        for k in range(K):
            if k in FIXED_MODES:
                continue
            base = r[k]
            span = [-300, -150, -80, -40, -20, 20, 40, 80, 150, 300] if k == 0 else [-200, -120, -80, -40, -20, -10, 10, 20, 40, 80, 120, 200]
            for c in span:
                rr = r.copy()
                rr[k] = base*2**(c/1200)
                e = P.loss(g, rr, gm)
                if e < best[0] - 1e-4:
                    best[0] = e
                    r[:] = rr
                    print(f'ratio {k + 1} {e:.3f} {r[k]:.3f}', flush=True)
                    save()
        for k in range(K):
            for dg in [-12, -6, -3, 3, 6, 12]:
                gg = gm.copy()
                gg[k] += dg
                e = P.loss(g, r, gg)
                if e < best[0] - 1e-4:
                    best[0] = e
                    gm[:] = gg
                    print(f'height {k + 1} {e:.3f} {gm[k]:+.1f} dB', flush=True)
                    save()
        print('round', rnd, round(best[0], 3), flush=True)
    save()


def finalize():
    P = Problem()
    s = json.loads((HERE/'bore.json').read_text())
    g = np.array([s['g'][k] for k in GNAMES])
    r, gm = np.array(s['r']), np.array(s['gm'])
    tab = P.tables(g, r, gm)
    Gk, err, picks = P.fit_G_b(tab, iters=10)
    # body EQ: fit the cascade where the reference had harmonics
    f = np.geomspace(80, 6000, 200)
    target = np.interp(np.log(f), LOGF, Gk)
    sections, trim = fit_peaking(f, target, np.ones_like(f), sections=12)
    fitted = peaking_response(f, sections) + trim
    print(f'stage-1 harmonic error {err:.2f} dB; body cascade max dev {np.abs(fitted - target).max():.2f} dB')
    # Breath as the player's control: scaled so the recorded pedal A2 sits at 0.6.
    pedal = [float(BGRID[j]) for (ni, _, _), j in zip(P.ref, picks) if P.notes[ni]['partial'] == 1]
    scale = float(np.median(pedal))/0.6
    breath = [dict(t=t, b=float(BGRID[j])/scale) for (_, t, _), j in zip(P.ref, picks)]
    fit = dict(
        globals=dict(zip(GNAMES, map(float, g))) | dict(breath_scale=scale),
        modes=dict(ratio=list(map(float, r)), gain=list(map(float, gm))),
        body=dict(sections=sections, trim=trim, knots_hz=list(map(float, np.exp(LOGF))), knots_db=list(map(float, Gk))),
        stage1=dict(error_db=err, breath=breath),
        defaults=dict(breath=0.8, breath_ms=30.0, air=0.0, tune=31.0, slide=25.0, vib_cent=0.0, vib_hz=1.5,
                      attack=20.0, release=60.0, hall=0.3, hall_s=1.5))
    old = HERE/'fit.json'
    if old.exists():
        fit['defaults'] = json.loads(old.read_text()).get('defaults', fit['defaults'])
    old.write_text(json.dumps(fit, indent=1))
    print('wrote fit.json')


if __name__ == '__main__':
    ap = argparse.ArgumentParser()
    ap.add_argument('--rounds', type=int, default=4)
    ap.add_argument('--resume', action='store_true')
    ap.add_argument('--finalize', action='store_true')
    a = ap.parse_args()
    finalize() if a.finalize else search(a.rounds, a.resume)
