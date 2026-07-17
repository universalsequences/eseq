# UI Invalidation Notes

Goal: keep large declarative Lisp UIs fast without requiring hand-tuned buffer structure.

## Core idea

The runtime should treat the widget tree as a stable, incrementally reusable graph, not as something that must be rebuilt and fully relaid out every time any reactive input changes.

## What the system needs

1. Stable subtree identity.
Each emitted widget/subtree should have a stable identity across reactive runs so the runtime can match "the same thing with new props" instead of treating it as a fresh node.

2. Structural snapshots.
Widget trees stored for buffers should be deep, immutable snapshots. Reuse logic should never observe later mutations through shared cells.

3. Fine-grained reactive ownership.
Instead of one large effect per buffer, the runtime should be able to know which subtree depends on which reactive inputs, so a slider change only reruns the affected subtree.

4. Subtree-level layout reuse.
If size-affecting props are unchanged, the runtime should reuse prior layout geometry for that subtree and only mark render props dirty.

5. Container-specific reuse rules.
Some widgets, like `tabs`, only lay out the selected body. Reuse must compare against the effective rendered children, not the full declarative child list.

6. Incremental render scene updates.
After layout reuse succeeds, the renderer should patch only dirty widget primitives/instance data instead of rebuilding the full scene for the whole buffer.

## Likely runtime model

- Reactive evaluation produces subtree snapshots with stable IDs.
- Dirty reactive inputs mark a bounded set of subtree roots dirty.
- Dirty subtree roots are reevaluated.
- Unchanged subtrees are reused by equality/identity checks.
- Changed subtrees go through subtree relayout only.
- Renderer patches cached primitives for the changed widget IDs.

## Why this is better than Lisp-side tuning

- Buffer scripts can stay simple and declarative.
- Performance does not depend as heavily on manually splitting effects/buffers.
- Complex UIs become fast by default if most edits only affect a small part of the tree.
- The same engine behavior helps every UI, not just `metal_seq`.

## Practical next steps

1. Add stable subtree keys/identity through widget emission.
2. Keep improving size-affecting prop rules per widget type.
3. Track reactive dependencies at subtree granularity, not just whole-buffer effect granularity.
4. Use `dirty_widget_ids` to patch cached Metal scene data incrementally.
5. Preserve compact profiling so hot effects, relayout misses, and full-scene rebuilds remain visible.

## Implementation plan

This plan assumes the current architecture:

- Reactive writes are collected in `ReactiveRegistry` as a flat dirty list.
- `Runtime::run_reactive_cycle()` applies those changes in the VM, then flushes pending widget trees.
- Buffers store one `widget_tree` snapshot each.
- The active buffer keeps one `current_widget_tree` and one `current_layout`.
- Layout reuse already exists, but only by comparing the old full layout tree to the new full tree.

The goal is to move from "dirty reactive field -> rerun buffer effect -> rebuild/relayout buffer tree" toward "dirty reactive field -> mark subtree roots dirty -> reevaluate/reuse only those roots -> patch renderer".

## Non-goals for phase 1

- Do not redesign the Lisp surface syntax for UI authoring.
- Do not require hand-authored dependency declarations in Lisp.
- Do not attempt cross-buffer partial rendering before subtree ownership works inside one buffer.
- Do not block on a perfect renderer patch system before shipping subtree dependency tracking.

## Phase 0: Instrument the current pipeline

Objective: make the bottlenecks visible before behavior changes.

Deliverables:

- Add profiling counters for:
  - reactive fields changed per cycle
  - reactive effects executed per cycle
  - widget trees flushed per cycle
  - full buffer relayout count
  - layout reuse success/failure by widget type and reason
  - dirty widget count produced by reuse
  - active buffer vs inactive buffer reactive work
- Log top hot reactive effects with:
  - effect label
  - owner buffer
  - execution time
  - resulting widget tree size
- Add a debug view or trace dump that shows:
  - changed reactive fields
  - affected buffers
  - reevaluated roots
  - full-relayout fallback reason

Why first:

- The runtime already tracks useful timings in `runtime.rs`, but not enough to validate subtree ownership.
- This phase gives a baseline to prove later phases are actually reducing work.

## Phase 1: Stable subtree identity

Objective: make it possible to match "the same subtree" across reruns.

Deliverables:

- Introduce a subtree identity model in emitted widget trees.
- Every widget node should carry:
  - stable widget id
  - stable subtree/root id
  - parent subtree/root id
  - optional explicit key from Lisp
  - structural ordinal fallback when no key exists
- Define identity rules for:
  - normal children
  - `each` expansions
  - `tabs`
  - conditionals that add/remove branches
  - overlay/widget-buffer targets

Implementation notes:

- Reuse of positional child index alone is not enough for `each` lists or tab bodies.
- Prefer explicit keys where available, but support stable fallback IDs so existing UIs still work.
- Root identity must survive reruns of the surrounding effect.

Likely code areas:

- VM widget emission path
- any `effect-buffer` / widget-tree construction helpers
- layout node construction and reuse helpers

Exit criteria:

- The same unchanged subtree in repeated reruns gets the same identity.
- Reordering keyed children does not look like delete+recreate for every child.

## Phase 2: Immutable structural snapshots

Objective: make reuse and dependency indexing safe.

Deliverables:

- Replace ad hoc tree reuse assumptions with explicit immutable snapshot structures.
- Separate:
  - declarative widget snapshot
  - layout snapshot
  - renderer scene snapshot
- Ensure buffer-stored widget trees are deep, immutable snapshots with no shared mutable cells that can change after commit.

Implementation notes:

- `Buffer::set_widget_tree()` already deep-clones `Value`; keep that guarantee and tighten it.
- Layout reuse must compare against a frozen snapshot produced at commit time, not live VM-owned cells.
- Snapshot format should be designed for subtree lookup by id.

Data model to add:

- `WidgetSnapshot`
- `SubtreeSnapshot`
- `BufferUiSnapshot`

Suggested contents:

- root subtree ids
- widget node table keyed by widget id
- subtree root table keyed by subtree id
- child relationships
- props hash
- structural hash
- dependency set

Exit criteria:

- Old and new snapshots can be compared without touching the VM.
- A subtree can be fetched directly by id in O(1) or close to it.

## Phase 3: Reactive dependency capture per subtree

Objective: know which reactive fields each subtree actually depends on.

Deliverables:

- Add dependency tracking during reactive evaluation.
- Record reads of reactive fields at subtree granularity, not just effect granularity.
- Build an index:
  - `(namespace, field)` -> set of subtree root ids
- Store the reverse mapping too:
  - subtree root id -> set of reactive fields

Implementation notes:

- The VM already has a reactive graph for effect execution order; this is a separate UI dependency index.
- Dependency capture should start when a subtree root begins emission and stop when it completes.
- If a helper function reads `SEQ.foo` while building a subtree, that read belongs to the current subtree root.
- For nested subtrees, reads should belong to the nearest active subtree root.

Design decision:

- Define explicit subtree root boundaries during emission.
- Good initial root candidates:
  - `effect-buffer` root
  - children of major layout containers
  - keyed children of `each`
  - selected `tabs` body
  - explicit Lisp helper such as a future `subtree`/`keyed` wrapper if needed

Why not per-widget immediately:

- Per-widget tracking is more expensive and harder to reason about.
- Subtree roots give most of the win with simpler ownership and cheaper indexes.

Exit criteria:

- Given a change to `SEQ.master-peak-l`, the runtime can list the exact subtree roots affected.
- Unrelated buffers/subtrees are not reevaluated.

## Phase 4: Subtree reevaluation pipeline

Objective: rerun only dirty subtree roots instead of full buffer effects.

Deliverables:

- Extend the runtime/VM so a reactive cycle can:
  - map dirty fields to dirty subtree roots
  - reevaluate only those roots
  - merge updated subtree snapshots back into the owning buffer snapshot
- Keep current whole-buffer reevaluation as fallback.

Required runtime changes:

- Replace or augment `pending_widget_trees` with subtree-aware pending UI updates.
- Add buffer-local UI state that can commit a new subtree snapshot without replacing the whole tree.
- Keep owner metadata:
  - source effect/eval chunk
  - owner buffer id
  - subtree root id

Execution model:

1. Reactive field changes arrive.
2. VM marks reactive dependents dirty as it already does.
3. Runtime queries subtree dependency index for affected roots.
4. Runtime reevaluates only those roots.
5. Updated roots are merged into the buffer snapshot.
6. Layout reuse runs only on changed roots.
7. Renderer receives dirty widget ids and/or dirty subtree ids.

Fallback rules:

- If subtree reevaluation cannot preserve identity, fall back to buffer-level rerun.
- If a subtree changes its root widget type unexpectedly, fall back to subtree full relayout.
- If ownership becomes ambiguous, fall back to buffer-level rerun and log it.

Exit criteria:

- Changing one slider value in a large buffer reevaluates one small bounded set of subtree roots.
- Whole-buffer reruns become the exception and are visible in profiling.

## Phase 5: Subtree-level layout reuse

Objective: stop relaying out the whole buffer when only one subtree changed.

Deliverables:

- Extend `reuse_layout_node` so it can start from a subtree root id instead of only the full tree root.
- Store layout nodes in an id-addressable structure.
- Allow relayout of only the dirty subtree and ancestor geometry propagation when required.

Layout behavior rules:

- If size-affecting props are unchanged, reuse geometry and mark render props dirty.
- If local geometry changes but parent constraints are unchanged, relayout only that subtree.
- If subtree min/max size changes in a way that affects ancestors, bubble relayout upward only until geometry stabilizes.
- If root-level constraints or viewport change, keep existing full-buffer relayout behavior.

Container-specific work:

- `tabs`: compare/render only the effective selected body.
- `each`: keyed children must preserve identity across insert/remove/reorder.
- `tree`: currently forces full relayout; phase 5 should either:
  - keep that fallback deliberately, or
  - add explicit expansion-state-aware reuse rules.
- scroll containers: preserve scroll state while updating descendants.

Exit criteria:

- Dirty subtree relayout updates only affected geometry.
- Unchanged siblings keep layout nodes and widget ids.

## Phase 6: Incremental renderer patching

Objective: stop rebuilding the entire render scene for localized changes.

Deliverables:

- Replace full-scene rebuild paths with patching driven by:
  - dirty widget ids
  - dirty subtree ids
  - layout revision changes
- Maintain renderer caches by widget id and subtree/root id.
- Patch only changed primitives/instance data when geometry is unchanged.

Renderer invalidation rules:

- geometry changed:
  - rebuild primitives for the affected subtree
  - update clip/transform state as needed
- props/shader uniforms changed only:
  - patch cached primitive data or uniform payloads only
- full layout revision bump:
  - keep current full rebuild fallback

Suggested milestones:

- first patch SDF/shader-backed widgets
- then patch basic labels/boxes/sliders
- leave full fallback for rare/complex widgets until coverage is broad enough

Exit criteria:

- Meter updates or slider drags no longer rebuild the full active scene.
- Dirty widget ids are small and stable under common interactions.

## Phase 7: Multi-buffer ownership and inactive buffer policy

Objective: avoid scanning or rebuilding inactive buffers unnecessarily.

Deliverables:

- Introduce per-buffer UI snapshots and dependency indexes.
- Maintain `(namespace, field)` -> affected buffer ids before looking at subtree roots.
- Only touch inactive buffers when:
  - their UI snapshot truly changed
  - they are visible in another pane/tile
  - a background update policy explicitly allows it

Policy proposal:

- active visible buffers: update eagerly
- visible inactive buffers in tiled layout: update incrementally
- hidden buffers: mark dirty, defer reevaluation until visible unless explicitly requested

Why:

- This removes the "loop through every open buffer and inspect the tree" behavior in practice.
- It bounds work by actual visibility and dependency ownership.

Exit criteria:

- A change to one reactive field does not trigger work for unrelated hidden buffers.

## Phase 8: API and authoring refinements

Objective: expose enough control to make subtree ownership robust without making UI code painful.

Deliverables:

- Add optional Lisp authoring helpers for stable identity, for example:
  - keyed child wrappers
  - explicit subtree boundaries
  - keyed `each`
- Document best practices:
  - where to provide keys
  - how to avoid accidental dependency widening
  - how tabs/conditionals should be structured

Important constraint:

- These APIs should improve determinism, not be required for basic performance.
- Existing UIs should still work with fallback identity heuristics.

## Testing plan

Add tests at four layers.

1. VM/reactive dependency tests
- reactive field read is attributed to the correct subtree root
- unrelated field changes do not dirty the subtree
- nested subtree ownership is deterministic

2. Snapshot/reuse tests
- stable ids survive repeated reruns
- keyed reorder preserves identity
- conditional branch swaps fall back cleanly
- tabs reuse only selected body

3. Layout tests
- subtree relayout preserves unchanged sibling geometry
- size-prop change bubbles upward only as needed
- viewport changes still trigger safe full relayout

4. Integration/perf tests
- large mixer UI with meters
- step sequencer drag
- effect parameter drag
- paused idle with selection
- multiple visible buffers in a tiled layout

For each integration case, capture:

- reactive cycle count
- reevaluated subtree count
- full-buffer rerun count
- relayout count
- dirty widget count
- render patch count
- CPU time

## Rollout strategy

Ship this in guarded stages.

1. Land instrumentation first.
2. Land stable ids + snapshots behind a debug flag.
3. Land dependency capture and subtree index with whole-buffer fallback.
4. Enable subtree reevaluation for a narrow widget subset.
5. Expand container coverage.
6. Enable renderer patching by default once profiling is solid.

Recommended feature flags:

- `ui_subtree_identity`
- `ui_subtree_dependencies`
- `ui_partial_relayout`
- `ui_incremental_render`

## Risks and mitigations

Risk: dependency capture is wrong and produces stale UI.
Mitigation:
- keep whole-buffer fallback
- add debug tracing of field -> subtree mapping
- add assertions in test builds

Risk: identity heuristics break under `each` or conditionals.
Mitigation:
- add explicit keys support early
- log fallback reasons when reuse fails

Risk: subtree merging causes snapshot corruption.
Mitigation:
- use immutable committed snapshots
- keep merge operations pure and testable

Risk: renderer patch logic diverges from layout state.
Mitigation:
- preserve full rebuild fallback on revision mismatch
- add scene-cache consistency assertions in debug builds

Risk: hidden buffers still accumulate too much deferred work.
Mitigation:
- store dirty markers by buffer/root and coalesce repeated updates

## Recommended order of implementation

If only one path is pursued now, do it in this order:

1. Phase 0 instrumentation
2. Phase 1 stable subtree identity
3. Phase 2 immutable snapshots
4. Phase 3 subtree dependency capture
5. Phase 4 subtree reevaluation with whole-buffer fallback
6. Phase 5 subtree relayout
7. Phase 6 incremental renderer patching
8. Phase 7 multi-buffer visibility policy
9. Phase 8 authoring refinements

That order gives useful wins early without forcing renderer work before the runtime can actually isolate smaller units of UI work.

## Engineering checklist

This checklist covers the immediate implementation scope for Phase 0 through Phase 4.

## `../eseqlisp/src/runtime.rs`

### Phase 0 instrumentation

- [ ] Extend runtime perf stats with counters for:
  - [ ] dirty reactive field count
  - [ ] affected buffer count
  - [ ] reevaluated subtree root count
  - [ ] full-buffer rerun count
  - [ ] subtree rerun count
  - [ ] widget tree flush count
  - [ ] pending widget tree count
  - [ ] pending subtree patch count
- [ ] Record relayout metrics separately for:
  - [ ] full-tree relayout
  - [ ] subtree-only relayout
  - [ ] geometry-reused subtree updates
  - [ ] relayout fallback reasons
- [ ] Add a compact debug dump method that prints:
  - [ ] dirty reactive fields
  - [ ] affected buffers
  - [ ] affected subtree roots
  - [ ] rerun mode: subtree or full buffer
  - [ ] relayout mode: subtree or full tree

### Phase 1 stable ownership plumbing

- [ ] Introduce runtime-side snapshot structs:
  - [ ] `CommittedUiSnapshot`
  - [ ] `CommittedBufferUiSnapshot`
  - [ ] `CommittedSubtreeSnapshot`
- [ ] Replace the single `current_widget_tree: Option<Value>` ownership model with a committed snapshot object that can answer:
  - [ ] root subtree ids for the active buffer
  - [ ] subtree by id
  - [ ] widget by id
  - [ ] owning buffer/source metadata
- [ ] Keep `current_widget_tree()` temporarily as a compatibility accessor backed by the new snapshot until old callers are migrated.

### Phase 2 immutable snapshots

- [ ] Add a commit step after widget tree flush that produces immutable snapshot tables instead of only storing the raw `Value`.
- [ ] Ensure snapshot commit deep-clones any mutable cells before indexing.
- [ ] Store hashes on committed nodes:
  - [ ] props hash
  - [ ] structural hash
  - [ ] children hash

### Phase 3 dependency indexing

- [ ] Add runtime indexes:
  - [ ] `(namespace, field) -> buffer ids`
  - [ ] `(namespace, field) -> subtree root ids`
  - [ ] `subtree root id -> reactive field set`
- [ ] Add runtime methods:
  - [ ] `mark_dirty_subtrees_for_fields(...)`
  - [ ] `affected_buffers_for_fields(...)`
  - [ ] `affected_subtrees_for_fields(...)`
- [ ] Preserve whole-buffer fallback if dependency data is absent or incomplete.

### Phase 4 subtree reevaluation

- [ ] Add a pending update type that can represent both:
  - [ ] full buffer tree replacement
  - [ ] subtree root replacement
- [ ] Update `flush_widget_trees()` so it can:
  - [ ] apply subtree updates into the committed snapshot
  - [ ] fall back to whole-buffer replacement when needed
- [ ] Add merge helpers:
  - [ ] `replace_subtree_snapshot(...)`
  - [ ] `commit_buffer_snapshot(...)`
  - [ ] `rebuild_active_buffer_snapshot(...)`
- [ ] Add a subtree-aware relayout entry point that receives dirty subtree ids.

## `../eseqlisp/src/lang/vm.rs`

### Phase 0 instrumentation

- [ ] Extend `ReactiveExecTiming` or add a sibling struct with:
  - [ ] owner buffer id
  - [ ] target buffer id/name
  - [ ] subtree root id
  - [ ] effect label
  - [ ] elapsed time
  - [ ] emitted node count
- [ ] Track and expose:
  - [ ] reactive reads captured per subtree
  - [ ] reactive effects that produced full-tree output
  - [ ] subtree reevaluation fallbacks

### Phase 1 stable subtree identity

- [ ] Add VM-side emission context for subtree identity:
  - [ ] current buffer owner
  - [ ] current subtree root stack
  - [ ] next emitted stable id seed
  - [ ] explicit key stack
- [ ] Introduce emitted metadata on widget-tree nodes:
  - [ ] `__widget-id`
  - [ ] `__subtree-root-id`
  - [ ] `__parent-subtree-root-id`
  - [ ] `__stable-key` if provided
- [ ] Define stable id generation rules for:
  - [ ] normal child emission
  - [ ] `each`
  - [ ] conditional branches
  - [ ] `tabs`

### Phase 2 immutable subtree payloads

- [ ] Extend `PendingWidgetTree` or add a new pending type that can emit:
  - [ ] full widget tree for a buffer
  - [ ] subtree widget tree for a specific root id
- [ ] Include metadata on each pending payload:
  - [ ] source buffer id
  - [ ] target buffer
  - [ ] subtree root id
  - [ ] subtree dependency set
  - [ ] subtree structural hash

### Phase 3 dependency capture

- [ ] Add dependency-capture state to the VM:
  - [ ] current subtree root stack
  - [ ] map of subtree root id -> reactive fields read
  - [ ] map of reactive field -> subtree root ids
- [ ] Hook reactive-field reads so they register against the nearest active subtree root.
- [ ] Add helpers:
  - [ ] `begin_subtree_capture(root_id, owner, target)`
  - [ ] `end_subtree_capture()`
  - [ ] `record_reactive_read(namespace, field)`
- [ ] Ensure helper functions called during subtree emission still attribute reads to the active root.

### Phase 4 subtree reevaluation

- [ ] Add a subtree reevaluation path alongside `apply_reactive_changes(...)`.
- [ ] Given dirty fields, compute dirty subtree roots before executing VM work.
- [ ] Reevaluate only dirty roots when possible.
- [ ] Fall back to existing `process_dirty_reactive()` buffer-level behavior if:
  - [ ] no stable root exists
  - [ ] target ownership is ambiguous
  - [ ] root identity changed incompatibly
- [ ] Expose debug data describing why fallback occurred.

## `../eseqlisp/src/ui/layout.rs`

### Phase 1 stable identity support

- [ ] Extend `LayoutNode` with subtree metadata:
  - [ ] `subtree_root_id`
  - [ ] `parent_subtree_root_id`
  - [ ] optional `stable_key`
- [ ] Update layout node builders to preserve emitted subtree metadata from the widget snapshot.
- [ ] Stop relying on only positional child order for identity-sensitive reuse decisions.

### Phase 2 snapshot compatibility

- [ ] Add helpers to rebuild layout from committed subtree snapshots, not only from raw `Value`.
- [ ] Add subtree lookup helpers:
  - [ ] `find_layout_node_by_widget_id`
  - [ ] `find_layout_node_by_subtree_root_id`
  - [ ] `replace_layout_subtree`

### Phase 5 prep work needed early

- [ ] Refactor `reuse_layout_node(...)` internals so the same reuse logic can operate on:
  - [ ] full tree roots
  - [ ] subtree roots
- [ ] Split reuse failure reasons into structured enums instead of plain strings.
- [ ] Preserve current container rules and add explicit TODO coverage for:
  - [ ] `tabs`
  - [ ] `tree`
  - [ ] keyed `each`
  - [ ] scroll containers

## `../eseqlisp/src/buffer.rs`

### Phase 2 immutable committed state

- [ ] Add committed UI snapshot fields to `Buffer`:
  - [ ] last full widget snapshot
  - [ ] subtree table
  - [ ] dependency index summary
  - [ ] committed snapshot revision
- [ ] Keep `widget_tree` as compatibility state initially, but stop treating it as the sole source of truth.
- [ ] Add buffer helpers:
  - [ ] `set_committed_ui_snapshot(...)`
  - [ ] `replace_committed_subtree(...)`
  - [ ] `clear_committed_ui_snapshot()`

### Phase 4 subtree patch application

- [ ] Add a buffer-level merge method that applies subtree replacements without replacing unrelated subtrees.
- [ ] Bump revision fields carefully:
  - [ ] subtree-only content changes should not look like unrelated full-buffer changes
  - [ ] true full replacement should still bump the full widget revision path

## `../eseqlisp/src/editor/mod.rs`

### Buffer application path

- [ ] Update pending widget application so inactive buffers can receive subtree patches, not only whole-tree replacement.
- [ ] Preserve current behavior for:
  - [ ] active buffer overlay clear
  - [ ] `ViewMode::UiOnly`
  - [ ] scratch buffer creation by name
- [ ] Add logic to defer hidden-buffer subtree work if the visibility policy says to postpone it.

## Cross-cutting data structures to add

- [ ] `ReactiveFieldKey { namespace, field }`
- [ ] `SubtreeRootId`
- [ ] `WidgetStableId`
- [ ] `SubtreeDependencyIndex`
- [ ] `PendingUiUpdate`
- [ ] `SubtreeRerunResult`
- [ ] `RelayoutMode`
- [ ] `UiInvalidationTrace`

## Concrete milestones

### Milestone 1: Visibility and profiling only

- [ ] land counters and debug trace output
- [ ] no behavior change

### Milestone 2: Stable IDs in emitted trees

- [ ] emitted widget trees carry stable subtree metadata
- [ ] layout preserves those ids
- [ ] no subtree reevaluation yet

### Milestone 3: Committed snapshots and dependency capture

- [ ] runtime can answer "which subtree roots depend on this reactive field?"
- [ ] still allowed to rerun full buffer effects

### Milestone 4: Subtree reevaluation behind a flag

- [ ] subtree rerun works for a narrow safe subset
- [ ] full-buffer fallback stays available

## Tests to add while implementing

- [ ] reactive read attribution test for nested subtree roots
- [ ] keyed `each` reorder identity test
- [ ] `tabs` selected-body dependency isolation test
- [ ] subtree snapshot merge test
- [ ] subtree relayout reuse test
- [ ] fallback-to-full-buffer test on incompatible root change
- [ ] hidden buffer dirty-mark-only test

## Suggested implementation order by commit

- [ ] Commit 1: runtime/VM profiling and debug trace scaffolding
- [ ] Commit 2: stable subtree id metadata in emitted trees
- [ ] Commit 3: committed snapshot structs and buffer storage
- [ ] Commit 4: dependency capture and indexes
- [ ] Commit 5: pending subtree update plumbing
- [ ] Commit 6: subtree reevaluation behind feature flag

## Explicit Subtree Owner Plan

This is the next concrete track for getting `*metal*`, `*mixer*`, and `*transport*` fast under live playback.

Problem statement:

- Today a large effect tree still owns too much UI.
- `SEQ.playhead` in `*metal*` dirties a broad step-grid region instead of just the affected step cells.
- `SEQ.track-peaks` in `*mixer*` still lives inside a track row that is too coarse.
- Consumer-side subtree upgrade helps only when an emitted tree is already subtree-shaped; it does not create finer reactive ownership by itself.

The fix is to introduce **explicit subtree owners** as real reactive reevaluation units.

Target model:

- Lisp can mark a subtree as a hot-path ownership boundary.
- The compiler preserves that as a special form, not just a visual wrapper.
- The VM begins and ends dependency capture around that owner.
- The runtime can rerun that owner directly and emit `ReplaceSubtree(...)`.

### Phase A: Add an explicit `subtree` special form

Files:

- `../eseqlisp/src/lang/compiler.rs`
- `../eseqlisp/src/lang/vm.rs`
- `../eseqlisp/src/widgets.rs`

Scope:

- [ ] Add a new Lisp special form:
  - [ ] `(subtree :key expr body)`
- [ ] Require a stable key for now.
- [ ] Treat `subtree` as a semantic ownership boundary, not merely another widget helper.
- [ ] Keep the body expression result as the emitted widget tree; do not introduce a visible extra layout wrapper unless strictly necessary.

Compiler work in `compiler.rs`:

- [ ] Add new opcodes for subtree ownership:
  - [ ] `SubtreeBegin`
  - [ ] `SubtreeEnd`
- [ ] Add `compile_subtree_form(...)`.
- [ ] Parse the form shape:
  - [ ] `subtree`
  - [ ] `:key`
  - [ ] key expression
  - [ ] body expression
- [ ] Compile the key expression before the body so the VM can derive a stable subtree owner id.
- [ ] Emit begin/end ownership opcodes around the body evaluation.

VM work in `vm.rs`:

- [ ] Add owner-stack state:
  - [ ] current subtree owner stack
  - [ ] current subtree root id stack
  - [ ] current subtree dependency capture table
- [ ] On `SubtreeBegin`:
  - [ ] evaluate/pop key value
  - [ ] derive a stable owner/root id using source buffer id + target + key
  - [ ] push subtree owner context
  - [ ] start dependency capture for that root
- [ ] On `SubtreeEnd`:
  - [ ] finalize dependency capture
  - [ ] annotate the produced body tree with the owner/root metadata
  - [ ] pop subtree owner context
- [ ] Ensure nested subtrees attribute reads to the nearest active subtree owner.

Rules:

- [ ] The subtree root id should be stable across reruns for the same key.
- [ ] Nested children inside the subtree should inherit the subtree root id unless they open another subtree owner.
- [ ] If the form is malformed or the key is unstable/unusable, fall back to current full-tree behavior and log it in debug traces.

### Phase B: Make dependency capture owner-accurate

Files:

- `../eseqlisp/src/lang/vm.rs`
- `../eseqlisp/src/buffer.rs`
- `../eseqlisp/src/runtime.rs`

Scope:

- [ ] Stop copying one effect-wide dependency set to every subtree in the emitted tree when inside explicit subtree owners.
- [ ] Record dependency sets per explicit subtree owner/root id.
- [ ] Persist those sets into committed snapshots.

Implementation details:

- [ ] `record_reactive_read(...)` should register against the nearest active subtree owner, not just the whole effect scope.
- [ ] `PendingUiUpdate::ReplaceSubtree` should carry the captured dependency set for that owner.
- [ ] Snapshot commit should preserve:
  - [ ] `subtree root id -> reactive fields`
  - [ ] `reactive field -> subtree root ids`

Success criteria:

- [ ] In `*metal*`, `SEQ.playhead` maps to specific step-cell owner ids, not one broad step-grid owner.
- [ ] In `*mixer*`, `SEQ.track-peaks` maps to the owning track-meter/track-strip subtree ids.

### Phase C: Add direct subtree rerun in the VM/runtime

Files:

- `../eseqlisp/src/lang/vm.rs`
- `../eseqlisp/src/runtime.rs`

Scope:

- [ ] Add a registry of explicit subtree owners:
  - [ ] root id
  - [ ] source buffer id
  - [ ] target buffer
  - [ ] rerun chunk/body metadata
  - [ ] parent owner id if nested
- [ ] Given dirty reactive fields, resolve affected explicit subtree owners before rerunning broad effects.
- [ ] Rerun only those owners when metadata is complete.
- [ ] Emit `PendingUiUpdate::ReplaceSubtree(...)` directly from owner reruns.
- [ ] Preserve full-buffer effect rerun fallback when:
  - [ ] owner metadata is missing
  - [ ] owner target is ambiguous
  - [ ] identity changed incompatibly
  - [ ] owner rerun errors or produces an invalid root

Instrumentation additions:

- [ ] Count:
  - [ ] explicit subtree owner reruns
  - [ ] explicit subtree owner fallback-to-full reruns
  - [ ] owner ids affected by each dirty field
- [ ] Trace:
  - [ ] why an owner rerun fell back
  - [ ] which fields mapped to which owner ids

### Phase D: Convert `*metal*` to step-cell owners

Files:

- `ui/main.lisp`

Hot-path targets:

- [ ] One subtree owner per visible step cell in the main step grid.
- [ ] Optional nested subtree owner for the playhead-sensitive label/highlight if needed after the first pass.
- [ ] Keep non-hot controls outside this first pass unchanged.

Initial rewrite shape:

```lisp
(grid :cols 16 :col-width 4
  (each (range 0 page-size) |i|
    (let ((step (step-index i)))
      (subtree :key (str "step-cell-" step)
        (step-cell i step)))))
```

Expected ownership per step cell:

- [ ] `SEQ.steps[step]`
- [ ] `SEQ.selected-steps[step]`
- [ ] visible step param value for current `param-mode`
- [ ] `(= SEQ.playhead step)`

Expected payoff:

- [ ] playhead movement should dirty only the old/new step-cell owners
- [ ] dragging a slider should dirty only the current step-cell owner, not the whole visible 16-step region

### Phase E: Convert `*mixer*` to track-strip and meter owners

Files:

- `ui/legacy/mixer.lisp`

Hot-path targets:

- [ ] One subtree owner per track row.
- [ ] Optional nested subtree owner around each `mixer-track-meter`.

Initial rewrite shape:

```lisp
(each SEQ.track-names |name i|
  (subtree :key (str "mixer-track-" i)
    (mixer-track-row name i)))
```

If track rows are still too broad:

```lisp
(subtree :key (str "mixer-track-meter-" i)
  (mixer-track-meter :level (nth SEQ.track-peaks i)))
```

Expected ownership:

- [ ] track meter owner depends on `SEQ.track-peaks[i]`
- [ ] track volume slider owner depends on `SEQ.track-volumes[i]`
- [ ] current-track styling depends on `SEQ.current-track`

Expected payoff:

- [ ] a peak change for one track should not dirty unrelated tracks

### Phase F: Convert `*transport*` to transport-playhead and master-meter owners

Files:

- `ui/transport.lisp`

Hot-path targets:

- [ ] subtree owner for the transport playhead LED/readout cluster
- [ ] subtree owner for master left meter
- [ ] subtree owner for master right meter

Suggested split:

- [ ] group the bar/beat/sixteenth labels under one owner keyed `"transport-playhead"`
- [ ] group left meter under one owner keyed `"master-meter-l"`
- [ ] group right meter under one owner keyed `"master-meter-r"`

Expected payoff:

- [ ] `SEQ.transport-playhead` changes do not dirtify the master meters
- [ ] `SEQ.master-peak-l` / `SEQ.master-peak-r` do not dirtify the transport counters

### Phase G: Validation and profiling checkpoints

Files:

- `../eseqlisp/src/runtime.rs`
- `../eseqlisp/src/editor/tests.rs`
- `ui/main.lisp`
- `ui/legacy/mixer.lisp`
- `ui/transport.lisp`

Add tests:

- [ ] nested subtree-owner dependency attribution
- [ ] subtree owner rerun emits `ReplaceSubtree(...)`
- [ ] fallback-to-full-buffer on missing owner metadata
- [ ] `*metal*` playhead change affects only bounded step-cell owners
- [ ] mixer peak change affects only the owning track row/meter owner

Profiler checkpoints to capture before/after:

- [ ] `SEQ.playhead` -> affected owner count in `*metal*`
- [ ] `SEQ.track-peaks` -> affected owner count in `*mixer*`
- [ ] subtree rerun count vs full-buffer rerun count during playback
- [ ] relayout mode for step-cell owner updates

### Recommended implementation order

- [ ] Commit 7: `subtree` special form + VM owner-stack plumbing
- [ ] Commit 8: per-owner dependency capture + direct `ReplaceSubtree(...)` emission
- [ ] Commit 9: convert `ui/main.lisp` step cells to subtree owners
- [ ] Commit 10: convert mixer track rows/meters to subtree owners
- [ ] Commit 11: convert transport playhead/meters to subtree owners
- [ ] Commit 12: subtree relayout entry point for explicit owner updates

### Success condition for this track

- [ ] While playback is running, editing a slider in a modest buffer stays responsive because playhead/meter churn is confined to separate subtree owners.
- [ ] In `*metal*`, playhead movement no longer causes broad step-grid reevaluation.
- [ ] In `*mixer*`, one track's meter movement no longer causes broad mixer reruns.
