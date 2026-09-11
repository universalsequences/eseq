#!/usr/bin/env python3
"""Regenerate the manual's real UI illustrations on macOS (no app window)."""

import argparse
import json
import pathlib
import subprocess

ROOT = pathlib.Path(__file__).resolve().parents[1]
FIXTURES = ROOT / "crates/sequencer/ui/capture-fixtures"
CAPTURES = json.loads((FIXTURES / "manual-images.json").read_text())


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=pathlib.Path,
                        help="Use an already built metal_seq instead of building it")
    parser.add_argument("names", nargs="*", help="Capture only these image names (default: all)")
    args = parser.parse_args()
    unknown = set(args.names) - {entry["name"] for entry in CAPTURES}
    if unknown:
        parser.error("unknown captures: " + ", ".join(sorted(unknown)))
    binary = args.binary
    if binary is None:
        result = subprocess.run(
            ["cargo", "build", "-p", "sequencer", "--bin", "metal_seq",
             "--message-format=json-render-diagnostics"],
            cwd=ROOT, check=True, stdout=subprocess.PIPE, text=True)
        for line in result.stdout.splitlines():
            message = json.loads(line)
            if message.get("reason") == "compiler-artifact" and message.get("target", {}).get("name") == "metal_seq":
                if message.get("executable"):
                    binary = pathlib.Path(message["executable"])
        if binary is None:
            raise RuntimeError("Cargo did not report the metal_seq executable")
    binary = binary.resolve()
    for entry in CAPTURES:
        if args.names and entry["name"] not in args.names:
            continue
        print("Capturing " + entry["name"], flush=True)
        command = [str(binary), "capture", "--hide-status", "--script", str(FIXTURES / entry["script"]),
                   "--buffer", entry["buffer"], "--width", str(entry["width"]),
                   "--height", str(entry["height"]),
                   "--out", str(ROOT / "docs/manual/images" / (entry["name"] + ".png"))]
        if entry["key"]:
            command += ["--key", entry["key"], "--padding", "4"]
        subprocess.run(command, cwd=ROOT, check=True)



if __name__ == "__main__":
    main()
