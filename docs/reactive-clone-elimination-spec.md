# Reactive-Cycle Clone Elimination Spec

Status: rev 5 (2026-08-05) — COMPLETE; review notes added (§2.2
reader-aliasing caveat, §3.4 opt-in-assert rule). P0+P1 BUILT (e2db824b), W1 closed
at its floor; P2 BUILT (freeze registry + all §3.1 storage sites shallow);
P3 probe re-run confirms W2 storage collapsed to ~0 (§P3 results below).
Remaining cost is the intentionally-kept scoped replacement-subtree deep
clone (bursts ~3 ms/s during interaction; possible follow-up in §P3).

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

Semantics are unchanged with respect to the *writer*: the store never
aliases the caller's value. This is a ~20-line change confined to one
function.

**Reader-aliasing caveat (review, 2026-08-05):** `OpCode::LoadState`
(`vm.rs:5199`) and `read_tracked_state_value` (`vm.rs:3534`) hand out
*shallow* clones of the stored value, so programs share the store's
element Rcs. Before the patch, every changed write replaced the whole
stored value with fresh cells, severing any program-held alias at the
next write; now unchanged indices keep their cells indefinitely. A
program that mutated a loaded state value in place would silently edit
the store forever *and* defeat the `value_change_scope` diff (cell
already equal → nothing marked dirty). That failure mode already
existed between writes — not a new bug class, but the window widens
from "until next write" to "unbounded for untouched indices". If a
stale-dependent bug ever shows up on a list source, look here first.

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
Sites: the W1 family in `mark_source_dependents_dirty` — `w1:patch`
(per-index arm) and shape-split fallbacks `w1:full-map` / `w1:full-list`
/ `w1:full-other` — and the `w2:*` family wrapping every deep-clone
listed in §3.1 (`probed_deep_clone` in `vm.rs`; call sites in
`runtime.rs` and `buffer.rs`).

### Probe results (P0 baseline, 2026-08-05, large project, release)

Steady-state during playback + editing (the project that motivated the
effort). These are the W1-*after* / W2-*before* numbers P3 compares
against.

| site                       | calls/s | allocs/s (nodes) | ms/s      |
|----------------------------|---------|------------------|-----------|
| w2:buffer-set-widget-tree  | 9–11    | 56k–65k          | 4.4–5.7   |
| w2:buffer-replace-subtrees | 5–23 (bursty during interaction) | up to 44k | up to 3.6 |
| w1:patch                   | 9–18    | 16k–21k          | 1.7–2.6   |
| w2:snapshot-layout-store/commit | 1  | ~3.6k each       | ~0.3 each |
| w2:flush-replace-subtree / flush-subtree-batch | 1 | ~1.4k each | ~0.1 each |
| w1:full-other              | 53–96   | = calls (1 node/call) | ~0.02 |
| w1:full-map / w1:full-list | absent  | —                | —         |

**W1 verdict — closed at its floor.** The residual is the patch arm
working as designed: ~1.2k–2k nodes cloned per call means list sources
whose *elements* are large structures that genuinely changed (step maps
with p-locks). Maps never hit the fallback, so the §2.2-deferred
key-scoped `ValueChange` variant is NOT needed. `w1:full-other` is
scalar sources at 1 node/call — free. Going deeper would mean recursive
intra-element diffing; not justified at ~2.3 ms/s. Optional cheap
diagnostic if this ever grows: log the source label when a patch call
clones >500 nodes — patch cadence tracks the reactive-save cadence, so
one producer rebuilding elements wholesale (where a targeted write would
do) is the plausible upstream fix.

**W2 verdict — confirmed as the target.** `buffer-set-widget-tree`
alone deep-copies a ~6k-node tree ~10×/s (~0.5 ms per call, landing as
latency spikes inside reactive cycles that are already doing eval/layout
work); with `buffer-replace-subtrees` bursts, W2 totals ~8–9 ms/s vs
W1's ~2.3. Proceed with §3.2/§3.4, freeze assertions first.

### Probe results (P3, after P2 conversion, 2026-08-05, same project, release)

| site                       | calls/s | allocs/s | ms/s        | vs baseline |
|----------------------------|---------|----------|-------------|-------------|
| w2:buffer-set-widget-tree  | 9–11    | 9–11 (1/call) | 0.003–0.008 | was 4.4–5.7 |
| w2:buffer-replace-subtrees | 20 (one interaction burst) | 33k | 3.2 | unchanged (kept deep, §3.2) |
| w2:flush-* / snapshot-*    | 1       | 1/call   | ~0.001 each | was ~0.1–0.3 |
| w1:patch                   | 9–18    | 16k–22k  | 1.8–2.7     | unchanged (floor) |
| w1:full-other              | 141–164 | = calls  | ~0.03       | unchanged (free) |

**Verdict: W2 closed.** Every converted storage site is one Rc bump per
call; steady-state W2 copy work dropped from ~5–6 ms/s to ~0, total W2
(including interaction bursts) from ~8–9 ms/s to ~3. The surviving cost
is exactly the scoped replacement-subtree deep clone this spec chose to
keep (buffer.rs:1917/1972). The freeze registry caught zero violations
across the eseqlisp lib + UI-script suites, confirming the P0 audit.

**Possible follow-up (not done, would need its own review):** under the
freeze invariant the replacement trees arriving at buffer.rs:1917/1972
are already frozen at the VM push, so the splice could share them
instead of deep-cloning — erasing the remaining ~3 ms/s bursts. Kept
deep for now because those sites are the scoped-to-mutation discipline
this spec standardizes on; flipping them widens the invariant's blast
radius from "storage copies" to "merged trees share replacement cells
with pending-queue entries" and should be probed/justified separately.

### 3.4 Enforcement (debug-only)

Add `#[cfg(debug_assertions)]` freeze checking so the invariant survives
future edits: a thread-local "frozen trees" registry keyed by the
top-level `Rc` pointer, checked by a debug assertion in the (few) tree
mutation helpers; freezing happens where trees are handed to the runtime.
Cheap, zero release cost, and turns a silent aliasing bug into a panic in
dev runs and the UI-script test suite.

**Enforcement is opt-in per mutation helper**: any new
`*cell.borrow_mut() = …` write on a `Value` cell must call
`debug_assert_cell_not_frozen` first, or it silently bypasses the
invariant. Reviewers should enforce this on every new cell-write path.

### 3.5 Pre-annotation subtree cache sealing (2026-08-27 follow-up)

Identity-preserving stable-id annotation adds a narrower, release-build
invariant. `SubtreeRenderCache` values are pre-annotation Lisp graphs, and the
annotation memo is valid only while those cells remain unchanged. Such inputs
are sealed in a separate weak-cell registry when cached. The same mutation
helper checks this registry in every build (and fails loudly on violation),
while ordinary committed-tree freezing remains debug-only and zero-cost in
release. This avoids paying a production freeze walk for every posted tree and
makes pointer-keyed annotation reuse sound rather than probabilistic.

## 4. Phasing

- **P0**: W2 audit (3.3). Output decides P2 scope. Also wire the release
  probes around both clone sites to get a big-project baseline number.
- **P1**: W1 incremental patch (2.2) + a `value_change_scope`/patch unit
  test: list source, one index write → exactly one element cloned, dirty
  scope unchanged; length-change and shape-change fallbacks covered.
- **P2**: W2 conversion, **freeze assertions FIRST** (ordering amended
  after the audit): land the §3.4 debug freeze registry before flipping
  any site, so the three latent hazards listed in §3.3 surface as dev
  panics rather than stale-UI bugs. Then convert site-by-site:
  `buffer.rs:952` and the `runtime.rs` deep-clone cluster → shallow,
  mutation sites → scoped deep-clone. **Constraint (audit corollary):
  annotation must stay on the storage path** — the per-prop deep clone
  in `annotate_widget_tree_stable_ids` (`vm.rs:1620`) is what severs
  sharing with the reactive store's cells.
- **P3**: re-run the Allocations capture + probes on a large project;
  compare against the §Probe-results baseline and update this spec.

P1 is independent of P0/P2 and shipped first (e2db824b).

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
