# sampledelica viz

A fast Bun/JS region explorer over the scanner's sidecar output. Replaces the
slow Streamlit app for *understanding the data* — sources with slice markers on
a waveform, click a region to inspect its harmonic data and find its CLAP
neighbors across albums.

## Run

```bash
bun run viz/server.ts            # http://localhost:5173
OUT=./out bun run viz/server.ts  # point at a different scan output dir
PORT=8080 bun run viz/server.ts
```

No install step — it reads `out/**/sidecar.json` at startup and holds the CLAP
vectors in memory for cosine similarity.

## What you see

- **Left** — every scanned source, grouped by album (cover, key, bpm, region
  count, kind dots).
- **Center** — the selected source as a **waveform with colored region
  markers** (orange break / blue chord / green stab). Click a marker or a
  ribbon chip to play that region *from the original mix* and select it.
- **Right** — the selected region's detail: pitch-class keyboard, root/bass,
  `pc_set` / `pc_set_norm`, confidence, and **CLAP neighbors**. Toggle
  cross-album-only and same-kind. Each neighbor previews from *its* original mix
  and clicking it jumps to that source.

## Notes

- All audio is served from the **original files** (or the original-mix slice
  wavs) — never the analysis stems. `/audio` and `/cover` only serve paths under
  `OUT` and the music-library dirs the sidecars reference (range-aware, 206).
- Endpoints: `/api/index`, `/api/similar?uid=&crossAlbum=&sameKind=&k=`,
  `/audio?path=`, `/cover?path=`.
