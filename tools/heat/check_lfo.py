#!/usr/bin/env python3
"""Check audio-rate Heat LFO note timing and report reference waveform errors.

Control-rate emulation is explicitly outside scope. Reference comparisons
report whole-waveform errors, including Analog's interpolation transitions;
they do not claim that an unfinished waveform calibration has passed.
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
    macro = ROOT / "content/defmacros/heat-lfo/macro.lisp"
    source = macro.read_text() + """
(def note_on (in 1 @name signal))
(param rate @default 0.930166 @min 0 @max 100)
(param width @default 0.5 @min 0 @max 1)
(param shape @default 0 @min 0 @max 4)
(param retrigger @default 1 @min 0 @max 1)
(param phase_offset @default 0 @min -1 @max 1)
(param delay_ms @default 0 @min 0 @max 10000)
(param fade_ms @default 0 @min 0 @max 10000)
(out (heat-lfo rate width shape note_on retrigger phase_offset delay_ms fade_ms) 1)
"""
    report = {
        "scope": "Audio-rate LFO timing checks and provisional reference waveform errors; control-grid parity excluded",
        "macro_sha256": hashlib.sha256(macro.read_bytes()).hexdigest(),
        "timing_cases": {}, "reference_cases": {},
    }
    with tempfile.TemporaryDirectory(prefix="heat-lfo-") as directory:
        path = Path(directory) / "dsp.lisp"
        path.write_text(source)
        for rate in (44100, 48000, 96000):
            instrument = Instrument(path, sample_rate=rate)
            report["compiler_sha256"] = instrument.compiler_sha256
            subprocess.run([sys.executable, str(ROOT / "tools/audition/check_fusion.py"),
                            str(Path(instrument.build_dir) / "patch.c")], check=True)
            frames = np.arange(rate)
            second_note = round(0.7 * rate)
            pulse = np.zeros(rate, np.float32)
            pulse[[0, second_note]] = 1
            elapsed = (frames - np.where(frames >= second_note, second_note, 0)) / rate
            gain = np.clip((elapsed - 0.1) / 0.1, 0, 1)
            for retrigger in (0, 1):
                phase_time = elapsed if retrigger else frames / rate
                expected = np.sin(2 * np.pi * 1.3 * phase_time) * gain
                actual = render_input(instrument, pulse, {
                    "rate": 1.3, "retrigger": retrigger, "delay_ms": 100, "fade_ms": 100,
                })
                error = float(np.max(abs(actual - expected)))
                name = f"{rate}-retrigger{retrigger}"
                report["timing_cases"][name] = {"max_error": error, "passed": error < 0.00001}
                if error >= 0.00001:
                    raise ValueError(f"LFO phase or per-note delay/fade failed: {name}: {error}")
                print(f"{name}: maximum error {error:.7f}")
            for offset in (-0.75, 0.25):
                actual = render_input(instrument, pulse, {
                    "rate": 0, "phase_offset": offset, "delay_ms": 100, "fade_ms": 100,
                })
                error = float(np.max(abs(actual - gain)))
                name = f"{rate}-stopped-offset{offset}"
                report["timing_cases"][name] = {"max_error": error, "passed": error < 0.00001}
                if error >= 0.00001:
                    raise ValueError(f"Stopped LFO phase or delay/fade failed: {name}: {error}")
                print(f"{name}: maximum error {error:.7f}")
            if rate == 48000:
                begin, end = 24000, 480000
                reference_path = folder / "lfo-amplitude unmodulated.wav"
                carrier = load(reference_path)[begin:end, 0].astype(float)
                valid = abs(carrier) > 0.015
                pulse = np.zeros(end - begin, np.float32)
                pulse[0] = 1
                for shape in range(3):
                    for width in ([0.5] if shape == 0 else [0, 0.25, 0.5, 0.75, 1]):
                        name = f"lfo-shape{shape}-width{width}"
                        case_path = folder / f"lfo-amplitude {name}.wav"
                        audio = load(case_path)[begin:end, 0].astype(float)
                        expected = audio[valid] / carrier[valid] - 1
                        actual = render_input(instrument, pulse, {"shape": shape, "width": width})[valid]
                        error = actual - expected
                        rms = float(np.sqrt(np.mean(error**2)))
                        report["reference_cases"][name] = {
                            "reference_sha256": hashlib.sha256(case_path.read_bytes()).hexdigest(),
                            "rms_error": rms, "max_error": float(np.max(abs(error))),
                            "mean_error": float(np.mean(error)),
                        }
                        print(f"{name}: reference RMS error {rms:.6f}")
    return report


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("folder", type=Path)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    args.out.write_text(json.dumps(check(args.folder), indent=2) + "\n")
