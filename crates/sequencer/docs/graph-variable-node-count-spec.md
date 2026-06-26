# Graph Sequencer Variable Node Count Spec

> Status: design spec. Not implemented.
> Scope: graph-mode `def-sequencer` line-shaped neural demos and any future graph
> sequencer that wants a pattern-serializable active node count.

## Goal

Allow a graph sequencer authored with a variable-capable line shape to expose an
editable active node count, e.g. 8, 12, or 16 neurons, without forcing users to load
a different script and without destroying existing per-node or per-edge edits when
the active count shrinks.

The key behavior is dormant persistence:

- Shrinking from 16 to 12 hides and deactivates nodes 12..15 and any edges touching
  them.
- Overrides for nodes 12..15 and their edges remain serialized.
- Growing back to 16 restores those dormant overrides exactly.

## Non-Goals

- Arbitrary topology editing. This spec only covers changing the materialized count
  of an existing compatible shape.
- Multi-prototype graphs.
- Preserving live runtime energy, pending propagation queues, or dampening state
  across a node-count change. Count changes rebuild the runtime.
- Deleting dormant data automatically. Destructive cleanup can be a future explicit
  command, not the default behavior.

## Terminology

- Manifest shape: the static shape declared by `def-sequencer`.
- Active node count: the count used to materialize the current pattern's runtime.
- Dormant override: any serialized node or edge override whose index is outside the
  current active node count.
- Capacity: the maximum supported count for a variable-count shape. Dormant
  overrides may exist up to this capacity.

## Surface Syntax

The current fixed syntax remains valid:

```lisp
(def-sequencer "neural-8x8-reset-demo"
  :shape (line 8)
  ...)
```

A variable-capable script should declare an explicit capacity and default active
count:

```lisp
(def-sequencer "neural-variable-demo"
  :shape (line :default 8 :min 1 :max 16)
  ...)
```

Accepted shorthand:

```lisp
(def-sequencer "neural-variable-demo"
  :shape (line 8 :max 16)
  ...)
```

This means default active count 8, minimum 1, maximum 16.

Fixed `(line N)` remains exactly fixed. It must not silently become variable.

## Authoring API

Add a sequencer-level graph config field:

```lisp
(graph-config "neural-variable-demo" :node-count 12)
(graph-config-value "neural-variable-demo" :node-count)
(bind-graph-config "neural-variable-demo" :node-count)
(graph-config-key "neural-variable-demo" :node-count)
```

Rules:

- `:node-count` is available only for variable-capable line shapes.
- Values are clamped to the shape's `[min, max]` and rounded to an integer.
- The value is per-pattern, like `:reset-bars` and `:max-poly`.
- Loading a script must not write a node-count override. It publishes the manifest
  and UI only.

The UI script should derive rows, trigger matrices, energy matrices, and weight
matrices from the resolved active node count:

```lisp
(def active-count (graph-config-value graph-name :node-count))
(each (range 0 active-count) |n| ...)
```

## Storage Model

Extend `ProjectGraphOverrides`:

```rust
pub struct ProjectGraphOverrides {
    pub sequencer_id: u64,
    pub sequencer_name: String,
    pub node_intrinsics: Vec<ProjectGraphNodeIntrinsicOverride>,
    pub node_params: Vec<ProjectGraphNodeParamOverride>,
    pub edge_params: Vec<ProjectGraphEdgeParamOverride>,
    pub reset_every_beats: Option<f64>,
    pub max_poly: Option<u32>,
    pub node_count: Option<u32>,
}
```

Serialization contract:

- `node_count: None` means use the manifest default active count.
- Node and edge override vectors are not truncated when `node_count` shrinks.
- Existing project files without `node_count` load as fixed/default-count graphs.
- Out-of-range dormant overrides remain in project files during normal save/load.

## Runtime Materialization

`GraphManifest::runtime_config_with_overrides` must resolve active count before
materializing nodes and topology.

For variable line shapes:

```text
active_count = overrides.node_count.unwrap_or(shape.default)
active_count = clamp(active_count, shape.min, shape.max)
```

Then materialize exactly `active_count` nodes and all edges produced by the manifest
topology for `0..active_count`.

Override application rules:

- Apply node overrides only when `instance < active_count`.
- Apply edge overrides only when `from < active_count && to < active_count`.
- Leave all dormant overrides untouched in storage.
- A dormant override becoming active again must be applied without special migration.

Fixed shapes continue to use their declared node count and ignore `node_count` if
present. Prefer not to write `node_count` for fixed shapes.

## Runtime Reconciliation

Changing active node count changes graph structure. It must be treated as
incompatible with the existing runtime:

- Rebuild `GraphRuntime`.
- Clear energy, pending propagation, input accumulators, trigger visuals, cycle
  positions, and runtime edge dampening.
- Preserve serialized graph overrides.

This matches current graph shape-change behavior and avoids invalid vector reuse.

`GraphManifest::structurally_compatible` remains strict for resolved runtime shape:
same graph id, same active node count, same materialized edge count, and same
materialized edge endpoints.

## UI Requirements

A variable-count graph demo must expose a top-level node count control.

Recommended control:

```lisp
(number-picker
  :value (bind-graph-config graph-name :node-count)
  :min 1
  :max 16
  :step 1
  :decimals 0
  :on-change (lambda (v) (graph-config graph-name :node-count v)))
```

The row table and matrices must use the resolved count. They must not depend on a
hard-coded script constant except for capacity-oriented helpers.

When active count changes:

- Rows outside the active range disappear.
- The weight matrix changes to active `N x N`.
- Telemetry matrices change to active `N x 1` or `N x N`.
- Controls for remaining nodes keep their values.
- Restoring a larger count restores dormant row/matrix values.

## Matrix Editing Semantics

Matrix writes only edit active cells because only active cells are visible.

When shrinking:

- No edge overrides are deleted.
- Edges touching inactive nodes are dormant.

When growing:

- Existing dormant edge overrides reappear.
- Missing edge overrides use manifest defaults.

Bulk helpers such as "ring defaults" should be explicit about whether they affect:

- Active nodes only.
- Full capacity.

Default recommendation: UI actions affect active nodes only unless named otherwise.
For example, `init-ring-defaults` on a 12-node active graph writes a 12-node ring and
does not erase dormant 13..16 data.

## Scheduler and Snapshot Behavior

The scheduler already receives graph overrides in snapshots. Node count changes must
flow through the same snapshot path as reset/max-poly overrides.

Required behavior:

- A node-count edit bumps the scheduler snapshot/version.
- Scheduler reconciliation rebuilds that graph runtime on the next snapshot update.
- Other graph sequencers in the snapshot are unaffected.
- Builtin MIDI FX and target-track routing continue to work for emitted graph events.

## Example User Flow

1. User loads variable neural graph with default 8 nodes.
2. User sets node count to 16.
3. User edits node 14 transpose, route, delay, and some edges involving node 14.
4. User sets node count to 12.
5. Node 14 disappears and no longer fires, routes, receives seeds, or appears in
   matrices.
6. Project is saved and reloaded.
7. User sets node count to 16.
8. Node 14 returns with the same transpose, route, delay, and edge weights.

## Acceptance Tests

### Manifest parsing

- `(line 8)` remains fixed with node count 8.
- `(line 8 :max 16)` parses as variable line with default 8, min 1, max 16.
- `(line :default 8 :min 4 :max 16)` parses as variable line.
- Invalid min/default/max combinations fail with clear errors.

### Authoring config

- `graph-config :node-count 12` writes `ProjectGraphOverrides.node_count = Some(12)`.
- `bind-graph-config :node-count` seeds a finite nonzero reactive value.
- Fixed line shapes reject or ignore `:node-count` consistently; prefer a diagnostic
  on write.

### Dormant persistence

- Create overrides for node 14 and edge 14 -> 3 under count 16.
- Shrink to 12.
- Export project/snapshot and verify those overrides still exist in storage.
- Materialized runtime has 12 nodes and no active edge touching node 14.
- Grow to 16.
- Materialized runtime applies node 14 and edge 14 -> 3 overrides.

### Runtime rebuild

- Start with count 8, seed graph so it has nonzero pending/energy state.
- Change count to 12.
- Reconciliation creates a 12-node runtime with cleared transient state.
- Serialized overrides remain.

### UI layout

- Variable demo loads without writing overrides.
- Node-count control has finite nonzero layout.
- Row controls and matrices have finite nonzero layout for 8, 12, and 16 active
  counts.
- Pattern switch reseeds row/matrix controls from the destination pattern's resolved
  active count and dormant overrides.

### Scheduler route regression

Run the deterministic scheduler lookahead harness for graph seed propagation through
MIDI FX after adding node-count support:

```sh
cargo test -p sequencer scheduler::tests::scheduler_lookahead_routes_lisp_graph_seed_and_propagation_through_midi_fx -- --nocapture
```

If scheduler-side scratch loading changes, also run:

```sh
cargo test -p sequencer scheduler::tests::scheduler_runtime_keeps_builtin_midi_fx_when_project_scratch_fails -- --nocapture
```

## Open Decisions

- Should variable count be allowed for `grid` and `ring`, or only `line` initially?
  Recommendation: implement `line` first. `ring` can use the same count model later.
  `grid` needs row/col semantics and should be separate.
- Should there be an explicit "prune dormant graph data" command? Recommendation:
  yes, later, clearly destructive, never automatic.
- Should the UI expose capacity-wide matrix editing? Recommendation: not for v1.
