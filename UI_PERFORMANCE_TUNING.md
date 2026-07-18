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

Run the narrowest exact target. For example, the Escape deselection probe is:

```sh
cargo test -p sequencer --bin metal_seq \
  tests::project_92_escape_clears_48_of_64_selected_steps_end_to_end_perf \
  --release -- --exact --ignored --nocapture
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
