# Metal Retained Rendering Progress

## Context

The sequencer UI now spends much less time in Lisp/reactive/layout work than it did before the earlier layout-cache fixes. With `*sequencer*` visible during playback, profiles shifted the main renderer cost toward Metal-side CPU work:

- scene primitive collection and conversion
- proportional text geometry
- transient Metal buffer upload/allocation
- per-frame widget draw preparation
- dynamic fallback paths for changing widgets

The goal of this work was to move the Lisp-authored widget UI toward retained rendering without replacing the sequencer with Rust-only widgets.

## What Changed

### GPU Upload Arena

Commit: `1475ed6 gpu upload arena`

Added shared upload infrastructure so UI draw paths do not need to allocate fresh Metal buffers for every small transient upload. This reduced direct `newBufferWithBytes` pressure and provided the instrumentation foundation for later renderer work.

### Simple Widget Run Cache

Commit: `bbb2777 Cache simple Metal widget runs`

Added a compiled cache for simple widget primitive runs. The renderer now tags Metal primitive runs by stable widget identity and can compile simple runs into reusable Metal buffers.

Covered simple widgets include labels, buttons, badges, sliders, toggles, knobs, tabs, boxes, and number labels. Unsupported or complex primitives still fall back to the dynamic draw path.

Important correctness fix:

- Dirty tagged frames now refresh the full-scene cache. Without that, clean frames could briefly draw stale cached state, which caused playhead/meter flicker.

### Retained Dirty-Run Collection

Commit: `0343a56 Reuse clean Metal primitive runs on dirty frames`

Added a retained tagged-run collection path. Dirty frames can reuse clean widget primitive runs and rebuild dirty widgets or dirty-descendant subtrees.

This preserved the Lisp UI as the source of truth. The renderer still walks the layout tree, but it avoids calling widget primitive builders for clean runs where the cached run remains valid.

Tests cover:

- unchanged runs are reused
- dirty runs are rebuilt
- dirty ancestors rebuild descendants
- scroll containers do not double-offset reused children

### Static Widget Time Key Fix

Commit: `b048d57 Ignore widget time for static run cache keys`

Profiles showed `compiled_simple_widget_run` stayed hot because widget instance `itime` changed every frame and was included in the compiled-run cache key. For non-animated widgets, `itime` is not part of visual identity.

Changed the cache key so:

- non-animated widget instances ignore `itime`
- animated SDF widget instances keep `itime` in the key
- animated SDF material overrides request animation frames and bypass static caching

This removed `compiled_simple_widget_run` from the main hot path in subsequent profiles.

### In-Place Retained Run Refresh

Commit: `79210da Refresh retained Metal runs in place`

The retained dirty path was still rebuilding a previous-run lookup and cloning the full run list each dirty frame. Cached run scenes now keep a persistent `MetalPrimitiveRunKey -> index` map, and dirty frames mutate cached runs in place.

If a retained update sees missing or invalid run structure, it falls back to a full tagged collect and rebuilds the index.

Effect:

- `widget_primitive_runs_for_dirty_layout` / retained refresh dropped proportionally from about `3.0%` to about `1.9%` in the user profiles.

### Avoid Dirty-Frame Flattening

Commit: `4d3ace5 Avoid flattening dirty Metal run scenes`

Dirty frames no longer flatten retained runs into a flat `CachedWidgetScene` every frame. Instead:

- dirty frames invalidate/remove the flat scene cache for that key
- the retained run scene remains the source of truth
- a later clean frame can rebuild the flat cache once if needed

This removed `flatten_metal_primitive_runs` from the dirty-frame hot list and eliminated the broad full-run `Vec::clone` cost that appeared after the in-place refresh.

## Current Profile Interpretation

The retained-scene work is functioning, but total `render_tiled` CPU has not dropped dramatically. The work has mostly moved from scene construction toward draw preparation and dynamic fallback.

Latest profile characteristics:

- `render_tiled` remains around `12%` of sampled work.
- `refresh_widget_run_scene_for_dirty_layout` is down to about `1.9%`.
- `draw_dynamic_segment_all` is now one of the largest renderer costs.
- `draw_widget_run_cached_segment` remains visible.
- animation/layout scans are now visible:
  - `layout_contains_agent_instrument_stub_animation`
  - `layout_wants_animation_frames`
- `compiled_simple_widget_run` is no longer the dominant cost, but still appears at a much lower level in some samples.

This suggests the incremental retained primitive cache has reached diminishing returns. The renderer still pays per-frame costs to:

- offset primitives into tile coordinates
- split primitive segments at clip boundaries
- decide cached vs dynamic draw handling per segment
- dynamically rebuild/draw any segment containing unsupported or dirty primitives
- scan layouts for animation bypass conditions

## What Worked

- Text and compiled-run caching reduced earlier hot symbols substantially.
- The flicker bug was fixed by keeping scene caches synchronized after dirty renders.
- Dirty scene assembly improved proportionally after in-place retained updates.
- The retained run model preserved Lisp-authored widgets and stable identity checks.
- The implementation remained generic at the renderer/widget layer rather than sequencer-specific.

## What Did Not Move Enough

The current cache still operates at the primitive-run level. Even when clean runs are retained, every dirty frame still has to prepare a tile-local draw stream:

1. iterate retained runs
2. clone run primitives for time refresh and offsetting
3. extend right-edge primitives
4. build offset primitive arrays
5. split into clipped segments
6. run cached/dynamic segment dispatch logic

That means we reduced primitive construction but did not fully retain the compiled per-tile draw command stream.

## Recommended Next Steps

### 1. Cache Compiled Draw Commands Per Tile/Segment

This is the next non-micro step.

Move from retained primitive runs to retained compiled draw commands for stable tile segments. A clean segment should be drawable without:

- rebuilding offset primitives
- splitting clip segments
- rechecking every run for cacheability
- rebuilding/uploading stable vertex buffers

Dirty widgets would invalidate only commands whose source run intersects the dirty widget or a dirty ancestor. Dynamic overlays and unsupported widgets can remain dynamic bypasses.

Key requirements:

- preserve current draw order
- preserve scissor/clip semantics
- keep dynamic widgets explicit
- invalidate on layout, viewport, scroll, theme, focus, atlas generation, or widget-state generation changes

### 2. Split Static and Dynamic Segment Streams

Today, if a segment contains a dirty or unsupported run, the renderer may fall back to `draw_dynamic_segment_all` for that segment. That can pull static labels/chrome back into dynamic work.

A better structure is:

- static compiled commands for clean runs
- dynamic commands for dirty/animated/unsupported runs
- same segment/scissor ordering, but without forcing the whole segment dynamic when only one run changes

This is more invasive than the current run cache but directly targets the remaining dynamic fallback cost.

### 3. Cache Animation-Bypass Decisions Per Layout Key

The profiles now show repeated animation scans:

- `layout_contains_agent_instrument_stub_animation`
- `layout_wants_animation_frames`

These should be cached on the same layout/cache key used for scene reuse. The result changes when layout identity, widget state generation, or relevant props change.

This is smaller than compiled segment caching and likely safe, but it is not the primary bottleneck.

### 4. Avoid Per-Frame Primitive Cloning for Offset/Time

Dirty-frame rendering still clones per-run primitives so it can refresh time and offset them to tile coordinates.

A cleaner model is to keep primitives in layout-local coordinates and apply tile offset at draw-command build time, or store compiled geometry in a coordinate-local form plus a cheap per-frame transform/uniform.

This requires careful handling for:

- text vertices
- widget instances
- clip rects/scissors
- scroll offsets
- right-edge extension

It should probably be part of the compiled tile/segment command work rather than another standalone micro-optimization.

## Suggested Stopping Point

The current state is a reasonable checkpoint:

- correctness appears stable
- no sequencer playhead or meter flicker was observed after the cache synchronization fixes
- focused sequencer tests pass
- patcher visual capture passes
- the remaining costs are architectural rather than obvious cache-key bugs

Before continuing, it would be useful to decide whether the next investment should be:

1. a larger retained compiled segment-command cache, or
2. smaller cleanup/instrumentation work around animation scans and dynamic fallback reasons.

The first option is more likely to produce a meaningful CPU reduction. The second is lower risk and can improve observability before attempting the larger change.
