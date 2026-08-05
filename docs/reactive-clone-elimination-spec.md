# Reactive-Cycle Clone Elimination Spec

Status: DRAFT (rev 1, 2026-08-05)

Eliminate the two per-tick `Value::deep_clone` hot paths found in the
Instruments Allocations capture (2-track sampler project, ~28k allocs /
2.4 MB per window, all main-thread):

- **W1 — DAG source store**: `VM::mark_source_dependents_dirty`
  (`crates/eseqlisp/src/lang/vm.rs:4098`) deep-clones the entire new value
  into the `ReactiveNode::Source` store on every changed write.
  ~3.2k allocs / 244 KB in the capture.
- **W2 — widget-tree snapshots**: `refresh_runtime_side_effects` and the
  pending-tree machinery deep-copy whole rendered widget trees, sometimes
  twice back-to-back. ~10.9k allocs / 877 KB in the capture
  (`Editor::save_current_widget_tree` → `buffer.rs:950`, plus the
  `runtime.rs` cluster at 2964/3055/3082/3336/3401/3468/3494/3497/3506/3526
  and `Buffer` replacement trees at `buffer.rs:1907/1959`).

## 1. Motivation and sizing (be honest)

This is a scalability fix, not a hot-spot fix. Measured cost today is
sub-millisecond per tick. The problem is the shape: both sites do
O(project-size) copy work per reactive cycle, which is exactly the pattern
that produced the 22ms fx-panel rock and the pool-clone cost fixed in the
clip-launch round. Goals:

- Source-write cost becomes O(changed indices), not O(list length).
  A 512-step list with one playhead cell changing should clone one cell.
- Storing a rendered widget tree becomes an Rc bump, not an O(tree) copy.
- Secondary: less short-lived heap churn per frame → less cache pollution
  attributed to layout/eval by the profiler.

Acceptance measurement: wrap both sites with the existing release-probe
timing pattern; on a large project, cumulative clone time per second of
playback should drop to ~0, and the Allocations capture should lose the
`deep_clone` rows under `run_reactive_cycle` / `refresh_runtime_side_effects`.

## 2. W1 — DAG source store: patch changed indices, don't re-clone the world

### 2.1 Why NOT the pure fingerprint design

First idea was replacing `ReactiveNode::Source { value }` with a hash
digest. **Rejected**: the Source node is the authoritative state store,
not a diffing shadow. Readers that serve real values from it:

- `OpCode::LoadState` (`vm.rs:4929`) — program reads of defstate.
- `VM::read_tracked_state_value` (`vm.rs:3325`).
- `mark_owner_path_dirty` re-marks using the stored value (`vm.rs:4148`).

So the stored `Value` must remain. The equivalent win with the store kept:

### 2.2 Design: incremental patch on write

In `mark_source_dependents_dirty` (`vm.rs:4088`):

- `value_change_scope(current_value, &value)` already computes exactly
  which list indices changed (`ValueChange::Indices`) or `Full`.
- On `ValueChange::Indices(changed)` where both old and new are
  `Value::List`: instead of `*current_value = value.deep_clone()`, patch
  in place — for each changed index `i` (skipping `LEN_READ_SENTINEL`),
  `old_items[i] = Rc::new(RefCell::new(new_items[i].borrow().deep_clone()))`.
  Handle length change by truncating / extending with deep-cloned tail
  elements.
- On `ValueChange::Full` (shape change, non-list, or map): keep the
  existing whole-value `deep_clone`. Maps could get the same treatment
  later (`ValueChange` would need a key-scoped variant); out of scope for
  rev 1 — list sources (step buffers, playheads, meters) are where the
  per-tick volume is.

Semantics are unchanged: the store still holds a private deep copy,
aliasing guarantees identical, readers untouched. This is a ~20-line
change confined to one function.

### 2.3 Optional sidecar (defer unless probes justify)

A `Vec<u64>` per-index hash sidecar does NOT beat the existing compare:
equality short-circuits on the first difference and is the same order as
hashing the new element. Only revisit if a future need arises to diff
without holding the old value. Recorded here so the idea isn't re-derived.

## 3. W2 — widget trees: immutable-after-annotation, share the Rc

### 3.1 Current state

Rendered trees are `Value` graphs (`Rc<RefCell<...>>` children). Storage
sites are inconsistent — some already shallow-share, some deep-copy:

- Shallow `tree.clone()` already used: `runtime.rs:3111, 3146, 3185, 3213`
  and the getter `current_widget_tree()` (`runtime.rs:2877`).
- Deep `tree.deep_clone()`: `runtime.rs:926, 938, 2964, 2966, 3055, 3058,
  3082, 3085, 3336, 3401, 3468, 3494, 3497, 3506, 3526`;
  `buffer.rs:950` (`set_widget_tree`), `buffer.rs:1907, 1959`.

The half-shallow status quo means either (a) trees are already effectively
immutable after evaluation and the deep clones are dead weight, or (b)
there is a post-render mutation path and the shallow sites are latent
bugs. Phase 0 decides which.

### 3.2 Design: freeze-after-annotation discipline

- **Invariant**: a widget tree is mutable only during evaluation
  (construction + `annotate_widget_tree_stable_ids`, `vm.rs:1482`, which
  runs inside `execute_with_frames` per the Instruments stacks). From the
  moment a tree enters `pending_widget_trees` / `current_widget_tree` /
  `Buffer::widget_tree`, it is read-only.
- All storage/copy sites listed in 3.1 become shallow `clone()` (Rc bumps
  on the top-level list/map). No `Value` type change in rev 1 — wrapping
  trees in a dedicated `Rc<WidgetTree>` newtype is a possible follow-up,
  not needed for the win.
- Consumers that need a *mutated variant* of a stored tree (the
  `buffer.rs:1907/1959` replacement-tree path, subtree splicing) deep-clone
  **at the mutation site, scoped to the subtree they modify**, not at the
  storage site. Copy work moves from every-cycle to actual-edit.

### 3.3 Phase 0 audit (gates everything in W2)

Enumerate writers that touch a tree after storage. Known suspects to
check, from the perf rounds that touched these paths:

- layout passes that write back into widget `Value`s (in-place subtree
  layout reuse from the full-layout-selection round — did it mutate the
  tree or a separate layout store?),
- stable-id / subtree-key annotation running a second time on a stored
  tree,
- the merged-lane / subtree splice paths (`merged.tree` at
  `runtime.rs:3185/3213` — already shallow, so if splicing mutates in
  place there is a live aliasing bug today worth knowing about
  regardless of this spec),
- `Buffer::replace_widget_subtree`-style editing (`buffer.rs:1907/1959`).

Deliverable: a list of post-storage mutation sites, each either proven
absent or moved behind a scoped deep-clone.

### Audit findings (P0, 2026-08-05)

**Headline: no live aliasing bug found.** Every path that touches a tree
after it reaches `pending_widget_trees` / `current_widget_tree` /
`Buffer::widget_tree` is either read-only or a purely functional rebuild.
Strongest single fact: `buffer.rs`, `ui/layout.rs` and `tile.rs` contain
zero `borrow_mut()` calls, and all 83 in `runtime.rs` are on
`self.shared` (`RuntimeShared`) or a thread-local shader cache
(`runtime.rs:663`) — no tree `Rc<RefCell<Value>>` cell is ever mutably
borrowed in the storage/layout/splice layer.

Per-suspect verdicts:

- **In-place subtree layout reuse — proven-clean.** The full-layout-
  selection round mutated the `LayoutNode` store, not the tree.
  `reuse_layout_node_for_subtree_paths_in_place` (`ui/layout.rs:1157`)
  writes via `Arc::make_mut` on `Arc<LayoutNode>` (`layout.rs:1172`);
  the tree enters as `&Value` (immutable by type). Plan phase
  (`plan_layout_reuse_at_paths`, `layout.rs:1177`) is read-only; apply
  phase (`layout.rs:1290`) writes only `LayoutNode` fields.
  `relayout_subtree_path_result` (`layout.rs:1310`) builds a fresh
  `LayoutNode`. `relayout_current_layout_for_subtrees`
  (`runtime.rs:3221`) holds a shared borrow of the tree across the whole
  loop — possible only because every callee is `&self`/`&Value`.
- **Repeated stable-id annotation — proven-clean, twice over.**
  `annotate_widget_tree_stable_ids` (`vm.rs:1563`) is purely functional:
  it builds a fresh map, deep-cloning every non-children prop cell
  (`vm.rs:1620`) and rebuilding children into new cells; the input is
  never mutably borrowed. And it never runs on a stored tree anyway: its
  only call sites (`vm.rs:4395`, `vm.rs:5405`) annotate freshly rendered
  trees immediately before they enter `pending_widget_trees`.
  `annotate_explicit_subtree_root` (`vm.rs:1490`): same construction,
  both call sites pre-storage.
  - **Corollary (load-bearing for P2):** the per-prop deep clone at
    `vm.rs:1620` is what severs sharing with the reactive store —
    `OpCode::LoadReactive` (`vm.rs:5169`) pushes the namespace's shared
    `Rc<RefCell<Value>>` (Rc bump), and `ReactiveStore::set` writes
    through that same cell (`reactive.rs:231`). Annotation makes the
    stored tree privately owned. If P2 ever moves annotation off the
    storage path, that becomes a real aliasing bug.
- **Merged-lane / subtree splice — proven-clean; the spec's suspicion of
  a live bug at `runtime.rs:3185/3213` is NOT borne out.**
  `replace_subtree_in_value` (`buffer.rs:1898`) and
  `replace_subtrees_in_value` (`buffer.rs:1949`) are purely functional:
  deep-clone of the *replacement* at the hit (`buffer.rs:1907/1959`),
  fresh `Value::Map` rebuild on the path up, untouched siblings shared
  by Rc bump but never written. `replacing_subtree`/`replacing_subtrees`
  (`buffer.rs:1612/1637`) take `self` by value and build a whole new
  snapshot. The shallow `merged.tree.clone()` stores are safe because
  `merged.tree` is freshly built.
- **`Buffer` replacement-tree paths — proven-clean, and already the
  target discipline.** The deep clones at `buffer.rs:1907/1959` are
  scoped exactly to the incoming replacement subtree; the stored tree is
  only structurally shared. `Buffer::replace_widget_subtree(s)`
  (`buffer.rs:1003/1029`) store shallow and restore the snapshot
  untouched on failure. `Buffer::set_widget_tree`'s deep clone
  (`buffer.rs:952`) is redundant per this audit — P2 conversion
  candidate.
- **Undo/snapshot paths — proven-clean.** `Runtime::snapshot_state` /
  `restore_state` (`runtime.rs:1733/1753`) are plain field moves;
  post-restore consumers are read-only or functional.
  `layout_snapshot_for_tree_with_geometry_and_offset`
  (`runtime.rs:2931`) installs deep clones, relayouts, restores saved
  handles verbatim. `VM::snapshot_state` (`vm.rs:3218`) shallow-clones
  `pending_widget_trees`, so snapshot and live entries share tree Rcs —
  safe only under the freeze invariant; cover in §3.4.
- **Other writers (broad `borrow_mut` sweep) — proven-clean.** The one
  genuine in-place `Value::Map` prop write in the UI layer is
  `set_map_bool` (`widget_render/timeline.rs:4320`), but it writes into
  `WidgetGesture::gesture_data`, a drag-scoped map built fresh from
  scalars in `begin_gesture` (`timeline.rs:3082`, `timeline.rs:4352`) —
  no cell originates from the tree. `reactive.rs:232/320/343` write the
  reactive store, which trees never embed post-annotation (corollary
  above). Everything else is thread-local per-widget UI state/caches
  keyed by `widget_id`. The one true post-storage mutation of a
  `pending_widget_trees` entry, `attach_reactive_dependencies_to_
  pending_trees` (`vm.rs:3917`), assigns only the
  `reactive_dependencies` metadata field, never `.tree`.
- **Metal backend / tile cache — reads only.** `metal_backend.rs` has
  two `borrow_mut()` (lines 77/82), both on the font-measure cache; all
  widget access is pattern-matching on `LayoutNode.props`. `tile.rs`:
  zero. `render_widget_tree` (`widget_render/mod.rs:1121`) never sees a
  tree `Value` at all.

**Latent hazards (not bugs today — encode in §3.4 enforcement):**

1. `LayoutNode.props` aliases tree cells: `collect_props` → `get_map`
   (`ui/layout.rs:1824/1835`) does a shallow `Value` clone, so
   container-valued props in `LayoutNode.props` share Rc cells with the
   stored tree. Nothing writes through them today; a future widget that
   does would edit the committed tree for every holder. Most likely
   place a future regression lands — the freeze registry should cover
   it.
2. The splice preserves prop-cell sharing: `replace_subtree(s)_in_value`
   copy non-children props shallowly (`buffer.rs:1939/1992`), so merged
   trees share prop cells with the pre-splice snapshot tree. Correct
   under the freeze invariant; fatal without it.
3. `VM::snapshot_state` shallow-shares pending trees (`vm.rs:3218`) —
   include in the freeze registry.

### Probe wiring (P0)

Both clone families are instrumented behind `ESEQLISP_PROFILE_CLONES=1`
(near-zero cost when unset), following the `ESEQLISP_PROFILE_UI`
release-probe pattern: per-site cumulative time and allocation counts
(cloned Value nodes ≈ `Rc<RefCell<..>>` allocations) emitted once per
second as `[clone-probe] site=<name> calls/s=<n> allocs/s=<n> ms/s=<n>`.
Sites: `w1:dag-source-store` (`vm.rs`, covers both the patch and the
full-clone fallback) and the `w2:*` family wrapping every deep-clone
listed in §3.1 (`probed_deep_clone` in `vm.rs`; call sites in
`runtime.rs` and `buffer.rs`).

Baseline capture: run a release build with playback on a large project —
`ESEQLISP_PROFILE_CLONES=1 cargo run --release ...` — and record the
steady-state `[clone-probe]` lines during ~30s of playback. Not captured
yet in this round (needs an interactive session with a real project);
P3 re-runs this alongside the Allocations capture.

### 3.4 Enforcement (debug-only)

Add `#[cfg(debug_assertions)]` freeze checking so the invariant survives
future edits: a thread-local "frozen trees" registry keyed by the
top-level `Rc` pointer, checked by a debug assertion in the (few) tree
mutation helpers; freezing happens where trees are handed to the runtime.
Cheap, zero release cost, and turns a silent aliasing bug into a panic in
dev runs and the UI-script test suite.

## 4. Phasing

- **P0**: W2 audit (3.3). Output decides P2 scope. Also wire the release
  probes around both clone sites to get a big-project baseline number.
- **P1**: W1 incremental patch (2.2) + a `value_change_scope`/patch unit
  test: list source, one index write → exactly one element cloned, dirty
  scope unchanged; length-change and shape-change fallbacks covered.
- **P2**: W2 conversion site-by-site: `buffer.rs:950` and the
  `runtime.rs` deep-clone cluster → shallow, mutation sites → scoped
  deep-clone, debug freeze assertions on.
- **P3**: re-run the Allocations capture + probes on a large project;
  update this spec with before/after.

P1 is independent of P0/P2 and can ship first.

## 5. Risks / gotchas

- **Aliasing regressions in W2** are the real risk: a missed post-storage
  mutator silently edits history for every holder of the Rc. Mitigations:
  Phase 0 audit + debug freeze assertions + the UI-script test pattern
  (drive real handlers, assert rendered output) rather than layout tests
  only — `each`-vs-`map`-style bugs showed layout tests can pass while
  live behavior breaks.
- **Undo/snapshot paths**: `runtime.rs:1733/1753` snapshot/restore
  `current_widget_tree` via shallow clone already — consistent with the
  new discipline, but include in the audit.
- **W1 `LEN_READ_SENTINEL`**: the changed-index list can contain the
  sentinel; the patch loop must skip it and handle truncate/extend
  explicitly or it will index out of bounds.
- **W1 in-place borrow**: patching `old_items[i]` while `new_items[i]`
  is borrowed is fine (distinct Rcs), but the `Full` fallback must not
  run after a partial patch — compute the decision before mutating.
- Trees handed to the Metal backend / tile cache: confirm the renderer
  reads only (expected — render caching keys off fingerprints/epochs).
