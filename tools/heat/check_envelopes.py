#!/usr/bin/env python3
"""Compare compiled contour state transitions with single-note Analog captures.

Amplitude is referenced to a separate sustained sine capture, never normalized
per case. A short smoothing window removes the carrier's measured harmonic
ripple. These cases cover slope/loop/Free interactions at one time setting;
they do not certify velocity or retrigger behavior. The time-range corpus also
checks the fixed control-to-time mapping. Its paired raw-sample comparison
cancels the common oscillator/startup behavior between the two envelope slopes.
"""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile

import numpy as np
from scipy.signal import hilbert, savgol_filter

from analyze_reference import load

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools/audition"))
from audition import Instrument


def check(folder):
    macro = ROOT / "content/defmacros/heat-envelope/macro.lisp"
    source = macro.read_text() + """
(def gate (in 1 @name gate))
(def restart (in 2 @name trigger))
(param exponential @default 0 @min 0 @max 1)
(param loop_mode @default 0 @min 0 @max 3)
(param free_run @default 0 @min 0 @max 1)
(param sustain_seconds @default -1 @min -1 @max 1000)
(param duration_ms @default 282.631189 @min 5 @max 15000)
(param sustain @default 0.403149606 @min 0 @max 1)
(out (heat-envelope gate restart duration_ms duration_ms sustain sustain_seconds duration_ms exponential loop_mode free_run) 1)
"""
    baseline = load(folder / "sources-v2 source-sine.wav")[13 * 48000:14 * 48000, 0]
    amplitude = float(np.mean(abs(hilbert(baseline))))
    report = {
        "scope": "Envelope slope/loop/Free plus six A/D/R settings, with fixed control-to-time mapping",
        "macro_sha256": hashlib.sha256(macro.read_bytes()).hexdigest(),
        "sample_rate": 48000,
        "sine_reference_amplitude": amplitude,
        "cases": {},
    }
    with tempfile.TemporaryDirectory(prefix="heat-envelope-") as directory:
        path = Path(directory) / "dsp.lisp"
        path.write_text(source)
        instrument = Instrument(path)
        report["compiler_sha256"] = instrument.compiler_sha256
        subprocess.run([sys.executable, str(ROOT / "tools/audition/check_fusion.py"),
                        str(Path(instrument.build_dir) / "patch.c")], check=True)
        for exponential in range(2):
            for loop in range(4):
                for free in range(2):
                    name = f"env-exp{exponential}-loop{loop}-free{free}"
                    reference_path = folder / f"dynamic-v2 {name}.wav"
                    reference = load(reference_path)[:, 0]
                    envelope = savgol_filter(abs(hilbert(reference)), 481, 2) / amplitude
                    rendered, _ = instrument.render(10, gate_off=2, params={
                        "exponential": exponential, "loop_mode": loop, "free_run": free,
                    })
                    expected = envelope[24000:504000]
                    error = savgol_filter(rendered, 481, 2) - expected
                    rms = float(np.sqrt(np.mean(error**2)))
                    maximum = float(np.max(abs(error)))
                    report["cases"][name] = {
                        "reference_sha256": hashlib.sha256(reference_path.read_bytes()).hexdigest(),
                        "rms_full_scale_error": rms,
                        "max_full_scale_error": maximum,
                        "passed": rms < 0.015 and maximum < 0.04,
                    }
                    print(f"{name}: RMS {rms:.6f}, max {maximum:.6f}")
        # This second corpus varies duration while retaining a ten-second gate.
        # Values are predicted from one control law, not fitted per waveform.
        time_baseline = load(folder / "envelope-times calibration-sine.wav")[13 * 48000:14 * 48000, 0]
        time_amplitude = float(np.mean(abs(hilbert(time_baseline))))
        for setting in [0, 0.125, 0.25, 0.5, 0.625, 0.75]:
            centered = (128 * setting if setting <= 0.5 else 126 * setting + 1) / 127
            duration_ms = 5 * 3000**centered
            pair = []
            for exponential in range(2):
                name = f"env-time{setting}-exp{exponential}"
                reference_path = folder / f"envelope-times {name}.wav"
                reference = load(reference_path)[:, 0]
                envelope = savgol_filter(abs(hilbert(reference)), 97, 2) / time_amplitude
                rendered, _ = instrument.render(19, gate_off=10, params={
                    "exponential": exponential, "loop_mode": 0, "free_run": 0,
                    "duration_ms": duration_ms,
                })
                expected = envelope[24000:936000]
                pair.append((rendered, reference[24000:936000]))
                # Both slopes share a short source onset that is not part of
                # the contour. Exclude it from the demodulated comparison;
                # the paired raw comparison below includes those samples.
                # Apply identical measurement smoothing to both contours.
                mask = np.arange(len(rendered)) >= 128
                error = savgol_filter(rendered, 97, 2)[mask] - expected[mask]
                rms = float(np.sqrt(np.mean(error**2)))
                maximum = float(np.max(abs(error)))
                report["cases"][name] = {
                    "reference_sha256": hashlib.sha256(reference_path.read_bytes()).hexdigest(),
                    "predicted_duration_ms": duration_ms,
                    "rms_full_scale_error": rms,
                    "max_full_scale_error": maximum,
                    "demodulation_start_frame": 128,
                    "passed": rms < 0.015 and maximum < 0.04,
                }
                print(f"{name}: {duration_ms:.6f} ms, RMS {rms:.6f}, max {maximum:.6f}")
            linear, linear_audio = pair[0]
            exponential, exponential_audio = pair[1]
            # Multiplication avoids dividing by carrier zero crossings. There
            # is no fitted gain, phase, or time shift. A shared oscillator or
            # startup mismatch cancels; a slope-specific contour error does not.
            error = (exponential * linear_audio - linear * exponential_audio) / time_amplitude
            rms = float(np.sqrt(np.mean(error**2)))
            maximum = float(np.max(abs(error)))
            report["cases"][f"env-time{setting}-paired-slopes"] = {
                "rms_full_scale_error": rms,
                "max_full_scale_error": maximum,
                "passed": rms < 0.002 and maximum < 0.01,
            }
            print(f"env-time{setting}-paired-slopes: RMS {rms:.6f}, max {maximum:.6f}")
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
