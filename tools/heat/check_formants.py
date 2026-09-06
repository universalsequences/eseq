#!/usr/bin/env python3
"""Check the compiled formant resonators, independently predicting F12 from F6.

The F6 identification supplies center frequencies at each cutoff setting and
one set of signed weights. F12 is predicted with fixed Q/gain changes. This
validates the resonator architecture, not the unresolved vowel/control mapping.
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
from check_filters import Instrument, ROOT, complex_harmonics, render_input


def check(folder):
    macro = ROOT / "content/defmacros/heat-formant-bank/macro.lisp"
    measurements = ROOT / "tools/heat/measurements/formants-boundaries.json"
    identified = json.loads(measurements.read_text())
    cases = {case["name"]: case for case in identified["cases"]}
    weights = cases["type8-cut0.5"]["gain"]
    source = macro.read_text() + "\n(def signal (in 1 @name signal))\n"
    for name, default in zip(["f1", "f2", "f3"], [270, 2290, 3010]):
        source += f"(param {name} @default {default} @min 30 @max 22000)\n"
    source += "(param steep @default 0 @min 0 @max 1)\n"
    source += f"(out (heat-formant-bank signal f1 f2 f3 {' '.join(map(str, weights))} steep) 1)\n"
    bypass_path = folder / "formants-boundaries calibration-saw.wav"
    bypass = load(bypass_path)[:3 * 48000, 0]
    frequencies = np.arange(1, 257) * 32.68445
    source_h = complex_harmonics(bypass, frequencies)
    report = dict(scope=__doc__, sample_rate=48000, cases={},
                  macro_sha256=hashlib.sha256(macro.read_bytes()).hexdigest(),
                  identification_sha256=hashlib.sha256(measurements.read_bytes()).hexdigest(),
                  source_sha256=hashlib.sha256(bypass_path.read_bytes()).hexdigest())
    with tempfile.TemporaryDirectory(prefix="heat-formant-") as directory:
        path = Path(directory) / "dsp.lisp"
        path.write_text(source)
        instrument = Instrument(path)
        report["compiler_sha256"] = instrument.compiler_sha256
        subprocess.run([sys.executable, str(ROOT / "tools/audition/check_fusion.py"),
                        str(Path(instrument.build_dir) / "patch.c")], check=True)
        for cutoff in [0, 0.125, 0.25, 0.375, 0.5, 0.625, 0.75, 0.875, 1]:
            centers = cases[f"type8-cut{cutoff}"]["frequencies"]
            for mode in [8, 9]:
                name = f"type{mode}-cut{cutoff}"
                case = cases[name]
                reference_path = folder / case["capture_file"]
                reference = load(reference_path)[:3 * 48000, 0]
                params = dict(zip(["f1", "f2", "f3"], centers), steep=mode - 8)
                actual = complex_harmonics(render_input(instrument, bypass, params), frequencies)
                expected = complex_harmonics(reference, frequencies)
                mask = (abs(source_h) > 0.12) & (abs(expected / source_h) > 0.001)
                ratio = actual[mask] / expected[mask]
                rms_db = float(np.sqrt(np.mean((20 * np.log10(abs(ratio)))**2)))
                rms_phase = float(np.sqrt(np.mean(np.angle(ratio)**2)))
                passed = bool(mask.sum() >= 32 and rms_db < 0.05 and rms_phase < 0.01)
                report["cases"][name] = dict(
                    reference_sha256=hashlib.sha256(reference_path.read_bytes()).hexdigest(),
                    harmonics_compared=int(mask.sum()), rms_error_db=rms_db,
                    rms_phase_error_radians=rms_phase, passed=passed)
                print(f"{name}: {rms_db:.6f} dB, {rms_phase:.6f} rad", flush=True)
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
