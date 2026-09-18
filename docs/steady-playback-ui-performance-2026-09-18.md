# Steady playback UI work

The `garageddd` profiles exposed two independent costs. With panels open,
retained scene preparation repeatedly called widget painters. With scratch
alone, syntax highlighting repeatedly copied completion names and rebuilt
symbol membership. These are CPU costs on the UI thread; the profiles do not
establish GPU saturation or an audio deadline failure.

## Invalidation and reuse contracts

Completion names are immutable shared snapshots. A tile caches its highlight
vocabulary by buffer identity, text revision, mode, and runtime symbol revision.
Highlighted spans additionally depend on the visible line range. Scrolling
reuses the vocabulary; cursor, selection, and evaluation-flash overlays still
build from current editor state.
Active and inactive text frame/GPU keys also include the symbol revision, so
evaluating a definition refreshes token colors without requiring a text edit.

Retained painting snapshots supported reactive props once. The painter reads
those same frozen values, preserving their Lisp types. A second live read is
never used to certify a paint result: an A/B/A change during painting cannot
make stale output look reusable. Unknown painters, opaque values, and cyclic
values still decline input reuse. Animation, focus, hover, theme, layout, and
widget state retain explicit invalidation.

Host paint resources record dependencies at their getters. Each resource entry
has a version shared with its readers; publication, removal, and an initially
missing resource becoming available invalidate those readers. Observation and
publication occur under the data store's existing lock. A publication racing
painting remains pending. This removes whole-scene invalidation from scope,
meter, and glyph publication. Clearing an empty overlay is a no-op.

Display demand follows live reactive effects and the visible binding registry,
including custom named buffers and nonvisual observers. Hidden process-scope
publication leaves the last consumed generation unchanged, so reopening reads
the newest history. Inline runtime polling and pattern glyph preparation check
mounted consumers. Glyph paint changes request redraw independently of Lisp
effect scheduling.

Scene and rack model locks expose monotonic revisions, advanced by mutable
guard access before unlocking. Stored sound cohorts need not be fingerprinted
on every tick. Reads do not advance revisions; replacement, edits, and undo do.
Tree flattening uses exact owned item observations and expansion state; edits
inside shared Lisp cells invalidate the cached rows. Per-layout indices route
dirty widget IDs and query mounted source consumers without one tree traversal
per dirty ID.
Tree row snapshots also retain cell identity: replacing an item with an equal
value must update the item cells passed to event callbacks.

## Event loop

`metal_seq` owns the render loop. SDF transitions now use the normal input,
control, and render path. Presentation deadlines advance independently of
render duration at 60 Hz; expired slots are skipped. A static view does not
request frames merely because a deadline expired. Existing bounded input
batching remains in place.

`ESEQLISP_PROFILE_UI=1` reports sync and UI-work percentiles, software input to
render-return timing, and per-root retained-paint reasons. Paint reasons are
nonexclusive: a node can have both changed inputs and changed widget state.
These diagnostics do not measure OS input delivery or display scanout.
Unpresented render attempts keep the pending input observation for the next
successful submission.

## Reproduction

The manual saved-project replay loads real project state and the production
panel layout, then drives the production reactive sync, frame builder, and
Metal renderer. It exercises panels with playheads, panels with a changing
compressor history, scrolling with that history, and scratch. Compressor keys
come from the mounted widgets; history length and stride match the real DSP
ring. Playheads and histories are deterministic inputs; the scheduler is not running.
The headless audio pump services graph commands, so its timing is not an
audio-callback deadline measurement. The saved project is never written.

```sh
ESEQ_UI_REPLAY_PROJECT="$PWD/.local/projects/PROJECT.json" \
ESEQ_UI_REPLAY_OUT=/tmp/ui-replay.json \
cargo nextest run --release -p sequencer --bin metal_seq \
  -E 'test(=tests::saved_project_ui_playback_replay)' \
  --run-ignored only --no-capture
```

The JSON records every measured iteration after warmup, including whether it
requested a frame, sync/build/render CPU time, GPU time, paint counts/reasons,
and geometry uploads. PNGs beside it capture each phase. Compare the same
project snapshot, panel layout, dimensions, and build profile. Do not compare
timings while another build or benchmark is competing for CPU resources.
The `cpu_ms` fields use elapsed main-thread work time, not OS thread CPU
accounting. Render timing excludes the explicit GPU-completion wait used for
capture readback and subtracts geometry-storage backpressure wait. Input
publication before the tick, analyzer polling, audio callback deadlines, and
physical input-to-display latency are outside this replay.

## Measured result — 2026-09-18

Apple M1 Max, optimized release, 2000 × 1200 pixels at scale 1. Baseline is
`1941c3858fbd2bb5b9f4f8873421180e659936d0` plus only the same replay/offscreen
initialization harness. The frozen `garageddd` snapshot has 19 tracks, 26 loaded
patterns, all three groups open, and PM Piano selected. Its SHA-256 is
`f6d61c3e71b3a30d0a8f65ddf3ab77bda9db62211f3c8b3b673c49fc867a0e54`.
The scratch phase shows an empty scratch pane; the separate highlighting probe
also exercises 200 lines and 4,000 additional runtime symbols.

Two alternating before/after runs each discard 30 warmup iterations and measure
180 per phase (360 observations per phase per version). No other Cargo/rustc
jobs were observed during these four runs. The author's original app stayed
open. Times below are milliseconds of synchronous UI work per replay iteration,
using the scope described above, including iterations that need no redraw.

| Phase | Before p50 / p95 / p99 | After p50 / p95 / p99 | Redraws before → after |
| --- | --- | --- | --- |
| Panels, playhead changes | 5.473 / 6.679 / 11.814 | 0.333 / 4.370 / 4.953 | 360 → 60 |
| Panels, live compressor history | 13.302 / 14.830 / 15.246 | 4.004 / 4.954 / 5.348 | 360 → 360 |
| Scroll with live compressor history | 13.779 / 15.532 / 16.773 | 4.204 / 4.974 / 5.745 | 360 → 360 |
| Scratch only | 7.269 / 8.082 / 8.316 | 0.081 / 0.138 / 0.212 | 360 → 0 |

The live-compressor phase reduces median UI work by 70%. Average widget paints
per frame fall from 3,092 to 3.89; median retained-scene preparation falls from
8.436 ms to 0.800 ms. Changing histories remain visible, and scrolling still
renders every iteration. The warmed candidate performs no layout reindexing in
any phase. This preserves earlier layout/GPU reuse work while removing the CPU
painting that happened before that reuse could help.

The proposed p95 UI-work target of 4 ms is **not yet met**. The replay's p99 is
below the proposed 8 ms target, but neither result establishes real application
input-to-display latency or audio callback reliability. Live validation is
tracked as `eseq-yjbz`; the new `ESEQLISP_PROFILE_UI` diagnostics are available
for that run after restarting into the new executable.

Evidence is in `.local/benchmarks/garageddd-ui-diagnosis-2026-09-17/`:
`comparison-final.json`, `replay-{before,after}-final-{1,2}.json`, their logs and
PNGs, both replay executables and the baseline harness patch. Earlier smoke and
contended validation runs are excluded from the table. The report records
binary hashes and the project hash. The production `metal_seq capture --project`
path also loaded this snapshot and produced `final-saved-project-capture.png`.
Both the final replay image and production capture were inspected.

## Validation and remaining limits

43 focused correctness checks passed: 36 library checks including Metal pixel
comparisons, six sequencer input/pacing/glyph checks, and the model revision
guard test. The separate highlighting timing probe, four final replay runs,
release build, and production saved-project capture also passed. No full
workspace suite was run.

The queued-frame ownership test fails before its assertions because
`device.newSharedEvent()` returns `None` here. The same exact failure occurs on
clean `1941c385`; it is documented in `AGENTS.md` and tracked as `eseq-wej3`.
GPU buffer ownership code was not changed and this test was not weakened or
silently skipped.

No known fragile workaround was introduced. Unknown/opaque painter inputs
retain conservative invalidation. Model revisions can conservatively rebuild
glyph inputs after any scene/rack write; they prioritize correctness over
claiming perfect per-track invalidation. Physical latency and audio deadline
budgets remain unverified, and the author's running app was not restarted.

## Scratch-only follow-up

The next live profile exposed tab-accessor execution and hidden visualization
publication that the original replay did not fully exercise. See
[scratch-only playback baseline](scratch-only-ui-performance-2026-09-18.md)
for the additional fixes and a 28-track replay with display polling forced due.
The earlier scratch figures describe the original probe and should not be
treated as a complete live scratch-only CPU measurement.
