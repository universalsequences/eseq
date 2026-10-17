#!/usr/bin/env python3
"""gatecheck.py variant: the shell does not define u0 yet, so define it as
(noise) before the engines (as plh's driver does) and compile.

usage: python3 tools/gatecheck_pm.py name [name ...]
"""
import os, subprocess, sys
sys.path.insert(0, os.path.dirname(__file__))
import plh
names = sys.argv[1:]
d = os.path.join(plh.WORK, "tests", "gatecheckpm_" + "_".join(names))
subprocess.run([sys.executable, os.path.join(plh.ROOT, "tools", "assemble.py"), d, "--only", ",".join(names)], check=True)
p = os.path.join(d, "dsp.lisp")
src = open(p).read()
if "(def u0 " not in src:
    src = src.replace("(def pl_ref_tune 1)", "(def pl_ref_tune 1)\n(def u0 (noise))", 1)
    open(p, "w").write(src)
try:
    inst = plh.A.Instrument(d)
    print("OK: compiles in the assembled instrument")
except RuntimeError as e:
    print("FAIL:", str(e)[-600:])
    sys.exit(1)
