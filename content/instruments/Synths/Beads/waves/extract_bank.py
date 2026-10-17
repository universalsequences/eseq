#!/usr/bin/env python3
"""Extract Beads' integrated wavetables into bank.json.

Usage: python3 extract_bank.py <path/to/pichenettes/eurorack>

Reads `wav_integrated_waves` from beads/resources.cc verbatim (int16 values,
12 banks x 16 waves x 260 samples) and writes it wave-major, which is both
Beads' own layout and the DGenLisp `[samples waves]` tensor convention, so
`(peek bank i (+ (* bank_index 16) wave))` reads exactly what Beads reads.

Also writes view.json for the panel's wavetable viewer: the tables are
integrated (Beads differentiates them while playing, for anti-aliasing), so
the viewer gets each wave's first difference, peak-normalized.
"""
import json
import os
import re
import sys

NUM_BANKS = 12
WAVES_PER_BANK = 16
SAMPLES = 256 + 4

BANK_NAMES = [
    "Harmonics", "Tri Stack", "Drawbars", "Tri/Saw", "Burst/Formant",
    "Formants", "Digi/Metal", "Braids", "FM 2:1", "FM 1:0.5", "FM 1:3",
    "FM 1:8",
]


def main():
    root = sys.argv[1]
    source = open(os.path.join(root, "beads", "resources.cc")).read()
    body = re.search(r"wav_integrated_waves\[\] = \{(.*?)\};", source, re.S).group(1)
    data = [int(v) for v in re.findall(r"-?\d+", body)]
    expected = NUM_BANKS * WAVES_PER_BANK * SAMPLES
    if len(data) != expected:
        sys.exit(f"expected {expected} samples, found {len(data)}")
    out = {
        "shape": [SAMPLES, NUM_BANKS * WAVES_PER_BANK],
        "kind": "wavetable-bank",
        "layout": "wave-major: index = (bank * 16 + wave) * 260 + sample",
        "source": "Mutable Instruments Beads, beads/resources.cc "
                  "wav_integrated_waves (MIT, Emilie Gillet)",
        "banks": BANK_NAMES,
        "data": data,
    }
    path = os.path.join(os.path.dirname(os.path.abspath(__file__)), "bank.json")
    with open(path, "w") as f:
        json.dump(out, f, separators=(",", ":"))
    print(f"wrote {path} ({len(data)} samples)")

    view = []
    for w in range(NUM_BANKS * WAVES_PER_BANK):
        table = data[w * SAMPLES:(w + 1) * SAMPLES]
        wave = [table[i + 2] - table[i + 1] for i in range(256)]
        peak = max(abs(v) for v in wave) or 1
        view += [round(v / peak, 4) for v in wave]
    view_path = os.path.join(os.path.dirname(path), "view.json")
    with open(view_path, "w") as f:
        json.dump({"shape": [256, NUM_BANKS * WAVES_PER_BANK],
                   "waves_per_set": WAVES_PER_BANK, "sets": BANK_NAMES,
                   "data": view}, f, separators=(",", ":"))
    print(f"wrote {view_path}")


if __name__ == "__main__":
    main()
