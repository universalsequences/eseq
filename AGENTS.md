# AGENTS.md

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
cargo test -p sequencer scheduler::tests::scheduler_lookahead_routes_lisp_graph_seed_and_propagation_through_midi_fx -- --nocapture
```

This test covers the important route:

```text
step seed -> Lisp graph sequencer -> target track MIDI FX -> routed target event with target track params
```

If scheduler-side Lisp/project scratch runtime loading changes, also run:

```sh
cargo test -p sequencer scheduler::tests::scheduler_runtime_keeps_builtin_midi_fx_when_project_scratch_fails -- --nocapture
```

That regression protects the case where project scratch source, such as a graph sequencer demo, fails in the scheduler VM but builtin MIDI FX like `arp` and `trigger-to-track` must remain registered.

## UI/layout testing

When changing Lisp UI structure or widget wrapper argument order, do not stop at parse/render-tree tests. Add or run a layout test that proves expected text/debug nodes have finite, nonzero measured rects inside the visible panel; regressions often look like "the panel exists, but its children measured to zero or disappeared."

When adding reactive props to a widget, update that widget's `bindable_props` contract. `build_widget` rejects unsupported reactive bindings by returning a string diagnostic instead of a widget map; if that rejected widget is passed through a wrapper, the wrapper may render as an empty measured container with no children. Add a regression test that passes the new prop as a `ReactiveRef`, not only as a literal number.

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

When changing animation cadence, redraw scheduling, frame pacing, or FPS diagnostics, first identify which binary owns the active event/render loop. Shared backends such as `crates/eseqlisp/src/ui/metal_backend.rs` perform drawing, but app binaries may decide when drawing happens. In particular, `metal_seq` owns its render loop in `crates/sequencer/src/bin/metal_seq/main.rs`; changes to `eseqlisp::run_metal` do not affect `metal_seq`. If a backend log appears but loop-level logs or frame pacing changes do not, check for a binary-specific loop before continuing.

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
