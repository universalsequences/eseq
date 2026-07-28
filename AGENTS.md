# AGENTS.md

## Test selection and runtime policy

Prefer `cargo nextest run` over `cargo test` when nextest is installed (`brew
install cargo-nextest`; check with `cargo nextest --version`). It runs each
test in its own process, which isolates tests that touch shared global state
(e.g. the shared `$TMPDIR/sequencer_dgenlisp` compile output dir), reports
per-test wall times, and schedules the suite better. Fall back to `cargo test`
if nextest is unavailable.

Use the narrowest test target that validates the behavior changed. Do not run a
full package or workspace test suite as a default validation or finishing step.

For a unit test in a library, select the exact test:

```sh
cargo nextest run -p <package> -E 'test(=<fully-qualified-test-name>)'
# or: cargo test -p <package> --lib <fully-qualified-test-name> -- --exact
```

For an integration test, select the integration-test binary and the exact test:

```sh
cargo nextest run -p <package> -E 'binary(<test-target>) and test(=<test-name>)'
# or: cargo test -p <package> --test <test-target> <test-name> -- --exact
```

nextest notes: use `--no-capture` where a `cargo test` command would use
`-- --nocapture`; `-E 'test(/regex/)'` selects by regex; nextest does not run
doctests (this repo's tests are all unit/integration tests, so that does not
matter here).

Do not run any of the following unless the user explicitly requests exhaustive
testing, or the change is genuinely cross-cutting and no narrower validation
exists:

```sh
cargo test
cargo test --workspace
cargo test -p sequencer
```

Before starting an exhaustive suite, tell the user which suite will run and why.
Full package and workspace suites should normally be left to CI. Do not infer
that a full suite is required merely because a change touches the `sequencer`
package or because the task asks you to "run tests."

If a targeted test runs for more than three minutes without producing progress,
stop it and investigate the delay. Report the command and the phase that stalled
if the cause cannot be resolved. Do not respond by running a broader suite.

If a broader run does surface a failing test you did not touch, do not assume
you broke it and do not start bisecting with stashes. First check whether the
failing test's subject overlaps your diff at all; if it does not, verify the
failure pre-exists by running that one test in a temporary `git worktree` of
HEAD (see "Working tree safety"), then report it as pre-existing and move on.

Known pre-existing failures (July 2026, do not re-investigate; unrelated to
effects/DSP work):

- `tui::effects::tests::bus_effect_wiring_resolves_graph_nodes_by_bus_id_after_reordering`
- `tui::graph::tests::rack_rebuild_defers_old_sampler_nodes_until_forced_reap`
- `tui::graph::tests::replacing_expanded_rack_instrument_preserves_slot_fx_and_defers_old_engine`

## Working tree safety

The author works in this repository concurrently while agents run: editors
auto-save files mid-task, and uncommitted work is often present. Therefore:

- NEVER run `git stash` (or `git checkout`/`git restore` over files you did not
  modify). A stash taken from a tree the author is editing will conflict on pop
  and can strand both your changes and theirs.
- To compare against a clean HEAD (pre-existing failures, baselines, bisects),
  use an isolated checkout instead: `git worktree add /tmp/<name> HEAD`, run
  there, then `git worktree remove /tmp/<name>`.
- Before any operation that rewrites working-tree files, re-check `git status`
  — the set of dirty files may have changed since the task started.

## Formatting

Do not run `cargo fmt` on a package or the workspace. The repository's existing
style does not match stock rustfmt defaults, so a package-wide format produces
hundreds of lines of import-reordering and rewrapping churn across files you
did not touch. Note that `cargo fmt -p <package> -- <files>` does NOT limit
formatting to those files — it still formats the whole package. Match the
surrounding code's style by hand; if you must use rustfmt, invoke the `rustfmt`
binary directly on only the files you created or modified.

## Instrument testing

Use `instrument_probe` when changing DGenLisp instruments, wavetable/tensor loading, or host-side instrument initialization. It exercises the same host compile/load/init path as the app and gives quick signal checks without launching the UI.

Example:

```sh
cargo run --bin instrument_probe -- emulations/monomachine-dpro-wave-v2 \
  --frames 4096 \
  --min-peak 0.01 \
  --min-rms 0.001
```

For saved instrument names, the probe resolves local assets from the saved instrument directory. For direct file paths, it resolves assets from the source file's parent directory. Use `--param name=value` for parameter overrides and `--json` for machine-readable output.

## Sequencer scheduler routing testing

When changing scheduler trigger routing, MIDI FX routing, graph-mode `def-sequencer` playback, or pattern-parameter application for neural/graph generated events, use the deterministic scheduler lookahead harness instead of loading a whole project or relying only on UI/audio behavior. The harness drives the extracted production scheduler lookahead pass directly, without instruments, DSP, project load, or UI:

```sh
cargo test -p sequencer --lib \
  scheduler::tests::scheduler_lookahead_routes_lisp_graph_seed_and_propagation_through_midi_fx \
  -- --exact --nocapture
```

This test covers the important route:

```text
step seed -> Lisp graph sequencer -> target track MIDI FX -> routed target event with target track params
```

If scheduler-side Lisp/project scratch runtime loading changes, also run:

```sh
cargo test -p sequencer --lib \
  scheduler::tests::scheduler_runtime_keeps_builtin_midi_fx_when_project_scratch_fails \
  -- --exact --nocapture
```

That regression protects the case where project scratch source, such as a graph sequencer demo, fails in the scheduler VM but builtin MIDI FX like `arp` and `trigger-to-track` must remain registered.

## UI/layout testing

When changing Lisp UI structure or widget wrapper argument order, do not stop at parse/render-tree tests. Add or run a layout test that proves expected text/debug nodes have finite, nonzero measured rects inside the visible panel; regressions often look like "the panel exists, but its children measured to zero or disappeared."

When adding reactive props to a widget, update that widget's `bindable_props` contract. `build_widget` rejects unsupported reactive bindings by returning a string diagnostic instead of a widget map; if that rejected widget is passed through a wrapper, the wrapper may render as an empty measured container with no children. Add a regression test that passes the new prop as a `ReactiveRef`, not only as a literal number.

### Sequencer panel visual capture

When changing a `metal_seq` panel whose contents depend on project state (tracks, instruments, racks, processes, MIDI FX, or audio FX), use the headless sequencer capture command. Do not substitute a standalone `eseqlisp_capture` expression for this check: `metal_seq capture` creates a real headless sequencer project, evaluates the normal authoring Lisp, synchronizes the resulting state into `SEQ`, isolates the requested buffer, and renders it through the production Metal widget path without opening the interactive app or an audio device.

Capture the process/instrument strip with the checked-in process fixture:

```sh
cargo run -p sequencer --bin metal_seq -- capture \
  --script crates/sequencer/ui/capture-fixtures/process-panel.lisp \
  --buffer fx \
  --track 0 \
  --width 2000 \
  --height 420 \
  --out /tmp/metal-seq-process-panel.png
```

Capture a real saved custom instrument UI with:

```sh
cargo run -p sequencer --bin metal_seq -- capture \
  --script crates/sequencer/ui/capture-fixtures/instrument-panel.lisp \
  --buffer fx \
  --track 0 \
  --width 1800 \
  --height 600 \
  --out /tmp/metal-seq-instrument-panel.png
```

Open and inspect the PNG before claiming visual correctness. Layout assertions remain required for finite/nonzero geometry; the PNG check covers typography, clipping, spacing, hierarchy, and overall composition that geometry tests cannot judge.

Capture scripts contain exactly one declarative project form, followed by ordinary sequencer Lisp:

```lisp
(capture-project
  (track :sampler :name "Sampler"))

(load "../../scripts/processes/process-inlet-patch-demo.lisp")
(process-inlet-demo-attach-track 0)

;; Optional: runs after project/process state has populated SEQ.
(def capture-after-sync ()
  (process-panel-select-slot (nth SEQ.process-slots 0)))
```

Supported track kinds are `:sampler`, `:instrument`, `:modulator`, `:drum-rack`, and `:layer-rack`; tracks may also declare `:midi-fx` and built-in `:audio-fx`. Use `capture-after-sync` for UI state that depends on populated reactive data, such as selecting a process row or opening an instrument tab. Add durable fixtures under `crates/sequencer/ui/capture-fixtures/`. Full usage is documented in `docs/metal-seq-ui-capture.md`. The capture path is macOS-only because it uses Metal.

### Patcher visual capture

When changing the patcher renderer, cable geometry, node layout, ports, or other visual behavior, use the macOS Metal capture test to generate a PNG for inspection:

```sh
cargo test -p eseqlisp --test capture capture_patcher_lexilush_png -- --ignored --nocapture
```

The test writes `/tmp/eseqlisp-patcher-lexilush.png`. Open that image and inspect it before claiming a visual change is correct. The fixture renders:

```lisp
(effect
  (patcher
    :intent :effect
    :path "crates/sequencer/effects/lexilush/dsp.lisp"))
```

For custom one-off captures, run `eseqlisp_capture` directly:

```sh
cargo run -p eseqlisp --bin eseqlisp_capture -- \
  --source '(effect (patcher :intent :effect :path "crates/sequencer/effects/lexilush/dsp.lisp"))' \
  --width 2050 \
  --height 1218 \
  --out /tmp/eseqlisp-patcher.png
```

## Render loop ownership

When changing animation cadence, redraw scheduling, frame pacing, or FPS diagnostics, first identify which binary owns the active event/render loop. Shared backends such as `crates/eseqlisp/src/ui/metal_backend.rs` perform drawing, but app binaries may decide when drawing happens. In particular, `metal_seq` owns its render loop in `crates/sequencer/src/ui/main.rs`; changes to `eseqlisp::run_metal` do not affect `metal_seq`. If a backend log appears but loop-level logs or frame pacing changes do not, check for a binary-specific loop before continuing.

>> EXTREMELY IMPORTANT <<<
NO HACKS. The user is EXTREMELY concerned about code quality, much more so than immediate results. If they ask you to build something and, while doing so, you hit a wall, and realize that the only way to ship the requested feature is to
IMMEDIATELLY. Either fix the underlying flaw that blocked you in a ROBUST, WELL
DESIGNED, PRODUCTION READY manner, or be honest that the prompt can't be completed without hacks.
To make it very clear:
- DO NOT INTRODUCE HACKS IN THE CODEBASE.
- DO NOT COMMIT CODE THAT COULD BREAK THINGS LATER.
- DO NOT COMMIT PARTIAL SOLUTIONS OR WORKAROUNDS.
THIS IS VERY IMPORTANT.
THIS IS VERY IMPORTANT.
THIS IS VERY IMPORTANT.
The author appreciates honestly and he WILL be glad and thankful if you respond
a request with "I couldn't complete your request because the repository lacked support for X". He will be even happier if you go ahead and update the repo to provide the necessary support in a well designed, robust way. But he will be VERY ANGRY if, while attempting to implement a feature, you introduce a workaround that will potentially break things later.
NEVER introduce hacks in the codebase.
Also assume that none of the code you're working in is in production, so, backwards compatibility is NOT IMPORTANT. If you find something that is poorly designed and fixing it would require breaking existing APIs or behavior, DO SO.
Do it properly rather than preserving a flawed design. Prioritize clarity,  correctness, and maintainability over compatibility with existing code.
Core values:
- ABSOLUTE code quality over speed of delivery.
- Correctness over convenience.
- Clarity over cleverness.
- Maintainability over short-term productivity.
- Robust design over quick fixes.
- Simplicity over complexity.
- Doing it right over doing it now.
- Honesty above everything.
-  After every change you make, provide a clear, honest report on ANY change that you are not confident about and that could be considered a fragile hack.
