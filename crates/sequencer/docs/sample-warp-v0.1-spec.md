# Sample Warp v0.1 Spec

## Problem

Loading a break or loop into the sampler today is a manual exercise: the user
has to know the sample's BPM, set the project tempo to match, and hand-trim the
start point to a downbeat. For drum & bass / jungle workflows, this friction
kills flow. The user wants Ableton-like behavior — drop a sample in, it
analyzes, locks to project tempo, and starts on a downbeat.

This spec covers the v0.1 cut: aubio-driven offline analysis on load, plus a
"transient-locked, natural-decay" warp engine selectable per-sampler. It is
deliberately scoped to a single warp mode and an in-memory analysis cache.
Persistent storage (SQLite), better detection (madmom), and additional warp
engines (Rubber Band, sustain-stretch) are deferred to follow-up specs and are
sketched at the end.

## Goals

- Detect BPM and onset positions automatically when a WAV is loaded into a
  sampler track.
- Run analysis on a worker thread; never block the UI or audio threads.
- Show analysis status in the sampler UI ("Analyzing…", "120.0 BPM", etc.).
- Provide a per-sampler warp toggle and warp-mode dropdown (one mode for now).
- When warp is on, time-align playback to the project BPM using onset-locked
  slice triggering with natural decay between slices.
- Allow the user to correct a wrong BPM via a number input plus 2× and ½×
  buttons; downbeat/start-point can be nudged separately.
- Keep all warp processing in the existing audio callback and audiograph
  sampler node — no new realtime threads, no allocations on the audio thread.

## Non-Goals

- Persistence of analysis results across app restarts (no SQLite yet).
- High-quality time stretching (Rubber Band / phase vocoder). v0.1 leans on the
  fact that each slice plays at native pitch through its natural envelope.
- Pitched / melodic warp modes (Tones, Texture, Complex).
- Multi-sample analysis batches or a library import pipeline.
- Re-analyzing on the fly when the user changes start/end trim.
- Tempo automation. v0.1 assumes a static project BPM at trigger time.
- Tag and metadata management.

## Architecture Overview

```text
File load (UI thread)
  -> spawn analysis job on worker thread
  -> aubio: tempo + onsets
  -> AnalysisResult written to in-memory cache keyed by buffer_id
  -> UI polls cache for status; redraws sampler waveform with onset markers
                                    |
                                    v
Sampler trigger (scheduler thread)
  -> if warp_enabled and analysis ready:
       compute warp_ratio = project_bpm / sample_bpm
       attach onset_table + ratio to voice
  -> push to audio thread via per-track AtomicU32 state slots
                                    |
                                    v
Sampler process (audio thread)
  -> existing playback path, plus warp branch:
       - playhead advances at native rate within current slice
       - on slice boundary (project-time), jump to next onset's source frame
       - 5 ms equal-power crossfade across the jump
```

The three threads communicate only through:

1. The existing audiograph state buffer (atomic f32 slots) for per-voice warp
   parameters.
2. A new shared-readable `OnsetTable` keyed by buffer_id, populated by the
   analysis worker and read-only on the audio thread.
3. The existing UI mailbox for status updates.

## Analysis Pipeline

### Worker Thread

A single long-lived `analysis_worker` thread, spawned at app startup, owns a
`std::sync::mpsc::Receiver<AnalysisJob>`. New jobs are submitted from the UI
thread when a WAV is loaded (`load_wav_buffer` in `src/sampler.rs`).

```rust
pub struct AnalysisJob {
    pub buffer_id: i32,
    pub samples: Arc<Vec<f32>>,   // mono mixdown of stereo buffer, owned
    pub sample_rate: u32,
}

pub struct AnalysisResult {
    pub buffer_id: i32,
    pub bpm: f32,
    pub bpm_confidence: f32,
    pub onsets_frames: Vec<u32>,  // sample-frame indices into the source buffer
    pub downbeat_frame: Option<u32>, // best-guess downbeat (v0.1: first strong onset on a beat)
}
```

The worker calls into a new `src/analysis.rs` module:

```rust
pub fn analyze(samples: &[f32], sr: u32) -> AnalysisResult { ... }
```

Internally `analyze` runs aubio's tempo and onset detectors. We add the
`aubio-rs` crate (which links libaubio via cc, similar to how audiograph is
linked). Detector parameters for v0.1:

- `aubio_tempo_new("default", buf_size=1024, hop_size=512, sr)`
- `aubio_onset_new("hfc", buf_size=1024, hop_size=512, sr)` with
  `set_threshold(0.3)` and `set_minioi_ms(40.0)`

Both detectors are fed the mono buffer in 512-sample hops. Tempo is read from
the final `aubio_tempo_get_bpm`; confidence from `aubio_tempo_get_confidence`.
Onsets are accumulated into a `Vec<u32>` of frame indices.

`downbeat_frame` for v0.1 is heuristic: take the first onset whose position
agrees with the BPM grid to within ±20 ms, treating the first beat slot in the
sample as a candidate downbeat. This is intentionally crude and is the place
where madmom replaces it later (see *Further Steps*).

### Cache

`AnalysisCache` lives in the `App` struct (or a sub-struct alongside
`sampler_paths`). It is the shared store the UI and scheduler read from:

```rust
pub struct AnalysisCache {
    inner: Arc<RwLock<HashMap<i32, Arc<AnalysisEntry>>>>,
}

pub enum AnalysisEntry {
    Pending,
    Ready(AnalysisResult),
    Failed(String),
}
```

`Arc<AnalysisEntry>` lets the audio thread hold a stable read snapshot for the
duration of a voice without holding the lock. The audio thread never takes the
write lock; only the analysis worker does, and only once per buffer.

### Submission Points

- `load_wav_buffer` → after `create_buffer`, send an `AnalysisJob` with the
  pre-trimmed mono mix-down. Insert `AnalysisEntry::Pending` synchronously so
  the UI shows "Analyzing…" immediately.
- A re-analyze command (Ctrl+R on a sampler, or button in the UI) re-submits
  the job and resets to `Pending`. Useful when the user fixes a bad detection.

## Sampler State Additions

Extend `src/sampler.rs` state layout. New atomic slots (all `f32`):

| Slot | Name | Purpose |
|---|---|---|
| 31 | `STATE_WARP_ENABLED` | 0.0 = off, 1.0 = on |
| 32 | `STATE_WARP_MODE` | 0.0 = transient-locked (only mode in v0.1) |
| 33 | `STATE_WARP_RATIO` | project_bpm / sample_bpm (1.0 = no stretch) |
| 34 | `STATE_WARP_ONSET_TABLE_PTR_LO` | Lower 32 bits of `*const OnsetTableShared` |
| 35 | `STATE_WARP_ONSET_TABLE_PTR_HI` | Upper 32 bits |
| 36 | `STATE_WARP_CURRENT_SLICE` | Index into onset table of the slice currently playing |
| 37 | `STATE_WARP_SLICE_PROJECT_FRAME_START` | Project-time frame at which current slice was triggered |
| 38 | `STATE_WARP_XFADE_REMAINING` | Samples remaining in the slice-boundary crossfade |

Update `SAMPLER_STATE_SIZE` to 39. Add `pub const PARAM_WARP_*` constants
mirroring existing convention.

The pointer-pair encoding for `OnsetTableShared` is the same trick already used
elsewhere in the codebase for passing non-atomic data to the audio thread: the
control thread writes the `Arc::into_raw` pointer split across two atomic
slots; the audio thread reads them back, recombines, and dereferences (with
the lifetime guaranteed by an Arc clone held in a parallel control-thread
table that releases only when the voice ends).

```rust
pub struct OnsetTableShared {
    pub onsets_frames: Vec<u32>,
    pub sample_len_frames: u32,
}
```

Once an `Arc<OnsetTableShared>` is published to a voice, it is treated as
immutable for the voice's lifetime. Re-analysis allocates a new `Arc` and
swaps the pointer; in-flight voices keep their old table.

## Warp Process: Transient-Locked, Natural Decay

This is the only warp mode in v0.1.

### Concept

The sample's analyzed onsets define slice boundaries. Each slice plays at
native pitch — no time stretching, no resampling beyond the existing transpose
path — but the time at which we *start* each slice is determined by the
project's beat grid and the warp ratio.

- If the project BPM matches the sample BPM, slices line up exactly with their
  original spacing.
- If the project is slower, each slice is given more time than its natural
  length: it plays through, then rings out into silence/decay, then the next
  slice's transient hits at the right project-time moment.
- If the project is faster, each slice is cut short by an equal-power fade
  before its natural end, and the next transient fires earlier.

The trick that makes this sound musical is that **transients are never
stretched and never repeated**. Every onset in the source plays exactly once,
at its native sample rate, with its natural envelope. We only manipulate the
*spacing between* onsets.

### Algorithm

Per audio-thread sample, when `warp_enabled == 1.0` and the onset table is
present:

```text
sample_frames_into_slice = (current_project_frame - slice_project_frame_start)
                           * warp_ratio
source_frame = onset_table[current_slice] + sample_frames_into_slice

if current_slice + 1 < onset_table.len():
    next_onset_project_frame = slice_project_frame_start
        + (onset_table[current_slice + 1] - onset_table[current_slice]) / warp_ratio

    if current_project_frame >= next_onset_project_frame:
        # advance slice
        previous_source_frame = source_frame
        current_slice += 1
        slice_project_frame_start = current_project_frame
        source_frame = onset_table[current_slice]
        xfade_remaining = XFADE_SAMPLES   # ~5 ms = 220 samples at 44.1k
```

`XFADE_SAMPLES` is a compile-time constant set to `(sr * 0.005)`. During the
xfade window, the output is `equal_power(prev_slice_tail, new_slice_head)`. The
previous slice's read pointer continues to advance at native rate so its
natural decay rings into the crossfade.

If `warp_ratio < 1.0` (project slower than sample), the next-onset-project-
frame is later than the natural end of the current slice. The current slice's
read pointer hits `onset_table[current_slice + 1]` first, then continues
reading past it (into the next slice's region) at native rate. This is fine —
the audio is just the natural sample content — but it means the new slice's
*transient* is consumed before its scheduled trigger. To prevent that, when
`warp_ratio < 1.0` we let the read pointer freely run *up to but not past* the
next onset, then hold (read silence) until the trigger. Concretely:

```text
if warp_ratio < 1.0:
    natural_end = onset_table[current_slice + 1]
    if source_frame >= natural_end:
        emit silence (or apply a short release envelope)
```

If `warp_ratio > 1.0`, the next onset's project time arrives before the
current slice has finished playing. We trigger the new slice and cross-fade
out the old one; the old slice's tail fades to zero over `XFADE_SAMPLES`.

### Interaction With Existing Sampler Features

- **Start/end trim** still applies as the outer playback region. Onsets
  outside `[start_sample, end_sample]` are ignored.
- **Loop modes** (`gate`, `loop`, `ping-pong`) are disabled when warp is on in
  v0.1. The dropdown grays them out and the audio path skips loop logic. This
  avoids combinatorial complexity for the first cut; combined warp+loop is a
  follow-up.
- **Reverse** is disabled for the same reason.
- **Transpose / speed** still apply on top of warp: they multiply the per-
  slice native read rate. Pitch and warp are independent.
- **Attack / release** envelopes apply per *trigger*, not per slice. A single
  note-on starts the warp engine; release fades out whichever slice is
  playing.
- **Gate length** determines how long the warp engine runs; on gate-off the
  envelope releases.

## UI Changes

### Sampler Instrument Panel

Add to the right of the existing `sr` knob in the sampler param row:

```
... sr 44100.0    warp [OFF]    mode [transient]    bpm [120.0] [½×] [2×]    [↺ re-analyze]
```

- `warp` toggle: existing button widget (like `gate`/`reverse`).
- `mode` dropdown: same widget as the `loop` dropdown. v0.1 has one option,
  `transient`. Future modes appear here.
- `bpm` numeric input: editable, same widget as numeric knobs but with a
  text-entry mode (the user can type "174.0" directly). Shows the analyzed
  BPM, italicized while `Pending`, replaced with the user's value once edited.
- `½×` / `2×`: buttons that halve or double the displayed BPM. Useful when the
  detector locks an octave off, which aubio does ~10% of the time on breaks.
- Re-analyze button: re-submits the current buffer to the worker.

### Waveform Display

Augment the existing waveform widget to draw onset markers as thin vertical
lines (1 px) over the waveform when an `AnalysisEntry::Ready` is available for
this buffer. Highlight the chosen downbeat marker in a different color (e.g.
the same cyan used for the playhead in the screenshot).

The user can click-drag the downbeat marker to nudge the start point. This
edits `STATE_START_POINT` to align with the chosen onset and updates the
displayed BPM's "phase" (which beat the sample begins on) — but does not
re-run analysis.

While analysis is `Pending`, render a thin progress bar along the bottom of
the waveform region or a "Analyzing…" overlay.

### Status Polling

The `metal_seq` UI loop already polls per-track sampler state. Add a poll of
`AnalysisCache::get(buffer_id)` per redraw for the currently focused track and
skip when status hasn't changed.

## Project BPM Source

The project BPM lives in `state.transport.bpm: AtomicU32` (see
`src/sequencer/state.rs:680`, default `DEFAULT_BPM = 120` from
`src/sequencer/data.rs:10`). It is set from Lisp via `seq-set-bpm`
(`metal-seq-transport.lisp:366`) and is already an atomic that other audio-
thread nodes read directly (see `STATE_BPM` slots in `filter.rs`, `delay.rs`,
and `voice_modulator.rs`, which receive BPM via the existing param push in
`audio.rs:1978`).

The warp engine reads `state.transport.bpm` at trigger time on the scheduler
thread and computes:

```text
warp_ratio = project_bpm / sample_bpm
```

This ratio is written into the voice's `STATE_WARP_RATIO` atomic slot before
the trigger is dispatched to the audio thread. v0.1 does *not* update
`STATE_WARP_RATIO` mid-note in response to BPM changes — a tempo edit takes
effect on the next trigger. (Live tempo ramping is a follow-up; the cleanest
way is to push BPM into the sampler state slot the same way it is already
pushed to filter/delay/voice_modulator and have the audio thread recompute
the ratio per block.)

`sample_bpm` is read from the user-editable BPM field on the sampler, *not*
from the analysis result directly — a corrected value persists. The analysis
result populates the field as a default; the user override wins.

## Failure Modes & Edge Cases

- **Analysis fails or BPM confidence is below 0.1**: leave warp disabled by
  default, surface a warning glyph next to the BPM field, but still allow the
  user to type a BPM and turn warp on.
- **Onset table is empty or has fewer than 2 onsets**: warp falls back to
  unwarped playback and a warning is shown. v0.1 will not synthesize fake
  onsets on a fixed grid.
- **Warp toggled mid-note**: take effect on the next trigger. Mid-note state
  swaps in the audio thread are out of scope.
- **Buffer replaced (user loads a different sample on the same track)**:
  release the old `Arc<OnsetTableShared>` (control-thread-side), submit a new
  analysis job, reset the warp ratio to 1.0 until results land.

## Implementation Plan

1. Add `aubio-rs` to `Cargo.toml`. Verify it links cleanly on macOS (libaubio
   is on Homebrew). Document the dependency in the README install steps.
2. Add `src/analysis.rs` with `AnalysisJob`, `AnalysisResult`,
   `AnalysisEntry`, `AnalysisCache`, and a `spawn_analysis_worker(rx)`
   function returning a `JoinHandle`. Unit-test on a known-BPM WAV from
   tests/.
3. Wire the worker into `metal_seq` startup. Pass the cache `Arc` into `App`.
4. Hook `load_wav_buffer` to submit jobs and insert `Pending` entries.
5. Extend sampler state slots and `PARAM_WARP_*` constants. Bump
   `SAMPLER_STATE_SIZE`.
6. Implement the warp branch inside `sampler_process`. Reuse existing
   click-prevention machinery for the slice-boundary crossfade.
7. Add the warp/bpm UI controls in the sampler panel; route their writes
   through the existing param-edit pathway.
8. Add onset-marker rendering to the waveform widget; gated on
   `AnalysisEntry::Ready`.
9. Smoke-test on a half-dozen amen variants at 120 / 140 / 160 / 174 / 87 BPM
   project tempos. Verify slices are seamless at native tempo and the
   half-time pull rings out cleanly.

## Further Steps

These are explicitly out of scope for v0.1 but are the planned trajectory.

### Persistent Library: SQLite

Replace the in-memory `AnalysisCache` with a SQLite-backed library. Schema
sketch:

```sql
CREATE TABLE samples (
    id INTEGER PRIMARY KEY,
    path TEXT NOT NULL UNIQUE,
    sha256 TEXT NOT NULL,
    duration_frames INTEGER NOT NULL,
    sample_rate INTEGER NOT NULL,
    channels INTEGER NOT NULL,
    bpm REAL,
    bpm_confidence REAL,
    bpm_user_override REAL,
    detected_at INTEGER,         -- unix seconds
    analyzer_version TEXT,
    source_album TEXT,
    source_artist TEXT,
    notes TEXT
);

CREATE TABLE beats (
    sample_id INTEGER NOT NULL REFERENCES samples(id) ON DELETE CASCADE,
    frame INTEGER NOT NULL,
    kind TEXT NOT NULL,          -- 'onset' | 'beat' | 'downbeat'
    confidence REAL,
    PRIMARY KEY (sample_id, frame, kind)
);

CREATE TABLE tags (
    sample_id INTEGER NOT NULL REFERENCES samples(id) ON DELETE CASCADE,
    tag TEXT NOT NULL,
    PRIMARY KEY (sample_id, tag)
);

CREATE INDEX idx_samples_sha ON samples(sha256);
CREATE INDEX idx_tags_tag    ON tags(tag);
```

Hash-keyed records mean moving or renaming a file does not lose its analysis;
on import, a SHA-256 lookup reuses any existing row. The library DB lives at
`~/Library/Application Support/sequencer/library.sqlite` on macOS.

This unlocks tag-based browsing, source/album filtering, and instant load —
analysis only re-runs for samples not seen before.

### Better Detection: madmom Sidecar

Aubio gets the wrong BPM (usually octave-confused) on roughly 10–15% of
breaks. madmom's RNNDownBeatProcessor is meaningfully better but is Python and
heavy. The plan: ship madmom as an optional sidecar invoked at import time
(not realtime), launched as a subprocess with a stdin/stdout JSON protocol.
Results are written back into SQLite alongside aubio's, with `analyzer_version`
recording which produced the row. A "Deep Analyze" UI command runs madmom on
demand for samples where aubio looks wrong.

The Rust side stays madmom-free; users without Python don't need it; the
realtime engine only ever reads SQLite.

### Better Warp: Sustain-Stretch and Rubber Band

v0.1's transient-locked mode is good for breaks at modest stretch ratios.
Beyond that:

- **Sustain-stretch** (preserve the first 10–30 ms of each slice at native
  rate, granular-loop the sustain to fill the target duration). Adds a second
  warp mode at low engineering cost; sounds better than natural-decay at large
  pull-down ratios.
- **Rubber Band** (LGPL, `rubberband` crate). Real phase-vocoder time
  stretching with phase-reset events fed from the onset table. The mode of
  choice for non-percussive material — vocal loops, pads, melodic samples.
  Needs careful handling of the LGPL boundary, per-voice instantiation in the
  control thread, and a real-time mode configuration. Probably also worth
  pre-rendering the warped buffer when the project tempo is static, to keep
  the audio thread free of the vocoder's allocations.

### Multi-Sample Library Operations

Once SQLite is in place, batch operations become natural: a sample browser
that lists by tag, BPM range, or source album; bulk re-analysis with a newer
detector; export of curated kits. None of this requires changes to the warp
engine itself, only to the UI and library layer.
