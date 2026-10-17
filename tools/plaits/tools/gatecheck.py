#!/usr/bin/env python3
"""Check that engines compile inside the assembled instrument (block-gate rules).

usage: python3 tools/gatecheck.py name [name ...]
"""
import os, subprocess, sys
sys.path.insert(0, os.path.dirname(__file__))
import plh
names = sys.argv[1:]
d = os.path.join(plh.WORK, "tests", "gatecheck_" + "_".join(names))
subprocess.run([sys.executable, os.path.join(plh.ROOT, "tools", "assemble.py"), d, "--only", ",".join(names)], check=True)
try:
    plh.A.Instrument(d)
    print("OK: compiles in the assembled instrument")
except RuntimeError as e:
    print("FAIL:", str(e)[-600:])
    sys.exit(1)
