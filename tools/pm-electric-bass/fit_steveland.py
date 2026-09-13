#!/usr/bin/env python3
"""Fit PM Electric Bass to the 'Bass Steveland' library samples (C2, vinyl).

Loss: per-partial (h1..h6) dB trajectories and octave-band spectrum of the
model at the sample's pitch versus the recordings. Nelder-Mead with restarts.
Writes fit-steveland.json and .local/pm-electric-bass/steveland-ab.wav.
"""
import json, sys, wave
from pathlib import Path
import numpy as np
from verify import ROOT, OUT, instrument, write_wav

SAMPLES = {'steveland1': '1e7cc72d58e1ecc05297629f316f490722640b45b024eeb594a8bd40c384d53e',
           'steveland2': '0ca83c2030a238535d00874c54a2e12ad6b6cb4bc53cc11d8d77957dfacad6be'}
SR = 44100
F0 = 65.5

def read(path):
    w = wave.open(str(path)); assert w.getframerate() == SR
    x = np.frombuffer(w.readframes(w.getnframes()), np.int16).reshape(-1, w.getnchannels()).mean(1)/32767
    return x.astype(np.float32)

def tracks(x, f0=F0, ks=range(1, 7)):
    t = np.arange(len(x))/SR
    out = []
    for k in ks:
        win = int((0.03 if k < 3 else 0.02)*SR); ker = np.hanning(win); ker /= ker.sum()
        a = np.convolve(x*np.exp(-2j*np.pi*f0*k*t), ker, 'same')
        out.append(20*np.log10(np.abs(a)+1e-7))
    return np.array(out)

def bands(x):
    n = 1 << 15; mag = np.abs(np.fft.rfft(x[:int(.15*SR)]*np.hanning(min(len(x), int(.15*SR))), n))**2
    fr = np.fft.rfftfreq(n, 1/SR); edges = [40, 80, 160, 320, 640, 1280, 2560, 5120, 10240, 20000]
    return np.array([10*np.log10(mag[(fr >= a) & (fr < b)].sum()+1e-12) for a, b in zip(edges[:-1], edges[1:])])

def features(x, upto):
    idx = np.arange(int(.01*SR), int(upto*SR), int(.01*SR))
    return tracks(x)[:, idx], bands(x)

BOUNDS = {
    'pluck.position': (0.08, 0.4), 'pluck.softness': (0, 1), 'pluck.attack_ms': (0, 80),
    'string.decay_s': (1, 12), 'string.damping': (0, 1), 'string.friction': (0, 12),
    'string.stiffness': (0, 0.5), 'pickup.position': (0.05, 0.45), 'pickup.aperture': (0.005, 0.1),
    'output.tone_hz': (140, 900), 'output.resonance': (0.5, 1.4), 'output.steep': (0, 1),
}
NAMES = list(BOUNDS)

def unpack(v):
    return {n: float(np.clip(lo + (hi-lo)*np.clip(u, 0, 1), lo, hi)) for (n, (lo, hi)), u in zip(BOUNDS.items(), v)}

class Fit:
    def __init__(self):
        self.inst = instrument(sr=SR)
        self.targets = []
        for name, h in SAMPLES.items():
            x = read(ROOT/'.local/samples'/f'{h}.wav')
            upto = 0.25 if name == 'steveland1' else 0.15
            self.targets.append((name, x, upto, features(x, upto)))
        self.evals = 0

    def render(self, params, seconds=0.32, gate_off=None):
        y, _ = self.inst.render(seconds=seconds, pitch=F0, vel=0.8, params=params | {'output.gain': 1.0, 'string.mute': 0.0}, gate_off=gate_off)
        return y[:, 0]

    def loss(self, v, detail=False):
        self.evals += 1
        p = unpack(v)
        y = self.render(p)
        if not np.isfinite(y).all():
            return 1e9
        total = 0; parts = {}
        for name, x, upto, (tr_t, bd_t) in self.targets:
            tr_m, bd_m = features(y, upto)
            # h1 level offset is absorbed by output gain: align on h1 median
            off = np.median(tr_t[0]) - np.median(tr_m[0])
            tr_m = tr_m + off; bd_m = bd_m + off
            w = np.array([3, 3, 2, 1.5, 0.7, 0.5])[:, None]
            e_tr = np.mean(w*np.minimum(np.abs(tr_t - tr_m), 25)**2)
            e_bd = np.mean(np.minimum(np.abs(bd_t[:6] - bd_m[:6]), 25)**2)
            parts[name] = (float(e_tr), float(e_bd), float(off))
            total += e_tr + 0.5*e_bd
        return (total, parts) if detail else total

def nelder_mead(f, x0, step=0.15, iters=400):
    n = len(x0); pts = [np.array(x0, float)]
    for i in range(n):
        p = np.array(x0, float); p[i] = np.clip(p[i] + step, 0, 1); pts.append(p)
    vals = [f(p) for p in pts]
    for _ in range(iters):
        order = np.argsort(vals); pts = [pts[i] for i in order]; vals = [vals[i] for i in order]
        c = np.mean(pts[:-1], 0)
        xr = c + (c - pts[-1]); fr = f(xr)
        if fr < vals[0]:
            xe = c + 2*(c - pts[-1]); fe = f(xe)
            pts[-1], vals[-1] = (xe, fe) if fe < fr else (xr, fr)
        elif fr < vals[-2]:
            pts[-1], vals[-1] = xr, fr
        else:
            xc = c + 0.5*(pts[-1] - c); fc = f(xc)
            if fc < vals[-1]:
                pts[-1], vals[-1] = xc, fc
            else:
                pts = [pts[0] + 0.5*(p - pts[0]) for p in pts]; vals = [vals[0]] + [f(p) for p in pts[1:]]
    i = int(np.argmin(vals)); return pts[i], vals[i]

def main():
    fit = Fit()
    rng = np.random.default_rng(int(sys.argv[1]) if len(sys.argv) > 1 and sys.argv[1].isdigit() else 7)
    # seeded guess from measurement: pluck ~0.2 (h5 null), tone ~300 Hz steep, friction ~5, attack ~40 ms
    guess = {'pluck.position': .2, 'pluck.softness': .6, 'pluck.attack_ms': 40, 'string.decay_s': 8,
             'string.damping': .3, 'string.friction': 5, 'string.stiffness': .05, 'pickup.position': .2,
             'pickup.aperture': .03, 'output.tone_hz': 300, 'output.resonance': .9, 'output.steep': 1}
    prior = ROOT/'tools/pm-electric-bass/fit-steveland.json'
    if prior.exists() and '--fresh' not in sys.argv:
        guess = json.loads(prior.read_text())['params']
    x0 = np.array([(guess[n]-lo)/(hi-lo) for n, (lo, hi) in BOUNDS.items()])
    restarts = 1 if prior.exists() and '--fresh' not in sys.argv else 5
    best = (fit.loss(x0), x0)
    print('seed loss', round(best[0], 2), flush=True)
    starts = [x0] + [np.clip(x0 + rng.normal(0, .12, len(x0)), 0, 1) for _ in range(restarts)]
    for i, s in enumerate(starts):
        x, v = nelder_mead(fit.loss, s, iters=300)
        print(f'restart {i}: loss {v:.2f} (evals {fit.evals})', flush=True)
        if v < best[0]:
            best = (v, x)
    x, v = nelder_mead(fit.loss, best[1], step=0.05, iters=400)
    if v < best[0]: best = (v, x)
    params = unpack(best[1]); total, parts = fit.loss(best[1], detail=True)
    # calibrate gain so the model's h1 matches steveland1 at vel 0.8
    off = parts['steveland1'][2]
    params['output.gain'] = float(np.clip(10**(off/20), 0, 1))
    report = {'loss': total, 'parts': parts, 'params': params, 'evals': fit.evals}
    (ROOT/'tools/pm-electric-bass/fit-steveland.json').write_text(json.dumps(report, indent=2)+'\n')
    print(json.dumps(report, indent=2))
    # diagnostics: trajectories side by side
    y = fit.render(params | {'output.gain': 1.0})
    name, x, upto, (tr_t, _) = fit.targets[0]
    tr_m, _ = features(y, upto); tr_m += off
    print('t(ms)  ' + '  '.join(f'h{k} smp/mod' for k in range(1, 7)))
    for j, t in enumerate(np.arange(10, upto*1000, 10)):
        if j % 2 == 0:
            print(f'{t:5.0f}  ' + '  '.join(f'{tr_t[k, j]:5.1f}/{tr_m[k, j]:5.1f}' for k in range(6)))
    # A/B: sample1, model same length, sample2, model muted, then model 2 s note
    seg = lambda a: np.concatenate([a, np.zeros(int(.3*SR), np.float32)])
    p_ab = params | {'vinyl.hiss': 0.6, 'vinyl.rumble': 0.3}
    ab = [seg(fit.targets[0][1]), seg(fit.render(p_ab, .28)), seg(fit.targets[1][1]),
          seg(fit.render(p_ab | {'string.mute': 0.0}, .31, gate_off=.16)), seg(fit.render(p_ab, 2.5, gate_off=2.0))]
    write_wav(str(OUT/'steveland-ab.wav'), np.concatenate(ab), SR)
    print('wrote', OUT/'steveland-ab.wav')

if __name__ == '__main__':
    main()
