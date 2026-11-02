"""Export a sample pack as a training dataset folder.

A pack is a curated list of library samples (browser: right-click a sample,
Add to <pack> / New Pack..., or the Pack field of Save Region). This links
each member's stored WAV into OUT as <name>.wav and writes OUT/manifest.json
in the shape the latent-from-samples dataset scripts use (name, title, hash,
sr, ch, bits, frames), plus where a region was cut from.

usage: python3 tools/sample-packs/export_pack.py "training set 1" OUT [--library .local]
       python3 tools/sample-packs/export_pack.py --list [--library .local]

--library is the folder holding samples.db, samples.jsonl, sample-packs.json
and samples/ (a dev checkout's .local; the release app's data folder).
"""
import argparse
import json
import os
import re
import sys
import struct


def load_packs(library):
    path = os.path.join(library, "sample-packs.json")
    if not os.path.exists(path):
        return []
    with open(path) as f:
        return json.load(f).get("packs", [])


def load_journal(library):
    """Titles and regions by hash, replaying the journal's user facts."""
    titles, regions = {}, {}
    path = os.path.join(library, "samples.jsonl")
    if not os.path.exists(path):
        return titles, regions
    with open(path) as f:
        for line in f:
            if not line.strip():
                continue
            fact = json.loads(line)
            kind, sample = fact.get("kind"), fact.get("hash")
            if kind == "sample":
                titles[sample] = fact.get("title")
                if fact.get("region"):
                    regions[sample] = fact["region"]
            elif kind == "set_title":
                titles[sample] = fact.get("title")
    return titles, regions


def file_name(title, sample, taken):
    stem = re.sub(r"[^A-Za-z0-9._ +-]+", "_", title or "").strip(" ._") or sample[:12]
    name, n = stem, 2
    while name.lower() in taken:
        name, n = f"{stem} {n}", n + 1
    taken.add(name.lower())
    return name


def wav_info(path):
    """(rate, channels, bits, frames) from the RIFF header; unlike the wave module this accepts
    float (format 3) and WAVE_FORMAT_EXTENSIBLE files."""
    with open(path, "rb") as f:
        head = f.read(12)
        if head[:4] != b"RIFF" or head[8:12] != b"WAVE":
            raise ValueError(f"{path}: not a WAV file")
        fmt = None
        while True:
            chunk = f.read(8)
            if len(chunk) < 8:
                raise ValueError(f"{path}: no data chunk")
            cid, size = chunk[:4], struct.unpack("<I", chunk[4:])[0]
            if cid == b"fmt ":
                body = f.read(size)
                _, ch, sr, _, align, bits = struct.unpack("<HHIIHH", body[:16])
                fmt = (sr, ch, bits, align)
            elif cid == b"data":
                if fmt is None:
                    raise ValueError(f"{path}: data before fmt")
                sr, ch, bits, align = fmt
                return sr, ch, bits, size // align
            else:
                f.seek(size, 1)
            if size % 2:
                f.seek(1, 1)


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("pack", nargs="?")
    parser.add_argument("out", nargs="?")
    parser.add_argument("--library", default=".local")
    parser.add_argument("--list", action="store_true", help="list the packs and exit")
    args = parser.parse_args()

    packs = load_packs(args.library)
    if args.list:
        for pack in packs:
            print(f"{pack['name']}\t{len(pack['samples'])} samples")
        return
    if not args.pack or not args.out:
        parser.error("name a pack and an output folder (or pass --list)")
    pack = next((p for p in packs if p["name"].lower() == args.pack.lower()), None)
    if pack is None:
        sys.exit(f"no pack named {args.pack!r}; packs: {', '.join(p['name'] for p in packs) or 'none'}")

    titles, regions = load_journal(args.library)
    os.makedirs(args.out, exist_ok=True)
    manifest, taken, missing = [], set(), []
    for sample in pack["samples"]:
        src = os.path.abspath(os.path.join(args.library, "samples", sample + ".wav"))
        if not os.path.exists(src):
            missing.append(sample)
            continue
        name = file_name(titles.get(sample), sample, taken)
        dst = os.path.join(args.out, name + ".wav")
        if os.path.lexists(dst):
            os.remove(dst)
        os.symlink(src, dst)
        sr, ch, bits, frames = wav_info(src)
        row = dict(name=name, title=titles.get(sample), hash=sample, sr=sr, ch=ch, bits=bits, frames=frames)
        if sample in regions:
            row["region"] = regions[sample]
        manifest.append(row)
    with open(os.path.join(args.out, "manifest.json"), "w") as f:
        json.dump(manifest, f, indent=1)
    print(f"{len(manifest)} samples -> {args.out}")
    if missing:
        print(f"{len(missing)} not in the store: {', '.join(m[:12] for m in missing)}", file=sys.stderr)


if __name__ == "__main__":
    main()
