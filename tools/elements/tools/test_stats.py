#!/usr/bin/env python3
"""Noise-driven paths with real noise on both sides: RMS envelope (100 ms
windows) and spectral centroid, ref vs port."""
import os, sys
import numpy as np
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import elh

CASES = [
    ("particles", dict(strike_meta=0.85, gate=1.5, seconds=2)),
    ("particles dense", dict(strike_meta=0.98, strike_timbre=0.8, gate=1.5, seconds=2)),
    ("particles sparse", dict(strike_meta=0.82, strike_timbre=0.3, gate=1.5, seconds=2, note=36)),
    ("bow", dict(bow=0.8, strike=0, gate=1.5, seconds=2)),
    ("blow", dict(blow=0.6, strike=0, gate=1.5, seconds=2)),
    ("blow tube", dict(blow=0.95, strike=0, gate=1.5, seconds=2, note=40)),
    ("strings blow", dict(model=2, blow=0.6, strike=0, gate=1.5, seconds=2)),
    ("string disp noise", dict(model=1, geometry=1.0, gate=1.5, seconds=2)),
    ("reverb tail", dict(space=1.0, seconds=3, gate=0.2)),
]


def centroid(y, sr=32000):
    s = np.abs(np.fft.rfft(y * np.hanning(len(y))))
    f = np.fft.rfftfreq(len(y), 1 / sr)
    return float(np.sum(f * s) / max(np.sum(s), 1e-12))


if __name__ == "__main__":
    t = elh.Test()
    for label, kw in CASES:
        kw = dict(dict(space=0), **kw)
        r = t.ref(**kw)[:, 0]
        p = t.port(**kw)[:, 0]
        n = min(len(r), len(p))
        w = 3200
        rr = np.array([np.sqrt(np.mean(r[k:k + w] ** 2)) for k in range(0, n - w + 1, w)])
        pr = np.array([np.sqrt(np.mean(p[k:k + w] ** 2)) for k in range(0, n - w + 1, w)])
        db = 20 * np.log10(np.maximum(pr, 1e-9) / np.maximum(rr, 1e-9))
        print(f"{label:18s} rms ref {np.sqrt(np.mean(r[:n]**2)):.4f} port {np.sqrt(np.mean(p[:n]**2)):.4f}  "
              f"env dB diff max {np.max(np.abs(db)):.2f} mean {np.mean(db):+.2f}  "
              f"centroid ref {centroid(r[:n]):.0f} port {centroid(p[:n]):.0f} Hz", flush=True)
