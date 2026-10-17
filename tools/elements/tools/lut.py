#!/usr/bin/env python3
"""Extract arrays from elements/resources.cc into data/<name>.json tensors.

usage: python3 tools/lut.py lut_sine lut_4_decades ...
Writes {"shape": [N, 1], "data": [...]} (1 column; read with (peek t i 0)).
Integer tables (the int16 sample data) keep their raw integer values.
"""
import json, os, re, sys
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
# Data files live in the instrument folder (ELEMENTS_DATA overrides).
DATA = os.environ.get("ELEMENTS_DATA", os.path.join(os.path.dirname(os.path.dirname(ROOT)), "content", "instruments", "Synths", "Elements", "data"))
SRC = os.path.join(os.environ["EURORACK"], "elements", "resources.cc")


def array(name):
    src = open(SRC).read()
    m = re.search(r"\b" + re.escape(name) + r"\[\]\s*=\s*\{(.*?)\};", src, re.S)
    if not m:
        sys.exit(f"{name}: not found")
    body = re.sub(r"//.*", "", m.group(1))
    return [float(v) if any(c in v for c in ".eE") else int(v, 0)
            for v in re.findall(r"-?(?:0x[0-9a-fA-F]+|\d+\.?\d*(?:e[-+]?\d+)?)", body)]


if __name__ == "__main__":
    os.makedirs(DATA, exist_ok=True)
    for name in sys.argv[1:]:
        data = array(name)
        path = os.path.join(DATA, name + ".json")
        with open(path, "w") as f:
            json.dump({"shape": [len(data), 1], "source": f"elements/resources.cc {name} (MIT, Emilie Gillet)", "data": data}, f, separators=(",", ":"))
        print(f"{path}: {len(data)} values, first {data[:4]}")
