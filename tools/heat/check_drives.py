#!/usr/bin/env python3
"""Compare compiled drive curves with isolated Analog recordings.

At high resonance, Analog's Drive Off output is itself limited. Reconstruct
the linear filter output from a quieter recording and an independently
measured unfiltered source-level ratio. No per-drive gain or alignment fit is
allowed. A second, lower-resonance corpus stays below the output limiter knee.
The amplifier scale is identified here; this does not validate its knob law.
"""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile

import numpy as np

from analyze_reference import load
from check_filters import Instrument, ROOT, render_input


def check(folder):
    clip_macro = ROOT / "content/defmacros/heat-soft-clip/macro.lisp"
    drive_macro = ROOT / "content/defmacros/heat-drive/macro.lisp"
    source = clip_macro.read_text() + drive_macro.read_text().replace(
        "(use-defmacro heat-soft-clip)", "") + """
(def signal (in 1 @name signal))
(param mode @default 0 @min 0 @max 6)
; Independently identified amplifier/pan scale, in reference output units.
(def drive_scale 0.0904724)
(def driven (* drive_scale (heat-drive (/ signal drive_scale) mode)))
(out (heat-soft-clip driven 2.8 4) 1)
"""
    window = slice(48000, 96000)
    files = {}

    def capture(name):
        path = folder / (name + ".wav")
        files[name] = hashlib.sha256(path.read_bytes()).hexdigest()
        return load(path)[window, 0].astype(np.float64)

    quiet = capture("drive-independent linear-level0.5-filter0")
    loud = capture("drive-independent linear-level1-filter0")
    source_ratio = float(np.dot(quiet, loud) / np.dot(quiet, quiet))
    source_error = float(np.max(abs(loud - quiet * source_ratio)))
    if source_error > 1e-6:
        raise ValueError("Unfiltered level controls are not a pure gain change")
    linear = capture("drive-independent linear-level0.5-filter1") * source_ratio
    low = capture("drive-steady drive0-q0.5")
    if np.max(abs(low)) >= 2.8:
        raise ValueError("Low-resonance control exceeds the output limiter knee")
    report = {
        "scope": "Six drive modes and output limiting, two resonance levels; amplifier control mapping excluded",
        "sample_rate": 48000,
        "comparison_frames": [48000, 96000],
        "macro_sha256": {p.parent.name: hashlib.sha256(p.read_bytes()).hexdigest()
                         for p in (clip_macro, drive_macro)},
        "source_level_ratio": source_ratio,
        "source_gain_max_residual": source_error,
        "drive_output_scale": 0.0904724,
        "cases": {},
        "reference_sha256": files,
    }
    with tempfile.TemporaryDirectory(prefix="heat-drive-") as directory:
        path = Path(directory) / "dsp.lisp"
        path.write_text(source)
        instrument = Instrument(path)
        report["compiler_sha256"] = instrument.compiler_sha256
        subprocess.run([sys.executable, str(ROOT / "tools/audition/check_fusion.py"),
                        str(Path(instrument.build_dir) / "patch.c")], check=True)
        for corpus, input_signal, q in [
                ("drive-independent", linear, "1"), ("drive-steady", low, "0.5")]:
            for mode in range(7):
                name = f"{corpus} drive{mode}-q{q}"
                expected = capture(name)
                actual = render_input(instrument, input_signal, {"mode": mode})
                error = actual - expected
                rms = float(np.sqrt(np.mean(error**2)))
                maximum = float(np.max(abs(error)))
                report["cases"][name] = {
                    "rms_full_scale_error": rms,
                    "max_full_scale_error": maximum,
                    "passed": rms < 0.0002 and maximum < 0.002,
                }
                print(f"{name}: RMS {rms:.8f}, max {maximum:.8f}")
        # Exercise both polarities far beyond the capture's amplitude range.
        # The composed output must remain finite, monotonic and bounded.
        sweep = np.concatenate((-np.geomspace(1e6, 1e-6, 4096), [0], np.geomspace(1e-6, 1e6, 4096)))
        for mode in range(7):
            output = render_input(instrument, sweep, {"mode": mode})
            if (np.max(abs(output)) > 4.000001 or np.min(np.diff(output)) < -1e-6
                    or abs(output[4096]) > 1e-7):
                raise ValueError(f"Drive {mode} failed bounded monotonic transfer check")
        report["extreme_input_checks_passed"] = True
    return report


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("folder", type=Path)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    report = check(args.folder)
    args.out.write_text(json.dumps(report, indent=2) + "\n")
    if not all(case["passed"] for case in report["cases"].values()):
        raise SystemExit("Drive comparison failed")
