#!/usr/bin/env python3
"""Assemble an Elements dsp.lisp from the shell and the src modules.

usage: python3 tools/assemble.py OUT_DIR [--test] [--data-subdir data]
--test builds the reference-comparison driver (shell/test_header.lisp +
shell/test_footer.lisp) instead of the instrument shell, and copies the data
files next to it.
"""
import os, re, shutil, sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DATA = os.environ.get("ELEMENTS_DATA", os.path.join(os.path.dirname(os.path.dirname(ROOT)), "content", "instruments", "Synths", "Elements", "data"))
MODULES = ["lib", "envelope", "exciter", "resonator", "string", "reverb", "ominous", "voice"]


def assemble(test, data_subdir):
    parts = []
    shell = os.path.join(ROOT, "shell")
    if not test:
        parts.append(open(os.path.join(shell, "banner.lisp")).read() + "\n")
    parts.append(open(os.path.join(ROOT, "src", "lib.lisp")).read() + "\n")
    parts.append(open(os.path.join(shell, "test_header.lisp" if test else "header.lisp")).read())
    for m in MODULES[1:]:
        parts.append("\n" + open(os.path.join(ROOT, "src", m + ".lisp")).read())
    parts.append("\n" + open(os.path.join(shell, "test_footer.lisp" if test else "footer.lisp")).read())
    src = "".join(parts)
    if data_subdir:
        src = re.sub(r'@file "([^"/]+)"', lambda m: f'@file "{data_subdir}/{m.group(1)}"', src)
    return src


def main():
    out = sys.argv[1]
    test = "--test" in sys.argv
    sub = None
    if "--data-subdir" in sys.argv:
        sub = sys.argv[sys.argv.index("--data-subdir") + 1]
    src = assemble(test, sub)
    os.makedirs(out, exist_ok=True)
    with open(os.path.join(out, "dsp.lisp"), "w") as f:
        f.write(src)
    if sub is None:
        for name in sorted(set(re.findall(r'@file "([^"]+)"', src))):
            s, d = os.path.join(DATA, name), os.path.join(out, name)
            if not os.path.exists(d) or open(s, "rb").read() != open(d, "rb").read():
                shutil.copyfile(s, d)


if __name__ == "__main__":
    main()
