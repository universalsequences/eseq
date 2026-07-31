# Modal Widget Spec

A first-class `modal` overlay widget: declared inline anywhere in a lisp UI tree,
but laid out and rendered as a centered overlay on top of the entire frame —
like a React/Radix portal. Replaces ad-hoc hacks such as the sound-selector
strip currently prepended to the arrangement view.

Metal-only. TUI renders nothing (same posture as the dropdown popup).

## 1. Lisp API

```lisp
(modal
  :is-open (sound-selector-open?)   ; reactive binding — controlled component
  :on-close (fn () (close-sound-selector!))
  :width 80                          ; cells; optional (defaults below)
  :height 30
  (v-stack
    (label "Select a sound")
    (scroll
      (each (pool-sounds) |s| (sound-row s)))))
```

Design rules:

- **Controlled component.** The widget never owns its open state. `:is-open`
  is a reactive binding; the widget merely reflects it. `:on-close` is invoked
  on scrim click and Escape — it is a *request* to close; the app closes by
  mutating the bound state. This keeps open/close scriptable, testable, and
  consistent with how the rest of the reactive layer works.
- **Children are ordinary widgets.** `v-stack`, `scroll`, `each`, `text-input`,
  `dropdown` — anything. No modal-specific child protocol.
- **Zero layout footprint in the parent.** Whether open or closed, the modal
  contributes `Size { 0, 0 }` to its parent. Opening it must never shift the
  surrounding layout. Its subtree is positioned against the frame viewport,
  not the parent's rect.
- Parsing needs no new machinery: `build_widget` (`crates/eseqlisp/src/widgets.rs:315`)
  already interleaves `:prop value` pairs and child widget maps for any form.

Props:

| prop | type | notes |
|---|---|---|
| `:is-open` | bool, bindable | required; listed in `bindable_props` |
| `:on-close` | handler | fired on scrim click / Escape |
| `:width` / `:height` | number (cells) | optional; default 70% of frame viewport, clamped to content + viewport |
| `:title` | string | optional convenience header; can also just use a `label` child |

`is-open` must be listed in `size_affecting_props` so the subtree-relayout
reuse path (`ui/layout.rs`, `size_affecting_props_equal`) rebuilds the modal
node when it flips, instead of geometry-reusing a stale closed/open subtree.

## 2. Current infrastructure (what exists, what's missing)

The dropdown already proves out the render + input mechanism, but in a
single-widget, flat-primitive form:

- **Overlay side channel** — `OVERLAY_INFO` (owner widget id + hit rect) and
  `OVERLAY_PRIMITIVES` (flat `Vec<MetalPrimitive>`), single-slot thread-locals
  at `crates/eseqlisp/src/widget_render/mod.rs:193-270`. All three collect
  entry points (`collect_metal_primitives` `mod.rs:1431`,
  `collect_metal_primitive_runs` `mod.rs:1446`, retained variant `mod.rs:1468`)
  drain it and return overlay prims separately from the tile scene.
- **Global overlay pass** — the backend accumulates per-tile overlay prims
  (offset by tile origin, `ui/metal_backend.rs:5335-5361`) and draws them after
  everything else with the scissor reset to the full viewport
  (`metal_backend.rs:5768-5880`). This is what lets overlays escape tile clip.
- **Input intercepts** — pointer events are routed to the overlay owner before
  normal hit-testing (`editor/widget_interaction.rs:781-828`); clicks outside
  the overlay rect dismiss it and are consumed so they can't hit the widget
  underneath (`editor/widget_focus.rs:115-139`); wheel events route to
  `dropdown::scroll_overlay` (`widget_interaction.rs:1844`).
- **Cache discipline** — widget scene/run caches are bypassed while an overlay
  is open (`metal_backend.rs:2647, 5183, 5255`; `mod.rs:1219`), and cache keys
  hash `overlay_viewport_bottom`.

What's missing for a modal:

1. The overlay channel carries **flat primitives drawn by one widget**; a modal
   needs a **laid-out subtree** (real `LayoutNode`s) so children get normal
   hit-testing, focus, scrolling, and dropdowns.
2. The channel is **single-slot**; dropdown-inside-modal needs a small stack.
3. The input intercept dispatches every event to **one owner widget id**; it
   must instead hit-test within the overlay's subtree.
4. The **layout engine doesn't know the frame viewport** — `LayoutEngine` is
   built per-tile with tile cols/rows (`ui/layout.rs:123-182`). The dropdown
   dodges this by computing menu geometry at render time from
   `WidgetViewport::overlay_viewport_bottom`; a modal needs the frame bounds at
   *layout* time.

## 3. Design

### 3.1 Layout: real subtree, viewport-anchored, zero parent footprint

`ModalWidget` implements `WidgetDefinition` with `is_container() -> true`.

- `measure` → `Some(Size { width: 0.0, height: 0.0 })` always. The parent
  (`v-stack`, `box`, root) sees nothing and never reflows on open.
- `layout_children`:
  - `:is-open` false → `vec![]`. No nodes, no hit-testing, no primitives.
  - `:is-open` true → compute the modal rect **in tile-local layout space**
    against the frame viewport (see 3.2): centered, `:width`/`:height` or
    default 70% of the frame, clamped to the frame. Lay out the single child
    subtree into that rect via `build_child` exactly like `box` does
    (`widget_render/box_widget.rs:365`). Children become ordinary
    `LayoutNode`s in the tree — at coordinates that may lie outside the
    modal's parent, outside the tile, even at negative rows/cols. That is
    fine: rendering diverts them to the overlay pass (3.3) and input routes
    to them through the overlay intercept (3.4), so tile clip and parent
    bounds never apply.

Because children are real nodes, `scroll`, `each`, focus rings, `text-input`,
nested `dropdown` all work with no modal-specific code.

### 3.2 Threading the frame viewport into layout

Add frame-viewport bounds (tile-local coords) to the layout pass:

- New field on `LayoutEngine` / `MeasureCtx` (`ui/layout.rs:112-131`), e.g.
  `frame_viewport: Option<Rect>` — the whole-window viewport expressed in this
  tile's local cell coordinates (so typically negative `row`/`col` origin for a
  non-top-left tile).
- The backend already computes exactly this arithmetic for
  `overlay_viewport_bottom` (`metal_backend.rs:5160`: `vp_h / cell_h -
  content_row`); extend it to all four edges and pass it wherever a
  `LayoutEngine` is constructed for a tile.
- Fallback when absent (tests, TUI, headless layout): use the tile's own
  root area. Layout tests then exercise modal geometry without a Metal
  backend.

### 3.3 Rendering: divert the modal subtree into the overlay pass

In the recursive collectors (`collect_metal_primitives_recursive` and the run
variants, `widget_render/mod.rs`), when visiting an **open modal node**:

1. Do not emit anything into the tile scene.
2. Recurse the modal's subtree into a fresh primitive vec.
3. Push into the overlay channel, in order:
   - a full-frame-viewport scrim rect (dim, e.g. black @ ~45% alpha),
   - the modal panel background/border (rounded rect, matching dropdown
     styling: `dropdown.rs:1127` `emit_rounded_rect_overlay`),
   - `PushClipRect(modal_rect)`, the subtree prims, `PopClipRect`.
4. Register the overlay entry: `push_overlay(OverlayEntry { widget_id, rect:
   modal_rect, kind: Modal })` (see 3.5).

The existing final overlay pass (`metal_backend.rs:5768`) then draws it above
all tiles and patch cables with the full-viewport scissor — zero backend
changes beyond what already exists. Run-collection: emit the modal subtree as
overlay prims (not retained runs); overlay content is already excluded from
scene caching, and caches are bypassed while any overlay is open.

A nested dropdown opened *inside* the modal keeps working unchanged: it pushes
its own prims into the same channel after the modal's (later = on top).

**Capture support** (`metal_seq capture --script … --buffer …`,
`crates/sequencer/src/ui/capture.rs`): the offscreen path currently *discards*
drained overlay prims — `widget_scene_for_layout` binds them as `_overlay`
(`metal_backend.rs:2649, 2662, 2736`) and `render_frame_into_texture` never
draws them (an open dropdown's menu is missing from captures today too). Fix:
return the drained overlay from `widget_scene_for_layout` to
`render_frame_into_texture` and draw it as a final stage into the same texture,
after the main scene. Because a capture is a single buffer filling the whole
texture, the tile-bounds fallback for `frame_viewport` (3.2) already centers
the modal correctly in the image. This makes hardcoded `:is-open true` scripts
renderable for visual iteration.

This fix has no modal dependency, so it lands **first** (phase 0) and is
verified against the existing dropdown: a capture script that renders a
dropdown with its menu forced open confirms overlay prims reach the PNG before
any modal code exists. If it somehow balloons into a capture redesign, drop it
and iterate live instead — but the discard sites above suggest it won't.

### 3.4 Input: subtree hit-testing behind the overlay gate

Generalize the overlay intercepts:

- **Overlay stack** replaces the single `OverlayInfo` (3.5). "An overlay is
  active" gates stay the same (`overlay_widget_id().is_some()` becomes
  `!overlay_stack_empty()`).
- **Pointer** (`widget_interaction.rs:781`): for the **topmost** overlay entry:
  - `kind: Dropdown` → current behavior (dispatch to owner widget id).
  - `kind: Modal` → if the point is inside the modal rect, run **normal
    subtree hit-testing** (`hit_test_layout` / `hit_test_focusable`,
    `ui/layout.rs:725/778`) rooted at the modal's node and dispatch through the
    ordinary widget-event path (`map_mouse_event` → `handle_event` →
    `apply_widget_output`). If outside: fire the modal's `:on-close` handler
    and **consume** the event (same can't-click-through guarantee as
    `widget_focus.rs:122-138`).
  - Layered dismissal: a click outside a dropdown that sits above a modal
    closes only the dropdown (pop one entry); the modal stays. Next click
    outside the modal fires `:on-close`.
- **Focus click** (`widget_focus.rs:115`): inside the topmost overlay → fall
  through to the intercept (which may focus a child); outside → dismiss
  topmost, consume.
- **Wheel** (`widget_interaction.rs:1844`): topmost `Dropdown` → existing
  `scroll_overlay`; `Modal` → normal scroll routing but hit-tested within the
  modal subtree only.
- **Hover/cursor**: same principle — hover resolution restricted to the modal
  subtree while it is topmost (a dropdown above it takes precedence).

### 3.5 Overlay stack

Replace `OVERLAY_INFO: Option<OverlayInfo>` with a small vec:

```rust
enum OverlayKind { Dropdown, Modal }
struct OverlayEntry { widget_id: u64, rect: Rect, kind: OverlayKind }
static OVERLAY_STACK: RefCell<Vec<OverlayEntry>>;
```

- Expected depth ≤ 2 today (modal + dropdown); no artificial limit.
- `set_overlay` (dropdown call sites, `dropdown.rs:943`) becomes push-or-replace
  of the topmost `Dropdown` entry; `close_dropdown`/`clear` pop only that
  entry. `close_other_dropdowns` (`dropdown.rs:74`) is unchanged in spirit:
  one dropdown at a time, but it may coexist above a modal.
- One modal at a time (v1). A second modal opening while one is up replaces it.
- `OVERLAY_PRIMITIVES` stays one flat vec — stacking order is push order
  (modal scrim+panel first, dropdown menu later), which the single final pass
  already renders correctly.
- All existing "is an overlay open" checks (`overlay_widget_id`,
  `overlay_contains`, cache bypasses, `metal_backend` gates) re-express over
  the stack: `topmost()`, `any_active()`. Audit call sites for which semantic
  each needs — cache-bypass wants `any_active()`; input wants `topmost()`.

### 3.6 Focus containment

While a modal is topmost-or-below-a-dropdown (i.e. on the stack):

- **Trap**: `collect_focusable_nodes` (`editor/widget_focus.rs:667`) and tab
  cycling restrict to the modal's subtree. Simplest implementation: when the
  stack contains a modal, root the focusable scan at the modal node instead of
  the layout root.
- **On open**: remember the previously focused widget id; focus the first
  focusable child in the modal (if any).
- **On close**: restore the remembered focus (if the widget still exists —
  fall back to none).
- **Escape**: routed to the modal (via the trapped focus path or a stack-level
  key intercept) → fire `:on-close`. If a dropdown is above the modal, Escape
  closes the dropdown first (existing `dropdown.rs:658` behavior), second
  Escape reaches the modal.
- `renders_own_focus() -> true` for the modal itself; the panel chrome is its
  focus presentation.

### 3.7 Registration checklist

- `modal` added to `BUILTIN_WIDGET_NAMES` (`widgets.rs:10-58`).
- `ModalWidget` in `WIDGET_DEFINITIONS` (`widget_render/mod.rs:978`), new file
  `widget_render/modal.rs`.
- `bindable_props: ["is-open"]`; `size_affecting_props: ["is-open", "width", "height"]`.
- `tui_render`: nothing (metal-only).
- Check `runtime.rs:1656`-style widget-name lists for anything that must know
  about the new container.

## 4. Sound-selector migration (first consumer)

Replace the strip prepended to the arrangement view (the `apply`/`+mix` rows)
with:

```lisp
(modal :is-open (sound-selector-open?)
       :on-close (fn () (set-sound-selector-open! false))
  (v-stack
    (label "Link sound to clip")
    (scroll
      (each (palette-sounds) |s|
        (sound-selector-row s)))))   ; row shows patch name, apply / +mix actions
```

The arrangement view stops mutating its own child list; opening the selector
is a pure state flip. Existing masked-relink funnel and palette metadata stay
as-is — this is a presentation change only.

## 5. Testing

- **Layout tests (ui-script pattern)**: open/closed modal — parent geometry
  identical in both states (zero-footprint invariant); modal rect centered in
  the (fallback) tile viewport; children laid out inside it. Use `each` for
  child lists, never `map` (map passes layout tests but breaks live).
- **Input regression tests**, modeled on
  `dropdown_overlay_captures_move_and_mouse_up_without_selecting_the_tile_below`
  (`editor/tests.rs:7573`):
  - click inside modal hits a child button, not the tile below;
  - click outside fires `:on-close` and does not activate the widget beneath;
  - dropdown-in-modal: outside-click closes dropdown only, modal survives;
  - Escape ordering (dropdown first, then modal);
  - focus is trapped inside while open and restored after close.
- **Overlay-geometry test**: modal in a short/offset tile emits overlay prims
  beyond the tile bounds (analog of
  `open_menu_emits_a_finite_overlay_beyond_a_short_tile`, `dropdown.rs:1303`).
- **Visual verification via capture**: check in a minimal ui script with
  `:is-open` hardcoded true (`(modal :is-open true …)`) and render it with
  `metal_seq capture --script … --out …` once the capture path draws overlay
  prims (§3.3). Use it to iterate on scrim alpha, panel chrome, and default
  sizing before wiring the reactive open state.
- Scoped test runs only (`-p eseqlisp` / `-p sequencer` with name filters);
  never `git stash`.

## 6. Phases

0. **Capture draws overlays** — stop discarding overlay prims in the offscreen
   path (§3.3 capture fix); verify with a capture script that forces a
   dropdown menu open. No modal code involved — this proves the
   overlay-in-capture path independently and fixes an existing capture gap
   (dropdown menus missing from PNGs).
1. **Overlay stack** — replace single-slot `OverlayInfo` with the
   kind-tagged stack; re-express dropdown + all gate call sites over it. Pure
   refactor, existing dropdown tests must pass unchanged.
2. **Modal widget, render path** — registration, zero-measure, viewport-anchored
   `layout_children` (with frame-viewport threading + tile-bounds fallback),
   subtree diversion into the overlay channel, scrim + panel chrome. From the
   first commit, a hardcoded `(modal :is-open true …)` capture script renders
   PNGs for visual iteration via phase 0.
3. **Input + focus** — subtree hit-testing behind the overlay gate, scrim
   dismissal, layered dismissal, focus trap/restore, Escape ordering.
4. **Sound-selector migration** — swap the arrangement-view strip for the
   modal; delete the prepended-row code path.

Phases 0–3 each land with their tests; phase 4 is the payoff and the proof the
primitive is sufficient (it exercises scroll + each + buttons + dismissal).

## 7. Deferred / non-goals

- Multiple simultaneous modals / modal-over-modal.
- Animations (fade/scale on open). The overlay pass redraws every frame while
  open, so this is purely additive later.
- Anchored popovers/tooltips (position relative to a trigger widget instead of
  centered). The overlay stack + subtree mechanism is the foundation; only the
  placement policy differs.
- TUI rendering.
