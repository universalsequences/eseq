# Main-Thread Perf Spec — reactive tick / render hot path

## Background

Instruments profile of a real project (`downtmpexoticarubba`, release build) showed the
**main thread using ~2x the CPU of the audio thread**: 4.74 s inside
`metal_seq::event_loop::run_event_loop` over the capture window. The cost is not one
hotspot — it is full-tree/full-vec work re-done every frame that should be cached,
delta-gated, or skipped outright.

Measured breakdown (main-thread weight, from the profile):

| Cost | Symbol | Root cause |
|---|---|---|
| ~1.05 s (22%) | `layout_wants_animation_frames` (728 ms via `Editor::visible_widgets_animating` + ~330 ms inside `render_tiled`) | uncached recursive walk of every visible layout tree, several times per frame |
| 654 ms (13%) | `pull_shared_bus_state` (515 ms `Vec<BusChannelState>` clone + 137 ms drop) | deep-clones full bus state every tick just to compare 3 scalar fields |
| 733 ms | `MetalBackend::widget_scene_for_layout` (458 ms `collect_metal_primitives`) | full primitive-scene rebuild for layouts flagged as animating |
| ~250 ms | `sync_track_params_with_neural_selection` (114 ms) + `sync_fx_param_binding_fields_with_neural_selection` (73 ms) + `sync_neural_visualization_fields` (47 ms) | unconditional per-tick rebuild of reactive `Value` trees; the neural-viz one runs with **zero** neural graphs in the project |
| 420 ms | `poll_backend_event` (self weight 0) | unattributed — needs subtree expansion, see Item 6 |

Items are ordered by leverage. Items 1 and 2 are mechanical and low-risk; do them first.

---

## Item 1 — Cache `layout_wants_animation_frames` per layout (~1.05 s)

**Where:**
- `crates/eseqlisp/src/widget_render/mod.rs:1028` — `layout_wants_animation_frames`
  (recursive; per node does `widget_definition()` lookup, `sdf_widget_def` lookup, and
  prop-map string inspection via `node_uses_animated_sdf_material`).
- Callers, all per-frame:
  - `crates/eseqlisp/src/editor/mod.rs:3967` — `Editor::visible_widgets_animating`
    walks **every** visible tile's full `cached_layout` tree. Called from
    `crates/sequencer/src/ui/event_loop.rs:326`, `crates/eseqlisp/src/lib.rs:119` and `:210`.
  - `crates/eseqlisp/src/ui/metal_backend.rs:2582`, `:2679`, `:5095` — inside
    `render_tiled` / `widget_scene_for_layout`.

**Problem:** the answer cannot change between layout rebuilds — the `LayoutNode` tree is
immutable once built — yet it is recomputed from scratch on every call, every frame.

**Fix:** compute once per layout build and cache.

- Preferred seam: compute the bool at the point a leaf's `cached_layout` is
  built/replaced (and clear it wherever `cached_layout` is invalidated — e.g.
  `refresh_all_inactive_tile_layouts` at `editor/mod.rs:3976` nulls `cached_layout`;
  find all such sites). Store alongside `cached_layout` on the tile leaf, e.g.
  `cached_layout_wants_animation: Option<bool>`.
- Alternatively (broader, also helps Item 3): compute bottom-up during layout
  construction and store a `wants_animation_frames: bool` field on `LayoutNode` itself,
  so any subtree query is O(1). This is more invasive but makes the per-subtree checks
  in `metal_backend.rs` free too.
- `visible_widgets_animating` then reduces to OR-ing cached bools.

**Caution:** `wants_animation_frames` on a `WidgetDefinition` takes `node` — verify no
widget's implementation depends on mutable external state that could change without a
layout rebuild (grep implementations of `wants_animation_frames`). If one does, that
widget must force layout-dirty when its animation state changes, or be exempted from
the cache.

**Verify:** existing tests at `crates/eseqlisp/src/widget_render/patcher/tests.rs:2581`
and `:2596` assert `visible_widgets_animating` both ways — they must still pass.

---

## Item 2 — Stop cloning bus state every tick (~654 ms)

**Where:** `crates/sequencer/src/ui/edit_sessions.rs:866` — `pull_shared_bus_state`.

**Problem:** `bus_state.lock().unwrap().clone()` deep-clones the whole
`Vec<BusChannelState>` every reactive tick. `BusChannelState`
(`crates/sequencer/src/app/mod.rs:708`) carries `name: String`, `gate_sequence`,
`effect_descriptors: Vec<EffectDescriptor>`, `effect_slots: Vec<EffectSlotSnapshot>`,
`custom_effect_names: Vec<Option<String>>` — but the comparison only reads
`volume`/`mute`/`solo`. 515 ms cloning + 137 ms dropping, per the profile.

**Fix:** hold the lock, compare in place, clone only on change:

```rust
let guard = bus_state.lock().unwrap();
let changed = app.buses.len() != guard.len()
    || app.buses.iter().zip(guard.iter())
        .any(|(a, b)| a.volume != b.volume || a.mute != b.mute || a.solo != b.solo);
if !changed { return false; }
// same-length path: copy the 3 scalars per bus (no clone at all);
// length-change path: clone the vec (rare), then drop the guard before mutating app if
// borrow rules require it.
```

Keep the existing semantics exactly: same-length ⇒ scalar copy only (do NOT overwrite
descriptors/slots), length-change ⇒ full replace. The lock is uncontended-cheap; the
clone was the cost. If lock-hold time is a concern, an alternative is publishing a
generation counter (`AtomicU64`) bumped by the writer and early-outing when unchanged —
but the in-place compare is sufficient and simpler.

---

## Item 3 — Incremental `widget_scene_for_layout` for animating layouts (~733 ms)

**Where:** `crates/eseqlisp/src/ui/metal_backend.rs:2564` — `widget_scene_for_layout`;
458 ms of it in `collect_metal_primitives`
(`crates/eseqlisp/src/widget_render/mod.rs:1323`). The gate at `metal_backend.rs:2582`
(`if widget_render::layout_wants_animation_frames(layout)`) puts any layout containing
one animating widget on the full-rebuild path every frame.

**Problem:** one animating widget (a meter, a playhead) forces re-collection of the
entire layout's primitives per frame, including all static widgets.

**Fix direction:** rebuild only animating subtrees; keep static primitives cached.

- The run-cache machinery already exists (`draw_widget_run_cached_segment`,
  `refresh_widget_run_scene_for_dirty_layout`, `cacheable_widget_primitives` at
  `widget_render/mod.rs:1050`) — extend it so the animating path reuses cached runs for
  non-animating widgets and re-collects only widgets whose subtree
  `wants_animation_frames` (O(1) after Item 1's per-node variant).
- Do this **after** Items 1 and 4: cheaper checks shrink the rebuild scope, and Item 4
  may reveal that most "animating" layouts shouldn't be on this path at all.
- This is the highest-risk item (visual correctness). Compare rendered output before/
  after with the UI-capture harness (`docs/metal-seq-ui-capture.md`) on a
  meter-heavy view (mixer) and an idle view.

---

## Item 4 — Audit what claims `wants_animation_frames` (multiplier)

**Where:** all implementations of `wants_animation_frames` across widget definitions,
plus `animates` on SDF widget defs (`sdf_widget`).

**Problem:** the entire per-frame render cost (Items 1 and 3, plus the 2.68 s
`reactive_tick_and_render`) is paid whenever `visible_widgets_animating` is true. One
always-true widget makes the whole UI render at full frame rate even when visually
idle.

**User-confirmed policy (from the project owner):** almost nothing in the main
sequencer, fx panel, mixer panel, or sidebar should be time-animated. Meters are the
only possible exception, and even those are data-driven (values streamed in via
reactive fields), not time-driven — they should NOT claim `wants_animation_frames`.
(Confirmed: `mixer_meter.rs` and `multiband_meter.rs` do not override it today; keep it
that way.)

**Known offender to remove:** at some point steps in the step sequencer (main
sequencer and/or the track sequencer's expanded-step view) were made animatable. The
owner explicitly wants this **undone** — step animation is not useful. Where to look:

- Rust widget overrides of `wants_animation_frames` are only: `matrix.rs:635`
  (`state_animation_active` transient — check what triggers it and how long it runs),
  `event_view.rs:674`, `piano_keyboard.rs:447`, `patcher/mod.rs:1025`,
  `phaser_notch.rs:281`. The static SDF registry (`sdf_widget.rs`) has `animates:
  false` on all real entries.
- Therefore the step animation most likely arrives via a **Lisp-registered SDF
  shader** with `animates` set, applied as a step widget's `shader-type`/`background`
  prop — that's the `node_uses_animated_sdf_material` path
  (`widget_render/mod.rs:1037`). Grep the sequencer/expanded-step UI lisp for animated
  shader/background props on step widgets and remove them (or register those shaders
  as non-animating).

**Task:**
1. Add a temporary debug log (or use an env-var probe, matching the
   `ESEQ_DEBUG_*` convention) listing which widget types / shader types return true
   per frame.
2. Run with a representative project, idle transport, on each of: main sequencer, fx
   panel, mixer, sidebar, expanded-step view. Per the policy above, expected result is
   **zero** animating widgets on all of these when idle.
3. Remove the step animation and fix any other offenders widget-by-widget. Each fix
   must be paired with the invalidation caveat from Item 1: if a widget's animation
   state can flip without a layout rebuild, flipping must mark the layout dirty (or
   the cached bool must be recomputed on the state flip).

**Payoff:** if the gate goes quiet when idle, most of the render cost disappears rather
than being optimized.

---

## Item 5 — Gate / delta the reactive `sync_*` rebuilds (~250 ms)

### 5a. `sync_neural_visualization_fields` — early-out when no graphs (47 ms, pure waste)

**Where:** `crates/sequencer/src/ui/state_values/meters_and_modulation.rs:325`, called
from `crates/sequencer/src/ui/reactive_tick.rs:473` on
`NEURAL_VISUALIZATION_POLL_INTERVAL` with **no** check that a neural graph exists. The
profiled project has zero neural sequencer scripts, yet it builds six `Value` trees
(energy/trigger/dampening matrices, `graph-visualizations`, `track-events`,
`track-event-current-beat`) and diffs them through `set_reactive` every poll.

**Fix:** before building anything, check the state for liveness — e.g.
`state.neural_visualization()` neuron count == 0 AND `state.graph_visualizations()`
empty AND no pending track-output events — and skip. **One transition edge matters:**
when the last graph disappears, push the empty values one final time so stale UI
clears, then stay skipped. (A simple `was_live: bool` in `ctx.meters` handles this.)
Also confirm whether `track-events` / `track-event-current-beat` are consumed by
non-neural graph features (graph engine `:event` nodes) — if so, gate those two on
their own liveness, not on neural liveness.

### 5b. `sync_track_params_with_neural_selection` (114 ms) + `sync_fx_param_binding_fields_with_neural_selection` (73 ms) — change-gate

**Where:** `crates/sequencer/src/ui/state_values/project_state.rs:648` and
`crates/sequencer/src/ui/state_values/param_fields_and_sync.rs:835`. Despite the names,
these are the **general** track-param / p-lock syncs (the neural selection is an
optional parameter). They rebuild `track-plocks` / `track-plock-variants` and the fx
param-binding `Value` trees on many tick paths (see call sites in `reactive_tick.rs`,
`reactive_sync.rs`, `host_commands/*`).

**Fix:** rebuild only when inputs changed. The repo already has the pattern:
`sync_track_playhead_field_delta` costs 11 ms via delta-gating. Gate on a composite of
(selected track, selection-set revision, pattern/p-lock revision for that track, scene
identity). If no single revision counter covers p-lock edits, add one at the write
seam rather than diffing full trees per tick. Host-command call sites (explicit edits)
can stay unconditional — the per-tick paths in `reactive_tick.rs` are the hot ones
(lines 235, 303, 573, 782, 920).

**Optional rename:** `sync_track_params_with_neural_selection` →
`sync_track_params_and_plocks` (the current name misled this investigation); keep the
optional neural-selection arg.

---

## Item 6 — Diagnose `poll_backend_event` (420 ms, unattributed)

`MetalBackend::poll_backend_event` (`crates/eseqlisp/src/ui/metal_backend.rs:6424`) has
420 ms weight but zero self weight in the captured table — the cost is in an
unexpanded subtree (event pump / layer callbacks / timeout handling). Before changing
anything: re-profile with that subtree expanded (or add `ESEQ_DEBUG`-style timing
around its internals) and file findings. Do not guess-fix this one.

---

## Verification

- **Scoped tests only** (workspace-wide runs are slow and have known failures):
  - `cargo test -p eseqlisp widget_render` (covers the animation-flag tests)
  - `cargo test -p sequencer ui::edit_sessions` and the state_values/reactive_tick test
    modules touched.
  - Note: known pre-existing failures exist in metal_seq layout tests and some tui
    tests — do not chase failures you didn't cause; diff against a baseline run first.
- **Perf validation:** release build, open a real project, re-profile with Instruments.
  Success criteria:
  - `layout_wants_animation_frames` aggregate < 50 ms (from ~1.05 s)
  - `pull_shared_bus_state` < 20 ms (from 654 ms)
  - `sync_neural_visualization_fields` ≈ 0 in a project with no graphs
  - main-thread total meaningfully below the audio thread's.
- **Visual regression (Item 3 only):** UI-capture comparison per
  `docs/metal-seq-ui-capture.md` on mixer + arrangement views.
- No `cargo fmt` at package scope; format only touched files.

## Sequencing

1. Item 2 (isolated, zero-risk) → 2. Item 1 → 3. Item 5a → 4. Item 5b → 5. Item 4 →
6. Item 3 (build on Items 1+4; highest risk) → Item 6 is investigation-only, any time.

Items 1, 2, 5a are independent and can be one PR each or one combined PR. Items 3–4
should be separate PRs with before/after profiles attached.
