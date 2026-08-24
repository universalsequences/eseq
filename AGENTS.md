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

### DGenLisp compiler (fetched, not tracked)

The DGenLisp compiler binary is not in git. `content/dgenlisp.lock` pins the
published distribution per target; run `./scripts/fetch_dgenlisp.sh` once per
fresh checkout (idempotent, sha256-verified) to install it under
`crates/sequencer/tools/` (gitignored). Anything that needs the compiler and
cannot find it hard-fails naming that command. `ESEQ_DGENLISP_TOOL=/abs/path`
overrides it with a locally built compiler.

### Cheap clean-HEAD check

Do not stash and do not cold-clone the repository to determine whether one test
fails at HEAD. Reuse an isolated worktree and a dedicated target directory:

```sh
wt=/tmp/eseq-head-test; target=/tmp/eseq-head-test-target
[ -e "$wt/.git" ] || git worktree add --detach "$wt" HEAD
git -C "$wt" checkout --detach HEAD
(cd "$wt" && \
  CARGO_TARGET_DIR="$target" \
  cargo nextest run -p <package> -E 'test(=<fully-qualified-test-name>)')
```

The dedicated target directory keeps Cargo artifacts from the clean checkout
separate from working-checkout artifacts; sharing a target directory between
worktrees can make Cargo run a binary built from the wrong source tree. The
worktree is disposable and isolated, so resetting it never touches the working
checkout. Remove it with `git worktree remove /tmp/eseq-head-test` when it is no
longer useful; remove `/tmp/eseq-head-test-target` too if its cached artifacts
are no longer needed.

### Test stack budget

`.cargo/config.toml` applies one 16 MiB `RUST_MIN_STACK` budget automatically to
Cargo-launched test processes. The same number is
`sequencer::REQUIRED_THREAD_STACK_SIZE` for explicitly spawned scheduler/UI test
threads. Do not add local 32/64 MiB literals or rely on a remembered shell
prefix.

LLDB investigation for eseq-4tl found that debug overflows while loading the UI
run through recursive `Expression::clone` and then
`Compiler::compile_expression -> compile_list -> compile_if_statement /
compile_let_statement / compile_function`. Expression cloning is now iterative.
Compiler traversal is still proportional to authored Lisp nesting and is
tracked as `eseq-4tl.1`; release builds load the checked-in UI on a normal stack,
but adversarially deep user source remains a production crash risk until that
bead is complete. With the configured budget, any remaining overflow is
isolated by nextest and reported as the named test rather than aborting a shared
test binary.

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

Known pre-existing failures: **none**.

As of 2026-08-20 both full workspace profiles are green: debug has 4,272 passed
and 32 skipped; release has 4,270 passed and 32 skipped (the two-test difference
is intentional `cfg(debug_assertions)` coverage). Commands and timings are in
`docs/test-suite-performance.md`. If a broader run fails, treat it as a real
signal — verify against the reusable clean-HEAD worktree described above, and if
a failure genuinely pre-exists your change, update this list with the test name
and evidence rather than leaving it undocumented.

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

Do not assert on cosmetic UI copy — header/title strings, decorative labels, button captions ("MAGNITUDE TABLE"-style section titles). The author tweaks frontend text and layout freely, and those tests break without protecting any behavior. Layout tests should assert *functional* structure instead: parameter labels that drive controls, widget types (dropdown, knob, viewer), prop bindings, and finite nonzero rects. If a test only fails when someone rewords or deletes a label, it should not exist.

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
    :path "content/effects/lexilush/dsp.lisp"))
```

For custom one-off captures, run `eseqlisp_capture` directly:

```sh
cargo run -p eseqlisp --bin eseqlisp_capture -- \
  --source '(effect (patcher :intent :effect :path "content/effects/lexilush/dsp.lisp"))' \
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

<!-- BEGIN BEADS INTEGRATION v:1 profile:minimal hash:46cd31e7 -->
## Beads Issue Tracker

This project uses **bd (beads)** for issue tracking. Run `bd prime` to see full workflow context and commands.

### Quick Reference

```bash
bd ready              # Find available work
bd show <id>          # View issue details
bd update <id> --claim  # Claim work
bd close <id>         # Complete work
```

### Rules

- Use `bd` for ALL task tracking — do NOT use TodoWrite, TaskCreate, or markdown TODO lists
- Run `bd prime` for detailed command reference and session close protocol
- Use `bd remember` for persistent knowledge — do NOT use MEMORY.md files

**Architecture in one line:** issues live in a local Dolt DB; sync uses `refs/dolt/data` on your git remote; `.beads/issues.jsonl` is a passive export. See https://github.com/gastownhall/beads/blob/main/docs/core-concepts/sync-concepts.md for details and anti-patterns.

## Agent Context Profiles

The managed Beads block is task-tracking guidance, not permission to override repository, user, or orchestrator instructions.

- **Conservative (default)**: Use `bd` for task tracking. Do not run git commits, git pushes, or Dolt remote sync unless explicitly asked. At handoff, report changed files, validation, and suggested next commands.
- **Minimal**: Keep tool instruction files as pointers to `bd prime`; use the same conservative git policy unless active instructions say otherwise.
- **Team-maintainer**: Only when the repository explicitly opts in, agents may close beads, run quality gates, commit, and push as part of session close. A current "do not commit" or "do not push" instruction still wins.

## Session Completion

This protocol applies when ending a Beads implementation workflow. It is subordinate to explicit user, repository, and orchestrator instructions.

1. **File issues for remaining work** - Create beads for anything that needs follow-up
2. **Run quality gates** (if code changed) - Tests, linters, builds
3. **Update issue status** - Close finished work, update in-progress items
4. **Handle git/sync by active profile**:
   ```bash
   # Conservative/minimal/default: report status and proposed commands; wait for approval.
   git status

   # Team-maintainer opt-in only, unless current instructions forbid it:
   git pull --rebase
   bd dolt push
   git push
   git status
   ```
5. **Hand off** - Summarize changes, validation, issue status, and any blocked sync/commit/push step

**Critical rules:**
- Explicit user or orchestrator instructions override this Beads block.
- Do not commit or push without clear authority from the active profile or the current user request.
- If a required sync or push is blocked, stop and report the exact command and error.
<!-- END BEADS INTEGRATION -->

<!-- BEGIN BEADS CODEX SETUP: generated by bd setup codex -->
## Beads Issue Tracker

Use Beads (`bd`) for durable task tracking in repositories that include it. Use the `beads` skill at `.agents/skills/beads/SKILL.md` (project install) or `~/.agents/skills/beads/SKILL.md` (global install) for Beads workflow guidance, then use the `bd` CLI for issue operations.

### Quick Reference

```bash
bd ready                # Find available work
bd show <id>            # View issue details
bd update <id> --claim  # Claim work
bd close <id>           # Complete work
bd prime                # Refresh Beads context
```

### Rules

- Use `bd` for all task tracking; do not create markdown TODO lists.
- Run `bd prime` when Beads context is missing or stale. Codex 0.129.0+ can load Beads context automatically through native hooks; use `/hooks` to inspect or toggle them.
- Keep persistent project memory in Beads via `bd remember`; do not create ad hoc memory files.

**Architecture in one line:** issues live in a local Dolt DB; sync uses `refs/dolt/data` on your git remote; `.beads/issues.jsonl` is a passive export. See https://github.com/gastownhall/beads/blob/main/docs/core-concepts/sync-concepts.md for details and anti-patterns.
<!-- END BEADS CODEX SETUP -->
