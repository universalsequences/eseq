# Patcher Z-Index Implementation Spec

## Context

The patcher renderer currently lacks a real node z-stack. When two nodes overlap, lower-node text can render above both node bodies because the Metal backend batches primitives by type instead of preserving each node's intended sublayer order.

The Swift patch editor at `~/code/swift/patch-editor` solves this with a `ZStack`:

- `Sources/PatchEditor/Mouse/ZStack.swift` keeps an ordered node-id stack where the last node is topmost.
- Each node gets multiple z slots via `Z_SLOTS_PER_NODE = 10`.
- Node sublayers are assigned slots: outline/background, content, top layer.
- Moving a node brings it to the front by removing it from its old stack position and appending it.
- `Sources/PatchEditor/Shaders/Core/NodeShaders.metal` maps higher z-index values to closer depth values.

The Rust patcher implementation lives primarily in `crates/eseqlisp/src/widget_render/patcher/mod.rs`, with rendering in `render.rs`, interaction in `interaction.rs`, geometry/hit testing in `geometry.rs`, and persistent widget interaction state in `state.rs`.

## Problem

`draw_patch` currently emits cables, then iterates `patch.nodes`, and for each node emits:

1. node chrome
2. ports
3. edit selection
4. text
5. drill-in affordance
6. edit cursor

That local ordering is not enough because the Metal backend later renders by primitive category:

1. widget instances
2. quads/glyphs
3. patch cables
4. foreground widget instances
5. foreground rects
6. circles
7. proportional text

As a result, lower node text can render above higher node bodies. Sorting `patch.nodes` alone is not a complete fix.

## Desired Behavior

The visible order for overlapping nodes must be grouped by node z-stack:

```text
node 1:
  background/body
  ports/inlets/outlets
  text

node 2:
  background/body
  ports/inlets/outlets
  text

cables:
  all patch cables above all nodes
```

Moving or selecting a node for drag should bring it to the top of the node stack. Hit testing must use the same stack order as rendering, so the visually topmost node is also the interactive target.

## Data Model

Add a view-scoped z-order stack to `PatcherInteractionState` in `state.rs`:

```rust
pub(super) z_order: HashMap<String, Vec<String>>
```

The key is the active patcher view key:

- `root`
- `macro:<name>`

The value is an ordered list of node ids, bottom to top.

This belongs in interaction state rather than `PatchNode` because it is currently editor/UI state. If patch layout is later persisted, z-order can be promoted into source/writeback deliberately.

## Z Slots

Mirror the Swift approach with multiple slots per node:

```rust
pub(super) const PATCHER_Z_SLOTS_PER_NODE: i32 = 10;

pub(super) enum PatcherZSlot {
    NodeChrome = 0,
    EditSelection = 1,
    Ports = 2,
    Text = 3,
    DrillIn = 4,
    EditCursor = 5,
}
```

The absolute z index for a node sublayer is:

```rust
stack_index * PATCHER_Z_SLOTS_PER_NODE + slot as i32
```

Reserve additional slots for future node-local controls.

## State Helpers

Add helpers in `state.rs`:

- `sync_patcher_z_order(state, view_key, patch)`
  - Remove ids that are no longer live in the patch.
  - Append missing ids in deterministic `patch.nodes` order.
  - Preserve existing relative order for live ids.

- `bring_nodes_to_front(state, view_key, node_ids)`
  - Remove the specified ids from their current positions.
  - Append them to the end.
  - Preserve their current relative order when bringing multiple selected nodes forward.

- `ordered_patch_nodes<'a>(patch, state, view_key) -> Vec<&'a PatchNode>`
  - Return nodes in bottom-to-top z order.
  - Include any nodes missing from state at the end after syncing.

- `node_z_index(state, view_key, node_id, slot) -> i32`
  - Return the node sublayer z index.

There is no current Rust equivalent of the Swift `PanelOperator`; do not add panel-specific behavior yet. If panel nodes are introduced later, they should be inserted at the back in `sync_patcher_z_order` and rechecked when node kind changes.

## Interaction

Update `handle_patcher_pointer_down` in `interaction.rs`:

- After resolving `hit_patcher_node`, bring the clicked node to front.
- If the clicked node is already part of a multi-selection and the drag moves the whole selection, bring all selected nodes to front while preserving their relative stack order.
- If shift-click toggles selection, bring the clicked node to front after the selection update.
- Clear or leave cable z behavior unchanged; cables render above nodes globally.

Update node creation:

- `allocate_created_node` should ensure the new node is present at the front/top of the active view's z stack.
- Double-click-created draft nodes should appear above existing nodes.

## Hit Testing

Current hit testing in `geometry.rs` uses `patch.nodes.iter().rev()`. Replace or overload node/port hit testing so it accepts an ordered node list or z-order lookup:

- `hit_patcher_node`
- `hit_patcher_output_port`
- `hit_patcher_macro_drill_in`

These should iterate top-to-bottom according to the z stack, not source/vector order.

Nearest-port cable targeting can continue using geometric nearest distance unless overlapping ports create ambiguity. If ambiguity matters later, use z order as a deterministic tie-breaker.

## Rendering

The robust fix must make z-order explicit across backend primitive passes.

Do not rely only on changing the order of `push_node` calls. The backend currently batches primitive categories, so node-local ordering is lost.

Preferred approach:

1. Add z-index support to Metal primitives, either by:
   - adding a `z_index: i32` field to relevant primitive structs, or
   - adding a wrapper variant such as `MetalPrimitive::ZLayer { z_index, primitive }`.

2. Update backend primitive collection/rendering so z-layered primitives are rendered in ascending z order, preserving the intended grouping across primitive types.

3. Assign patcher node primitives to node-local z slots:
   - node body/border: `NodeChrome`
   - edit selection: `EditSelection`
   - ports: `Ports`
   - label/diagnostic text: `Text`
   - macro drill-in affordance: `DrillIn`
   - edit cursor: `EditCursor`

4. Assign all patcher cables to a global z layer above the maximum node z index for the current patch.

5. Assign selected cable handles and active drag overlays above cables.

If using actual Metal depth instead of explicit sorted passes, the implementation must add/configure depth attachments and depth state consistently for every relevant 2D pipeline. A partial depth-only solution would be fragile because text, circles, widget instances, and cable pipelines are currently rendered in separate passes.

## Tests

Add focused Rust tests:

- `z_order_initializes_from_patch_nodes`
- `z_order_removes_deleted_nodes_and_appends_created_nodes`
- `pointer_down_bring_node_to_front`
- `hit_testing_uses_z_order_not_patch_vector_order`
- `metal_render_groups_node_sublayers_by_z_order`

For render ordering, construct two overlapping nodes and assert the emitted/rendered ordering is:

```text
node A chrome < node A ports < node A text
node B chrome < node B ports < node B text
cables above both nodes
```

Also keep existing tests that verify cables and node primitives are emitted.

## Visual Verification

Per `AGENTS.md`, after implementing the visual renderer change, run:

```sh
cargo test -p eseqlisp --test capture capture_patcher_lexilush_png -- --ignored --nocapture
```

Inspect:

```text
/tmp/eseqlisp-patcher-lexilush.png
```

Do not claim the visual change is correct without inspecting the generated PNG.

## Non-Goals

- Do not persist z-order to source files in the first implementation.
- Do not add panel-specific ordering until the Rust patcher has a real panel node concept.
- Do not work around the issue by hiding lower-node text, clipping text manually, or sorting only `patch.nodes`.
- Do not introduce a backend special case that only fixes proportional text while leaving ports/edit overlays inconsistent.

## Quality Bar

The implementation must treat z-order as a first-class render concept. A workaround that happens to fix one overlapping text case but leaves backend pass ordering inconsistent is not acceptable.
