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

`arrangement_view_interactions_end_to_end_perf` loads the checked-in
`crates/sequencer/tests/fixtures/projects/pianohold.json` fixture (7 tracks,
137 clips, 18 scene events, end beat 708.75), Tab-toggles into the Arr view
through the real binding at 180×160,
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

## Same-instrument track-switch probe (Linux)

`drift_same_instrument_track_switch_end_to_end_perf` (eseq-pgru) measures the
gesture the user complained about: clicking between two tracks that use the
*same* custom instrument, with nothing else changing — no fx owner change, no
group, no relayout that the destination track genuinely needs.

```sh
cargo nextest run -p sequencer --release \
  --run-ignored only --no-capture \
  -E 'test(=tests::drift_same_instrument_track_switch_end_to_end_perf)'
```

### Reproducer topology

The fixture is `crates/sequencer/tests/fixtures/projects/drift-switch.json`,
derived from the reported private project with
`scripts/make_drift_switch_fixture.py`. It is the reported project verbatim
except that every sample reference is replaced by the sentinel
`@PROBE_SAMPLE@`, which the probe rewrites to the checked-in
`content/impulses/prepared/king-tubby.wav` at load time — so the fixture has
no dependency on the author's sample library and the probe runs on any
machine. Nine tracks: two `factory:drums/synthid-808` (0, 1), a sampler (2),
two `factory:core/drift` (3, 4), a `factory:core/triton` (5), three more
samplers (6-8), one six-member group with a rack, one scene,
`use_arrangement`.

Four transitions are driven through the real sequencer header clicks
(`handle_tiled_mouse_precise` on `select-<track>`), 5 warmups + 20 samples
each, under the production multi-pane layout (transport, samples sidebar,
sequencer, step/track panels, mixer, *fx*) at 220x110 cells. Both drift
directions are measured because the acceptance gate is the *slower* of the
two; the two synthid-808 tracks are a same-project comparison that separates
instrument-UI complexity from the shared per-switch cost. Every sample
asserts that the application current track, the `*sel-sync*` highlight
projection, and the `*fx*` tile's instrument bindings all name the
destination track (the probe collects every `SEQ` `track-<n>-instrument-param-*`
`ReactiveRef` in the rendered fx layout and requires the set to be exactly
`{destination}`), and iteration 0 also asserts the fx tile shows a plain
instrument panel with the expected instrument header. After the timed
scenarios the probe switches to the destination drift track and edits one
continuous instrument param through the real `set-instrument-param` host
command, asserting the value lands on the destination instance, does not move
the source instance, and is published on the destination's bound `SEQ` field.

### Machine and build

x86_64 Linux workstation, Intel i5-8250U (4 cores / 8 threads, 1.6 GHz base),
Arch, `--release` with `wgpu`. This is a laptop part: an idle run and a run
with the desktop busy differ by up to ~1.6x, so quote medians from a quiet
machine and never compare these numbers with the Apple-silicon tables above.

### Before / after (2026-08-25)

| Transition | Pre-fix median | Pre-fix p95 | Tuned median | Tuned p95 | Ceiling |
| --- | ---: | ---: | ---: | ---: | ---: |
| drift 3 -> 4 | 235.7 ms | 301.0 ms | 149.2 ms | 180.2 ms | 260 ms |
| drift 4 -> 3 | 243.3 ms | 310.4 ms | 159.3 ms | 204.3 ms | 275 ms |
| synthid 0 -> 1 | 161.4 ms | 226.2 ms | 100.9 ms | 129.9 ms | 185 ms |
| synthid 1 -> 0 | 94.4 ms | 109.5 ms | 72.1 ms | 82.7 ms | 130 ms |

That is 1.58x / 1.53x / 1.60x / 1.31x. It does NOT meet the eseq-pgru goal of
8x; see "What is still slow" below and epic eseq-md1n.

Both the pre-fix and tuned columns are medians from running this probe ALONE
on a quiet machine. Rerun it that way; the numbers are not comparable
otherwise. Measured on this machine with the identical tuned binary,
`drift 4 -> 3` was 159 ms alone, 205 ms sharing one nextest invocation with a
second test, and 251 ms with the desktop also busy — a 1.6x spread with no
code change.

Because of that spread the enforced `DRIFT_SWITCH_CEILINGS_MS` are a coarse
guard, not a proof of the speedup: they clear the worst observed contended run
with margin, so they catch the fix being reverted or something becoming ~1.5x
worse, and nothing finer. Two of them sit above their pre-fix median for that
reason. A quieter CI host, or the VM profiler in eseq-md1n.5, would let this
become a real gate; that is deliberately left open rather than papered over
with a flaky assertion.

`synthid 0 -> 1` is dearer than `1 -> 0` because track 1 also carries a
`builtin:EQ8`, so its fx tile renders an extra effect panel.

### Phase breakdown

Per-effect rerun costs come from the probe's `-reruns` record, which reports
the slowest effect bodies the reactive cycle actually re-ran (from
`UiInvalidationTrace::reactive_exec_timings`). For `drift 4 -> 3`:

| Phase | Pre-fix | Tuned |
| --- | ---: | ---: |
| host dispatch (input) | 1.6 ms | 0.8 ms |
| track-switch publication | 14.3 ms | 13.8 ms |
| epoch resync | 5.9 ms | 5.4 ms |
| reactive cycle | 180.8 ms | 97.1 ms |
| — of which the `*fx*` root rerun | 141.5 ms | 87.5 ms |
| side effects (layout refresh) | 38.1 ms | 38.1 ms |
| frame construction | 1.3 ms | 1.3 ms |
| retained primitive refresh | 2.6 ms | 2.8 ms |

Work counts are unchanged and are the point: one track switch re-runs 5
buffer roots and dirties 10 reactive fields, and `subtree_reruns` stays 0.
The `*fx*` root re-runs because the destination track's panel really is
different content; everything else follows from that tree changing.

### Root cause

The `*fx*` buffer root re-renders the whole custom-instrument UI on every
track switch, and that render cost ~2.2 ms **per rendered control** — Drift
draws ~52 controls, hence ~126 ms. Bisecting the Lisp (each control stubbed
out in turn, medians taken as min-of-N smoke runs) found three pathologies,
all in shared VM/UI machinery rather than anything Drift- or project-specific:

1. **`Vm::current_reactive_value` cloned the whole namespace map to read one
   field.** It went through `global_value(namespace)`, which returns
   `value.borrow().clone()` — and `SEQV` holds one entry per bound widget
   field in the entire UI, so every `reactive-get` allocated tens of thousands
   of `String` keys. Measured at ~0.22 ms per call; custom panels call it
   several times per control. `Runtime::reactive_field_value` already existed
   for the Rust side with exactly this warning in its doc comment; the VM path
   the Lisp natives use had never been given the same treatment. Fixed by
   borrowing the namespace slot and reading one key.
2. **Find-first-by-key was a `filter` over the whole parameter list per
   control.** `inst-param` and its siblings ran
   `(nth (filter |p| (= (get p :name) name) (get inst :synth)) 0)`, and
   `filter` clones every element into the callback, so each of Drift's ~52
   controls deep-touched all 62 parameter maps. Fixed with a new
   `find-by-key` native — `(find-by-key list :key value)` — which scans
   without cloning or invoking a Lisp closure; the eleven hot call sites in
   `content/ui/effects/**` and `content/ui/macros.lisp` now use it.
3. **Every reactive read linearly scanned `global_names`.** Both
   `resolve_global_read_index` and the `LoadReactive` / `LoadReactiveNth` /
   `LoadReactiveLen` opcodes did `global_names.iter().position(...)`, so the
   cost of a reactive read grew with the total amount of loaded Lisp (the
   generated custom-instrument UI alone defines helpers for every installed
   instrument). Fixed with a self-validating name -> index cache: each entry is
   re-checked against the live `global_names` before use and recomputed on a
   miss, so the many paths that replace that Vec wholesale need no
   invalidation hook and a stale entry can only cost one extra scan.

Together these took the `*fx*` root rerun from ~126 ms to ~58 ms on a quiet
machine (~87 ms inside the full 25-iteration probe) and the drift switch from
243 ms to 159 ms.

### What is still slow (does not meet the eseq-pgru 8x goal)

The bead asks for at least 8x on the slower drift direction, i.e. a median
under ~30 ms. The delivered result is 1.5-1.6x. The remaining 159 ms is:
~87 ms `*fx*` root rerun, ~38 ms side-effect layout refresh, ~14 ms
track-switch publication, ~5 ms epoch resync, ~7 ms the other buffer roots,
~4 ms frame + retained.

Even a free fx render would leave ~72 ms, so 8x is not reachable by making the
panel render faster; the panel must stop being re-rendered at all. The
architectural blocker is that the fx panel's parameter descriptors are
*track-addressed*: each control's keyed subtree
(`custom-ui-lego-knob-<scope>-<name>` and friends) closes over the parameter
map `p`, which carries `:value-field` (`track-<n>-instrument-param-...`),
`:value`, `:text-value` and `:mod-targets`, all of which change when the
current track changes. So the eseq-4kd subtree cache can never hit across a
same-instrument switch, the widget tree always differs, and the full relayout
follows. Making those inputs track-invariant (the fx panel only ever shows the
current track, so it could bind "the current track's param N") would let every
control subtree cache, keep the widget tree identical, and remove the relayout
— but it touches p-locks, key locks, mod targets, patch-learn and the rack
panels, which is well beyond this bead.

Two dead ends worth recording so they are not retried:

- Wrapping the whole instrument panel in one keyed subtree
  (`(subtree :key (str "instrument-panel-" (get inst :track) ...))`) makes it
  **worse**: the cache never hits (the captured `inst` differs per track) and
  `store_subtree_render_cache` deep-clones every upvalue on each render, so
  the fx rerun went 126 ms -> 220 ms.
- eseq-tcx (the ~35 ms samples-sidebar rebuild on every track switch) does not
  contribute materially here: on this fixture the `*samples*` root rerun is
  2.0-2.3 ms, because an instrument track shows the presets tab rather than a
  large sample list. It is real but independent, and stays open.

## Expanded step-slider drag probe

`tests::project_92_full_layout_expanded_step_slider_drag_end_to_end_perf`
reproduces the expanded-editor gesture from eseq-z85k. It loads project 92 into
the production seven-pane layout (`*transport*`, `*samples*`, `*sequencer*`,
`*step*`, `*track*`, `*mixer*`, and `*fx*`), expands one track and then three
tracks, presses a velocity vslider, and sends one precise drag event across six
columns. The normal interpolation path produces 49 sub-samples and 51 hit tests
per event. Each configuration uses 5 warmups and 20 measured samples and asserts
all crossed values, both endpoints, the final cursor, and the bound header value.

Run the release probe with:

```sh
cargo nextest run --release -p sequencer --run-ignored all \
  -E 'test(=tests::project_92_full_layout_expanded_step_slider_drag_end_to_end_perf)' \
  --no-capture
```

### Available-host before and after (2026-08-26)

These provisional measurements use an Apple M1 Max (10 cores, 32 GiB), macOS
26.5.1, release profile, a 220x110-cell viewport, and the headless retained-
primitive path. The pre-fix run had load averages 12.97/17.96/15.74. The
post-fix run had elevated concurrent load, 14.29/27.79/31.35, so the result is
deliberately not used as a cross-machine absolute ceiling. The x86_64 Linux
before/after run and the final attribution are in the next section.

| Expanded tracks | Before median / p95 | After median / p95 | Median speedup |
|---|---:|---:|---:|
| 1 | 110.103 / 116.681 ms | 9.514 / 9.867 ms | 11.6x |
| 3 | 145.180 / 150.541 ms | 9.929 / 10.358 ms | 14.6x |

Per-event work changed as follows:

| Expanded tracks | Subtree reruns / roots | Relayout reused / subtree | LayoutNode clones | Interpolation / hit tests |
|---|---:|---:|---:|---:|
| 1 before | 7 / 7 | 7 / 7 | 5,045 | 49 / 51 |
| 1 after | 0 / 0 | 0 / 0 | 197 | 49 / 51 |
| 3 before | 9 / 9 | 7 / 7 | 6,317 | 49 / 51 |
| 3 after | 0 / 0 | 0 / 0 | 197 | 49 / 51 |

Full-buffer reruns and full relayouts were zero both before and after. The fixed
interpolation and hit-test counts prove the improvement did not drop or defer
pointer samples. Clone counts collapsed because the removed subtree rerenders
no longer replace large layout branches; no hit-test behavior was changed.

### x86_64 Linux before and after (2026-08-26, eseq-z85k.2)

Machine `omarchy`: Intel Core i5-8250U (4 cores / 8 threads, 1.6 GHz base,
3.4 GHz max), 7.7 GiB RAM, Omarchy on kernel 6.19.8-arch1-3-surface, rustc
1.98.0, release profile, 220x110-cell viewport, headless retained-primitive
path. The probe asserted the full production pane set in every run —
`*transport*`, `*samples*`, `*sequencer*`, `*step*`, `*track*`, `*mixer*`,
`*fx*`, with `*arrangement*` hidden.

"Before" is the checked-in probe at `1b1ee16b` in a detached worktree; "after"
is the same probe at `3260d893`. Each configuration used 5 warmups and 20
measured samples. This laptop is shared with other work, and the probe's own
audio graph takes roughly 80% of process CPU while the UI thread runs (see the
profile below), so the 1-minute load average was 8.17 entering the before run
and 5.74 entering the after run. Repeats spread the before 1-track median over
347-484 ms and the after 1-track median over 33-41 ms. Read these as this
machine's regime, not as a cross-machine ceiling.

| Expanded tracks | Before median / p95 | After median / p95 | Median speedup |
|---|---:|---:|---:|
| 1 | 376.826 / 478.348 ms | 41.482 / 45.255 ms | 9.1x |
| 3 | 457.304 / 622.519 ms | 43.039 / 53.196 ms | 10.6x |

Per-event medians by phase (each percentile is taken independently, so the
columns do not sum exactly to the total):

| Expanded tracks | Input dispatch | Host commands | Reactive + frame |
|---|---:|---:|---:|
| 1 before | 278.709 ms | 37.907 ms | ~60 ms |
| 1 after | 5.094 ms | 32.594 ms | ~4 ms |
| 3 before | 304.205 ms | 40.855 ms | ~112 ms |
| 3 after | 5.533 ms | 34.222 ms | ~3 ms |

Per-event work counts are identical to the macOS run, before and after:

| Expanded tracks | Subtree reruns / roots | Relayout reused / subtree | LayoutNode clones | Interpolation / hit tests |
|---|---:|---:|---:|---:|
| 1 before | 7 / 7 | 7 / 7 | 5,045 | 49 / 51 |
| 1 after | 0 / 0 | 0 / 0 | 197 | 49 / 51 |
| 3 before | 9 / 9 | 7 / 7 | 6,317 | 49 / 51 |
| 3 after | 0 / 0 | 0 / 0 | 197 | 49 / 51 |

The fan-out is therefore platform independent; Linux only multiplies the
constant by roughly 3.4x.

### Linux traces and CPU attribution (eseq-z85k.2)

`ESEQLISP_TRACE_UI=1` with
`ESEQLISP_TRACE_UI_FILTER=SEQ.velocities,SEQ.track-velocities` prints 49
reactive cycles across the before sweep, one per drag event, each of the form

```
[ui-trace] dirty=[SEQ.velocities x7,SEQ.track-velocities] affected=[*sequencer*]
  reruns=full:0 sub:3 roots:3 patches:3 apply_ms=52-83 flush_ms=38-55
  relayout=subtree-reuse relayout_ms=26-39
  hot=[*sequencer*|target:*sequencer*|root:<keyed track root>:17-30ms x3]
```

The same filter over the after sweep prints **zero** such cycles. That directly
confirms the hypothesized expanded-track subtree fan-out and its removal.

CPU profile method. `perf` was not installed when this run started (Arch
`extra/perf`, group `linux-tools`, root required); it was installed partway
through the session, and `kernel.perf_event_paranoid=2` restricts unprivileged
sampling to user space (`cpu/cycles/Pu`), which is all this attribution needs.
The release profile carries no frame pointers, so `perf --call-graph dwarf,8192`
truncated stacks to 1-4 frames and could not attribute a call tree; its flat
per-thread self time was still usable. The call-tree numbers below come from an
in-process SIGPROF sampler (pprof 0.15 at 997 Hz, 1024-frame depth) started
inside the test process around a replay of the same gesture with the probe's
assertions removed — a launch-time in-process profiler, not an attach
workaround. Both the sampler and the replay pass are diagnostic scaffolding and
are deliberately not checked in; timings above come from the uninstrumented
build. Thread names are inherited on Linux, so samples are attributed by
anchoring on the probe's own frame rather than by thread name.

Before, 1 expanded track (2,552 UI samples out of 12,489 process samples — the
other ~80% is the project's own audio graph, `worker_main` / `dgen_process_v1`):

| Path | Share of UI CPU |
|---|---:|
| `handle_tiled_mouse_precise` -> `try_handle_widget_drag_segment` -> `dispatch_slider_drag_to_node` -> `apply_widget_output` | 82% |
| ... of which `process_dirty_reactive` | 43% |
| ... of which relayout | 30% |
| ... of which Lisp compilation | 15% |
| `hit_test_layout` / `widget_node_at_local` | 0.8% |
| `dispatch_custom_host_command` | 4.7% |
| tiled-frame build + retained primitive refresh | 0.9% |

Three expanded tracks are the same shape: drag path 68%, reactive 42%, relayout
35%, hit tests 0.4%, host commands 4.1%.

After, both configurations (154 UI samples each):

| Path | Share of UI CPU |
|---|---:|
| `dispatch_custom_host_command` -> `apply_recorded_step_mutation` | 60% |
| ... of which `replay_step_patch` / `replay_step_patch_cells` | 42-46% |
| ... of which `publish_scheduler_track` -> `SequencerSnapshot::capture_live_track` | 33-34% |
| `try_handle_widget_drag_segment` (interpolation + hit tests) | 23-25% |
| ... of which `hit_test_layout` | 7-10% |
| `process_dirty_reactive` / relayout | 0% |

### Attribution verdict (eseq-z85k H1/H2/H3)

- **H1 — a non-reactive value read forcing whole-subtree re-evaluation:
  confirmed, and the dominant owner.** It held 82% of before UI CPU at one
  expanded track and 68% at three, and removing it accounts for essentially all
  of the 9.1x-10.6x median improvement. Already fixed in `78ad4c8c`; Linux only
  confirms it.
- **H2 — an O(distance x tree) drag hit-test path with deep clones: refuted as
  an independent owner.** Hit-testing is 0.8% of before UI CPU. The 5,045 /
  6,317 LayoutNode clones were a consequence of H1's subtree rerun and relayout,
  not of hit-testing: interpolation sub-samples (49) and hit tests (51) are
  unchanged before and after, yet clones fell to 197. Post-fix hit-testing is
  7-10% of a ~4.8 ms input phase, roughly 0.4 ms per event.
- **H3 — redundant per-tick selection work: refuted.** No selection-sync frames
  appear in either profile, and no selection field is dirtied during the sweep.
- **H4 — per-interpolated-write step-history replay and scheduler republish: a
  new owner, visible only on Linux.** Post-fix the residual is no longer the
  input path (~5 ms) but host-command application, 32.6 ms at one expanded track
  and 34.2 ms at three — 79% of the post-fix median. Each drag event dispatches
  seven `set-step-param-history` commands, and each one runs
  `apply_recorded_step_mutation` (capture-before, mutate, `replay_step_patch`)
  and republishes a complete scheduler track snapshot. On the M1 Max this phase
  was ~9 ms and hid inside an already fast frame. Tracked as `eseq-z85k.3`.

### Root cause and fix

The expanded step columns already consumed float slots from
`ExpandedStepProjectionRegistry`, but the sibling quick-control header read the
cursor value directly from `SEQ.velocities` / `SEQ.track-velocities` (and mode 5
read `SEQ.syncs`) while rendering the keyed track subtree. Every interpolated
slider write therefore dirtied the expanded track subtree, rebuilt its Lisp
widget tree, and relaid it out. With three expanded tracks the plain list write
also increased fan-out.

The projection now publishes the active mode's raw cursor value and a numeric
sync-label index. The header number-picker binds its `:value` to the former; the
mode-5 dropdown binds `:value-index` to the latter with static label options.
Cursor, mode, full-viewport, and incremental step writes synchronize those
fields alongside the existing slot projection. The post-fix zero rerun and
relayout counts identify this incomplete projection boundary as the owner; the
input interpolation and hit-test path was not changed speculatively.
