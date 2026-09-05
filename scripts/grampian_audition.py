#!/usr/bin/env python3
# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy>=2", "scipy>=1.14", "matplotlib>=3.9"]
# ///
"""Reproducible listening pack; never launches playback or an audio device.

uv run scripts/grampian_audition.py --out tuning_out/grampian-audition
Optional --input replaces the synthetic rim/snare/skank/bass/throw fixture.
WAVs are level-matched listening copies; gains and raw levels are recorded.
Reference convolution is ONLY an offline linear benchmark, not runtime DSP.
"""
import argparse
import json
from pathlib import Path
import subprocess
import time

import numpy as np
from scipy.io import wavfile
from scipy.signal import butter, fftconvolve, sosfilt
import grampian_tune as tuning

SR = tuning.SR


def fixture():
    rng = np.random.default_rng(201)
    x = np.zeros(SR*8)
    def add(start, sound, gain):
        offset = int(start*SR)
        x[offset:offset+len(sound)] += gain*sound
    t = np.arange(int(.5*SR))/SR
    rim = np.exp(-t/.008)*(np.sin(2*np.pi*1350*t)+.6*np.sin(2*np.pi*2370*t))
    noise = sosfilt(butter(2, [700, 10000], btype="bandpass", fs=SR, output="sos"), rng.normal(size=len(t)))
    snare = .5*noise*np.exp(-t/.07) + .4*np.sin(2*np.pi*180*t)*np.exp(-t/.035)
    skank = sum(sum(np.sin(2*np.pi*f*k*t)/k for k in range(1, 10)) for f in [220, 261.626, 329.628])
    skank *= np.minimum(t/.001, 1)*np.exp(-t/.045)*.15
    bass = (np.sin(2*np.pi*55*t)+.25*np.sin(2*np.pi*110*t))*np.minimum(t/.004, 1)*np.exp(-t/.15)
    add(.1, rim, .7); add(1.4, snare, .8); add(2.8, skank, 1); add(4.2, bass, .7)
    for start, sound in [(5.5, rim), (5.85, skank), (6.2, snare), (6.55, skank)]: add(start, sound, .8)
    return x*.7/max(abs(x).max(), 1e-20)


def stats(x):
    return dict(peak=float(abs(x).max()), rms=float(np.sqrt(np.mean(x*x))))


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--out", type=Path, default=tuning.ROOT/"tuning_out/grampian-audition")
    ap.add_argument("--bin", type=Path, default=tuning.ROOT/"target/release/spring_tune")
    ap.add_argument("--input", type=Path)
    args = ap.parse_args(); args.out.mkdir(parents=True, exist_ok=True); tuning.BIN = args.bin
    dry = tuning.decode(args.input) if args.input else fixture()
    wavfile.write(args.out/"dry.wav", SR, dry.astype(np.float32))
    ref, entry = tuning.reference()
    irs = {"reference-linear": ref, "v1-linear": tuning.rust_render("king-tubby-v1"),
           "grampian-linear": tuning.rust_render("grampian")}
    renders = {name: fftconvolve(dry, ir)[:len(dry)+SR*4] for name, ir in irs.items()}
    host_cases = {"space-echo-mono": {}, "space-echo-wide": {"width": 1},
                  "space-echo-driven": {"input_db": 12},
                  "space-echo-dub-throws": {"mode": 7, "intensity": .65, "echo": .8}}
    timings = {}
    for name, settings in host_cases.items():
        path = args.out/f"{name}.json"; path.write_text(json.dumps(settings, indent=2)+"\n")
        start = time.monotonic()
        raw = subprocess.check_output([str(args.bin), "--voice", "grampian", "--space-echo",
            "--input", str(args.out/"dry.wav"), "--amp", "1", "--seconds", "4", "--sr", str(SR),
            "--host-settings", str(path)])
        timings[name] = time.monotonic()-start
        renders[name] = np.frombuffer(raw, dtype="<f4").reshape(-1, 2).astype(float)
    # Use one RMS target, lowered if necessary to keep EVERY preview below
    # -1 dBFS without clipping. Mono and wide share a gain so width comparisons
    # do not secretly change the centered spring level.
    gains = {name: 1/max(stats(x)["rms"], 1e-20) for name, x in renders.items()}
    gains["space-echo-wide"] = gains["space-echo-mono"]
    common = min(.07, .891/max(abs(x).max()*gains[name] for name, x in renders.items()))
    report = dict(reference=entry, source=str(args.input) if args.input else "deterministic synthetic probes, not instrument emulations",
                  common_rms_target=common, files={})
    for name, x in renders.items():
        if not np.isfinite(x).all(): raise ValueError(f"non-finite render: {name}")
        gain = gains[name]*common
        wavfile.write(args.out/f"{name}.wav", SR, (x*gain).astype(np.float32))
        report["files"][name] = dict(raw=stats(x), preview_gain=gain, preview=stats(x*gain),
                                     render_wall_s=timings.get(name))
    (args.out/"levels.json").write_text(json.dumps(report, indent=2)+"\n")
    print(f"Listening pack: {args.out}\nNo playback started. See levels.json for matching gains.")


if __name__ == "__main__":
    main()
