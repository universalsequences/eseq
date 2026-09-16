# Retained renderer scroll measurements — 2026-09-16

Apple M1 Max, release builds, 2400 × 1400 physical pixels. These measurements exercise the production tiled renderer and frame construction using a headless project. They exclude audio playback, event delivery, and display presentation latency. They do not establish a 10× speedup or reproduce the original Instruments trace.

Each configuration has three interleaved baseline/candidate runs. A run warms 240 horizontal scroll positions and measures the same 240 positions. The table reports the median of the three per-run statistics; times are milliseconds. Scroll requests cover 220 cells and clamp at the actual FX limit, 163.525 cells.

| Tracks | Renderer CPU p50 before → after | Renderer speedup | Renderer CPU p95 before → after | Frame build + renderer p50 before → after | GPU p50 before → after |
| --- | --- | --- | --- | --- | --- |
| 1 | 0.991 → 0.794 | 1.25× | 1.198 → 0.976 | 1.169 → 0.955 | 0.864 → 0.872 |
| 10 | 1.565 → 1.281 | 1.22× | 1.827 → 1.513 | 1.751 → 1.453 | 1.118 → 1.066 |
| 30 | 2.761 → 1.868 | 1.48× | 3.108 → 2.152 | 2.950 → 2.053 | 1.404 → 1.106 |

## Work avoided

| Tracks | Submitted primitives, median before → after | Culled nodes, median after | New static GPU allocations / compiled misses, maximum after |
| --- | --- | --- | --- |
| 1 | 1449 → 1290 | 175 | 0 / 0 |
| 10 | 3213 → 2265 | 504 | 0 / 0 |
| 30 | 7133 → 3465 | 1704 | 0 / 0 |

The unchanged resident geometry stays in GPU buffers across scroll positions. All measured candidate frames had zero scene-index rebuilds and zero bounds refreshes. Shader animations use the dynamic path; hover scale uses a frame uniform with resident geometry. Offscreen dirty content is repainted when revealed. Empty clips suppress descendant work, and known paint bounds include SDF margins, hover growth, box backgrounds expanded over content, and box state indicators.

## Implementation

- `widget_render/retained_scene.rs`: one indexed scene per tile, dirty-subtree and ancestor bounds updates, scoped paint invalidation, conservative two-axis culling, nested scroll translations, and overlay escape handling. Metal and wgpu use this traversal.
- `ui/metal_backend.rs`: local-coordinate compiled runs keyed by paint/atlas/viewport revisions, GPU translation and hover time, per-run dynamic dispatch with the existing z/phase order, and bounded oldest-entry eviction. Superseded primitive-hash GPU caching was removed.
- `widget_render/box_widget.rs` and `sdf_widget.rs`: shared paint-extent calculations and scoped hover revisions.
- `metal_seq capture`: reusable offscreen production tiled rendering, scroll replay, actual-offset reporting, and named-tile selection for `--all-panels`. Selecting FX preserves the transport and other panels.

## Validation

17 focused nextest regressions passed, including offscreen reactive changes, nested clips, fractional scrolling, hover/focus/shared-state/theme/viewport invalidation, expanded backgrounds, and corner indicators. The GPU regression compares retained rendering against dynamic rendering and verifies zero warm static allocations, changed-control refresh, atlas invalidation, and hover animation on resident geometry.

A 201-node regression changes one control and refreshes three bounds (the control and its ancestors), rebuilds one paint node, and avoids reindexing. Reordering children still rebuilds the index to preserve identity and painter order.

`cargo check -p eseqlisp --lib --features wgpu,capture-harness` passed. The production Metal shader capture harness rendered all 40 scenes; no golden files were changed. Release `metal_seq` built successfully. PNGs were inspected at the beginning and maximum horizontal scroll, and at a constrained vertical viewport. No full workspace/package test suite was run.

## Reproduction and limits

See [the capture command documentation](metal-seq-ui-capture.md#tiled-capture-and-scroll-replay). The durable ten-track fixture is `crates/sequencer/ui/capture-fixtures/renderer-scroll.lisp`. Raw JSON, captures, generated 1/10/30-track fixtures, and saved binaries are under `.local/benchmarks/renderer-scroll-2026-09-16/`; `comparison.json` records every run.

The baseline binary was saved after adding replay support and before changing the renderer. Its early all-panel selection operated on the active tile, so the local comparison fixtures first focus the existing FX tile using the normal window-cycling commands. The resulting transport, sequencer, mixer, and FX panel layout was checked visually. The final harness selects the tile by buffer name. Malformed early captures (`baseline-all.png` and the initial custom-layout `baseline-tiled.png`) are excluded from the comparison. The final timing runs were conducted with no Cargo builds running; the earlier measurements in `comparison-build-overlap.json` are excluded.

Culling remains conservative: a painter without a proven bound is visited unless an ancestor clip excludes it. Consequently, preparation still grows with some offscreen content. The wgpu change shares CPU retention/culling; it does not add Metal-style GPU buffer retention to wgpu.

This quiet replay does not recreate the expensive dirty-scene work in the supplied profile. The original saved project and a live trace are needed to evaluate that case, playback, and input-to-presentation latency. Renderer speedup must not be reported as whole-app speedup. In the supplied trace, eliminating all 1.58 seconds of rendering from 3.61 seconds of main-thread work would cap that main-thread improvement at about 1.78×.

No known correctness workaround remains in the changed renderer. The material validation limits are the synthetic project and lack of live playback/presentation measurement.

## Binary identity

- `baseline-metal_seq` SHA-256: `916b7c426476584f803dfe40fc81d0b542ec8b1a080b6b5495128cd298892a7f`
- `candidate-metal_seq` SHA-256: `3778caf35b535a3f52d13bdaf03daf3df979b02fc8d8a3eb0ace46ed3e56f127`
