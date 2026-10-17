#!/usr/bin/env python3
"""Extract plaits wav_integrated_waves (192 waves x 132 int16 samples) to
data/wav_integrated_waves.json, shape [132, 192]: (peek t sample wave).
Shared by the wavetable and chord engines."""
import json, os, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import lut
data = lut.array("wav_integrated_waves")
assert len(data) == 192 * 132, len(data)
path = os.path.join(lut.DATA, "wav_integrated_waves.json")
with open(path, "w") as f:
    json.dump({"shape": [132, 192], "source": "plaits/resources.cc wav_integrated_waves (MIT, Emilie Gillet)", "data": data}, f, separators=(",", ":"))
print(path, len(data), data[:6])
