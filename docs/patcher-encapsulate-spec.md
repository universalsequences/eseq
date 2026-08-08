# Patcher: Encapsulate Selection into a `defmacro` (Cmd+E)

Status: design, unbuilt
Owner: alec
Related: `docs/patch-vs-code-editor-spec.md`, `docs/PATCHER_SPEC.md`,
`docs/patch-macro-sidebar` work, `~/code/swift/patch-editor/Sources/Engine/SubpatchEncapsulator.swift`

## 1. Problem

Select a handful of nodes, press Cmd+E, and they collapse into one node calling
a new `defmacro` that contains them. The macro's parameter list and return
value are inferred from which cables crossed the selection boundary — the same
trick Max/MSP does for "Encapsulate to subpatch".

Today the only ways to get a macro are typing `defmacro foo` into a fresh node
(which gives you an empty scaffold you then rebuild by hand) or asking the
agentic bubble (Cmd+K) to write one. Neither lets you factor out patching you
already did.

## 2. Prior art: the Swift patch editor

`SubpatchEncapsulator.encapsulate(selectedNodeIds:in:at:name:)` is 448 lines
with a clear spine. Reading it, the algorithm is:

1. **Bounds** — bounding box of the selection; the new `p` node lands at its
   center.
2. **Create the container** — a `p` operator node, its `subpatch` typed to
   match the parent (`core` / `gen`).
3. **Classify every connection** in the parent patch against the selection
   (`analyzeConnections`), into three buckets:
   - `fromSelected && toSelected` → **internal**, moves into the subpatch verbatim.
   - `!fromSelected && toSelected` → **external inlet**, keyed by
     `"\(fromNodeId)-\(fromOutlet)"`. One external *source port* = one subpatch
     inlet, so a node fanning out to three internal nodes yields **one** inlet
     that fans out inside.
   - `fromSelected && !toSelected` → **external outlet**, keyed by
     `"\(toNodeId)-\(toInlet)"`. One external *destination slot* = one outlet.
4. **Size the container** — `setInletOutletCounts(inletCount, outletCount)`,
   each floored at 1 (`max(1, nextInletIndex)`).
5. **Create interface operators** — `in 1..N` along the top of the subpatch,
   `out 1..M` along the bottom.
6. **Delete** every parent connection touching a selected node.
7. **Move nodes** — reposition relative to the bounds min plus padding, remove
   from the parent's node table, insert into the subpatch, update global
   ownership.
8. **Recreate connections** in four passes: internal edges inside the subpatch;
   `in K → internal destination`; `internal source → out K`; and in the parent,
   `external source → p:inletK` / `p:outletK → external destination`, each
   deduplicated by the same composite key used to allocate the index.

The load-bearing ideas we keep:

- **Three-bucket classification by selection membership.** Exact, cheap, and
  the whole thing.
- **Ports are allocated by dedup key, not per-connection.** Fan-out collapses
  to one port. Getting this wrong gives you a subpatch with six identical
  inlets.
- **Delete-then-recreate**, never mutate edges in place.
- **Positions are made relative to the bounds min** so the subpatch opens
  looking like what you selected.

What we deliberately change:

- **Outlet keying.** Swift keys outlets by external *destination*
  (`toNodeId-toInlet`). In eseq an input slot holds at most one cable
  (`inbound` is a `BTreeMap<(to_node, to_input), &PatchConnection>` in
  `generate.rs:247`), so destination-keying can never merge anything — but it
  *can* wrongly split: one internal node feeding two external nodes produces
  two outlets carrying the identical signal. We key outlets by the **internal
  source port** `(from_node, from_output)`, mirroring the inlet rule. One
  outlet, two parent cables.
- **Ordering.** Swift's indices fall out of `for connection in
  patchGraph.connections` — allocation order is whatever the array order is.
  We need determinism (the generator's whole contract is deterministic
  regeneration), so ports are sorted geometrically. See §5.3.
- **Legality.** Swift's `p` subpatch is not atomic to the signal graph, so a
  non-convex selection is harmless there. A DGenLisp macro *is* atomic — it
  becomes one `(def x (name ...))` call — so collapsing a non-convex selection
  creates a genuine cycle. We check. See §6.
- **`max(1, count)`.** A DGenLisp macro with zero parameters is legal; we do
  not floor the inlet count. We do floor outlets at 1, because a macro must
  return something.

## 3. What "the subpatch" is here

There is no `p` object. The container is a **local `defmacro`** and the
collapsed node is a **macro instance**:

```lisp
(defmacro fbk (input1 input2)
  (def a (* input1 twopi))
  (def b (+ a input2))
  (cos b))

;; …at root:
(def fbk1 (fbk osc_phase feedback_amt))
```

Three facts about the patcher's model make this tractable, all of them
already true:

1. **The model is the source of truth.** `generate.rs` regenerates the entire
   `dsp.lisp` from the `Patch` alone — "never prior source text or
   `NodeSource` positions" (`generate.rs:1-9`). Encapsulation therefore only
   has to produce a correct *model*; the text writes itself.
2. **A macro's signature is derived from its body nodes, not from a stored
   parameter list.** `ScopeEmitter::macro_param_list` (`generate.rs:726`)
   builds the `(defmacro name (…))` list purely from `NodeKind::In` nodes in
   the macro scope, ordered by channel; `emit_macro_return`
   (`generate.rs:747`) builds the return from `NodeKind::Out` nodes, emitting a
   `(tuple …)` when there is more than one. So creating `in`/`out` nodes *is*
   creating the interface.
3. **Every edit already lives in `PatchEditState`, scoped by view key.**
   Created nodes, created connections, deleted nodes, and created macros are
   all folded into the live model by
   `sidecar::root_patch_with_interaction` (`sidecar.rs:515`), which applies
   `patch_with_interaction_state` once per scope (`"root"`, `"macro:<name>"`).

Encapsulation is therefore **one `PatchEditState` mutation**. No source
rewriting, no writeback path, no new persistence format.

## 4. UX

- **Trigger:** Cmd+E with ≥1 node selected and no text edit open, in the
  patcher key handler (`patcher/mod.rs:1019` `key_event`), alongside the
  existing Cmd+Enter / Cmd+Up / Cmd+C / Cmd+V / Cmd+K cases.
- **Result:** the selected nodes vanish from the current view, replaced by a
  single macro-instance node at the selection's top-left. It is selected, and
  its text editor opens with the generated name pre-filled and fully selected,
  so typing replaces it. Esc keeps the generated name.
- **Feedback:** returns `patcher_semantic_event(true)` — the same
  regenerate-and-recompile payload every other structural edit returns.
- **Navigating in:** Enter on the selected instance opens the macro view
  (`open_selected_macro_node`, `interaction.rs:932`) — already works for any
  macro instance including created ones.
- **Undo:** Cmd+Z. Encapsulation is a single non-gesture store through
  `set_patcher_interaction_state`, so `record_patcher_history_transition`
  commits it as exactly one undo step with no extra work.
- **Contextual menu:** out of scope. When the patcher grows one, "Encapsulate"
  calls the same entry point.

## 5. Algorithm

Input: the projected, edit-applied patch for the active view
(`active_patcher_patch` → `patch_with_interaction_state`), the active
`view_key`, and `state.selected_nodes`.

### 5.1 Normalize the selection

Let `S` = `selected_nodes` ∩ live node ids, then remove:

- **Hidden inline `mod` accessors** (`hidden_inline_node_ids`). They are not
  user-visible and must follow their consumer, not be selected independently.
  Handled in §6.4.
- Nodes whose display text is empty (a freshly created, never-typed node) —
  same guard `copy_selected_patcher_nodes` uses (`interaction.rs:1261`).

Then **reject** the whole operation if `S` contains any node whose kind is
`Param`, `In`, `Out`, `MacroDefinition`, or `CodeIsland` (§6.1). Bail with no
state change if `S` is empty after normalization.

### 5.2 Classify connections

For every connection `c` in the view's patch (skipping any touching a hidden
inline node, per `connection_touches_hidden_inline_node`):

| `from ∈ S` | `to ∈ S` | bucket |
|---|---|---|
| yes | yes | **internal** |
| no  | yes | **crossing-in** |
| yes | no  | **crossing-out** |
| no  | no  | ignored |

Allocate ports by dedup key:

- `inlet_key(c) = (c.from_node, c.from_output)` — an ordered map
  `inlet_key → inlet_index`.
- `outlet_key(c) = (c.from_node, c.from_output)` — note this is the **internal**
  source port for crossing-out edges; likewise an ordered map
  `outlet_key → outlet_index`.

`inlet_count = inlet_map.len()`, `outlet_count = max(1, outlet_map.len())`.

### 5.3 Deterministic port order

Sort the inlet keys before assigning indices, by:

1. the canvas position of the **internal destination** the edge first reaches —
   `(y, x)` of the `to_node`, rounded to 0.5 cells to absorb float noise;
2. then `to_input`;
3. then the external source node id (lexicographic tiebreak).

Where one external source feeds several internal nodes, use the
lexicographically-smallest `(y, x, to_input, to_node)` among its edges as the
sort key. Outlets sort the same way on the **internal source** `(y, x)` then
`from_output` then `from_node`.

This makes inlet 1 the top-left entry point and outlet 1 the top-left exit,
which is what "reads left-to-right like the canvas" means here, and it is
stable across runs (required: the generator's output must be a pure function of
the model).

### 5.4 Build the macro body

New view key: `macro:<name>` (name from §7).

Let `origin = (min_x, min_y)` over `S`'s node rects. Body layout:

- `in K` nodes at `(origin.x + (K-1) * IN_SPACING_CELLS, origin.y - IN_ROW_CELLS)`
  before rebasing.
- `out K` nodes below the lowest selected node, similarly spaced.
- Each selected node keeps `position - origin + (PAD_X, PAD_Y)`, where the
  padding leaves room for the `in` row.

For each node in `S`, in a deterministic order (sorted by `(y, x, id)`):

- allocate a created node in the macro view
  (`allocate_created_node_avoiding(state, macro_view_key, position, taken_ids)`,
  where `taken_ids` are the macro scope's existing node ids — empty for a fresh
  macro, but pass it anyway so the helper stays honest);
- set `edit.text = node_display_label(patch_node)` and `edit.width =
  patch_node.width`.

This is exactly the fidelity contract paste already ships with
(`interaction.rs:1306`): a node round-trips through its editable header text.
Record `old_id → new_id`.

For each interface port:

- allocate a created node with text `in K` (inlet K) or `out K` (outlet K).
  Bare `in K` is enough — `macro_param_name` (`generate.rs:807`) falls back to
  `input{channel}` when the label carries no `@name`, and
  `macro_signatures_with_visual_edits` (`state.rs:1621`) already grows a created
  macro's param list from created `in N` nodes.

Then create connections in the macro view via `allocate_created_connection`:

- **internal**: `new_id[c.from_node]:c.from_output → new_id[c.to_node]:c.to_input`.
- **crossing-in**: for every crossing-in edge `c`, `in_node[inlet_index(c)]:0 →
  new_id[c.to_node]:c.to_input`. Multiple edges from one external source each
  get their own cable from the same `in` node — that is the fan-out.
- **crossing-out**: for every distinct outlet key,
  `new_id[from_node]:from_output → out_node[outlet_index]:0`.
- **degenerate case** (`outlet_map` empty — nothing left the selection): pick
  the internal node with no internal consumers, lowest `(y, x, id)`, and wire it
  to `out 1`. The macro still returns a value and the instance is simply
  unconnected at root.

### 5.5 Rewrite the root view

In the current view (`view_key`, usually `"root"`):

1. **Create the instance node** at `origin`, text = macro name.
   `refresh_macro_instance_outputs` gives it the right port shape once the
   signature resolves.
2. **Delete the selected nodes**: set `state.selected_nodes = S` and call
   `delete_selected_nodes(state, view_key)` (`state.rs:1121`) — it already
   removes created-node edits outright, marks source-backed ones deleted, and
   drops created connections touching them. Source-backed *connections* into or
   out of `S` need no explicit deletion: `patch_with_interaction_state` prunes
   any connection whose endpoint is not live (`state.rs:1277`).
3. **Wire the instance**: for each distinct inlet key,
   `external_source:from_output → instance:inlet_index`; for each distinct
   outlet key, `instance:outlet_index → external_destination:to_input`, once per
   external destination that the outlet's internal source port fed.
4. `note_touched_node(state, &instance_id)` and
   `bring_nodes_to_front(state, view_key, &[instance_id])`.
5. `state.selected_nodes = {instance_id}`; open `state.text_edit` on it.

### 5.6 Register the macro

```rust
state.edit_state.created_macros.insert(name.clone(), PatcherMacroEdit {
    name: name.clone(),
    instance_node_id: instance_id.clone(),
    source: Some(empty_created_macro_source(&name)),
});
```

`empty_created_macro_source(name)` is new:

```rust
fn empty_created_macro_source(name: &str) -> String {
    format!("(defmacro {name} ())")
}
```

The existing `default_created_macro_source` emits `(defmacro name (input) (* input 1))`,
whose scaffold body would appear as junk nodes inside our macro.
`project_defmacro` (`project.rs:324`) takes `body = &items[3..]`, which is
empty here, so this projects to a `MacroPatch` with zero nodes — a clean shell
for the created-node edits to populate.

**Verify before building:** that an empty-body `defmacro` survives
`parse_patch_source` and `patch_is_fully_projectable`, and that
`infer_macro_outputs(&[])` returns an empty vec rather than panicking. If the
empty body is rejected, fall back to `(defmacro {name} () 0)` and add the
projected constant node's id to `deleted_nodes` for the macro view.

The seed source never reaches disk: `generate_patch_source` re-emits every
`MacroOrigin::Local` macro from its `Patch` model (`generate.rs:56-91`), and by
then the model carries our created nodes.

## 6. Legality rules

Encapsulation refuses, atomically and with no partial state, when any of these
hold. (v1 refuses silently — returns `false`, no event. A later slice can
surface the reason as a node diagnostic.)

### 6.1 Scope-bound node kinds

`Param`, `In`, `Out` cannot move into a macro scope:

- `(param gain …)` declares a host-visible parameter with a positional index in
  the project format. Inside a macro it would be re-declared per instantiation.
- `(in N @name gate)` is the instrument's audio/control input; a macro's `in`
  means something else entirely (`NodeKind::In if is_macro => NodeRole::MacroParam`,
  `generate.rs:233`).
- `(out …)` is the instrument's output.

`MacroDefinition` and `CodeIsland` nodes are likewise refused —
`ScopeEmitter::new` already errors on a code island (`generate.rs:226`).

These nodes are the *normal* case on the boundary: a `param` feeding a selected
node simply becomes an inlet fed by the param at root. That is the desired
behavior and needs no special handling.

### 6.2 Convexity — the cycle check

A macro instance is one atomic call. If there is a path

```
selected → external → selected
```

then collapsing produces `instance → external → instance`, a cycle the
generator's topological ordering (`generate.rs` `ordered` / `collect_value_deps`)
cannot emit and DGenLisp cannot compile.

Check: build the view's forward-edge DAG (skip `ConnectionKind::Feedback`),
contract `S` to a single vertex, and refuse if the contracted graph has a
cycle. Equivalently and more cheaply: refuse if any external node is reachable
from `S` and also reaches `S`.

This is the one place we are strictly stricter than Max, and it is the check
Swift never needed.

### 6.3 History nodes must not straddle

A `NodeKind::History` node is the whole `make-history` / `read-history` /
`write-history` triple in one model node: the write edge is the
`ConnectionKind::Feedback` connection into input 0 (`project.rs:717`), the read
is its output.

Macros may own histories — `latch_on_trigger` in
`instruments/core/triton/dsp.lisp:29` does exactly that, and each expansion
gets its own. So a history moving wholly inside is fine, and a history staying
wholly outside is fine.

Refuse when the history node is **inside** `S` but its feedback write edge
originates outside, *and* it is also read outside — the state would have to
cross the instance boundary in both directions, which §6.2 already catches for
forward edges but not for the feedback edge (which §6.2 deliberately skips).
Treat a crossing feedback edge as follows:

- feedback edge **from outside into an inside history**: legal; the written
  value becomes an ordinary inlet. The macro's `write-history` reads its param.
- feedback edge **from inside to an outside history**: legal; becomes an
  ordinary outlet.
- both directions on the same history: refuse.

### 6.4 `param~` inline modulation

`(* x gain~)` is UI sugar desugared in the model into a hidden `(mod gain)`
accessor node feeding the consumer with `InputPresentation::InlineModParam`
(`state.rs`, `desugar_editor_mod_suffix_args`). `mod` takes a real modulatable
param name; a macro parameter is not one.

Handling: when a selected node has an `InlineModParam` input, the hidden `mod`
accessor **stays at root** and becomes a crossing-in source. Two things make it
work without new machinery:

- the new instance-inlet connection is created by `allocate_created_connection`,
  which sets an explicit `InputPresentation::Cable` override — so the accessor
  is no longer hidden (`hidden_inline_node_ids` only hides nodes whose consuming
  connection is `InlineModParam`) and appears on the canvas as a real
  `(mod gain)` node;
- it keeps a consumer, so `drop_orphaned_inline_mod_nodes` /
  `orphaned_inline_mod_node_ids` (`model.rs:410`) will not garbage-collect it.

Correspondingly, the moved node's body text must be emitted with the `~` sugar
stripped — the inlet fills that slot. Since we carry `node_display_label`, and
that helper renders inline inputs via `InlineInput::label()` (which appends
`~`), the encapsulator must render the moved node's text with the crossing-in
slots forced to `?`. Add a `node_display_label_with_slots_cleared(node,
&cleared: HashSet<usize>)` variant rather than post-processing the string.

## 7. Naming and rename

Generated name: the first free `sub1`, `sub2`, … checked against
`root_patch.macros`, `state.edit_state.created_macros`, and the defmacro
library package names (`autocomplete_macros_for_patch`).

The instance node's text editor opens on the generated name, fully selected.
Committing different text must rename the macro, not turn the instance into an
unknown op. This needs a new helper:

```rust
pub(super) fn rename_created_macro(
    state: &mut PatcherInteractionState,
    old: &str,
    new: &str,
) -> bool
```

which:

1. rejects if `new` fails `is_valid_created_macro_name` (`interaction.rs:1033`)
   or collides with any existing macro name;
2. re-keys `created_macros`, updating `PatcherMacroEdit::name` and its
   `source` seed;
3. re-keys every `edit_state.nodes` / `connections` / `input_presentations` /
   `deleted_nodes` / `deleted_connections` entry whose view key is
   `macro:{old}` to `macro:{new}` (they are all `"{view_key}::{id}"` strings —
   see `scoped_node_key`, `connection_edit_key`, `input_presentation_key`);
4. re-keys `state.z_order`;
5. updates `state.active_macro` if it pointed at `old`;
6. sets the instance node edit's text to `new`.

Rename only applies to **created** macros. Renaming a source-backed macro is
out of scope.

## 8. Infrastructure gap to fix first

`overlay_visible_layout` (`sidecar.rs:556`) only overlays a macro scope's
layout when the macro exists in **both** `original.macros` (the on-disk patch)
and `visible.macros`:

```rust
if let (Some(original_patch), Some(visible_patch)) = (
    original_macros.get(macro_patch.name.as_str()),
    visible_macros.get(macro_patch.name.as_str()),
) {
```

A macro created in this session — by Cmd+E, or today by the Cmd+K agentic
bubble — is absent from `original.macros`, so its body layout is silently
dropped from the emitted sidecar. The nodes reappear at `assign_layout`'s
auto-positions on the next reload, and the encapsulation you just laid out
scrambles.

Fix: require only `visible_macros`, passing `&Patch::default()` as `original`.
`original` is used solely by `apply_editor_text_presentation_overrides`, which
tolerates an empty patch (it looks up nodes by id and skips misses). This is a
standalone bug fix that should land — and get its own test — before the
encapsulation slice.

## 9. Where the code goes

- `patcher/encapsulate.rs` (new module, `mod.rs`-style directory module already
  in place): the analysis and materialization. Entry point:

  ```rust
  pub(super) fn encapsulate_selection(
      node: &LayoutNode,
      state: &mut PatcherInteractionState,
      view_key: &str,
  ) -> bool
  ```

  Structure it as pure analysis (`fn analyze(patch, selection) -> Result<Plan, Refusal>`)
  plus a materializer that applies a `Plan` to `PatchEditState`. The analysis
  half is where the tests live.
- `patcher/mod.rs` `key_event`: the `KeyCode::Char('e')` arm. Follow the Cmd+V
  arm's shape — `SUPER | CONTROL` intersects check, because
  `normalize_command_shortcuts` rewrites SUPER to CONTROL for letter keys.
- `patcher/state.rs`: `empty_created_macro_source`, `rename_created_macro`.
- `patcher/display.rs`: `node_display_label_with_slots_cleared`.
- `patcher/sidecar.rs`: the §8 fix.

## 10. Tests

In `patcher/tests.rs`, driving real handlers (not reimplemented logic) per the
established pattern.

Analysis (pure, on a hand-built `Patch`):

1. One external source fanning into three selected nodes → **one** inlet, three
   internal cables from the `in` node.
2. One selected node feeding two external nodes → **one** outlet, two root
   cables from the instance. (The Swift algorithm produces two outlets here;
   this is the regression guard for §2's divergence.)
3. Fully-internal selection (no crossing edges) → zero inlets, one synthesized
   outlet from the terminal node.
4. Port ordering is stable: same graph, shuffled `connections` vec ordering →
   identical inlet/outlet index assignment.
5. Refusals: selection containing a `param`; selection containing a root `in`;
   non-convex selection (`a(sel) → b(ext) → c(sel)`); history read outside and
   written from outside.

End-to-end:

6. Encapsulate two nodes, regenerate via `generate_patch_source`, assert the
   output contains a `(defmacro sub1 (input1) …)` and a root
   `(def … (sub1 …))`, and that reparsing the generated source round-trips to
   an equivalent model.
7. Encapsulate, then Cmd+Z → the model equals the pre-encapsulation model
   exactly (one undo step, not several).
8. Encapsulate, Enter to open the macro, assert body node positions match the
   relative layout from §5.4.
9. Save-path layout: after encapsulation, `emitted_layout_json_with_node_map`
   contains a `macros.sub1` scope with the body positions (the §8 fix).
10. A selected node using `gain~` → root keeps a visible `(mod gain)` node
    feeding instance inlet 1, and the macro body's node text has no `~`.

Run scoped: `cargo test -p eseqlisp patcher::` — never package-wide, never with
`git stash`.

## 11. Out of scope

- **De-encapsulate** (explode a macro instance back inline). Natural sequel and
  a strictly harder problem: the macro may have multiple instances.
- **Contextual menu.** Cmd+E only; the menu wires to the same entry point when
  it exists.
- **Encapsulating inside a macro view.** The code paths are view-key generic
  and it should work, but the "a macro must not gain an instance of itself"
  guard (`interaction.rs:991`, and the paste guard at `interaction.rs:1319`)
  applies. v1 refuses when `view_key != "root"` until that is tested.
- **Promoting the new macro to the shared defmacro library.** The existing
  save-to-library action (`PatcherMacroLibraryActionKind::SaveToLibrary`)
  already covers it once the macro exists.
