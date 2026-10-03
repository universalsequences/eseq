#!/usr/bin/env python3
"""Stage 2: fit the performance and the envelope/hall defaults.

Plays the reference phrase through the built instrument exactly as the step
sequencer will (performance.py: 100 BPM, 1/32 steps, a Breath p-lock on every
step, Tune locked per note, mono legato), and compares every 10 ms frame's
harmonic amplitudes with the recording (analysis.json):

  * Breath per step: iterative learning control on the frame loudness, seeded
    by stage 1's per-frame breath.
  * attack, release, breath_ms, slide, hall, hall_s: Nelder-Mead on the
    whole-phrase harmonic error (including the gaps, where only the hall rings),
    re-running the breath control at every candidate.

Writes phrase.json (the performance) and the fitted defaults into fit.json,
then rebuilds the instrument. Run build.py first.
"""
import json
import subprocess
import sys
from pathlib import Path

import numpy as np
from scipy.optimize import minimize

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
from common import analysis, frame_spectrum, harmonics, instrument  # noqa: E402
import performance as perf  # noqa: E402

FIT = HERE/'fit.json'
PHRASE = HERE/'phrase.json'
FITTED = ['attack', 'release', 'breath_ms', 'slide', 'hall', 'hall_s']
LO = np.array([1, 5, 2, 0, 0.0, 0.3])
HI = np.array([200, 600, 150, 120, 1.0, 4.0])


class Scorer:
    def __init__(self):
        A = analysis()
        self.notes = A['notes']
        self.frames = [f for f in A['frames'] if f['t'] <= 7.9]
        self.noise_hz = np.log(A['noise_hz'])
        self.noise_db = np.array(A['noise_db']) + 3      # pre-roll hum/hiss floor of the recording
        self.ref = [np.array(f['harm']) for f in self.frames]
        self.ref_level = np.array([self.level(h) for h in self.ref])
        self.in_note = np.array([f['note'] is not None for f in self.frames])

    @staticmethod
    def level(h):
        return 10*np.log10(np.sum(10**(np.asarray(h)/10)))

    def model(self, y):
        y = y.astype(np.float64)
        out = []
        for f in self.frames:
            X, fr = frame_spectrum(y, f['t'])
            out.append(harmonics(X, fr, self.notes[f['ref']]['hz'])[:len(f['harm'])])
        return out

    def errors(self, model):
        e = []
        for f, T, M in zip(self.frames, self.ref, model):
            fk = self.notes[f['ref']]['hz']*np.arange(1, len(T) + 1)
            floor = np.maximum(np.interp(np.log(fk), self.noise_hz, self.noise_db), T.max() - 40)
            e.append(np.sqrt(np.mean((np.maximum(M, floor) - np.maximum(T, floor))**2)))
        return np.array(e)


def seed_breath(fit):
    b = np.full(perf.STEPS, 0.0)
    pts = fit['stage1']['breath']
    t = np.array([p['t'] for p in pts])
    v = np.array([p['b'] for p in pts])
    for k in range(perf.STEPS):
        sel = (t >= k*perf.STEP) & (t < (k + 1)*perf.STEP)
        b[k] = v[sel].mean() if sel.any() else np.nan
    # hold the last value through gaps; unset leading steps take the first value
    last = v[0]
    for k in range(perf.STEPS):
        if np.isnan(b[k]):
            b[k] = last
        last = b[k]
    return b


def control(inst, sc, notes, breath, params, iters=8, gamma=1.5, tune=None):
    breath = breath.copy()
    for _ in range(iters):
        y = perf.render(inst, notes, breath, params, tune_steps=tune)
        lv = np.array([sc.level(h) for h in sc.model(y)])
        d = lv - sc.ref_level
        corr = np.zeros(perf.STEPS)
        have = np.zeros(perf.STEPS)
        for k in range(perf.STEPS):
            # breath acts after the smoothing lag; look half a step later
            t0, t1 = (k + 0.5)*perf.STEP, (k + 1.5)*perf.STEP
            sel = [i for i, f in enumerate(sc.frames) if t0 <= f['t'] < t1 and sc.in_note[i]]
            if sel:
                corr[k] = np.mean(d[sel])
                have[k] = 1
        # neighbouring steps share frames through the smoothing: a damped,
        # smoothed correction keeps the loop from zig-zagging
        kern = np.array([0.25, 0.5, 0.25])
        corr = np.convolve(corr*have, kern, 'same')/np.maximum(np.convolve(have, kern, 'same'), 1e-6)
        breath = np.where(have > 0, np.clip(breath*10**(-0.6*corr/(20*gamma)), 0.02, 3.0), breath)
    y = perf.render(inst, notes, breath, params, tune_steps=tune)
    return breath, y


def main():
    fit = json.loads(FIT.read_text())
    inst = instrument()
    sc = Scorer()
    notes = sc.notes
    breath = seed_breath(fit)
    tune = perf.tune_steps(notes, sc.frames)
    x0 = np.array([fit['defaults'][k] for k in FITTED], float)
    best = dict(err=1e9)

    def evaluate(x, iters=6):
        params = dict(zip(FITTED, x))
        b, y = control(inst, sc, notes, best.get('breath', breath), params, iters=iters, tune=tune)
        e = sc.errors(sc.model(y))
        err = float(np.sqrt(np.mean(e**2)))
        if err < best['err']:
            best.update(err=err, x=x.copy(), breath=b, frames=e)
            print(f'{err:.3f} ' + ' '.join(f'{k}={v:.3g}' for k, v in params.items()), flush=True)
        return err

    def f(u):
        return evaluate(LO + (HI - LO)/(1 + np.exp(-u)))

    evaluate(x0, iters=12)
    p = np.clip((x0 - LO)/(HI - LO), 1e-3, 1 - 1e-3)
    minimize(f, np.log(p/(1 - p)), method='Nelder-Mead', options=dict(maxfev=120, xatol=1e-3, fatol=1e-3))
    evaluate(best['x'], iters=12)
    for k, v in zip(FITTED, best['x']):
        fit['defaults'][k] = float(v)
    FIT.write_text(json.dumps(fit, indent=1))
    e = best['frames']
    per_note = {}
    for i, fr in enumerate(sc.frames):
        key = fr['note'] if fr['note'] is not None else 'gap'
        per_note.setdefault(key, []).append(e[i])
    report = {('gap' if k == 'gap' else f"{k}:{notes[k]['name']}"): round(float(np.sqrt(np.mean(np.square(v)))), 2)
              for k, v in per_note.items()}
    PHRASE.write_text(json.dumps(dict(
        bpm=perf.BPM, timebase=perf.TIMEBASE, steps=perf.STEPS, step_seconds=perf.STEP,
        notes=perf.events(notes), breath=[round(float(v), 4) for v in best['breath']], tune=tune,
        error_db=round(best['err'], 3), error_by_note=report), indent=1))
    print('per-note harmonic error (dB):', report)
    subprocess.run([sys.executable, str(HERE/'build.py')], check=True)


if __name__ == '__main__':
    main()
