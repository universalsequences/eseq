# UI Performance Tuning

Use this process when a UI action feels slow. The goal is not merely to make a
function faster; it is to make the exact user-visible interaction faster without
skipping required state, layout, or render work.

## 1. Reproduce the exact interaction

Write down the complete scenario before profiling:

- which buffer and widget have focus;
- the real key, mouse, or scroll event used by the user;
- which other buffers are visible;
- realistic project size and state (track count, step count, selections, effects);
- the visible result that must appear in the same frame.

Exercise the production input binding and event path. Calling an internal helper
directly can miss focus handling, host invalidations, reactive propagation, or
redraw scheduling. Build the fixture from a real project when project topology is
part of the cost.

## 2. Add a deterministic end-to-end probe

Prefer an ignored test near the code that owns the application/render loop. A
useful UI performance probe should:

1. initialize the normal headless application and UI;
2. create the realistic visible layout;
3. establish and assert the expensive precondition;
4. send the real input event;
5. apply host/UI invalidations and run the reactive cycle;
6. build the tiled frame;
7. update or rebuild the retained Metal primitives for every affected tile;
8. assert both the final application state and rendered primitive state.

The timed region must include all synchronous work required for the user-visible
result. If a later mouse move or scroll makes the result appear, the probe ended
too early or redraw work was lost. Do not count intentionally asynchronous GPU
execution unless that latency is the problem being investigated, but do verify
that the frame is submitted or scheduled by the real render-loop owner.

Keep correctness assertions inside the probe. Examples include exact changed
step IDs, expected dirty widget IDs, no missing retained runs, and no selected
step primitives after deselection.

## 3. Establish a trustworthy baseline

Run correctness checks in debug mode, but measure optimized code with `--release`.
Use warmup iterations followed by enough samples to report at least median and
p95; a single measurement is not useful. Keep the fixture, viewport, machine,
and build mode fixed when comparing results.

Run the narrowest exact target. Prefer nextest when it is installed. For
example, the Escape deselection probe is:

```sh
cargo nextest run -p sequencer --release \
  --run-ignored only --no-capture \
  -E 'test(=tests::project_92_escape_clears_48_of_64_selected_steps_end_to_end_perf)'
```

Record the scenario and sample count alongside the timing, not just the final
number.

## 4. Split the latency into phases

Add temporary or durable trace timings around meaningful boundaries, such as:

- input dispatch and Lisp binding;
- state mutation and invalidation creation;
- invalidation application;
- reactive effects and widget-tree flushes;
- layout reuse, subtree relayout, or full relayout;
- tiled frame construction;
- retained primitive refresh or full scene rebuild;
- render-loop wakeup and frame submission.

Also record work counts: changed fields, effects executed, affected buffers,
dirty widgets, relayouts, and primitive runs rebuilt. Counts often expose the
root cause more clearly than elapsed time. Remove noisy ad hoc logging when the
investigation is complete, or turn generally useful data into a compact trace.

## 5. Fix the unnecessary work at its owner

Optimize the earliest architectural cause, not the slow fixture. Typical fixes
include narrowing reactive fan-out, preserving stable widget identity, separating
paint-only changes from size-affecting changes, reusing layout geometry, patching
retained primitives, and preserving dirty work independently for every visible
layout.

Do not improve a benchmark by omitting a visible tile, bypassing the real input
path, suppressing redraw, or moving required work beyond the timed boundary.
Avoid project-specific fast paths; improve the shared ownership or invalidation
model so similar actions benefit automatically.

## 6. Verify correctness and the improvement

After each meaningful change:

- rerun the same release probe and compare median and p95;
- add the narrowest exact regression test for the discovered failure mode;
- assert that inactive visible tiles retain their dirty work across active-tile
  relayouts and unpresented frames;
- use the required headless capture and inspect the PNG if panel structure or
  visual output changed;
- check that focus, modal input, and alternate-buffer behavior still work.

A tuning change is complete only when the exact user action is faster, its visible
result appears without an unrelated event forcing a redraw, targeted regressions
pass, and the probe includes the final render work. Report any remaining excluded
work or uncertainty explicitly.

## Sequencer step-interaction probe

The project-92 probe exercises a 10-track, 64-step sequencer through the real
mouse/key handlers, host history application, UI invalidations, reactive update,
tiled-frame construction, and retained Metal update:

```sh
cargo nextest run -p sequencer --release \
  --run-ignored only --no-capture \
  -E 'test(=tests::project_92_step_interactions_end_to_end_perf)'
```

If nextest is unavailable, the equivalent fallback is:

```sh
cargo test -p sequencer --bin metal_seq \
  tests::project_92_step_interactions_end_to_end_perf \
  --release -- --exact --ignored --nocapture
```

The durable harness lives in `crates/sequencer/src/ui/tests.rs`. It loads saved
project 92, isolates the real `*sequencer*` buffer at its production viewport,
uses track 0 with 64 steps, performs 5 warmup iterations, and then records 20
samples for each action:

- **select-one**: Shift-click step index 8 through the real precise mouse-down
  path, leaving exactly that step selected;
- **cmd-a**: send the real Super+A shortcut with an empty selection and verify
  that all 64 steps are selected;
- **move-16**: select and activate step indexes 8 through 23, arm the production
  pointer-down gesture outside the timed region, then time one real drag tick
  from step 8 to step 9 through history, invalidation, frame, and retained
  update;
- **toggle-drag**: pointer-down on empty step 32 outside the timed region, then
  time one real paint-drag tick onto step 33 and verify that it becomes active.

Compilation and project setup are outside the samples. Each reported duration
starts immediately before the real key/mouse event (or the move/toggle drag
tick) and ends after host commands, targeted UI invalidations, the reactive
cycle, tiled-frame construction, and retained primitive update have completed.
The output also separates host dispatch from visible update time, and prints
invalidation, reactive, frame, and retained phases for the 16-step move.

### Step-interaction reference timings

These are the optimized timings recorded on 2026-07-22 on the developer's
Apple-silicon Mac, using the command and fixture above. Treat them as the
known-good order of magnitude, not a portable threshold across different
machines or thermal/load conditions.

| Action | Pre-tuning median | Enforced 10x ceiling | Good median | Good p95 |
| --- | ---: | ---: | ---: | ---: |
| select-one | 8.251 ms | 0.825 ms | 0.287 ms | 0.355 ms |
| Cmd+A | 7.773 ms | 0.777 ms | 0.359 ms | 0.451 ms |
| move-16 | 106.412 ms | 10.641 ms | 1.216 ms | 1.504 ms |
| toggle-drag | 21.020 ms | 2.102 ms | 0.853 ms | 1.058 ms |

The pre-tuning medians are checked into the harness. The test fails if any
current median exceeds one tenth of its baseline; the “good” columns are a
tighter reference for detecting meaningful regressions before that guardrail is
crossed. Record both median and p95 when rerunning. Compare release builds on the
same machine with similar power, temperature, and background load; never compare
these numbers with a debug build.

The expected output records are:

```text
[project-92-step-select-one] ... median_ms=... p95_ms=...
[project-92-step-cmd-a] ... median_ms=... p95_ms=...
[project-92-step-move-16] ... median_ms=... p95_ms=...
[project-92-step-toggle-drag] ... median_ms=... p95_ms=...
[project-92-step-move-16-visible-phases] invalidation_ms=... reactive_ms=... frame_ms=... retained_ms=...
```

### Arranged step-interaction probe

The arranged variant runs the same actions plus **delete-16** (sixteen selected
active steps cleared through the real Backspace shortcut) with a committed
arrangement at realistic scale (18 scene events, ~18 clips per track) and the
real reactive-tick song/sound-binding syncs inside the timed region:

```sh
cargo nextest run -p sequencer --release \
  --run-ignored only --no-capture \
  -E 'test(=tests::project_92_arranged_step_interactions_end_to_end_perf)'
```

Reference timings (2026-07-28, same machine): delete-16 pre-tuning median
18.969 ms plain / 19.243 ms arranged; post-tuning 1.036 / 1.104 ms. The delete
ceiling is 8x (not 10x) because its post-tuning median sits near the 10x line
under concurrent machine load. The move-16/delete-16 phase records include
`tick_sync_ms` — the per-frame song-state + sound-binding sync the real event
loop runs; at 180 clips it should stay ~0.03 ms.

### Pianohold selection probe

`pianohold_step_selection_end_to_end_perf` loads the real saved `pianohold`
project (7 tracks, take pools, `use_arrangement`, ~137 published clips — the
probe asserts at least 100) and measures Seq-view selection through the real
input paths: Cmd+A (0.32 ms median), a shift-click-drag range tick crossing 8
cells (3.5 ms — the drag-segment walker intentionally dispatches per crossed
cell, ~0.35 ms each; real mouse input delivers ~1 cell per event), a
cmd-click-drag multi-select tick (0.25 ms), and a toggle drag (0.68 ms).

```sh
cargo nextest run -p sequencer --release \
  --run-ignored only --no-capture \
  -E 'test(=tests::pianohold_step_selection_end_to_end_perf)'
```

The important ownership boundaries discovered by this probe are:

- gesture-only Lisp variables must not be `defstate`; making cursor/drag scratch
  state globally reactive flushes unrelated effects on every pointer event;
- sequencer cells bind to per-track/per-step scalar fields, so selection and
  paint updates dirty only the affected retained widgets;
- multi-step mutation uses one batch invalidation and one selection delta rather
  than rebuilding the visible state of every track;
- scheduler publication recaptures the complete edited track and shares immutable
  snapshots for untouched tracks, preserving same-frame step and device-p-lock
  correctness without recapturing the whole project;
- `Vm::eval_str` must move the compiled program into and out of the REPL
  compiler, never clone it: every keyboard shortcut evals a string, so a clone
  there taxes all input in proportion to total UI size;
- an edit that changes only specific steps must invalidate `StepBatch`, never
  `WholeTrack` — the whole-track resync is ~20x the batch cost.

## Arrangement interaction probe

`arrangement_view_interactions_end_to_end_perf` loads
`projects/pianohold.json` (7 tracks, 137 clips, 18 scene events, end beat
708.75), Tab-toggles into the Arr view through the real binding at 180×160,
and times six real-input actions through host commands, song publish, the
reactive cycle, tiled-frame construction, and retained Metal refresh, with
correctness assertions (selection state, ghost channels, in-frame retained
repaint, zero missing/invalid retained runs):

```sh
cargo nextest run -p sequencer --release \
  --run-ignored only --no-capture \
  -E 'test(=tests::arrangement_view_interactions_end_to_end_perf)'
```

### Arrangement reference timings

Recorded 2026-07-28 on the developer's Apple-silicon Mac. Pre-tuning medians
are checked into the harness and the test enforces the 10x ceiling
(`ARR_ENFORCE_TENFOLD`).

| Action | Pre-tuning median | Good median | Speedup |
| --- | ---: | ---: | ---: |
| commit-resize (renderer) | 146.8 ms | 5.7 ms | 25x |
| select-clip | 121.3 ms | 4.8 ms | 25x |
| resize-tick | 66.4 ms | 0.47 ms | 140x |
| move-tick | 62.8 ms | 0.52 ms | 120x |
| marquee-tick | 73.3 ms | 0.53 ms | 139x |
| scroll-tick | 61.0 ms | 1.01 ms | 60x |

The important ownership boundaries discovered by this probe are:

- fast-changing per-lane surfaces (view axis, drag ghosts, selection, region
  rects, cursor) must ride bound widget float channels, never effect-read
  state — one `defstate` write read by every lane rebuilds every lane's item
  list per pointer tick;
- arrangement edit primitives must borrow the scene store
  (`with_project_scenes`) — cloning it puts a full-project copy inside every
  single-clip gesture;
- item-list accumulation in Lisp must be linear (cons + reverse), never
  `append`-per-element, which is quadratic in list length.

## Group/track selection owner-switch probe

`project_92_full_layout_group_track_selection_end_to_end_perf` (eseq-4jv)
measures selecting a group or a track when the *fx* + instrument panels have
to change owner (a track's chain torn down for a group's bus chain and back).
The fixture reproduces the reported topology on top of project 92 through the
real App primitives: 14 tracks — an 8-member group whose members include a
1-slot instrument-rack track (`add_sampler_rack_track`) and a plain sampler
track, five builtin effects on the group's backing bus
(`add_builtin_bus_effect_sync`), and six tracks outside the group — under the
production multi-pane layout (transport, samples sidebar, sequencer,
step/track panels, mixer, *fx*), all groups expanded.

```sh
cargo nextest run -p sequencer --release \
  --run-ignored only --no-capture \
  -E 'test(=tests::project_92_full_layout_group_track_selection_end_to_end_perf)'
```

Four transitions are driven through the real sequencer header clicks
(`handle_tiled_mouse_precise` on the `select-<track>` / `group-select-<id>`
nodes), each with 5 warmups + 20 samples; setup clicks park the current track
on a plain track first so a "group -> track" sample pays the real track
switch the user's gesture pays. The visible update replays the reactive
tick's track-switch rebuild branch, the groups/selected-tracks reconciles,
and the ui/fx-epoch resyncs; `ESEQ_PROBE_BASELINE=1` skips the projection
assertions and ceilings so the probe can measure a pre-fix tree.

### Owner-switch reference timings

Recorded 2026-08-20 on the developer's Apple-silicon Mac (release). The
pre-tuning medians are the pre-eseq-4jv tree; the enforced absolute ceilings
hold the tuned medians with ~1.5x load headroom and sit far below every
pre-tuning median.

| Transition | Pre-tuning median | Tuned median | Ceiling |
| --- | ---: | ---: | ---: |
| group -> instrument-rack track | 145.0 ms | 51.2 ms | 80 ms |
| rack track -> group | 138.7 ms | 45.6 ms | 75 ms |
| group -> sampler track | 170.4 ms | 75.8 ms | 110 ms |
| same-instrument track -> track (reference) | 48.5 ms | 48.7 ms | 75 ms |

The root cause was NOT the *fx* panel teardown itself: the
`eseq.seq-core-state/selected-bus` defstate (the fx-owner discriminant) was
read at render time by every sequencer track row, every hidden arrangement
lane, the sequencer/mixer group blocks (which wrap all member rows), and the
mixer bus strips — so one selection click re-rendered ~30 subtrees
(~12 ms group block, ~7 ms mixer group, ~1-2 ms per row) before the
legitimate owner switch even started. The fix is the `*sel-sync*` projection
in `ui/seq-core-state.lisp`: one inert named effect owns the churn-prone
reads and publishes per-row SEQV float fields
(`sel-track-vis-*`, `sel-group-vis-*`, `sel-bus-vis-*`); rows bind those
fields (`:selected` + `selected-*` color props), so a selection change
dirties only the affected retained widgets. The probe asserts the projection
fields track the selection so the highlight can never silently break.

What remains after the fix is tracked as follow-up beads: the *fx* root's
full re-evaluation on a genuine owner switch rebuilds every builtin panel
(~30 ms for the 5-effect bus chain — keyed subtrees do not survive being
absent from a flush), and every real track switch re-renders the *samples*
sidebar (~35 ms) because `%sync-track-search` runs inside the browser root.

### eseq-4kd: keyed subtrees now survive absence

The first follow-up is fixed in the VM (`eseqlisp::vm`): a top-level effect
rerun now *detaches* the context's keyed subtree owners instead of removing
them, so an absent panel keeps its dag node, its dependency edges (dirtiness
still accrues offscreen), and a memoized copy of its last render. When a
later rerun re-registers the same key with the same body chunk, equal
captured inputs (deep-compared against a snapshot; any captured callable
disqualifies), and no dirty dependency, the cached tree is re-emitted
without invoking the panel body. The cache is dropped on import passes,
symbol redefinition, transactional-eval rollback, and the explicit
`clear_*` teardowns; a standalone subtree rerender invalidates its
ancestors' cached trees so they can never resurrect a stale child.

Post-fix medians (same probe, same machine): rack track -> group fell from
45.6 ms to **11.5 ms** (input phase 42.5 -> ~8 ms — the *fx* bus-chain
rebuild is gone on repeated toggles); the other three transitions are
unchanged within noise. VM-level regression tests:
`keyed_subtree_reused_across_absence_when_dependencies_unchanged` and
`keyed_subtree_rerenders_when_captured_inputs_change` (eseqlisp).
