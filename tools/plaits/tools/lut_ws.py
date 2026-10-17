#!/usr/bin/env python3
"""Pack lut_ws_inverse_tan/inverse_sin/linear/bump/double_bump (int16, 257
each) into data/lut_ws.json: shape [257, 5], column-major by table
(index = col * 257 + row), values / 32768 as float. Read (peek t row col)."""
import json, os, sys
sys.path.insert(0, os.path.dirname(__file__))
from lut import array, ROOT, DATA
names = ["lut_ws_inverse_tan", "lut_ws_inverse_sin", "lut_ws_linear", "lut_ws_bump", "lut_ws_double_bump"]
data = []
for n in names:
    a = array(n)
    assert len(a) == 257, (n, len(a))
    data += [v / 32768.0 for v in a]
with open(os.path.join(DATA, "lut_ws.json"), "w") as f:
    json.dump({"shape": [257, 5], "source": "plaits/resources.cc lut_ws_* / 32768 (MIT, Emilie Gillet)", "data": data}, f, separators=(",", ":"))
print("data/lut_ws.json")
