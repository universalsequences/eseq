#!/usr/bin/env python3
"""Three-way delta report over the text-capture goldens.

Prints, for each captured (kind, font_size, scale_factor), the field deltas of
the two comparisons that separate cause (see JUDGEMENT.md):

  1-vs-2  coretext-macos -> fontdue-macos   the rasterizer swap, same host
  2-vs-3  fontdue-macos  -> fontdue-linux   the platform, same rasterizer

Run from anywhere; paths are relative to this file. Requires only stdlib.
"""

import json
import pathlib

ROOT = pathlib.Path(__file__).resolve().parent
CAPTURES = [
    "coretext-macos-arm64",
    "fontdue-macos-arm64",
    "fontdue-linux-x86_64",
]
FIELDS = ["cell_w", "cell_h", "ascent", "descent", "leading"]


def load(name):
    with open(ROOT / name / "metrics.json") as handle:
        document = json.load(handle)
    return {
        (m["kind"], m["font_size"], m["scale_factor"]): m
        for m in document["measurements"]
    }


def report(a, b, keys, label):
    print(f"\n===== {label} =====")
    header = "".join(f"{field:>9}" for field in FIELDS)
    print(f"{'kind':<13}{'size':>6}{'scale':>6} | {header}{'m-adv':>9}{'maxAdvD':>9} maxAdvCh")
    worst = {field: 0.0 for field in FIELDS}
    for key in keys:
        row_a, row_b = a[key], b[key]
        deltas = [row_b[field] - row_a[field] for field in FIELDS]
        for field, delta in zip(FIELDS, deltas):
            worst[field] = max(worst[field], abs(delta))
        advances_a = row_a["advance_widths"]
        advances_b = row_b["advance_widths"]
        m_delta = advances_b["m"] - advances_a["m"]
        max_ch, max_delta = max(
            ((ch, advances_b[ch] - advances_a[ch]) for ch in advances_a),
            key=lambda item: abs(item[1]),
        )
        cells = "".join(f"{delta:>+9.3f}" for delta in deltas)
        print(f"{key[0]:<13}{key[1]:>6}{key[2]:>6} | {cells}{m_delta:>+9.3f}{max_delta:>+9.3f} {max_ch!r}")
    print("max |delta| per field: " + "  ".join(f"{f}={worst[f]:.3f}" for f in FIELDS))


def main():
    tables = [load(name) for name in CAPTURES]
    keys = sorted(tables[0])
    assert all(sorted(table) == keys for table in tables), "measurement matrices differ"
    report(tables[0], tables[1], keys, "1-vs-2  coretext-macos -> fontdue-macos  (rasterizer swap, same host)")
    report(tables[1], tables[2], keys, "2-vs-3  fontdue-macos -> fontdue-linux  (platform, same rasterizer)")


if __name__ == "__main__":
    main()
