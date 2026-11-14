"""Measure the Saturator builtin through the real engine.

    cargo build --release -p sequencer --bin saturator_render
    python3 tools/saturator/measure.py [workdir] [sections...]
    SAT_CURVES=13,14 python3 tools/saturator/measure.py   # other curves

Curves are the Saturator's "curve" indices: 12 triode (the default measured
here), 13 twin triode, 14 op-amp clip, 15 ge fuzz, 16 transformer; 0..11 are
the static curves.

Renders test signals with target/release/saturator_render and prints:
harmonic levels vs drive, aliasing (inharmonic energy on a 5 kHz tone),
small-signal frequency response, output level vs drive on pink noise, and CPU.
"""
import os
import subprocess
import sys

import numpy as np
from scipy.io import wavfile

SR = 48000
ROOT = os.path.normpath(os.path.join(os.path.dirname(os.path.abspath(__file__)), "../.."))
BIN = os.path.join(ROOT, "target/release/saturator_render")
WORK = sys.argv[1] if len(sys.argv) > 1 else "/tmp/saturator-measure"
os.makedirs(WORK, exist_ok=True)


def render(x, bench=False, **params):
    src, dst = os.path.join(WORK, "in.wav"), os.path.join(WORK, "out.wav")
    wavfile.write(src, SR, x.astype(np.float32))
    cmd = [BIN, "--in", src, "--out", dst]
    for name, value in params.items():
        cmd += ["--set", f"{name.replace('_', ' ').replace('dry wet', 'dry/wet')}={value}"]
    if bench:
        cmd.append("--bench")
    res = subprocess.run(cmd, capture_output=True, text=True, check=True)
    _, y = wavfile.read(dst)
    return (y[:, 0].astype(np.float64), res.stderr.strip()) if bench else y[:, 0].astype(np.float64)


def db(x):
    return 20 * np.log10(np.maximum(np.abs(x), 1e-12))


def rms_db(x):
    return 10 * np.log10(np.mean(x ** 2) + 1e-24)


def sine(freq, dbfs, seconds=2.0):
    t = np.arange(int(SR * seconds)) / SR
    return 10 ** (dbfs / 20) * np.sin(2 * np.pi * freq * t)


def pink(seconds=6.0, dbfs=-18.0, seed=1):
    rng = np.random.default_rng(seed)
    n = int(SR * seconds)
    spec = np.fft.rfft(rng.standard_normal(n))
    f = np.fft.rfftfreq(n, 1 / SR)
    spec[1:] /= np.sqrt(f[1:])
    spec[f < 30] = 0
    x = np.fft.irfft(spec, n)
    return x * 10 ** (dbfs / 20) / np.sqrt(np.mean(x ** 2))


def spectrum(y):
    seg = y[-SR:]  # 1 s: 1 Hz bins, integer-Hz tones land on bins
    return np.abs(np.fft.rfft(seg * np.hanning(len(seg)))) / (len(seg) / 4)


def harmonics(curve):
    print(f"\nharmonics, 1 kHz at -18 dBFS, curve {curve} (dB relative to fundamental)")
    print(" drive   fund   H2     H3     H4     H5     THD%")
    for drive in (0, 6, 12, 18, 24, 36):
        s = spectrum(render(sine(1000, -18), curve=curve, drive=drive, auto_gain=0))
        h = [s[k * 1000 - 3:k * 1000 + 4].max() for k in range(1, 11)]
        thd = 100 * np.sqrt(sum(v ** 2 for v in h[1:])) / h[0]
        print(f" {drive:4d}  {db(h[0]):6.1f} " + " ".join(f"{db(v / h[0]):6.1f}" for v in h[1:5]) + f"  {thd:6.2f}")


def aliasing(curve):
    print(f"\naliasing, 5 kHz tone, curve {curve}: worst inharmonic component below 20 kHz (dB re fundamental)")
    for dbfs, drive in ((-18, 12), (-6, 24), (0, 36)):
        s = spectrum(render(sine(5000, dbfs), curve=curve, drive=drive, auto_gain=0))
        fund = s[4997:5004].max()
        mask = np.ones(len(s), bool)
        for k in range(0, 5):
            mask[max(0, k * 5000 - 40):k * 5000 + 41] = False
        mask[20000:] = False
        mask[:40] = False
        worst = np.argmax(s * mask)
        print(f"  in {dbfs:4d} dBFS drive {drive:2d} dB: {db(s[worst] / fund):6.1f} dB at {worst} Hz")


def response(curve):
    print(f"\nsmall-signal response, curve {curve} (-60 dBFS sines, drive 0), dB re 1 kHz")
    ref = None
    row = []
    for f in (20, 30, 50, 100, 1000, 5000, 10000, 15000, 18000, 20000):
        y = render(sine(f, -60, 1.5), curve=curve, drive=0, auto_gain=0)
        level = rms_db(y[-SR // 2:])
        row.append((f, level))
    ref = dict(row)[1000]
    print("  " + "  ".join(f"{f}Hz {lv - ref:+.2f}" for f, lv in row), f"   (1 kHz gain {ref + 60 + 3.01:+.2f} dB)")


def loudness(curve):
    print(f"\npink noise at -18 dBFS RMS, curve {curve}: output RMS vs drive")
    x = pink()
    for auto in (0, 1):
        row = []
        for drive in (-12, 0, 6, 12, 18, 24, 30, 36):
            y = render(x, curve=curve, drive=drive, auto_gain=auto, output=0)
            row.append(f"{drive}:{rms_db(y[SR:]):+.1f}")
        print(f"  auto gain {auto}: " + "  ".join(row))


def cpu(curves):
    # Process CPU time (user + sys) of the whole render, wav I/O included, so
    # the figure holds on a busy machine; best of three.
    print("\ncpu, % of one core for a stereo instance (20 s pink noise at -18 dBFS)")
    x = pink(20.0)
    for curve, drive in [(c, d) for c in curves for d in (0, 12, 36)] + [(4, 12)]:
        best = 1e9
        for _ in range(3):
            before = os.times()
            render(x, curve=curve, drive=drive)
            after = os.times()
            best = min(best, (after.children_user - before.children_user)
                       + (after.children_system - before.children_system))
        print(f"  curve {curve} drive {drive:2d}: {100 * best / 20.0:.2f}%")


if __name__ == "__main__":
    which = sys.argv[2:] or ["harmonics", "aliasing", "response", "loudness", "cpu"]
    curves = [int(c) for c in os.environ.get("SAT_CURVES", "12").split(",")]
    for curve in curves:
        if "harmonics" in which:
            harmonics(curve)
        if "aliasing" in which:
            aliasing(curve)
        if "response" in which:
            response(curve)
        if "loudness" in which:
            loudness(curve)
    if "aliasing" in which:
        aliasing(4)
    if "cpu" in which:
        cpu(curves)
