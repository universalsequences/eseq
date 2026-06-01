"""`scan` CLI entrypoint.

    uv run scan --library ../../crates/musicplayer/Music --out ./out --limit 1
"""

from __future__ import annotations

import argparse
import sys
import traceback
from pathlib import Path

from .library import scan_library
from .pipeline import Options, process_track


def main(argv: list[str] | None = None) -> int:
    p = argparse.ArgumentParser(prog="scan", description="Mine an album library for breaks/stabs/chords.")
    p.add_argument("--library", required=True, type=Path, help="root folder of albums")
    p.add_argument("--out", type=Path, default=Path("./out"), help="output dir for sidecars + slices")
    p.add_argument("--cache", type=Path, default=Path("./cache/stems"), help="demucs stem cache")
    p.add_argument("--limit", type=int, default=0, help="max tracks (0 = all)")
    p.add_argument("--filter", default="", help="only tracks whose path contains this substring")
    p.add_argument("--device", default="mps",
                   help="demucs device: mps (Apple GPU, default, ~fast) | cpu | cuda")
    p.add_argument("--embed", action="store_true", help="compute CLAP embeddings (needs --extra clap)")
    p.add_argument("--no-breaks", action="store_true")
    p.add_argument("--no-stabs", action="store_true")
    p.add_argument("--max-stabs", type=int, default=24)
    p.add_argument("--render-from-stem", action="store_true",
                   help="render tonal slices from the separated stem instead of the full mix "
                        "(off by default — stem audio is gurgly)")
    p.add_argument("--force", action="store_true",
                   help="reprocess tracks even if their sidecar.json already exists "
                        "(default: skip done tracks so big batches are resumable)")
    args = p.parse_args(argv)

    tracks = scan_library(args.library)
    if args.filter:
        tracks = [t for t in tracks if args.filter.lower() in str(t.path).lower()]
    if args.limit:
        tracks = tracks[: args.limit]

    if not tracks:
        print(f"No audio found under {args.library}", file=sys.stderr)
        return 1

    opts = Options(
        cache_dir=args.cache,
        device=args.device,
        do_embed=args.embed,
        do_breaks=not args.no_breaks,
        do_stabs=not args.no_stabs,
        max_stabs=args.max_stabs,
        render_from_stem=args.render_from_stem,
    )

    print(f"Scanning {len(tracks)} track(s) -> {args.out}")
    ok = 0
    skipped = 0
    for i, t in enumerate(tracks, 1):
        print(f"[{i}/{len(tracks)}] {t.album} / {t.title}")
        if not args.force and (args.out / t.album / t.title / "sidecar.json").exists():
            print("    skip (sidecar exists; --force to redo)")
            skipped += 1
            ok += 1
            continue
        try:
            sc = process_track(t, args.out, opts)
            chordy = sum(1 for s in sc.slices if s.kind == "chord")
            breaks = sum(1 for s in sc.slices if s.kind == "break")
            key = f"{sc.key_pc}:{sc.key_mode}" if sc.key_pc is not None else "?"
            print(f"    key={key} bpm={sc.bpm:.0f} -> {chordy} chords, {breaks} breaks, "
                  f"{len(sc.slices)} slices")
            ok += 1
        except Exception as e:
            print(f"    FAILED: {e}", file=sys.stderr)
            traceback.print_exc()

    print(f"Done: {ok}/{len(tracks)} tracks. Audition with: uv run --extra app streamlit run app/audition.py")
    return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
