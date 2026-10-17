#!/usr/bin/env python3
"""Write engines/chordbank.lisp from gen_chordbank.py tables + template."""
import os, subprocess, sys
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
out = subprocess.run([sys.executable, os.path.join(ROOT, "tools", "gen_chordbank.py")], capture_output=True, text=True, check=True).stdout
lines = dict(l.split(" ", 1) for l in out.strip().split("\n"))
tmpl = open(os.path.join(ROOT, "tools", "chordbank.lisp.in")).read()
src = tmpl.replace("@RATIOS@", lines["ratios"]).replace("@INV@", lines["inv"])
open(os.path.join(ROOT, "engines", "chordbank.lisp"), "w").write(src)
print("wrote engines/chordbank.lisp")
