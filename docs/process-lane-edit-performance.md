# Process lane editing — 2026-09-14

Tracked as `eseq-y5pa`.

Expanded sequencer process lanes previously issued one host command and one
whole-project scheduler snapshot per selected step. Every lane write also
requested a process-chain refresh, rebuilding every track's lane/slot/patch
metadata and every expanded viewport. Lane metadata contained numeric values,
so changing a value invalidated the expanded row's structural readers.

The keyboard's soft number-picker handler committed directly to the cursor
step, bypassing the selection-aware Lisp handler.

## Changes

- `seq-set-process-lane-steps` sends one explicit selection to the host.
  Single-step and selection writes use the same batch mutation and gesture.
- `SequencerState::set_process_lane_steps` validates all targets before writing,
  takes the lane lock once and publishes the edited track once. Other tracks'
  scheduler snapshots are reused. Pattern epochs remain unchanged, and enclosing
  publication coalescing scopes still publish once at their outer boundary.
- `ProcessLaneValues` invalidation refreshes the edited track and its visible
  numeric bindings. It does not refresh process slots, ports or the library.
- `SEQ.process-lanes` and `SEQ.track-process-lanes` contain metadata;
  `SEQ.track-process-lane-values` contains numeric lists indexed by track, lane
  index, then step. The tracker and parameter accessors read that separate data.
  The first override of a shared project lane still updates its fork marker.
- Typed row-picker commits use the selected steps, falling back to the cursor
  only when the selection is empty. This applies to process and built-in lanes.
  A drag or typed commit remains one undo entry.

## Measurement

Apple Silicon macOS, release profile, eight tracks with twelve process lanes
per track and 64 steps. Each result is the median/p95 of 30 updates following
ten warmup updates. The probe uses the factory sequencer UI and measures state
publication, reactive synchronization and retained layout. It excludes pointer
dispatch, GPU drawing and the history captures at gesture start/end.

| Selected steps | Median | p95 |
| --- | ---: | ---: |
| 1 | 0.322 ms | 0.437 ms |
| 16 | 0.347 ms | 0.504 ms |
| 64 | 0.349 ms | 0.467 ms |

These are post-change timings, not a measured before/after speedup or a live
mouse-latency claim. The structural regressions assert one publication per
selection and no reruns of unchanged metadata readers.

```sh
cargo nextest run --release -p sequencer --bin metal_seq \
  -E 'test(=state_values::tests::process_lane_selection_edit_perf)' \
  --run-ignored only --no-capture
```

## Validation

Focused regressions cover selected and cursor-only typed entry, one batched
slider callback, immediate numeric projections, stable metadata readers,
tracker lane values, project-lane override isolation, undo/redo, invalid-target
atomicity, publication coalescing and unchanged playback epochs.

The headless production capture was inspected for slider/picker geometry and
selection rendering:

```sh
cargo run -p sequencer --bin metal_seq -- capture \
  --script crates/sequencer/ui/capture-fixtures/process-lane-edit.lisp \
  --buffer sequencer --width 2000 --height 900 \
  --out /tmp/eseq-y5pa-process-lane.png
```

The existing process-lane undo implementation still captures scene structure
at gesture boundaries. This change removes the repeated work during a drag;
it does not redesign those boundary captures.
