#!/usr/bin/env python3
"""Check the compiled research feedback solver against independent scalar roots.

This checks numerical implementation, not agreement with native Drift audio.
The normal compiler binary audit and generated-code fusion check remain enabled.
"""
import argparse
import json
from pathlib import Path
import subprocess
import sys
import tempfile

import numpy as np
from scipy.optimize import brentq

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools/heat"))
from check_filters import Instrument, render_input


def check(compiler, toolchain):
    rng = np.random.default_rng(9108)
    errors = []
    macro = ROOT / "tools/drift/prototypes/feedback-root.lisp"
    with tempfile.TemporaryDirectory(prefix="drift-root-") as directory:
        path = Path(directory) / "dsp.lisp"
        path.write_text(macro.read_text() + """
(def signal (in 1 @name signal))
(param a @default 1 @min .1 @max 100)
(param r @default 0 @min 0 @max 100)
(param threshold @default .2 @min .0001 @max 10)
(param curve @default 1 @min .0001 @max 10000)
(out (drift-feedback-root a signal r threshold curve) 1)
""")
        instrument = Instrument(path, compiler=str(compiler.resolve()),
                                toolchain_root=str(toolchain.resolve()))
        subprocess.run([sys.executable, str(ROOT / "tools/audition/check_fusion.py"),
                        str(Path(instrument.build_dir) / "patch.c")], check=True)
        for _ in range(30):
            a = float(np.float32(10**rng.uniform(-1, 2)))
            r = float(np.float32(a*rng.uniform(0, .999)))
            threshold = float(np.float32(10**rng.uniform(-4, 1)))
            curve = float(np.float32(10**rng.uniform(-4, 4)))
            signal = rng.uniform(-10, 10, 128).astype(np.float32)
            actual = render_input(instrument, signal, dict(
                a=a, r=r, threshold=threshold, curve=curve))
            expected = []
            for b in signal:
                def equation(u):
                    excess = max(abs(u)-threshold, 0)
                    saturated = np.sign(u)*(min(abs(u), threshold)
                                             + excess/(1+curve*excess))
                    return a*u-float(b)-r*saturated

                bound = abs(float(b))/(a-r)+threshold
                expected.append(brentq(equation, -bound, bound, xtol=1e-13))
            expected = np.array(expected)
            errors.append(float(np.max(abs(actual-expected)
                                        / np.maximum(abs(expected), 1e-10))))
        return dict(compiler_sha256=instrument.compiler_sha256,
                    cases=30, roots=3840, maximum_relative_error=max(errors),
                    fusion_clean=True, passed=max(errors) < 2e-5,
                    scope="Numerical solver only; no native-audio acceptance claim")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--compiler", type=Path, required=True)
    parser.add_argument("--toolchain", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    result = check(args.compiler, args.toolchain)
    args.out.write_text(json.dumps(result, indent=2)+"\n")
    print(json.dumps(result, indent=2))
    sys.exit(0 if result["passed"] else 1)
