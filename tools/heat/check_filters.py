#!/usr/bin/env python3
"""Compare compiled Heat filters with isolated, level-preserved Live captures.

The source capture is fed through the compiled DSP. Compare the steady-state
complex harmonic response: no output normalization, phase alignment, or fitted
gain is allowed. This isolates filters from oscillator/envelope differences.
It is a component check, not an end-to-end Heat sonic-match verdict.
"""
import argparse
import ctypes
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile

import numpy as np

from analyze_reference import load

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools/audition"))
from audition import Instrument


def render_input(instrument, signal, params):
    """Drive the actual ABI with a supplied mono signal and fresh voice state."""
    memory = instrument.fresh_memory()
    for name, value in params.items():
        memory[instrument.params[name]["cellId"]] = value
    block = instrument.max_frames
    inputs = [np.zeros(block, np.float32) for _ in range(instrument.n_in)]
    outputs = [np.zeros(block, np.float32) for _ in range(instrument.n_out)]
    pointer = ctypes.POINTER(ctypes.c_float)
    in_ptrs = (pointer * len(inputs))(*[x.ctypes.data_as(pointer) for x in inputs])
    out_ptrs = (pointer * len(outputs))(*[x.ctypes.data_as(pointer) for x in outputs])
    result = np.empty(len(signal), np.float32)
    for offset in range(0, len(signal), block):
        frames = min(block, len(signal) - offset)
        inputs[instrument.inputs["signal"]][:frames] = signal[offset:offset + frames]
        instrument.process_fn(in_ptrs, out_ptrs, frames,
                              memory.ctypes.data_as(ctypes.c_void_p),
                              ctypes.byref(instrument.context), None)
        result[offset:offset + frames] = outputs[0][:frames]
    if not np.isfinite(result).all():
        raise ValueError("Compiled filter emitted non-finite samples")
    return result


def complex_harmonics(signal, frequencies):
    window = np.hanning(48000)
    x = signal[48000:96000].astype(float) * window
    time = np.arange(len(x)) / 48000
    return np.array([np.sum(x * np.exp(-2j * np.pi * f * time)) for f in frequencies])


def check(folder):
    macro = ROOT / "content/defmacros/heat-linear-filter/macro.lisp"
    source = macro.read_text() + """
(def signal (in 1 @name signal))
(param cutoff @default 833.782 @min 30 @max 22000)
(param q @default 0.1 @min 0.1 @max 100)
(param mode @default 0 @min 0 @max 7)
(out (heat-linear-filter signal cutoff q mode) 1)
"""
    frequencies = np.arange(1, 257) * 32.68445
    bypass_path = folder / "sources-v2 source-saw.wav"
    bypass = load(bypass_path)[:3 * 48000, 0]
    source_h = complex_harmonics(bypass, frequencies)
    report = {
        "scope": "Linear filters only; cutoff 833.782 Hz, two measured Q settings",
        "macro_sha256": hashlib.sha256(macro.read_bytes()).hexdigest(),
        "source_sha256": hashlib.sha256(bypass_path.read_bytes()).hexdigest(),
        "sample_rate": 48000,
        "cases": {},
    }
    with tempfile.TemporaryDirectory(prefix="heat-filter-") as directory:
        path = Path(directory) / "dsp.lisp"
        path.write_text(source)
        instrument = Instrument(path)
        report["compiler_sha256"] = instrument.compiler_sha256
        subprocess.run([sys.executable, str(ROOT / "tools/audition/check_fusion.py"),
                        str(Path(instrument.build_dir) / "patch.c")], check=True)
        for mode in range(8):
            for setting, q in [("0", 0.1), ("0.75", 20.674644)]:
                name = f"filter-{mode}-q{setting}"
                reference_path = folder / f"static {name}.wav"
                reference = load(reference_path)[:3 * 48000, 0]
                rendered = render_input(instrument, bypass, dict(cutoff=833.782, q=q, mode=mode))
                expected = complex_harmonics(reference, frequencies)
                actual = complex_harmonics(rendered, frequencies)
                # Exclude spectral nulls where the capture noise dominates.
                mask = (abs(source_h) > 0.12) & (abs(expected / source_h) > 0.001)
                ratio = actual[mask] / expected[mask]
                db_error = 20 * np.log10(abs(ratio))
                phase_error = np.angle(ratio)
                rms_db = float(np.sqrt(np.mean(db_error**2)))
                rms_phase = float(np.sqrt(np.mean(phase_error**2)))
                report["cases"][name] = {
                    "reference_sha256": hashlib.sha256(reference_path.read_bytes()).hexdigest(),
                    "harmonics_compared": int(mask.sum()),
                    "rms_error_db": rms_db,
                    "rms_phase_error_radians": rms_phase,
                    "passed": bool(mask.sum() >= 32 and rms_db < 0.05 and rms_phase < 0.01),
                }
                print(f"{name}: {rms_db:.6f} dB, {rms_phase:.6f} rad")
    report["passed"] = all(case["passed"] for case in report["cases"].values())
    return report


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("folder", type=Path)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    result = check(args.folder)
    args.out.write_text(json.dumps(result, indent=2) + "\n")
    sys.exit(0 if result["passed"] else 1)
