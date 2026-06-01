# Sampledelica — Full Spec & Findings

> Status as of 2026-05-31. This document is the source of truth for the
> sampledelica effort: what it is, what we proved, what works, the data model,
> and the open work. Written so we can resume cold.

---

## 0. ✅ WHERE WE ARE RIGHT NOW (resume here)

**CLAP cross-album similarity is green.** Everything before CLAP works and is
validated (scanner, chords, breaks, full Bixio album, the album-aware audition
UI), and the CLAP layer now has two embedded albums for real cross-album tests.

**Exact state of the CLAP work:**
1. ✅ `uv sync --extra clap --extra app` installs the required runtime,
   including `torchvision` for `laion_clap`.
2. ✅ CLAP smoke test passes: `embed.available()` is `True`, text embeddings are
   `(512,)`, and audio region embeddings are length `512`.
3. ✅ Pipeline embeds the **original mix region** (not the stem/rendered clip)
   when `--embed` is passed — correct for the annotation model.
4. ✅ Bixio was rescanned with embeddings: 333 embedded regions
   (`312 chord`, `17 stab`, `4 break`).
5. ✅ DJ Shadow `Entroducing` was scanned with embeddings: 278 embedded regions.
6. ✅ `search.similar_to_region(...)` returns cross-album neighbors; Bixio break
   regions return DJ Shadow break regions when constrained to `kind=break`.
7. ✅ Text→audio ranking works; `"dusty tape-saturated drum break"` constrained
   to breaks ranks DJ Shadow break regions at the top.
8. ✅ Audition UI "similar" buttons are wired: a per-slice button sets
   `similar_to`, clears any text query on the next rerun, and shows cross-album
   sidebar results.

**Resume next:** annotation-only scanner refactor (§10 step 2). Stop writing
slice WAVs by default; sidecars should be timestamped annotations over original
tracks, and the audition UI should render audio clips on demand from the
original source.

---

## 1. The vision (what we're building)

Mine a personal album library for **breaks**, **"lone chords"/stabs**, and
**chords**, and make the harmonic content **searchable** — "find all maj7
stabs", "find samples containing notes C–E–G", "find key-compatible material",
and (the payoff) "find regions that *sound* alike across different albums".

This is a long-held creative goal (predates the AI era): figure out the chords
across a whole library so you can build "Avalanches-esque sampledelica" by
combining many micro-samples that sit together musically and sonically.

### The key pivot (decided 2026-05-31 after auditioning a full album)

**The product is NOT extracted sample files. It is a timestamped *annotation
layer* over the ORIGINALS, which stay pristine.**

- Rendered slice WAVs were only ever an audition convenience. They are not the
  deliverable. Originals are never copied or quality-degraded.
- An annotation = `(source_file, start_ms, end_ms, harmonic data, embedding)`
  pointing *into* an original track.
- DAW use case: load a song/section, then **"chop to drum rack"** (rhythmic) or
  **"chop to slice sampler"** (chromatic/harmonic) — the original audio is
  sliced live and mapped to pads/keys. Nothing is baked.
- Cross-album "fill": use CLAP timbre embeddings to fill empty pads with
  sonically-similar hits from across the whole library, or text→audio search
  ("dusty tape-saturated snare") to drop matching regions on pads.

### Decisions locked
- **Next focus:** CLAP cross-album similarity (prove it in Python before Rust).
- **Chop grid:** beat/bar grid (not transient grid) drives chop-to-pads mapping.
- **Stems are for ANALYSIS ONLY.** Output/audio always comes from the original
  mix. Stem audio is gurgly and must never reach the ear or the library.

---

## 2. What we built (working, validated)

A standalone Python prototype at `tools/sampledelica-scan/`, deliberately
**decoupled from the Rust crates** — it only emits sidecar JSON. When detections
are good, a Rust "bridge" ingests sidecars into the DAW's SQLite. Detection and
storage never couple.

### Pipeline (per track)
```
decode (soundfile/librosa, → 44.1k)
  → Demucs htdemucs separation  [cached on disk; ANALYSIS ONLY]
      → "other" (harmonic) stem  → break/stab location + BasicPitch notes
      → "drums" stem             → break energy detection
  → key (librosa Krumhansl-Schmuckler over chroma)
  → bpm + beats (librosa beat_track)
  → segment:
      stabs:  onset-segment harmonic stem → short isolated events
      breaks: drum-energy-high & harmonic-low, grouped on the beat grid
  → chords: BasicPitch notes in each tonal window → 12-bit pc_set →
            template-match (root, quality) → label + pc_set_norm
  → [optional] CLAP embedding per region (from the ORIGINAL mix slice)
  → render audition WAV (from ORIGINAL mix) + spectrogram + sidecar.json
```

### Files
- `src/sampledelica_scan/`
  - `library.py` — walk albums (folder = album; cover = cover/folder/front or
    first image). Mirrors the musicplayer crate's logic.
  - `audio_io.py` — decode/resample; soundfile first, librosa fallback (ffmpeg
    needed for m4a).
  - `separate.py` — Demucs subprocess wrapper; stems cached under
    `cache/stems/<key>` by file identity (size+mtime). Honors `--device`.
  - `segment.py` — `detect_stabs`, `detect_breaks`, `beat_grid`. **Heuristic —
    this is the main tuning surface.**
  - `chords.py` — `transcribe` (BasicPitch), `identify` (template match),
    `TEMPLATES` chord vocabulary, pc_set bitmask + `_rotate_norm`.
  - `keyfind.py` — Krumhansl key detection.
  - `embed.py` — CLAP load + `embed_region` (in-memory, from original mix),
    `embed_text`, `embed_audio` (file), `cosine`. No-ops if CLAP absent.
  - `search.py` — cross-album region index: `load_index`, `rank_by_vector`,
    `similar_to_region` ("more like this", cross-album-only option). **Python
    mirror of the eventual Rust `region_embeddings` similarity query.**
  - `schema.py` — `SliceRecord` + `TrackSidecar`. **Deliberately mirrors the
    SQLite Tier-2 tables.** If a field is awkward here, fix it before Rust.
  - `pipeline.py` — orchestration; `cli.py` — `scan` entrypoint.
- `app/audition.py` — Streamlit UI: per-album rollup (cover + all distinct
  chords + counts), per-track expanders (spectrogram + per-slice play), sidebar
  filters (kind, quality), and CLAP cross-album search (text + "similar to").
- `README.md` — usage. `pyproject.toml` — uv project.

### Run
```bash
cd tools/sampledelica-scan
uv sync                 # core (torch, demucs, torchcodec, basic-pitch, librosa)
uv sync --extra clap    # CLAP cross-album similarity (+ torchvision, see §6)
uv sync --extra app     # streamlit UI

uv run scan --library "/Users/alecresende/code/learning/anthropic/musicplayer/Music" \
            --out ./out --device mps --filter "Seven Notes In Black" --max-stabs 16
# --device defaults to mps (Apple GPU, ~100x faster than cpu). --embed adds CLAP.
# --render-from-stem exists but is OFF by default (stems gurgle).

uv run --extra app streamlit run app/audition.py
```

---

## 3. Critical environment facts

- **Library path: `/Users/alecresende/code/learning/anthropic/musicplayer/Music`**
  — a **SIBLING of the `eseq` repo**, i.e. `../musicplayer/Music` from repo root.
  NOT `eseq/crates/musicplayer/Music` (that crate has the *player code* only).
  62 albums, 548 tracks, 80 covers, `Album/cover.jpg + *.wav|mp3|m4a` layout.
- `uv` installed at `~/.local/bin/uv` (run `export PATH="$HOME/.local/bin:$PATH"`).
- Python 3.11 venv. `ffmpeg` present (needed for m4a). MPS confirmed available.
- Pyright "import could not be resolved" warnings are **false positives** — it
  analyzes with system Python, not the `.venv`. Code runs fine.

---

## 4. What the results proved

### DJ Shadow — "Best Foot Forward" (break record)
A♯ minor, 148 bpm → **3 chords, 2 breaks, 18 slices**. Found a real 5-note
**Em7/B** (correct inversion) plus two real drum breaks (8.5s, 4.1s). Confirms
breaks + chords + inversions all work.

### Bixio-Frizzi-Tempera — "Seven Notes In Black" (full 22-track album)
**333 regions: 312 chords (138 distinct labels), 17 stabs, 4 breaks.**
Quality histogram reads like a real giallo score:
`sus2:56, 5:48, min:36, dom7:30, maj:29, maj7:29, min7:22, sus4:15, add9:13,
min6:13, min7b5:6, maj6:6, dim:6, aug:3`.

Coherence checks that prove it's signal, not noise:
- "Sucidio": `Am6 → Am7 → Am6 → Am7 → Asus2` — a recognizable minor vamp @ conf 1.0.
- "Tunnels": same tonal world (`Am6 ×5, Asus2, Asus4, A5`) — consistent across tracks.
- `pc_set=657` (Am7) appears **identically across different tracks** → cross-album
  chord search will work.
- `pc_set_norm` collapses transposition: 22 min7 instances → **6 distinct shapes**.
  This is the transpose-invariant search key working as designed.
- Slash chords resolve with correct bass (`A#min/F`, `D#maj6/C`, `Em7/B`).

**Conclusion: on harmonically rich material the chord model is good enough to
build the Rust bridge against.**

---

## 5. Tuning learnings (bugs fixed + known weaknesses)

### Fixed
1. **Single sounding pitch must be a note one-shot, not a power "5" chord.**
   `chords.identify` now special-cases `len(present)==1` → quality `"note"`,
   and pipeline marks it `stab`, not `chord`. (Burial was emitting fake F#5/G5.)
2. **Breaks render from drum stem with `is_tonal=False` and skip chord-id** —
   prevents drums being mislabeled as chords (the Amen break had falsely come
   back as "Am7" in an early test).
3. **torchcodec required** — torchaudio 2.11 needs it to *write* Demucs stems.
   Added to core deps.
4. **`--device mps`** default — Apple GPU, dramatically faster than cpu.

### Known weaknesses (open tuning)
1. **Key detector is biased / shaky.** Called "Sucidio" A *major* though the
   chords are clearly A minor/dorian. Krumhansl over a whole modal track is
   unreliable. **Better: derive track key from the detected chords** (now that
   chords are good).
2. **Chord scoring under-calls 7ths** (Am vs Am7). Consider biasing toward
   richer chords when the 7th is present.
3. **Short stab windows catch only the attack note** → thin chords on sparse
   material (Burial). **Widen the note-collection window** past the onset to
   catch sustains.
4. **BPM octave ambiguity** (Burial read 120 vs true ~138).
5. **Burial-class murky/atonal material is worst-case** — detection is honest
   there (returns thin/low-conf), not wrong. Test chord quality on clean
   harmonic material.

---

## 6. CLAP status (validated)

- CLAP installs via `uv sync --extra clap --extra app` when both embeddings and
  the audition UI are needed. Use both extras together; syncing only `app` prunes
  the CLAP packages from the venv.
- `torchvision` is required because `laion_clap` imports it transitively. The
  `pyproject.toml` extra includes it:
  `clap = ["laion-clap>=1.1.6", "torchvision>=0.15"]`.
- Embeddings are computed from the **original mix region in memory**
  (`embed_region`), independent of slice rendering — correct for the annotation
  model.
- Bixio and DJ Shadow are embedded in `./out`, so cross-album checks are now
  meaningful.

### CLAP smoke test
```bash
export PATH="$HOME/.local/bin:$PATH"
uv run python3 -c "
import numpy as np
from sampledelica_scan import embed
print('available?', embed.available())
tv = embed.embed_text('dusty tape-saturated drum break')
print('text', None if tv is None else tv.shape)
y = (np.random.randn(48000)*0.1).astype('float32')
av = embed.embed_region(y, 48000)
print('audio', None if av is None else len(av))
print('cos', embed.cosine(tv, np.asarray(av)))
"
```

CLAP default checkpoint downloads ~2GB on first use (slow first call).

---

## 7. Data model — Tier-2 schema (the real deliverable)

Two tiers. Tier-1 reuses the EXISTING tag system; Tier-2 adds dedicated indexed
tables for harmonic + similarity search. The sequencer DB
(`crates/sequencer/src/sample_db.rs`) is already ~80% shaped for this.

### Existing schema we build on (already in sample_db.rs)
- `samples(id, hash, title, favorited, added_at)`
- `tags` + `sample_tags` + `adjacent_tags()` faceted browser
- `sources(kind, title, release_title, notes)` — provenance (the original track/album)
- `source_assets(kind='cover_art', hash, path, mime_type, w, h)` — cover art on disk
- `source_contributors(role, name)`, `source_refs(provider, ref_kind, ref_value, url)`
- **`sample_origins(sample_id, source_id, parent_sample_id, method,
  source_start_ms, source_end_ms, captured_at, notes)`** — THIS already models
  "region X of source Y". An annotation is a row here.
- `source_metadata_guesses(field, value, method, confidence, accepted)` — the
  curation pattern (auto-detect writes low-confidence guesses; human promotes).

### Tier-1 (works in the existing browser TODAY, zero schema change)
Ingest writes namespaced tags derived from detection:
`root:C`, `quality:maj7`, `chord:Cmaj7`, `key:A-minor`, `is:stab`, `is:break`.
`adjacent_tags()` already AND-filters with counts → clickable chord/key search.
Tier-1 tags are *derived from* Tier-2 at ingest (one source of truth).

### Tier-2 (the rich, indexed, numeric model)

```sql
-- one row per detected chord event (a stab = 1 row; a progression = N rows)
CREATE TABLE sample_chords (
  id          INTEGER PRIMARY KEY,
  sample_id   INTEGER NOT NULL REFERENCES samples(id) ON DELETE CASCADE,
  start_ms    INTEGER NOT NULL DEFAULT 0,
  end_ms      INTEGER,
  root_pc     INTEGER,            -- 0..11, NULL if atonal/unsure
  quality     TEXT,               -- maj,min,dom7,maj7,min7,min7b5,dim,dim7,aug,
                                  --   sus2,sus4,5,add9,maj6,min6,note
  bass_pc     INTEGER,            -- inversions / slash chords
  pc_set      INTEGER NOT NULL,   -- absolute 12-bit pitch-class bitmask (0..4095)
  pc_set_norm INTEGER,            -- pc_set rotated so root=0 → transpose-invariant
  label       TEXT,               -- 'Cmaj7', 'Am/E' display
  method      TEXT NOT NULL,      -- 'basicpitch+templates' | 'manual'
  confidence  REAL NOT NULL DEFAULT 0,
  accepted    INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX idx_chords_sample   ON sample_chords(sample_id);
CREATE INDEX idx_chords_rootqual ON sample_chords(root_pc, quality);
CREATE INDEX idx_chords_norm     ON sample_chords(pc_set_norm);

-- one row per sample/region: harmonic + rhythmic summary
CREATE TABLE sample_musical (
  sample_id   INTEGER PRIMARY KEY REFERENCES samples(id) ON DELETE CASCADE,
  key_pc      INTEGER,            -- 0..11
  key_mode    TEXT,               -- major | minor | modal | atonal
  key_conf    REAL,
  bpm         REAL,
  bpm_conf    REAL,
  is_tonal    INTEGER NOT NULL DEFAULT 1,  -- 0 = drums/noise/break
  chord_count INTEGER NOT NULL DEFAULT 0,  -- 1 = lone chord/stab, >1 = progression
  chroma      BLOB                -- 12×f32 averaged chroma, for harmonic similarity
);

-- cross-album timbre similarity (the CLAP layer) — NEW
CREATE TABLE region_embeddings (
  sample_id INTEGER PRIMARY KEY REFERENCES samples(id) ON DELETE CASCADE,
  model     TEXT NOT NULL,        -- 'clap-630k-audioset'
  dim       INTEGER NOT NULL,
  vec       BLOB NOT NULL         -- normalized f32[dim]
);
```

### The queries "search for chords" must support
```sql
-- exact: all Cmaj7
SELECT sample_id FROM sample_chords WHERE root_pc=0 AND quality='maj7' AND accepted=1;

-- transpose-invariant: every min7 voicing in any key
SELECT sample_id FROM sample_chords WHERE quality='min7';
--   or match a captured shape: WHERE pc_set_norm = :shape

-- CONTAINS notes C,E,G  (mask = 0b000010010001 = 145)
SELECT sample_id FROM sample_chords WHERE (pc_set & 145) = 145;

-- "lone chords" only (isolated harmonic stabs)
SELECT s.* FROM samples s JOIN sample_musical m ON m.sample_id=s.id
WHERE m.is_tonal=1 AND m.chord_count=1;

-- key-compatible: build diatonic pc-set mask for target key, subset-test
SELECT sample_id FROM sample_chords WHERE (pc_set & :keymask) = pc_set;
```
- Harmonic "sounds like this" = cosine over `chroma`.
- Timbre "sounds like this" (cross-album) = cosine over `region_embeddings.vec`.

### Why this shape
- **pc_set bitmask** = the whole game: integer AND/equality is indexable and
  instant across the whole library; `pc_set_norm` gives transpose-invariance
  that string tags fundamentally cannot.
- **Per-event rows** → a loop with a progression is searchable by any chord it
  contains; timestamps tie to `sample_origins.source_start_ms` for exact provenance.
- **confidence + accepted** mirror `source_metadata_guesses` curation. Keep
  that table for loose source-level guesses (e.g. album key); chords get typed
  tables (EAV text + LIKE is too slow and can't do bitmasks).

---

## 8. The annotation model — sidecar → SQLite mapping

A sidecar region maps to the DB as:
- The original track → one `sources` row (kind='recording', title, release_title).
- Cover art → `source_assets(kind='cover_art', path=...)`.
- Each region → a `samples` row (hash of region identity, NOT of copied audio)
  + a **`sample_origins`** row: `source_id`, `source_start_ms`, `source_end_ms`,
  `method`. **No extracted-WAV asset.** The "sample" is a pointer into the original.
- Region harmonic data → `sample_chords` + `sample_musical`.
- Region CLAP vector → `region_embeddings`.
- Derived Tier-1 tags → `tags` + `sample_tags`.

Current `SliceRecord` fields (schema.py) already carry: slice_id, kind,
start_ms, end_ms, method, root_pc, quality, bass_pc, pc_set, pc_set_norm, label,
chord_conf, note_count, is_tonal, chord_count, clap_vec. `TrackSidecar` carries:
source_path, album, album_path, title, cover_path, duration_ms, sample_rate,
bpm, bpm_conf, key_pc, key_mode, key_conf, slices[].

---

## 9. DAW integration (the chop actions) — design, not yet built

This is **not new DSP**. The sequencer already has the pieces:
- `analysis.rs` — aubio BPM, HFC onset detection, downbeat estimation, cached
  onset tables (`AnalysisCache`, `OnsetTableShared`).
- `sampler.rs` — onset-table pointer for live slicing, warp/bpm state.

### Chop → drum rack (rhythmic)
Load original, take a break region, slice on the **beat/bar grid** (decided),
map slices left→right onto pads. Each pad = one timed slice of the original.
(Transient-grid mode possible later, but beat/bar is the chosen default.)

### Chop → slice sampler (chromatic/harmonic)
Take tonal regions/stabs, map across keys. Use `root_pc`/`pc_set` to lay out by
pitch, transpose one stab to fill a scale, or assign chords to keys for
progression building.

### Cross-album fill (CLAP)
"Fill empty pads with similar kicks/snares from across the library" =
nearest-neighbor in `region_embeddings`, constrained by kind. "16 Rhodes stabs
that sit together from different albums" = cluster by embedding. Type "dusty
tape-saturated snare" = text→audio over `region_embeddings`. Rust path: ONNX via
the `ort` crate for CLAP inference, or keep CLAP as offline Python batch + store
vectors in `region_embeddings` (realtime only does cosine, which is trivial).

---

## 10. Roadmap / next steps (in order)

1. ✅ **Finish CLAP** (§6): re-sync with torchvision, pass smoke test, re-scan
   Bixio and DJ Shadow `--embed`, validate `similar_to_region` returns sane
   cross-album neighbors + audition "Vibe search" works.
2. **Annotation-only scanner refactor**: stop writing slice WAVs by default;
   make pure timestamped-region sidecars the deliverable (audition app renders
   on the fly). Locks in "originals stay pristine". ← next
3. **Tuning pass**: key-from-chords (fix A-bias), widen chord window, 7th bias.
4. **Rust bridge + schema**: add Tier-2 tables (§7) to sample_db.rs in its
   migration style; ingest annotation sidecars → sample_origins +
   sample_chords + sample_musical + region_embeddings; derive Tier-1 tags.
   Add query helpers: `find_by_chord(root, quality)`, `find_containing_pcs(mask)`,
   `find_similar(sample_id)`.
5. **DAW chop actions** (§9): beat/bar chop → drum rack; harmonic chop → slice
   sampler; cross-album fill via embeddings.

---

## 11. Open questions to settle next session
- Key detection: commit to key-from-chords? (Recommended.)
- Region hashing: how to make a stable `samples.hash` for a pointer-only region
  (e.g. hash of `source_hash + start_ms + end_ms + method`)?
- CLAP at runtime in Rust: ONNX `ort` vs offline-Python-only embeddings?
- Embedding storage: BLOB in SQLite (fine at ~13k samples) vs external vector
  index (only if we scale to the whole library × many regions)?
- Do breaks also get embeddings + chord=NULL rows, or live in a separate path?
