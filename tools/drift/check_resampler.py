#!/usr/bin/env python3
"""Check compiled research polyphase FIR state and block-boundary behavior."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile

import numpy as np
from scipy.signal import upfirdn

from generate_halfband import coefficients, generate

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools/heat"))
from check_filters import Instrument, render_input


def check(compiler, toolchain):
    macro = ROOT / "tools/drift/prototypes/halfband2x.lisp"
    source, ripple, stopband = generate()
    if macro.read_text() != source:
        raise ValueError("Regenerate halfband2x.lisp before testing")
    signal = np.random.default_rng(2315).normal(0, .2, 8193).astype(np.float32)
    h = coefficients()
    expected = upfirdn(h, upfirdn(2*h, signal, up=2), down=2)[:len(signal)]
    rows = []
    with tempfile.TemporaryDirectory(prefix="drift-resampler-") as directory:
        path = Path(directory) / "dsp.lisp"
        path.write_text(source+"""
(def signal (in 1 @name signal))
(def (even odd) (drift-research-up2 signal))
(out (drift-research-down2 even odd) 1)
""")
        for block in (1, 17, 128):
            instrument = Instrument(path, max_frames=block,
                                    compiler=str(compiler.resolve()),
                                    toolchain_root=str(toolchain.resolve()))
            subprocess.run([sys.executable, str(ROOT / "tools/audition/check_fusion.py"),
                            str(Path(instrument.build_dir) / "patch.c")], check=True)
            actual = render_input(instrument, signal, {})
            error = float(max(abs(actual-expected)))
            rows.append(dict(block_frames=block, maximum_absolute_error=error,
                             passed=error < 1e-6))
        return dict(scope="Resampling implementation only; no native-audio claim",
                    compiler_sha256=instrument.compiler_sha256,
                    macro_sha256=hashlib.sha256(macro.read_bytes()).hexdigest(),
                    base_frame_latency=63, passband_error_db=ripple,
                    stopband_maximum_db=stopband, cases=rows,
                    passed=all(row["passed"] for row in rows))


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
