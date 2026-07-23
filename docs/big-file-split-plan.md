# Big-File Split & Reorganization Plan

Targets (line counts as of 2026-07-22):

| File | Lines | Inline `mod tests` | Production code |
|---|---|---|---|
| `ui/state_values.rs` | 52,648 | ~39,820 (76%) | ~12,760 |
| `lisp_host.rs` | 26,861 | ~11,692 (43%) | ~15,169 |
| `ui/main.rs` | 24,433 | ~3,748 | ~20,685 (15.5k of it is `fn main()`) |
| `sequencer/state.rs` | 17,266 | ~5,243 (30%) | ~12,023 |
| `tui/graph.rs` | 11,575 | ~3,354 (29%) | ~8,221 |

**Headline: ~64k of these ~133k lines (~48%) is inline test code.** The single
highest-leverage, lowest-risk move is relocating tests into sibling
`tests.rs` submodules — no production logic moves at all.

All splits below are mechanical (move items, fix `use` paths, add re-exports)
**except** the `ui/main.rs` event-loop split, which needs a context struct
first. Every split preserves existing import paths via `pub use` re-exports in
the new `mod.rs`, so callers don't change.

Ground rules for execution:
- One file per PR/commit; run `cargo check` + the file's scoped tests after each.
- Do each split only when that file has no uncommitted local edits (user edits
  concurrently; never stash).
- Phase 1 (test extraction) can be done per-file independently and first.

---

## Phase 1 — Test extraction (pure mechanical, biggest win)

For each file: move the `#[cfg(test)] mod tests` block into a sibling file,
keep `use super::*;` working via the module tree. This alone takes
`state_values.rs` 52.6k → 12.8k and `lisp_host.rs` 26.9k → 15.2k.

Gotchas found by analysis:
- `state_values.rs` has **two production functions stranded AFTER the test
  module's closing brace** (`auto_follow_enabled`, `poll_pending_compile_status`,
  lines 52,583–52,648). They belong with `host_commands.rs`. Any line-range
  script must special-case this.
- Test modules are flat with shared local fixtures (`test_app`,
  `history_test_app`, `step_gesture_runtime`, etc.). Extract each `mod tests`
  **verbatim as one file first**; splitting tests by topic is a separate later
  pass (per-test triage, higher risk).
- `tui/graph.rs` has `#[cfg(test)]` production hooks (failure-injection fns,
  lines ~498–586) that are compiled into `GraphNodeBuildTransaction` — they stay
  with production code, NOT the test file.
- `lisp_host.rs` `ScratchControlRuntime` has `#[cfg(test)]` fields/branches
  woven into production functions — move with the struct.

## Phase 2 — Delete the legacy ratatui frontend, rename `tui/` → `app/`

`tui/` today is the **application-state and command core** (`App`,
`AudioBuses`, `EditorState`, `AppCommand`, `edit.rs`, `history.rs`, and the
`GraphController` seam) that the GPU `ui/` binary drives. A ratatui/crossterm
terminal path still compiles: `src/main.rs` (the `sequencer` bin),
`tui/draw.rs`, `tui/effects_draw.rs`, and ratatui references in ~10 files.

**Decision (confirmed 2026-07-22): the baked-in ratatui frontend is dead
legacy and gets deleted, not quarantined.** Terminal rendering is eseqlisp's
job — the eseqlisp project has its own terminal render target, so nothing in
the sequencer crate needs ratatui.

Steps:
1. **Delete the terminal frontend**: remove `src/main.rs`'s ratatui event
   loop / the `sequencer` bin's terminal mode, `tui/draw.rs`,
   `tui/effects_draw.rs`, and the ratatui-only halves of mixed files
   (`params.rs`, `cirklon.rs`, `projects.rs`, `browser.rs`, `synth.rs`,
   `recording.rs`, `effect_params.rs`, `effects.rs`, `mod.rs` — audit each
   with `grep -n "ratatui\|crossterm"` and cut only the rendering/input-
   translation code, keeping the state types those files also define).
   Note `lisp_host.rs` also touches crossterm (`RestoreTerminalGuard`,
   embedded editor session raw-mode handling) — verify whether the embedded
   editor flow still needs crossterm before removing the dependency, or keep
   crossterm only for that and drop ratatui outright.
2. Drop `ratatui`/`crossterm` from `crates/sequencer/Cargo.toml` (subject to
   the embedded-editor check above).
3. Rename `tui/` → `app/`: `mod tui;` → `mod app;` + crate-wide
   `crate::tui::` → `crate::app::` rewrite (~15 importing files). Mechanical.

Do the deletion (step 1–2) before the graph.rs directory split — several
mixed `tui/` files shrink substantially, and the rename means graph.rs lands
at `app/graph/` once instead of moving twice.

## Phase 3 — Per-file directory splits

### 3a. `sequencer/state.rs` → `sequencer/state/`

Structure: types + a 7,585-line `impl SequencerState` (241 methods). Rust
allows multiple `impl` blocks across files, so split by domain:

- `mod.rs` — re-exports (keeps `sequencer.rs`'s existing `pub use state::{...}` working)
- `ids.rs`, `track_registry.rs`, `rack_macro.rs`, `rack_slot.rs`,
  `bus_pattern.rs`, `step_snapshot.rs` — data types
- `track_pattern_data.rs` (~800), `scenes.rs` (~700, `ProjectScenes`),
  `pattern_snapshot.rs` (~900), `track_delete_remap.rs`
- `core.rs` — `SequencerState` struct def, transport/runtime structs
- `sequencer_state/` — the impl split by domain: `accessors`, `instrument_reset`,
  `rack_editing` (~1.3k), `effect_propagation`, `repository_edit`, `publish`,
  `topology`, `process_chain` (~800), `transport`, `scene_launch` (~660),
  `step_edit` (~2.4k — keep `capture_step_snapshot` + its restore counterpart
  together as a unit; the 1,100-line function itself is NOT split in this pass)
- `variant_lock_helpers.rs`, `tests.rs`

Risks: field visibility on `SequencerState` must allow descendant-module impls
(nest impl files under `state/`); keep `_no_publish`/public wrapper pairs in the
same file; preserve `snapshot.rs`'s `super::state::` imports via re-exports.
No external-crate consumers — all 24 users are in-crate.

### 3b. `tui/graph.rs` → `tui/graph/` (or `app/graph/`)

One 7,364-line `impl GraphController<'_>` (~188 methods) reached almost
entirely via `App::graph_controller()` — zero caller churn. Split:

- `mod.rs` (struct def + `App::graph_controller()` + re-exports),
  `types.rs` (build specs — re-export for `projects.rs`), `transaction.rs`
  (batch guard, rollback transaction, test hooks), `mod_routes.rs`,
  `bus_routing.rs`, `track_create.rs`, `rack_slots.rs` (~1.5k),
  `sync_clear.rs`, `teardown.rs` (~1.1k), `reorder.rs`, `engine_voice.rs`,
  `rack_rebuild.rs`, `node_build.rs`, `engine_connect.rs`,
  `slot_bookkeeping.rs`, `tests.rs`

Constraint: files must be **submodules of `graph`** (not `tui/` siblings) so
the private `app: &mut App` field stays visible to all impl blocks.
`refresh_rack_signature_from_live_state` is `pub(super)` — fine anywhere under
`graph`. Keep teardown + reorder adjacent (both mutate the same track vectors).

### 3c. `lisp_host.rs` → extend existing `lisp_host/`

Precedent already exists (`dylib_cache.rs`, `graph_authoring.rs`, etc.) —
follow the same declare + `pub use` template. `lisp_host.rs` becomes a
~400–700-line façade. New files:

- `dgen_ffi.rs`, `dgen_manifest.rs`, `effect_compile.rs`,
  `effect_chain_graph.rs`, `instrument_storage.rs`, `instrument_compile.rs`,
  `editor_flow.rs`, `midi_fx.rs`, `value_helpers.rs`
- `shared_state.rs` — the `Shared*` type aliases + backing registries
  (`ProcessAuthoringRegistry`, etc.). **Extract this first**: it's the seam
  both `scratch_runtime.rs` and every natives file depend on (avoids
  bidirectional deps). Verify where each `Shared*` alias is actually defined
  before cutting (`grep -n "type Shared" lisp_host.rs`).
- `scratch_runtime.rs` (`ScratchControlRuntime`), `process_natives.rs`
  (~1.6k), `process_dsl_parse.rs` (~2.1k), `sequencer_natives.rs` (~2.6k),
  `native_arg_parsing.rs` (~1.3k), `neural_natives.rs` (~1.1k), `tests.rs`

Risks: file-level statics (`COMPILE_COUNTER`, `MIDI_FX_DESCRIPTOR_CACHE`) move
with their consumers; `pub(crate)` items (`test_loaded_dgen_lib`,
`dgenlisp_tool_path`) need re-exports; heavy import surface (30+ files import
`lisp_host::`) is fully covered by façade re-exports.

### 3d. `ui/state_values.rs` → `ui/state_values/`

After Phase 1 it's ~12.8k lines of ~250 free functions in a repeating
build/field/sync trio pattern per domain. Split:

- `shared.rs` — `ExpandedStepViewport`/`ExpandedStepProjectionRegistry`
  (used by 4 other ui files), `ReactiveSetStats`, sync-profile structs,
  `field_safe_name`/`insert_string_prop`/`insert_param_ui_metadata` +
  `rack_slot_param_by_index/bounds/options` (shared by plocks AND rack panel —
  putting them here avoids a two-way dep)
- `steps_and_pattern.rs`, `process_and_macros.rs` (~1.0k),
  `expanded_step.rs`, `param_fields_and_sync.rs` (~1.5k),
  `track_and_mixer.rs` (~1.1k), `meters_and_modulation.rs`,
  `topology_and_visualization.rs`, `effects_panel.rs` (~1.3k),
  `instrument_panel.rs` (~1.3k), `drum_rack.rs`, `rack_panel.rs` (~1.5k),
  `misc_options.rs`, `track_params_sync.rs`, `plocks.rs` (~1.6k merged)
- Move `project_scratch.rs` cluster (lines ~10,610–10,973) OUT of
  state_values entirely → `ui/editor_scratch.rs` (it's editor-buffer
  bookkeeping, not reactive-state building)
- Move the two stranded EOF functions → `host_commands.rs`

`mod.rs` re-exports everything `pub(crate)` so `host_commands.rs`,
`editor_setup.rs`, and `main.rs`'s glob import keep working. Note the file
relies on `use super::*;` inheriting `main.rs`'s imports — rebuild explicit
imports per file via `cargo check` iteration, don't guess.

### 3e. `ui/main.rs` — the only non-mechanical one

This is the **`metal_seq` binary root** (`[[bin]] path = "src/ui/main.rs"`).
Its 15.5k-line `fn main()` contains a ~12.4k-line
`match name.as_str()` over ~180 host commands, where every arm mutates ~40
stack locals. Two-stage approach:

**Stage 1 (mechanical, do with Phase 3):** extract the ~5k lines of free
functions that already take explicit parameters:
- `edit_sessions.rs` (~1k) — edit-session structs + draft helpers
- `history_commands.rs` (~1k) — `apply_*_history_host_command` family + param mapping
- `reactive_sync.rs` (~2.5k) — `sync_*` family + `apply_ui_invalidations`
  (or fold into existing `ui_invalidation.rs`)
- `agent_finalize.rs` (~0.9k) — agent draft finalization
- move `mod tests` alongside

**Stage 2 (the real refactor, do last) — DONE 2026-07-22:** implemented as
`ui/loop_ctx.rs` (EditSessionState / MeterCache / FrameDiffState /
GestureState / SharedHandles + borrowing LoopCtx), `ui/host_commands/`
(14 domain modules + COMMANDS-membership router in dispatch.rs),
`ui/event_loop.rs` (run_event_loop owns the loop and its state), and
`ui/reactive_tick.rs` (TickFlow return; caller performs continue/break).
The `continue` hazard was smaller than feared: arm continues targeted the
drain-commands `for` loop and nothing followed the match in that arm, so
`continue` ≡ fall-through and each arm became a plain `return`.
`fn main` is now ~250 lines of setup. Follow-up (not done): decompose the
200+-line arms (`cancel-editor`, `save-new-effect`) inside their domain
modules.
1. Introduce `LoopCtx` struct bundling the ~40 event-loop locals (use grouped
   sub-structs — `EditSessionState`, `SelectionState`, `MeterCache` — to avoid
   a new god-object).
2. Split the match into `host_commands/` dispatch modules by the domain
   clusters already implicit in command naming: `tracks`, `effects`, `rack`,
   `instrument_authoring`, `step_history`, `bus_steps`, `routing`, `samples`,
   `scripts`, `agent`, `project`, `misc`.
3. Extract `event_loop.rs` (input polling, gestures, async polling) and
   `reactive_tick.rs` (post-event sync + render + shutdown).

Critical hazards: bare `continue` inside match arms targets the event loop —
extracted arms need an explicit outcome enum and the caller performs the
`continue`, audited per arm (~180); `std::thread::spawn` closures move-capture
session data; a handful of 200+-line arms (`cancel-editor`, `save-new-effect`)
need internal decomposition as follow-up, not in the move pass.

Also carve the shared types (`ActiveDeleteTarget`, `PendingInstrumentPreview`,
etc.) out of `main.rs` into a neutral module — today `main.rs` ⇄
`natives.rs`/`state_values.rs`/`input.rs` are coupled by bidirectional
`use super::*;` globs.

---

## Suggested execution order

1. Phase 1 test extraction, one file per commit (any order; state_values first
   for the biggest visual win). ~64k lines relocated, zero logic risk.
2. `lisp_host` split (existing directory precedent, well-understood seams).
3. `sequencer/state` split.
4. Phase 2: delete legacy ratatui frontend, then `tui/` → `app/` rename.
5. `graph.rs` split (into `app/graph/`).
6. `state_values` production split + `project_scratch` relocation.
7. `ui/main.rs` Stage 1, then Stage 2 (`LoopCtx` + `host_commands/`).

Nothing outside the `sequencer` crate imports any of these files, so no
public-API/semver concerns anywhere.

---

## Phase 4 — Loose-file reorganization (DONE 2026-07-22)

### 4a. `src/runtime/` — lisp-authorable sequencer extensions

Folder invariant: *the engine half of an eseqlisp `def-*` form, ticked/folded
by the scheduler each block*. Moved: `process.rs`, `accumulator.rs`,
`generator.rs`, `graph.rs`. Crate-root `pub use runtime::{accumulator,
generator, graph, process};` keeps every historical `crate::<name>::` /
`sequencer::<name>::` path working (the metal_seq bin uses
`sequencer::process::`).

`GridBoundaryClock` / `next_grid_boundary` / `process_grid_boundaries` were
split out of `neural.rs` into `runtime/grid_clock.rs`; `neural.rs` re-exports
them so `crate::neural::` paths still work. **`neural.rs` deliberately stays
at the crate root**: the builtin neural machine is not lisp-authorable
(graph-mode `def-sequencer` is its successor), so it sits outside the folder
on its own deprecation timeline. `warp_grid.rs` has an unrelated fn also named
`next_grid_boundary` — do not confuse them.

### 4b. `lisp_host/{dgen,eseq}/` — split by language side

- `lisp_host/dgen/` — DGenLisp DSP-compile pipeline: `dgen_ffi`,
  `dgen_manifest`, `dylib_cache`, `effect_compile`, `effect_chain_graph`,
  `instrument_compile`, `instrument_storage`
- `lisp_host/eseq/` — eseqlisp live-coding natives: `sequencer_natives`,
  `process_natives`, `process_dsl_parse`, `neural_natives`, `graph_authoring`,
  `graph_dsl`, `graph_manifest`, `graph_update`, `scratch_runtime`, `midi_fx`
- root keeps shared plumbing: `shared_state`, `native_arg_parsing`,
  `value_helpers`, `editor_flow` (mixed: holds the DGenLisp EFFECT_TEMPLATE
  but is editor-session flow)

Folder names are `dgen`/`eseq`, NOT `dgenlisp`/`eseqlisp`: an internal module
named `eseqlisp` shadows the external `eseqlisp` crate for every
`eseqlisp::vm::` path inside the lisp_host tree.

Mechanical pattern used (repeat if adding files): top-level `use super::*;` →
`use super::super::*;` (inner `mod tests` globs untouched), `pub(super)` →
`pub(in crate::lisp_host)`, `include_str!` paths gain one `../`. The façade
keeps module-name bindings (`pub use dgen::dylib_cache;`,
`use eseq::graph_update;`) because `effect_compile`/`instrument_compile`/
`process_natives`/`shared_state` call `dylib_cache::…` / `graph_update::…`
qualified.

Note: the executed split placed `ScratchControlRuntime` in `shared_state.rs`
(it is the seam both sides share), not `scratch_runtime.rs` as §3c planned;
`scratch_runtime.rs` holds runtime construction + lisp library loading.
