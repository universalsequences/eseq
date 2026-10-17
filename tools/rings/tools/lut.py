#!/usr/bin/env python3
"""Extract arrays from rings/resources.cc into data/<name>.json tensors.

usage: EURORACK=... python3 tools/lut.py lut_sine lut_stiffness ...
Writes {"shape": [N, 1], "data": [...]} (1 column; read with (peek t i 0)).
"""
import json, os, re, sys
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
# Data files live in the instrument folder (RINGS_DATA overrides).
DATA = os.environ.get("RINGS_DATA", os.path.join(os.path.dirname(os.path.dirname(ROOT)), "content", "instruments", "Synths", "Rings", "data"))
SRC = os.path.join(os.environ["EURORACK"], "rings", "resources.cc")


def array(name):
    src = open(SRC).read()
    m = re.search(r"\b" + re.escape(name) + r"\[\]\s*=\s*\{(.*?)\};", src, re.S)
    if not m:
        sys.exit(f"{name}: not found")
    body = re.sub(r"//.*", "", m.group(1))
    return [float(v) for v in re.findall(r"-?\d+\.?\d*(?:e[-+]?\d+)?", body)]


if __name__ == "__main__":
    os.makedirs(DATA, exist_ok=True)
    for name in sys.argv[1:]:
        data = array(name)
        path = os.path.join(DATA, name + ".json")
        with open(path, "w") as f:
            json.dump({"shape": [len(data), 1], "source": f"rings/resources.cc {name} (MIT, Emilie Gillet)", "data": data}, f, separators=(",", ":"))
        print(f"{path}: {len(data)} values, first {data[:3]}")
