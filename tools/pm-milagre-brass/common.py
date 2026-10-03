"""Shared helpers for the PM Milagre Brass identification tools."""
import json
import os
import platform
import sys
from pathlib import Path

import numpy as np

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
sys.path.insert(0, str(ROOT/'tools/audition'))
from audition import Instrument  # noqa: E402

NAME = 'PM Milagre Brass'
DEST = ROOT/'content/instruments/Physical Models'/NAME
SR = 48000
WIN = 2048
FMAX = 6000.0


def analysis():
    return json.loads((HERE/'analysis.json').read_text())


def instrument(path=DEST, sr=SR, block=128):
    target = {('Darwin', 'arm64'): 'DGenLisp-macos-arm64',
              ('Linux', 'x86_64'): 'DGenLisp-linux-x86_64'}[(platform.system(), platform.machine())]
    return Instrument(str(path), compiler=os.environ.get('ESEQ_DGENLISP_TOOL', str(ROOT/'crates/sequencer/tools'/target)),
                      toolchain_root=str(ROOT/'crates/sequencer/tools/dgen-toolchain'),
                      sample_rate=sr, max_frames=block)


def harmonics(X, fr, f0):
    half = min(0.45*f0, 40.0)
    out = []
    for k in range(1, int(FMAX//f0) + 1):
        band = (fr > k*f0 - half) & (fr < k*f0 + half)
        out.append(20*np.log10(X[band].max() + 1e-9))
    return np.array(out)


def frame_spectrum(x, centre, sr=SR):
    s = int(round(centre*sr)) - WIN//2
    seg = np.zeros(WIN)
    lo, hi = max(s, 0), min(s + WIN, len(x))
    if hi > lo:
        seg[lo - s:hi - s] = x[lo:hi]
    w = np.hanning(WIN)
    X = np.abs(np.fft.rfft(seg*w, 4*WIN))*2/w.sum()
    return X, np.fft.rfftfreq(4*WIN, 1/sr)


def peaking_response(f, sections, amount=1.0, sr=SR):
    """dB response of an RBJ peaking cascade [(hz, q, db), ...] at frequencies f."""
    z = np.exp(-2j*np.pi*np.asarray(f)/sr)
    total = np.zeros(len(z))
    for hz, q, db in sections:
        w0 = 2*np.pi*min(hz, 0.45*sr)/sr
        alpha = np.sin(w0)/(2*q)
        A = 10**(db*amount/40)
        b = np.array([1 + alpha*A, -2*np.cos(w0), 1 - alpha*A])
        a = np.array([1 + alpha/A, -2*np.cos(w0), 1 - alpha/A])
        H = (b[0] + b[1]*z + b[2]*z*z)/(a[0] + a[1]*z + a[2]*z*z)
        total += 20*np.log10(np.abs(H))
    return total


def fit_peaking(f, target_db, weights, sections=12, sr=SR):
    """Least-squares RBJ peaking cascade matching target_db (after a free trim)."""
    from scipy.optimize import least_squares
    centres = np.geomspace(max(f.min(), 60), min(f.max(), 5500), sections)
    x0 = np.concatenate([np.log(centres), np.zeros(sections), np.zeros(sections), [np.average(target_db, weights=weights)]])

    def unpack(x):
        hz = np.exp(x[:sections])
        q = 0.3 + 5.7/(1 + np.exp(-x[sections:2*sections]))
        db = 30*np.tanh(x[2*sections:3*sections]/30)
        return list(zip(hz, q, db)), x[-1]

    def resid(x):
        secs, trim = unpack(x)
        return np.sqrt(weights)*(peaking_response(f, secs, sr=sr) + trim - target_db)

    x0[2*sections:3*sections] = np.interp(np.log(centres), np.log(f), target_db) - x0[-1]
    r = least_squares(resid, x0, max_nfev=4000)
    secs, trim = unpack(r.x)
    return [(float(h), float(q), float(d)) for h, q, d in secs], float(trim)


def qualify(inst, params):
    """Map short param names (breath) to the manifest's group-qualified ones (blow.breath)."""
    out = {}
    for k, v in params.items():
        if k in inst.params:
            out[k] = v
            continue
        full = [p for p in inst.params if p.endswith('.' + k) and not p.startswith('__')]
        if len(full) != 1:
            raise KeyError(k)
        out[full[0]] = v
    return out
