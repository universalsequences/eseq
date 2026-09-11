#!/usr/bin/env python3
"""Regenerate the manual's real UI illustrations on macOS (no app window)."""

import argparse
import json
import pathlib
import re
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
FIXTURES = ROOT / "crates/sequencer/ui/capture-fixtures"


def load_captures():
    """Load named capture recipes shared by the full refresh and subset commands."""
    entries = json.loads((FIXTURES / "manual-images.json").read_text())
    if not isinstance(entries, list):
        raise ValueError("manual-images.json must contain a list of capture recipes")
    captures = {}
    for entry in entries:
        if not isinstance(entry, dict):
            raise ValueError("each manual capture recipe must be an object")
        name = entry.get("name", "")
        if not isinstance(name, str) or not re.fullmatch(r"[a-z0-9][a-z0-9-]*", name):
            raise ValueError(f"invalid manual capture name: {name!r}")
        if name in captures:
            raise ValueError(f"duplicate manual capture: {name}")
        for field in ("script", "buffer"):
            if not isinstance(entry.get(field), str) or not entry[field]:
                raise ValueError(f"{name}: {field} must be a nonempty string")
        script = pathlib.PurePosixPath(entry["script"])
        if script.is_absolute() or ".." in script.parts or not (FIXTURES / script).is_file():
            raise ValueError(f"{name}: script must name an existing file under {FIXTURES}")
        for field in ("width", "height"):
            if type(entry.get(field)) is not int or entry[field] <= 0:
                raise ValueError(f"{name}: {field} must be a positive integer")
        if entry.get("key") is not None and (not isinstance(entry["key"], str) or not entry["key"]):
            raise ValueError(f"{name}: key must be a nonempty string or null")
        captures[name] = entry
    return captures


def build_binary(package, name, *, release=False):
    """Use Cargo's artifact path, including when CARGO_TARGET_DIR is customized."""
    command = ["cargo", "build", "-p", package, "--bin", name,
               "--message-format=json-render-diagnostics"]
    if release:
        command.append("--release")
    print(f"Building {name} ({'release' if release else 'dev'})", flush=True)
    result = subprocess.run(command, cwd=ROOT, check=True, stdout=subprocess.PIPE, text=True)
    for line in result.stdout.splitlines():
        message = json.loads(line)
        if (message.get("reason") == "compiler-artifact"
                and message.get("target", {}).get("name") == name
                and message.get("executable")):
            return pathlib.Path(message["executable"]).resolve()
    raise RuntimeError(f"Cargo did not report the {name} executable")


def capture_image(binary, entry, output):
    output.parent.mkdir(parents=True, exist_ok=True)
    command = [str(binary), "capture", "--hide-status", "--script", str(FIXTURES / entry["script"]),
               "--buffer", entry["buffer"], "--width", str(entry["width"]),
               "--height", str(entry["height"]), "--out", str(output)]
    if entry.get("key"):
        command += ["--key", entry["key"], "--padding", "4"]
    subprocess.run(command, cwd=ROOT, check=True)
    if not output.is_file():
        raise RuntimeError(f"capture did not produce {output}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=pathlib.Path,
                        help="Use an already built metal_seq instead of building it")
    parser.add_argument("--release", action="store_true", help="Build with Cargo's release profile")
    parser.add_argument("--output-dir", type=pathlib.Path, default=ROOT / "docs/manual/images",
                        help="Destination for PNGs (default: docs/manual/images)")
    parser.add_argument("names", nargs="*", help="Capture only these image names (default: all)")
    args = parser.parse_args()
    captures = load_captures()
    unknown = set(args.names) - captures.keys()
    if unknown:
        parser.error("unknown captures: " + ", ".join(sorted(unknown)))
    binary = args.binary.resolve() if args.binary else build_binary("sequencer", "metal_seq", release=args.release)
    for entry in captures.values():
        if args.names and entry["name"] not in args.names:
            continue
        print("Capturing " + entry["name"], flush=True)
        capture_image(binary, entry, args.output_dir.resolve() / (entry["name"] + ".png"))

if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, RuntimeError, subprocess.CalledProcessError) as error:
        sys.exit(f"manual capture: {error}")
