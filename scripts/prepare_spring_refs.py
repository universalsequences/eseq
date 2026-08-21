#!/usr/bin/env python3
"""Convert factory spring IR captures into tuner-ready references
(16-bit / 44.1 kHz / mono wav, peak-trimmed, peak-normalized) under
content/impulses/prepared/, for scripts/spring_tune.py --ref.

Decoding goes through ffmpeg so wav/aif at any bit depth work.
"""

import os
import subprocess
import wave

import numpy as np

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
IMP = os.path.join(ROOT, "content", "impulses")
OUT = os.path.join(IMP, "prepared")
SR = 44100
PRE_S = 0.010  # keep a little pre-ring before the peak
TAIL_S = 6.0

# name -> (source, remove_direct). The park/yamaha captures are mic'd amps,
# so their IRs contain the dry speaker click on top of the spring tail; the
# model is wet-only (the effect has its own dry path), so the click is excised
# before fitting or it dominates the energy-decay objective by ~24 dB.
SOURCES = {
    "king-tubby-filter-500.wav": ("king-tubby/Grampian Filter 500.wav", False),
    "king-tubby.wav": ("king-tubby/Grampian Filter Sweep Spring Reverb 002.wav", False),
    "park-g10r.wav": ("park-g10R/flo x park classic sound.aif", True),
    "yamaha-g5.wav": ("yamaha-g5-spring/reverb full/flo x g5 treble full reverb full.aif", True),
}


def decode(path):
    out = subprocess.run(
        ["ffmpeg", "-v", "error", "-i", path, "-f", "f32le", "-ac", "1",
         "-ar", str(SR), "-"],
        capture_output=True, check=True,
    ).stdout
    return np.frombuffer(out, dtype="<f4").astype(np.float64)


def excise_direct(x, peak):
    """Zero out the dry click (through peak + 1.2 ms, 1 ms cosine fade back in)."""
    x = x.copy()
    cut = peak + int(0.0012 * SR)
    fade = int(0.001 * SR)
    x[:cut] = 0.0
    ramp = 0.5 - 0.5 * np.cos(np.pi * np.arange(fade) / fade)
    x[cut:cut + fade] *= ramp[: max(0, min(fade, len(x) - cut))]
    return x


def prepare(x, remove_direct=False):
    peak = int(np.abs(x).argmax())
    if remove_direct:
        x = excise_direct(x, peak)
    start = max(0, peak - int(PRE_S * SR))
    end = min(len(x), peak + int(TAIL_S * SR))
    x = x[start:end]
    return x * (0.891 / max(np.abs(x).max(), 1e-12))  # -1 dBFS peak


def write_wav(path, x):
    q = np.clip(np.round(x * 32767.0), -32768, 32767).astype("<i2")
    w = wave.open(path, "wb")
    w.setnchannels(1)
    w.setsampwidth(2)
    w.setframerate(SR)
    w.writeframes(q.tobytes())
    w.close()


def main():
    os.makedirs(OUT, exist_ok=True)
    for name, (rel, remove_direct) in SOURCES.items():
        x = prepare(decode(os.path.join(IMP, rel)), remove_direct)
        out_path = os.path.join(OUT, name)
        write_wav(out_path, x)
        print(f"{name}: {len(x)/SR:.2f}s  from {rel}"
              + ("  (direct click excised)" if remove_direct else ""))


if __name__ == "__main__":
    main()
