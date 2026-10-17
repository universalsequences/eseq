#!/usr/bin/env python3
"""Statistical comparison with real noise for the noise-excited models
(the string models' plucker burst and string 0's dispersion noise): level
over time, spectral centroid and band energies, port vs ref/rref, averaged
over a few seeds / retriggers."""
import os, subprocess

import numpy as np
import rh

SETTINGS = [
    (1, dict(structure=0.3, brightness=0.6, damping=0.6, position=0.3)),
    (1, dict(structure=0.8, brightness=0.3, damping=0.8, position=0.7, note=36)),
    (2, dict(structure=0.5, brightness=0.6, damping=0.5, position=0.3)),
    (2, dict(structure=0.95, brightness=0.8, damping=0.7, position=0.5)),   # dispersion noise
    (2, dict(structure=0.05, brightness=0.5, damping=0.6, position=0.6)),   # curved bridge
    (4, dict(structure=0.45, brightness=0.7, damping=0.7, position=0.2)),
    (5, dict(structure=0.9, brightness=0.5, damping=0.6, position=0.8)),
]


def stats(y):
    frames = [np.sqrt(np.mean(y[i:i + 2400] ** 2)) for i in range(0, len(y) - 2400, 2400)]
    w = np.abs(np.fft.rfft(y * np.hanning(len(y)))) ** 2
    f = np.fft.rfftfreq(len(y), 1 / 48000)
    cent = float((w * f).sum() / w.sum())
    bands = [float(w[(f >= lo) & (f < hi)].sum() / w.sum()) for lo, hi in ((0, 500), (500, 2000), (2000, 6000), (6000, 24000))]
    return np.array(frames), cent, bands


def ref_seed(t, seed, **kw):
    path = os.path.join(t.dir, "ref.raw")
    env = dict(os.environ, RREF_SEED=str(seed))
    subprocess.run([rh.REF, path] + [f"{k}={v}" for k, v in kw.items()], check=True, env=env)
    return np.fromfile(path, dtype=np.float32).reshape(-1, 2)


def main():
    """Each pluck is a short random burst, so single renders vary a lot:
    the port should fall inside the spread of the reference over seeds."""
    t = rh.VoiceTest(tag="voice_noise")
    for m, kw in SETTINGS:
        kw = dict(kw, model=m, retrig=0.25, seconds=10.0)
        refs = [stats(ref_seed(t, seed, **kw)[:, 0]) for seed in (33, 7, 12345)]
        fp, cp, bp = stats(t.port(**kw)[:, 0])
        lv = [20 * np.log10(np.mean(f)) for f, _, _ in refs]
        cs = [c for _, c, _ in refs]
        print(f"m{m} {dict((k, v) for k, v in kw.items() if k not in ('model', 'retrig', 'seconds'))}")
        print(f"   level ref {min(lv):.2f}..{max(lv):.2f} port {20 * np.log10(np.mean(fp)):.2f} dB   "
              f"centroid ref {min(cs):.0f}..{max(cs):.0f} port {cp:.0f} Hz   "
              f"low band ref {min(b[0] for *_, b in refs):.3f}..{max(b[0] for *_, b in refs):.3f} port {bp[0]:.3f}")


if __name__ == "__main__":
    main()
