#!/usr/bin/env python3
"""Per-scene pixel delta report over the shader-capture goldens.

Prints, for each scene, the deltas of the two comparisons that separate cause
(see JUDGEMENT.md):

  1-vs-2  msl-macos-arm64 -> wgsl-macos-arm64   the shader port, same GPU
  2-vs-3  wgsl-macos-arm64 -> wgsl-linux-x86_64 the GPU, same shader

Column meanings, per scene and per comparison:

  maxd    largest single-channel absolute difference, 0-255
  mean    mean absolute channel difference over every channel of every pixel
  diff%   share of pixels differing in any channel at all
  >8%     share of pixels whose largest channel difference exceeds 8/255

Run from anywhere; paths are relative to this file. Requires only stdlib.
Pass --pairs A:B to compare a different pair of capture directories, and
--root DIR to read them from somewhere other than this directory (handy for
diffing a scratch capture before committing it).
"""

import argparse
import json
import pathlib
import struct
import zlib

ROOT = pathlib.Path(__file__).resolve().parent
DEFAULT_PAIRS = [
    ("msl-macos-arm64", "wgsl-macos-arm64"),
    ("wgsl-macos-arm64", "wgsl-linux-x86_64"),
]


def read_png_rgba(path):
    """Decode a non-interlaced 8-bit PNG to (width, height, bytearray RGBA).

    The capture harnesses only ever write RGBA8; anything else is a bug in the
    harness rather than an input this needs to handle, so it is rejected.
    """
    raw = path.read_bytes()
    if raw[:8] != b"\x89PNG\r\n\x1a\n":
        raise ValueError(f"{path} is not a PNG")
    pos, idat, header = 8, bytearray(), None
    while pos < len(raw):
        (length,) = struct.unpack(">I", raw[pos : pos + 4])
        kind = raw[pos + 4 : pos + 8]
        body = raw[pos + 8 : pos + 8 + length]
        if kind == b"IHDR":
            width, height, depth, color, _, _, interlace = struct.unpack(">IIBBBBB", body)
            if (depth, color, interlace) != (8, 6, 0):
                raise ValueError(f"{path}: expected 8-bit RGBA, non-interlaced")
            header = (width, height)
        elif kind == b"IDAT":
            idat += body
        elif kind == b"IEND":
            break
        pos += 12 + length
    width, height = header
    data = zlib.decompress(bytes(idat))
    stride = width * 4
    out = bytearray(height * stride)
    prior = bytearray(stride)
    src = 0
    for row in range(height):
        filter_type = data[src]
        src += 1
        line = bytearray(data[src : src + stride])
        src += stride
        if filter_type == 1:
            for i in range(4, stride):
                line[i] = (line[i] + line[i - 4]) & 0xFF
        elif filter_type == 2:
            for i in range(stride):
                line[i] = (line[i] + prior[i]) & 0xFF
        elif filter_type == 3:
            for i in range(stride):
                left = line[i - 4] if i >= 4 else 0
                line[i] = (line[i] + ((left + prior[i]) >> 1)) & 0xFF
        elif filter_type == 4:
            for i in range(stride):
                a = line[i - 4] if i >= 4 else 0
                b = prior[i]
                c = prior[i - 4] if i >= 4 else 0
                p = a + b - c
                pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
                pred = a if (pa <= pb and pa <= pc) else (b if pb <= pc else c)
                line[i] = (line[i] + pred) & 0xFF
        elif filter_type != 0:
            raise ValueError(f"{path}: unknown PNG filter {filter_type}")
        out[row * stride : (row + 1) * stride] = line
        prior = line
    return width, height, out


def compare(a_path, b_path):
    wa, ha, a = read_png_rgba(a_path)
    wb, hb, b = read_png_rgba(b_path)
    if (wa, ha) != (wb, hb):
        raise ValueError(f"{a_path} is {wa}x{ha} but {b_path} is {wb}x{hb}")
    pixels = wa * ha
    max_delta = 0
    total = 0
    differing = 0
    over_eight = 0
    for i in range(pixels):
        base = i * 4
        worst = 0
        for c in range(4):
            d = abs(a[base + c] - b[base + c])
            total += d
            if d > worst:
                worst = d
        if worst:
            differing += 1
            if worst > 8:
                over_eight += 1
            if worst > max_delta:
                max_delta = worst
    return {
        "maxd": max_delta,
        "mean": total / (pixels * 4),
        "diff": differing / pixels,
        "over8": over_eight / pixels,
    }


def manifest(root, name):
    with open(root / name / "manifest.json") as handle:
        return json.load(handle)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--pairs",
        action="append",
        metavar="A:B",
        help="compare capture directory A against B (repeatable)",
    )
    parser.add_argument(
        "--root",
        type=pathlib.Path,
        default=ROOT,
        metavar="DIR",
        help="directory holding the capture subdirectories (default: this one)",
    )
    args = parser.parse_args()
    root = args.root
    pairs = (
        [tuple(pair.split(":", 1)) for pair in args.pairs] if args.pairs else DEFAULT_PAIRS
    )

    for left, right in pairs:
        print(f"\n{left}  ->  {right}\n")
        left_manifest = manifest(root, left)
        right_manifest = manifest(root, right)
        if left_manifest["schema_version"] != right_manifest["schema_version"]:
            print(
                f"WARNING: schema {left_manifest['schema_version']} vs "
                f"{right_manifest['schema_version']}. Scenes whose inputs changed "
                "between those schemas draw different things in the two captures "
                "and their rows below mean nothing; see README.md.\n"
            )
        shared = [s for s in left_manifest["scenes"] if s in set(right_manifest["scenes"])]
        skipped = [s for s in left_manifest["scenes"] if s not in set(right_manifest["scenes"])]
        if skipped:
            # Never let a capture that is missing scenes read as full coverage.
            print(f"not in {right}, skipped: {', '.join(skipped)}\n")
        print(f"{'scene':<32} {'maxd':>5} {'mean':>7} {'diff%':>8} {'>8%':>8}")
        print("-" * 63)
        worst = []
        for scene in shared:
            stats = compare(root / left / f"{scene}.png", root / right / f"{scene}.png")
            print(
                f"{scene:<32} {stats['maxd']:>5} {stats['mean']:>7.3f} "
                f"{100 * stats['diff']:>7.2f}% {100 * stats['over8']:>7.2f}%"
            )
            worst.append((stats["maxd"], stats["over8"], scene))
        worst.sort(reverse=True)
        print(
            f"\nworst maxd: {worst[0][2]} ({worst[0][0]}); "
            f"worst >8%: {max(worst, key=lambda w: w[1])[2]} "
            f"({100 * max(w[1] for w in worst):.2f}%)"
        )


if __name__ == "__main__":
    main()
