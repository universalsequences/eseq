#!/usr/bin/env python3
"""Assemble the Rings instrument dsp.lisp from src/ and lib/.

usage: python3 tools/assemble.py OUT_DIR [--data-subdir data] [--const-u U]
--noise-out adds the noise streams as outputs 4 and 5 (tests).
--const-u replaces the noise source with a constant (tests against
ref/rref run with RREF_U=U). Copies every data file referenced into OUT_DIR
(or OUT_DIR/<subdir>).
"""
import os, re, shutil, sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DATA = os.environ.get("RINGS_DATA", os.path.join(os.path.dirname(os.path.dirname(ROOT)), "content", "instruments", "Synths", "Rings", "data"))
ORDER = [("src", "header"), ("lib", "common"), ("src", "tables"), ("src", "modal"), ("src", "fm"),
         ("src", "strings"), ("src", "peace"), ("src", "fx"), ("src", "voice")]


def main():
    out_dir = sys.argv[1]
    sub = ""
    if "--data-subdir" in sys.argv:
        sub = sys.argv[sys.argv.index("--data-subdir") + 1].strip("/") + "/"
    noise_out = "--noise-out" in sys.argv
    const_u = None
    if "--const-u" in sys.argv:
        const_u = sys.argv[sys.argv.index("--const-u") + 1]
    os.makedirs(out_dir, exist_ok=True)
    parts = [open(os.path.join(ROOT, d, n + ".lisp")).read() for d, n in ORDER]
    src = "\n".join(parts)
    if const_u is not None:
        src = src.replace("(def u0 (noise))", f"(def u0 {const_u})")
        src, n = re.subn(r"\(def u1 .*\)\n", f"(def u1 {const_u})\n", src)
        assert n == 1, "u1 definition"
        assert f"(def u0 {const_u})" in src, "u0 definition"
    if noise_out:
        # test builds: expose the two noise streams for ref/rref RREF_NOISE
        src += "\n(out u0 4 @name noise0)\n(out u1 5 @name noise1)\n"
    data = sorted(set(re.findall(r'@file\s+"([^"]+)"', src)))
    # One tensor per data file: later definitions alias the first.
    seen = {}
    def alias(mt):
        key = (mt.group(2), mt.group(3))
        if key in seen:
            return f"(def {mt.group(1)} {seen[key]})"
        seen[key] = mt.group(1)
        return mt.group(0)
    src = re.sub(r'\(def (\S+) \(tensor @shape (\[[^\]]*\]) @file "([^"]+)"\)\)', alias, src)
    if sub:
        src = re.sub(r'@file\s+"([^"]+)"', lambda mt: f'@file "{sub}{mt.group(1)}"', src)
        os.makedirs(os.path.join(out_dir, sub), exist_ok=True)
    with open(os.path.join(out_dir, "dsp.lisp"), "w") as f:
        f.write(src)
    for d in data:
        s, t = os.path.join(DATA, d), os.path.join(out_dir, sub, d)
        if os.path.abspath(s) != os.path.abspath(t):
            shutil.copyfile(s, t)
    print(f"assembled {len(src)} bytes, {len(data)} data files -> {out_dir}")


if __name__ == "__main__":
    main()
