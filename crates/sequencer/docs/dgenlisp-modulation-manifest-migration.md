# DGenLisp Modulation Manifest Migration

Context: `tools/DGenLisp` was rebuilt from dgen branch `train-kick808-example` and copied here on May 17, 2026 at 22:51:43 local time. The new binary makes a clean break in DGenLisp modulation lowering and manifest schema.

## What Changed

Old DGenLisp generated one selected modulation source plus one depth per modulatable parameter:

```json
{
  "name": "cutoff",
  "sourceCellId": 101,
  "depthCellId": 102
}
```

New DGenLisp generates one active flag plus one depth lane per declared modulator:

```json
{
  "name": "cutoff",
  "activeCellId": 101,
  "depthLanes": [
    { "slot": 1, "depthCellId": 102 },
    { "slot": 2, "depthCellId": 103 }
  ]
}
```

The DSP fast path now checks `activeCellId`. When it is `0`, the generated code returns the base parameter without reading modulator inputs or depth lanes. When it is nonzero, it sums all lanes:

```text
modValue = mod1 * depth_slot1 + mod2 * depth_slot2 + ...
```

Then it applies the destination mode:

- `additive`: `clip(base + modValue, min, max)`
- `multiplicative`: `clip(base * (1 + modValue), min, max)`
- `semitone`: `base * exp(log(2) * modValue / 12)`

The host is responsible for maintaining `activeCellId`:

```text
active = any(depth lane is nonzero and route is enabled)
```

Do not recompute active inside the audio loop.

## Required Sequencer Changes

Update manifest structs in `src/lisp_host.rs`.

Current code still expects:

```rust
source_cell_id
depth_cell_id
```

Replace that with:

```rust
pub active_cell_id: usize,
pub depth_lanes: Vec<DGenModDepthLane>,

#[derive(Clone)]
pub struct DGenModDepthLane {
    pub slot: usize,
    pub depth_cell_id: usize,
}
```

Update manifest parsing near `parse_dgen_manifest`:

```rust
active_cell_id: m["activeCellId"].as_u64().unwrap_or(0) as usize,
depth_lanes: m["depthLanes"]
    .as_array()
    .map(|arr| {
        arr.iter()
            .map(|lane| DGenModDepthLane {
                slot: lane["slot"].as_u64().unwrap_or(0) as usize,
                depth_cell_id: lane["depthCellId"].as_u64().unwrap_or(0) as usize,
            })
            .collect()
    })
    .unwrap_or_default(),
```

Remove old reads of `sourceCellId` and top-level `depthCellId`.

## UI Migration

Update `src/ui/graph.rs` where instrument modulation UI currently builds one source enum and one amount control per `modDestination`.

New model:

- Do not create a source selector param descriptor from manifest data.
- For each `dest.depth_lanes[]`, create a depth amount control bound to `HEADER_SLOTS + lane.depth_cell_id`.
- Add or maintain one hidden/host-controlled active flag bound to `HEADER_SLOTS + dest.active_cell_id`.
- Whenever any depth assignment changes, set active to `1` if any lane should contribute, otherwise set it to `0`.

If the existing `InstrumentModulationTarget` still requires `source_param_idx`, either:

- refactor it to use `modulator_slot` plus `depth_param_idx`, or
- introduce a host-side UI-only route selector that is not written to DGenLisp memory.

The DGenLisp runtime no longer has a source selector cell for the destination.

## Remove Host-Side Lane Expansion

`src/lisp_host.rs` currently has host-side additive modulation expansion around `expand_additive_host_mod_lanes`. That code was compensating for the old DGenLisp one-source/one-depth model by generating extra source/depth params and expanding `(mod p)` manually.

With the new compiler, this should be removed or bypassed:

- Do not synthesize `__host_mod_*_source` params.
- Do not rewrite `(mod p)` into a sum of selectors.
- Let DGenLisp generate all destination lanes from declared `@modulator` inputs.

This should also simplify any references to `ADDITIVE_HOST_MOD_LANES_PER_PARAM`.

## Docs And Generated API

Update stale docs/schema references:

- `tools/DGenLispReadme.md`: currently says modulatable params generate source/depth params.
- `docs/dgenlisp-modulation-mini-spec.md`: should match the new active/depth-lane model.
- `scripts/generate_dgenlisp_api.py`: update `ManifestModDestination` schema from `sourceCellId`/`depthCellId` to `activeCellId`/`depthLanes`.
- Regenerate `docs/dgenlisp-api.json` after the script is updated.

## Compatibility

This is a clean break. Do not add a compatibility mode unless explicitly requested.

Old manifests compiled by the previous DGenLisp binary will not have `activeCellId` or `depthLanes`. New manifests compiled by the copied binary will not have `sourceCellId` or top-level `depthCellId`.

## Validation Checklist

1. Compile a DGenLisp instrument with two or more `@modulator` inputs and at least one `@mod true` parameter.
2. Confirm the manifest has `modDestinations[].activeCellId`.
3. Confirm `modDestinations[].depthLanes` has one entry per declared modulator slot.
4. Confirm no code path requires `sourceCellId` or top-level `depthCellId`.
5. With all depths zero and active zero, confirm modulation UI works and audio remains stable.
6. Set one depth nonzero, set active one, and confirm that modulator changes affect the destination.
7. Set multiple depths nonzero and confirm they sum.

