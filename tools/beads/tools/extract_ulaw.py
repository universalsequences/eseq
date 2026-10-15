#!/usr/bin/env python3
"""Copies beads/dsp/mu_law.cc lut_ulaw_f (the 8-bit "dirty tape" decoder) to
content/effects/Beads/data/ulaw.json.   EURORACK=... extract_ulaw.py"""
import json, os, re, sys

src = open(os.path.join(os.environ["EURORACK"], "beads", "dsp", "mu_law.cc")).read()
body = src[src.index("float lut_ulaw_f[256]"):]
body = body[body.index("{") + 1:body.index("}")]
vals = [float(v.rstrip("f")) for v in re.findall(r"-?\d+\.\d+f?", body)]
assert len(vals) == 256, len(vals)
out = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "..",
                   "content", "effects", "Beads", "data", "ulaw.json")
with open(os.path.normpath(out), "w") as f:
    json.dump({"shape": [256, 1], "source": "beads/dsp/mu_law.cc lut_ulaw_f (MIT, Emilie Gillet)", "data": vals}, f)
    f.write("\n")
