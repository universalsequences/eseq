# Lisp Hot Reload Spec

## Goal

Make split Lisp UI modules develop like production UI modules:

- evaluating a leaf buffer such as `metal-seq-fx/param-grid.lisp` updates the
  live UI that depends on the changed definitions;
- evaluating a root manifest such as `metal-seq-fx.lisp` uses unsaved edits
  from open child buffers instead of rereading stale disk contents;
- saved file changes from external editors or coding agents reload into the
  running app automatically;
- failed reloads keep the last working UI active and surface clear diagnostics.

The target experience is close to React/Next Fast Refresh: source changes are
loaded transactionally, affected render roots are invalidated, and visible UI
updates without manual buffer choreography.

## Current Problem

`eval-buffer-command` evaluates the active buffer text, so unsaved edits in the
current buffer are visible to the evaluator. However, `(load "path.lisp")`
currently reads `path.lisp` from disk.

After splitting large files such as `metal-seq-fx.lisp` into leaf modules, this
creates two bad workflows:

1. Evaluating `metal-seq-fx/param-grid.lisp` mutates definitions such as
   `fx-param-grid`, but it does not rerun the `effect-buffer "*fx*"` expression
   that previously called those functions.
2. Evaluating `metal-seq-fx.lisp` reruns the root manifest, but all child
   `(load ...)` calls read saved files from disk, ignoring unsaved child-buffer
   edits.

Adding top-level reload callbacks to every Lisp leaf file is not the right
solution. It couples ordinary definitions to a specific app lifecycle, makes
standalone evaluation brittle, and spreads hot-reload policy through source
files that should stay declarative.

## Design Principles

- Hot reload belongs in the editor/runtime, not in Lisp module footers.
- Source lookup must go through a source manager, not direct disk reads.
- Render roots must be first-class re-runnable units.
- Dependency tracking should prefer runtime observation over fragile static
  symbol analysis.
- Reloads must be transactional: failed source never replaces the last working
  UI.
- File-watcher reloads and manual eval-buffer reloads should use the same
  pipeline.
- Hot reload must not add measurable overhead to audio processing, steady-state
  drawing, or ordinary VM evaluation outside render-root capture.

## Core Concepts

### Source Manager

The source manager is the only way runtime code obtains Lisp source.

Responsibilities:

- canonicalize file paths;
- track open file-backed buffers and their dirty text;
- track disk contents and modification timestamps;
- resolve relative loads against the current loading file;
- choose source text in priority order:
  1. dirty open buffer text for the canonical path;
  2. clean open buffer text for the canonical path;
  3. disk contents.

`(load "metal-seq-fx/param-grid.lisp")` should ask the source manager for the
source instead of calling `std::fs::read_to_string` directly.

### Module Graph

The runtime records load relationships while evaluating source.

Example:

```text
metal-seq-fx.lisp
  -> metal-seq-fx/state.lisp
  -> metal-seq-fx/param-grid.lisp
  -> metal-seq-fx/buffers.lisp
```

Each module record should include:

- canonical path;
- current source revision/hash;
- symbols defined by the module;
- direct loaded children;
- direct parents;
- last successful diagnostics.

The graph lets the editor answer: "this leaf buffer belongs to which root
manifest?" When a user evaluates `param-grid.lisp`, the editor can evaluate the
owning root module if needed, using unsaved source overlays for every open
child file.

### Render Root Registry

The runtime should treat UI roots as re-runnable declarations.

Today, a form like:

```lisp
(effect-buffer "*fx*" ...)
```

evaluates once and stores a widget tree. Hot reload needs the runtime to retain
the render recipe as well as the latest rendered tree.

Possible Lisp surface:

```lisp
(defrender "*fx*"
  ...)
```

or keep the existing surface and make `effect-buffer` register the unevaluated
body when used at top level.

Render root metadata:

- target buffer name;
- canonical source module that declared it;
- unevaluated render body or compiled render closure;
- last successful widget tree;
- dynamic dependencies observed during the last render;
- last diagnostics.

This makes `*fx*`, `*track*`, browser panels, mixer panels, and future app UI
roots reloadable without hand-written callbacks.

### Dynamic Dependency Tracking

During a render root evaluation, the VM records the symbols that are actually
used:

- function calls;
- macro expansions;
- global variable reads;
- reactive field reads where available.

If the `*fx*` render root calls `fx-panel`, which calls `fx-param-grid`, then
the registry records:

```text
*fx* depends on fx-panel
*fx* depends on fx-param-grid
*fx* depends on fx-param-row
```

When `param-grid.lisp` redefines `fx-param-grid`, the runtime invalidates and
rerenders only render roots that observed that symbol.

Runtime dependency tracking is preferred over static analysis because Lisp
macros, higher-order functions, dynamic branches, and globals make static
call-graph analysis easy to get wrong.

## Performance Requirements

Hot reload is a development feature, but it must not make the running app feel
heavier or risk audio/UI regressions.

Dependency tracking must be scoped narrowly:

- only active while evaluating a registered render root;
- disabled for audio code generation and audio processing;
- disabled for ordinary Lisp evaluation that is not inside render-root capture;
- disabled during renderer drawing/layout traversal unless that traversal
  explicitly invokes a render-root evaluation.

Expected cost model:

- Module graph updates are load/eval-time bookkeeping and should be negligible.
- File watching should be path-filtered and debounced; it should do no parsing
  until a relevant save event is coalesced.
- Render dependency capture should mostly be interned-symbol ID inserts into a
  small per-root set.
- Transactional evaluation and rerendering can be comparatively expensive, but
  only run in response to manual eval or watched file changes.

Implementation guardrails:

- Represent tracked symbols with interned IDs or compact handles, not repeated
  string allocations.
- Deduplicate dependencies per render root.
- Recompute dependency sets only when that root rerenders.
- Start with root-level invalidation. Do not attempt subtree-level dependency
  capture until root-level behavior is correct and measured.
- Add debug counters before broad rollout:
  - render roots evaluated per reload;
  - dependencies captured per root;
  - dependency-capture time per root;
  - invalidated roots per changed module;
  - total reload transaction time;
  - file-watch events coalesced per transaction.
- File watching should be enabled by default, including `--release` builds,
  because release mode is the normal development/test mode for `metal_seq`.
- Add an environment variable override to disable watcher-driven reloads for
  profiling or debugging. Proposed name: `METAL_SEQ_DISABLE_LISP_HOT_RELOAD=1`.

Performance acceptance:

- With no reload in progress, hot-reload infrastructure should produce no
  per-frame work beyond checking already-existing dirty flags or empty queues.
- A normal reactive UI update that does not evaluate a render root should not
  allocate dependency sets.
- A render-root evaluation with dependency capture enabled should stay within
  the same order of magnitude as the previous render-root evaluation time.
- Profiling should show zero audio-thread work caused by source watching,
  module graph updates, dependency tracking, or transactional reload.

### Transactional Evaluation

Reloads should never leave the runtime half-mutated.

Target behavior:

1. Parse changed source.
2. Evaluate definitions in a staged environment.
3. Identify changed symbols/modules.
4. Rerender affected render roots in the staged environment.
5. If all required work succeeds, commit definitions and rendered roots.
6. If anything fails, keep the previous committed definitions/rendered UI and
   report diagnostics.

State rules:

- `defstate` values should be preserved across successful reloads by default.
- New `defstate` names get their declared initial values.
- Removed `defstate` definitions do not need to delete live state in V1.
- A later explicit state-migration mechanism can handle breaking state-shape
  changes.

## Manual Eval Workflows

### Evaluate Root Manifest

User edits `param-grid.lisp` without saving, switches to `metal-seq-fx.lisp`,
and evaluates the root buffer.

Expected behavior:

1. Editor snapshots all open file-backed Lisp buffers.
2. Runtime evaluates `metal-seq-fx.lisp`.
3. Every nested `(load ...)` reads from the source manager.
4. The unsaved `param-grid.lisp` buffer text is used.
5. Render roots declared by the root manifest rerender.

### Evaluate Leaf Buffer

User edits `metal-seq-fx/param-grid.lisp` and evaluates that buffer.

Expected behavior:

1. Runtime evaluates the leaf buffer transactionally.
2. Runtime detects changed definitions, such as `fx-param-grid`.
3. Render dependency index finds roots that used those symbols.
4. Affected roots, such as `*fx*`, rerender.
5. The visible UI updates without evaluating unrelated roots.

If the leaf file introduces a new dependency that requires root load order, the
module graph can escalate to re-evaluating the nearest root manifest using the
source overlay.

## File Watcher Workflow

Saved disk changes should use the same pipeline as manual eval.

Trigger sources:

- external editor saves;
- coding agents edit files;
- generated file writes from tools.

Pipeline:

```text
file changed
debounce by canonical path
source manager reads new source
transactional module eval
changed symbols/modules computed
affected render roots invalidated
render roots rerendered
diagnostics reported
```

Watcher rules:

- Ignore files not known to the module graph unless they match a configured
  Lisp source root.
- Debounce rapid writes to avoid evaluating partial save sequences.
- Coalesce multiple changed files into one transaction when they occur in the
  same debounce window.
- If a changed leaf is part of a known root manifest, prefer graph-aware reload
  over raw standalone eval.
- If a file is open and dirty in the editor, do not overwrite or replace the
  editor buffer from disk; show that disk changed behind the dirty buffer.

This is the agent-friendly mode: an agent can edit `param-grid.lisp` on disk
while the app is running, and the app becomes the live feedback loop.

## Proposed Implementation Phases

### Phase 1: Source Manager and Load Context

Objective: make `(load ...)` use the right source.

Deliverables:

- Add a source-provider abstraction to the runtime/editor bridge.
- Change `(load ...)` to use the provider instead of direct disk reads.
- Maintain a load stack so relative paths resolve relative to the loading file.
- During eval-buffer, provide an overlay of open file-backed buffers.

Acceptance:

- Evaluating a root manifest uses unsaved child-buffer edits.
- Relative loads continue to work from existing root files.
- Existing `(load ...)` tests still pass.

### Phase 2: Module Graph

Objective: know which files load and define what.

Deliverables:

- Record module parent/child relationships during `(load ...)`.
- Record symbols defined by each evaluated module.
- Expose diagnostics/debug view for the graph.
- Map an open leaf buffer to nearest root manifest candidates.

Acceptance:

- The graph shows `metal-seq-fx.lisp -> metal-seq-fx/param-grid.lisp`.
- Evaluating a leaf can find `metal-seq-fx.lisp` as an owning root.
- Graph updates correctly after root manifest changes.

### Phase 3: Render Root Registry

Objective: make UI roots explicitly rerunnable.

Deliverables:

- Introduce render-root storage for `effect-buffer` or a new `defrender` form.
- Preserve the current public Lisp API where possible.
- Store target buffer, source module, render body/closure, latest widget tree,
  and diagnostics.
- Rerender a root on demand without reevaluating the whole app.

Acceptance:

- `*fx*` can be rerendered by the runtime after its source functions change.
- A failed rerender leaves the previous `*fx*` tree visible.
- Existing UI buffer behavior remains unchanged for normal startup.

### Phase 4: Dynamic Dependency Tracking

Objective: invalidate only roots that used changed definitions.

Deliverables:

- Track function/macro/global usage while a render root runs.
- Store dependency sets per render root.
- Track symbol definitions changed by eval transactions.
- Invalidate render roots whose dependencies intersect changed symbols.

Acceptance:

- Editing `fx-param-grid` rerenders `*fx*`.
- Editing an unrelated browser helper does not rerender `*fx*`.
- Diagnostics can explain why a render root rerendered.

### Phase 5: Transactional Reloads

Objective: avoid half-applied source changes.

Deliverables:

- Stage definition changes before commit.
- Stage affected render-root rerenders before replacing visible trees.
- Preserve existing state values across successful commits.
- Report parse/eval/render errors without destroying the previous UI.

Acceptance:

- A syntax error in `param-grid.lisp` leaves the old `*fx*` UI visible.
- Fixing the syntax error reloads and updates `*fx*`.
- The status/minibuffer or diagnostics panel shows actionable errors.

### Phase 6: File Watcher

Objective: support external editor and agent-driven hot reload.

Deliverables:

- Watch known Lisp source roots.
- Debounce and batch changed paths.
- Run the same transactional reload pipeline used by manual eval.
- Surface reload success/failure in the app.
- Enable the watcher by default in `metal_seq`, including `--release` runs.
- Honor `METAL_SEQ_DISABLE_LISP_HOT_RELOAD=1` by skipping watcher startup while
  leaving manual eval-buffer reload behavior available.

Acceptance:

- Saving `metal-seq-fx/param-grid.lisp` from outside the app updates `*fx*`.
- Agent-written file changes reload without restarting `metal_seq`.
- Rapid multi-file saves produce one coherent transaction.
- Setting `METAL_SEQ_DISABLE_LISP_HOT_RELOAD=1` before launching `metal_seq`
  disables file-watcher reloads.

## Testing Strategy

Unit tests:

- `(load ...)` uses unsaved buffer overlay when present.
- Relative load resolution follows the load stack.
- Module graph records parent/child edges and defined symbols.
- Changed-symbol detection reports redefined functions.

Editor/runtime integration tests:

- Open `metal-seq-fx.lisp` and `param-grid.lisp`, edit the leaf without saving,
  evaluate the root, and assert the rendered `*fx*` tree reflects the unsaved
  leaf edit.
- Evaluate `param-grid.lisp` directly and assert `*fx*` rerenders.
- Introduce a bad edit, evaluate, assert old UI remains visible and diagnostics
  are shown.
- Fix the edit, evaluate, assert the new UI appears.

File watcher tests:

- Write a watched Lisp file and assert the corresponding module reloads.
- Batch two dependent file writes and assert a single transaction.
- Write invalid source and assert the previous render root remains committed.

Layout tests:

- For UI-affecting reloads, assert expected labels/debug nodes have finite,
  nonzero measured rects inside the visible panel.
- Do not stop at parse or widget-tree existence tests; hot reload must prove the
  rendered layout remains visible.

## Non-Goals for V1

- Perfect static dependency analysis.
- Full state schema migration.
- Cross-process collaborative editing.
- Preserving compatibility with direct disk-only `(load ...)` semantics where
  those semantics conflict with editor hot reload.
- Hand-authored per-file reload hooks.

## Open Questions

- Should `effect-buffer` become the render-root primitive, or should a new
  `defrender` form make render roots explicit in Lisp?
- Should leaf eval always prefer changed-symbol invalidation, or should files
  loaded by manifests default to re-evaluating the nearest root first?
- How visible should reload diagnostics be: minibuffer only, diagnostics buffer,
  inline overlay, or all three?
- Should the watcher disable variable also suppress debug counters, or only
  automatic reload triggers?
