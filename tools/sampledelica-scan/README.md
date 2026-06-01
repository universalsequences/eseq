# sampledelica-scan

Prototype scanner that mines an album library for **breaks**, **stabs**, and
**chords**, and writes *sidecar* files you can audition. **No database yet** —
when the detections are good, a separate Rust bridge ingests the sidecars into
the sequencer's SQLite Tier-2 tables.

Decoupled from the Rust crates on purpose. The only contract is the sidecar
JSON shape in `schema.py`, which mirrors the planned `sample_chords` /
`sample_musical` tables. If a field is awkward to fill or query here, fix the
schema *here* before writing Rust.

## Pipeline

```
decode → Demucs (htdemucs) → drums + other(harmonic) stems
  ├─ stabs:  onset-segment harmonic stem → short isolated events ("lone chords")
  ├─ breaks: drum-energy high & harmonic-energy low, snapped to the beat grid
  ├─ chords: BasicPitch on the harmonic stem → notes → pc_set → (root, quality)
  ├─ key:    Krumhansl-Schmuckler over averaged chroma
  └─ embed:  CLAP vector per slice (optional, for text↔audio search)
```

## Setup

```bash
cd tools/sampledelica-scan
uv sync                 # core deps (torch, demucs, basic-pitch, librosa)
# optional extras:
uv sync --extra clap    # text<->audio "vibe" search
uv sync --extra app     # streamlit audition UI
```

## Run

```bash
# scan ONE track end to end (downloads model weights on first run)
uv run scan --library ../../crates/musicplayer/Music --out ./out --limit 1

# scan a specific album/track by substring, force CPU if MPS misbehaves
uv run scan --library ../../crates/musicplayer/Music --filter "album name" --device cpu

# with embeddings (after: uv sync --extra clap)
uv run scan --library ../../crates/musicplayer/Music --limit 5 --embed
```

Output per track under `out/<album>/<title>/`:
- `slices/*.wav` — rendered candidate samples
- `spectrogram.png` — detected regions drawn on the spectrogram
- `sidecar.json` — one record per slice, shaped like Tier-2

## Audition

```bash
uv run --extra app streamlit run app/audition.py
```

Browse slices, see chord/key labels, listen, and (with CLAP) search by vibe.

## Tuning knobs

- `segment.py` — break/stab heuristics (energy ratios, durations, top-k)
- `chords.py` — `TEMPLATES` chord vocabulary and the scoring in `identify()`
- `--device cpu|mps` — Demucs backend; `mps` is faster on Apple Silicon but
  occasionally flaky, fall back to `cpu`.

## Notes

- mp3/flac/wav/ogg decode via libsndfile (no ffmpeg). m4a/aac need ffmpeg.
- Demucs stems are cached under `cache/stems/` keyed by file identity, so
  re-running with different segmentation settings is fast.
