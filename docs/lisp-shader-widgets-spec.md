# Lisp Shader Widgets — Full Specification

## Overview

Replace the current hardcoded Rust widget renderers and handwritten Metal shaders
with a system where **widgets are defined entirely in Lisp** using SDF
(signed distance field) primitives. The Lisp compiler produces Metal fragment
shader code directly from the SDF expressions, and hit regions are derived
automatically from the shader AST.

This eliminates the need to write Rust + Metal boilerplate for every new widget.
Users write Lisp; they get a GPU-rendered, interactive widget.

### Current state

- 14+ widget types hardcoded in Rust (`src/widget_render/*.rs`)
- Each widget implements `WidgetDefinition` with `measure()`, `tui_render()`,
  `metal_primitives()`
- Metal shaders are inline string literals in Rust, one per widget
- Hit testing uses a spatial grid over the layout tree (`src/ui/hit.rs`)
- Interaction is dispatched per-widget-type in `widget_interaction.rs`
- The Lisp compiler (`src/lang/compiler.rs`) already has `EmitTree` for widget
  trees and recognizes built-in widget type names

### End state

A user writes this Lisp and gets a fully interactive, GPU-rendered XY pad:

```lisp
(defstate xy-x 0.0)
(defstate xy-y 0.0)

(defwidget xy-pad
  :props ()
  :measure (fn (c) (dict :width 160 :height 160))

  :shader
  (let ((bg    (sdf/rounded-rect 0.95 0.95 0.04))
        (xhair (sdf/union
                 (sdf/rect 0.005 0.95)
                 (sdf/rect 0.95 0.005)))
        (knob  (sdf/translate xy-x xy-y
                 (sdf/circle 0.06))))
    (sdf/layer
      ;; background — single hit region (the whole pad is draggable)
      (sdf/fill bg :surface)
      ;; crosshair + knob are paint-only (no hit region)
      (sdf/paint xhair 0.01 :dim)
      (sdf/paint knob (if hit/active :accent :primary))))

  :on-drag (fn (region dx dy)
    (set! xy-x (clamp (+ xy-x dx) -0.9 0.9))
    (set! xy-y (clamp (+ xy-y dy) -0.9 0.9))))
```

---

## Design Principles

1. **`sdf/fill` = hit region, everything else = paint-only.** No annotation
   beyond choosing the right call.
2. **Shader-local contextual variables** (`hit/hover`, `hit/active`,
   `hit/region`) are scoped to the enclosing `sdf/fill`. The compiler resolves
   them to uniform comparisons.
3. **SDF stdlib is Lisp** — shipped as a `.lisp` file, loaded at startup via
   `defmacro`. The compiler doesn't special-case SDF math; it compiles whatever
   the macros expand to.
4. **One quad per widget instance.** The fragment shader does all the work.
   No mesh complexity.
5. **Theme colors are resolved at compile time** from keyword arguments
   (`:surface`, `:accent`, `:primary`, etc.) to concrete `float4` values.

---

## Milestones

### Milestone 0 — SDF Standard Library (Lisp-only, no codegen)

Ship a `sdf-stdlib.lisp` that defines the core SDF primitives as macros. These
are pure math — they expand to arithmetic expressions over `x` and `y`.

**Deliverables:**

- [ ] `sdf-stdlib.lisp` file loaded at editor startup (via `runtime.rs` init)
- [ ] Primitive shapes:
  - `(sdf/circle r)` → `(- (length (vec2 x y)) r)`
  - `(sdf/rect w h)` → box SDF
  - `(sdf/rounded-rect w h r)` → rounded box SDF
  - `(sdf/line x1 y1 x2 y2 thickness)` → segment SDF
- [ ] Transform combinators:
  - `(sdf/translate tx ty body)` → rebound `x`, `y` via `let`
  - `(sdf/scale sx sy body)` → rebound `x`, `y`
  - `(sdf/rotate angle body)` → rebound `x`, `y`
- [ ] Boolean combinators:
  - `(sdf/union a b)` → `(min a b)`
  - `(sdf/subtract a b)` → `(max a (- b))`
  - `(sdf/intersect a b)` → `(max a b)`
  - `(sdf/smooth-union a b k)` → smooth-min
- [ ] `vec2`, `length`, `abs`, `clamp`, `mix`, `smoothstep`, `atan2`, `dot`,
  `fract`, `sin`, `cos` — either as compiler intrinsics or native functions,
  whichever integrates faster

**Validation:** Write unit tests that evaluate SDF expressions in the VM and
verify distance values at known sample points. For example,
`(sdf/circle 0.5)` at `x=0, y=0` should return `-0.5` (inside).

**No Metal output yet.** This milestone is about getting the math right.

---

### Milestone 1 — SDF-to-Metal Codegen

Given a fully macro-expanded SDF expression tree, emit a Metal fragment shader
function as a string.

**Deliverables:**

- [ ] New module: `src/lang/sdf_codegen.rs`
- [ ] Input: a `Value` (the macro-expanded SDF expression tree)
- [ ] Output: a Metal Shading Language string containing a `fragment float4`
  function
- [ ] Coordinate mapping: the shader receives `uv` in `[0,1]×[0,1]`, mapped to
  `x,y` in `[-1,1]×[-1,1]` with aspect-ratio correction
- [ ] Expression compilation covering:
  - Arithmetic: `+`, `-`, `*`, `/`
  - Math intrinsics: `length`, `abs`, `min`, `max`, `clamp`, `mix`,
    `smoothstep`, `sin`, `cos`, `atan2`, `fract`, `dot`, `pow`, `sqrt`
  - `let` bindings → Metal local variables
  - `if`/`cond` → ternary chains
  - `vec2`, `vec3`, `vec4` constructors → Metal vector constructors
  - Number literals, bound variables (`x`, `y`, `uv`)
- [ ] Theme color resolution: `:surface`, `:accent`, `:primary`, `:highlight`,
  `:border`, `:dim`, `:surface-bright` keywords → `float4` constants looked up
  from a theme table at codegen time
- [ ] Anti-aliasing: `smoothstep` AA band width derived from `fwidth()`
- [ ] Uniform binding for reactive state: `defstate` variables referenced in the
  shader become Metal uniforms in a per-widget buffer

**Validation:** Generate a shader string for `(sdf/fill (sdf/circle 0.4) :accent)`,
compile it through Metal's `newLibraryWithSource:`, and render a colored circle
on a quad.

---

### Milestone 2 — `sdf/layer`, `sdf/fill`, `sdf/paint`, `sdf/stroke`

These are the compositing primitives that turn individual SDFs into a visible,
layered image with hit regions.

**Deliverables:**

- [ ] `(sdf/layer &body)` — back-to-front compositing container. Each child
  paints over the previous. Compiles to a Metal function that evaluates each
  child in order and alpha-blends the results.
- [ ] `(sdf/fill sdf-expr color-expr)` — fills the interior of the SDF
  (distance < 0) with the given color. **Creates a hit region.** The region
  index is assigned at compile time by the order of `sdf/fill` calls within the
  `sdf/layer`.
- [ ] `(sdf/paint sdf-expr color-expr)` — same visual as `sdf/fill` but does
  **not** create a hit region.
- [ ] `(sdf/stroke sdf-expr width color-expr)` — strokes the boundary of the
  SDF. No hit region.
- [ ] `(sdf/label tx ty text)` — renders a text glyph at the given position.
  Uses the existing glyph atlas. No hit region.

**Hit region encoding:**

The codegen walks the `sdf/layer` body. Each `sdf/fill` is assigned an
incrementing `region_id` (0, 1, 2, ...). The generated shader writes the
`region_id` of the topmost filled region at each pixel into a secondary output
(or encodes it in the alpha channel — see Milestone 3 for the hit-test
mechanism).

**Contextual variables injected per `sdf/fill`:**

| Variable      | Metal codegen                                    |
|---------------|--------------------------------------------------|
| `hit/hover`   | `(hit_region == <region_id>)`                    |
| `hit/active`  | `(hit_region == <region_id>) && (hit_pressed)`   |
| `hit/region`  | `hit_region` (the uniform, raw int)              |

These are passed to the shader as uniforms:
- `hit_region` (`int`): the region currently under the pointer (-1 if none)
- `hit_pressed` (`bool`): whether the pointer is currently down

**Validation:** Render a widget with two `sdf/fill` regions side by side. Verify
that hovering each region highlights only that region.

---

### Milestone 3 — Hit Testing for Shader Widgets

Connect the SDF hit regions to the input system so that mouse events dispatch to
the correct region.

**Approach — CPU-side SDF evaluation:**

Rather than reading back from the GPU (which introduces latency), evaluate the
SDF expressions **on the CPU** at the pointer coordinates. This reuses the
existing VM — the SDF macros expand to arithmetic that the VM can execute
directly.

**Deliverables:**

- [ ] `sdf_hit_test(widget: &CompiledWidget, local_x: f32, local_y: f32) -> i32`
  — evaluates each `sdf/fill` SDF at the given point, returns the index of the
  topmost region where distance < 0, or -1.
- [ ] Integrate with `widget_interaction.rs`: when a mouse event hits a
  shader-widget layout node, call `sdf_hit_test` to determine the region, then
  pass the region index to `:on-press`, `:on-release`, `:on-click`, `:on-drag`
  callbacks.
- [ ] The `hit_region` and `hit_pressed` uniforms are updated each frame from
  the last pointer position and button state.

**Why CPU-side?**
- No GPU readback latency
- Works identically on TUI backend (future milestone)
- The SDF expressions are simple arithmetic — evaluation is sub-microsecond
- The VM already supports all required math ops from Milestone 0

**Validation:** Log region indices during mouse-move over a multi-region widget.
Confirm correct region identification at boundaries.

---

### Milestone 4 — `defwidget` Macro & Lifecycle

Wire everything together: a single `defwidget` form that declares a complete
interactive widget.

**Deliverables:**

- [ ] `defwidget` recognized by the compiler (new special form or macro):
  ```lisp
  (defwidget name
    :props (prop-list)
    :bind state-var          ;; optional
    :measure (fn (constraints) ...)
    :shader (sdf/layer ...)
    :on-press (fn (region) ...)      ;; optional
    :on-release (fn (region) ...)    ;; optional
    :on-click (fn (region) ...)      ;; optional
    :on-drag (fn (region dx dy) ...) ;; optional
    :on-scroll (fn (region delta) ...)) ;; optional
  ```
- [ ] Registration: `defwidget` registers the widget name so it can be used in
  widget trees like any built-in widget:
  ```lisp
  (v-stack
    (label "XY Pad")
    (xy-pad))
  ```
- [ ] Shader compilation happens **once** at widget registration time (not every
  frame). The compiled Metal function is cached and reused for all instances.
- [ ] Per-instance data (reactive state values, `hit_region`, `hit_pressed`) is
  passed as a uniform buffer, updated each frame.
- [ ] `:measure` is called during layout, must return a map with `:width` and
  `:height` keys (in logical points).
- [ ] Event handlers receive the region index as the first argument. The runtime
  resolves which handler to call based on event type.
- [ ] `defstate` variables referenced in `:shader` are automatically tracked as
  dependencies. When they change, the widget is re-rendered (uniform buffer
  updated, frame redrawn).

**Validation:** Define a toggle widget purely in Lisp using `defwidget`. Verify
it renders, responds to clicks, and updates state.

---

### Milestone 5 — `each-indexed` & Dynamic Region Counts

Support widgets where the number of hit regions depends on runtime data (e.g., a
radial menu with N items).

**Deliverables:**

- [ ] `(each-indexed list |item i| body)` — loop construct within `sdf/layer`.
  Iterates over a list and evaluates `body` for each element, with `item` and
  `i` bound.
- [ ] Dynamic region counts: when `each-indexed` contains `sdf/fill`, the
  region count = list length. The compiler emits a shader loop.
- [ ] CPU hit-test loop: `sdf_hit_test` iterates the same list to determine the
  region.
- [ ] Metal codegen for bounded loops (Metal shading language supports `for`
  loops with uniform bounds).
- [ ] List data passed as a uniform buffer (item count + per-item payload).

**Constraints:**
- Maximum region count is bounded (e.g., 64) to keep the uniform buffer fixed-size.
- The shader loop iterates up to `item_count` (a uniform), not a compile-time
  constant.

**Validation:** Build a radial menu widget with a variable-length `items` list.
Verify that adding/removing items updates the shader correctly and hit regions
track each wedge.

---

### Milestone 6 — TUI Fallback Rendering

Render shader widgets in the terminal when Metal is unavailable.

**Deliverables:**

- [ ] `sdf_tui_render(widget: &CompiledWidget, rect: Rect, frame: &mut RenderFrame)`
  — evaluates the SDF expression at each cell center, maps the result to a
  character + color.
- [ ] Color mapping: theme colors map to terminal ANSI colors (already defined
  in the TUI renderer).
- [ ] Fill rendering: cells inside an SDF region (distance < 0) get filled with
  a block character (`█`) in the region's color.
- [ ] Stroke rendering: cells near the boundary (|distance| < threshold) get a
  border character.
- [ ] Labels: rendered as regular text at the cell position closest to the SDF
  coordinates.
- [ ] Hit testing works identically (CPU-side evaluation).

**Constraints:**
- Terminal resolution is coarse (~2:1 aspect ratio cells). Results won't look as
  polished as Metal, but should be functionally correct.
- Half-block characters (`▀`, `▄`) can double effective vertical resolution.

**Validation:** Render the XY pad widget in the TUI. Verify it's recognizable
and interactive.

---

### Milestone 7 — Migration & Parity

Rewrite existing built-in widgets as `defwidget` definitions, proving the system
can replace the hardcoded Rust implementations.

**Deliverables:**

- [ ] `widgets/slider.lisp` — horizontal/vertical slider
- [ ] `widgets/toggle.lisp` — toggle switch
- [ ] `widgets/knob.lisp` — rotary knob
- [ ] `widgets/meter.lisp` — level meter
- [ ] Verify visual parity with current Metal shaders (screenshot comparison)
- [ ] Verify interaction parity (drag, click, scroll behaviors match)
- [ ] Remove corresponding Rust widget render code once Lisp versions are stable
- [ ] Performance benchmark: ensure Lisp-defined widgets render within the same
  frame budget as hardcoded ones (< 1ms per widget at 60fps)

**Validation:** Run the existing `params-panel.lisp` demo with Lisp-defined
widgets. Confirm identical behavior.

---

## Architecture Summary

```
                    ┌─────────────────────────────────────────┐
                    │             defwidget form               │
                    │  :shader  :measure  :on-press  :on-drag  │
                    └────┬──────────┬──────────┬───────────────┘
                         │          │          │
                    macro expand    │     stored as closures
                         │          │     in widget registry
                         ▼          ▼
                  ┌──────────┐  ┌────────┐
                  │ SDF AST  │  │ VM fn  │
                  │ (Value)  │  │measure │
                  └────┬─────┘  └────────┘
                       │
            ┌──────────┼──────────┐
            ▼          ▼          ▼
     ┌────────────┐ ┌──────┐ ┌────────────┐
     │Metal codegen│ │CPU   │ │TUI codegen │
     │sdf_codegen  │ │hit   │ │sdf_tui     │
     │.rs          │ │test  │ │_render.rs  │
     └─────┬──────┘ └──┬───┘ └─────┬──────┘
           │            │           │
           ▼            ▼           ▼
     ┌──────────┐ ┌──────────┐ ┌──────────┐
     │ Metal    │ │ Event    │ │ TUI      │
     │ fragment │ │ dispatch │ │ cells    │
     │ shader   │ │ (region) │ │ (ratatui)│
     └──────────┘ └──────────┘ └──────────┘
```

## SDF Rendering Model

### Coordinate space

All SDF expressions operate in a normalized coordinate space:
- `x` ∈ `[-1, 1]` (left to right)
- `y` ∈ `[-1, 1]` (bottom to top)
- Aspect ratio is corrected: if the widget is wider than tall, `x` range
  expands proportionally (and vice versa)
- `(0, 0)` is the center of the widget

### Layer compositing

`sdf/layer` evaluates children back-to-front:

```
pixel_color = transparent
for each child in layer:
    child_color = evaluate(child, x, y)
    pixel_color = alpha_blend(pixel_color, child_color)
return pixel_color
```

### Fill vs paint distinction

| Form         | Renders? | Hit region? | Region index      |
|--------------|----------|-------------|-------------------|
| `sdf/fill`   | yes      | yes         | auto-incremented  |
| `sdf/paint`  | yes      | no          | —                 |
| `sdf/stroke`  | yes      | no          | —                 |
| `sdf/label`  | yes      | no          | —                 |

### Shader-local contextual variables

| Variable      | Type  | Meaning                                          |
|---------------|-------|--------------------------------------------------|
| `hit/region`  | int   | index of currently hovered region (-1 if none)   |
| `hit/hover`   | bool  | true if THIS `sdf/fill`'s region is hovered      |
| `hit/active`  | bool  | true if THIS `sdf/fill`'s region is pressed      |

`hit/hover` and `hit/active` are contextual — they resolve differently inside
each `sdf/fill` based on that fill's region index. The Metal codegen replaces
them with `(hit_region == N)` comparisons.

## Uniform Buffer Layout (per widget instance)

```metal
struct WidgetUniforms {
    float4 theme_colors[8];   // resolved theme palette
    int    hit_region;        // hovered region index (-1 = none)
    int    hit_pressed;       // 1 if pointer is down, 0 otherwise
    int    region_count;      // total sdf/fill regions
    float  time;              // elapsed seconds (for animation)
    // reactive state slots (variable count, determined at compile time):
    float  state[MAX_STATE_SLOTS];
};
```

## Risk & Open Questions

1. **Shader compilation latency.** `newLibraryWithSource:` is not instant. Plan:
   compile on a background thread, display a placeholder (solid color quad)
   until ready. Cache compiled pipelines keyed by shader source hash.

2. **`sdf/label` in Metal.** Text rendering inside a fragment shader requires
   sampling the glyph atlas texture. The codegen must emit texture sample calls
   and receive the atlas as a shader resource. This is achievable — the existing
   Metal backend already binds a glyph atlas — but adds complexity to the
   codegen.

3. **Debug story.** When a shader doesn't look right, how does the user debug
   it? Options: (a) `sdf/debug` mode that color-codes distance values,
   (b) log the generated Metal source to a buffer, (c) hot-reload on save.

4. **Performance ceiling.** Complex SDFs with many boolean operations can get
   expensive per-pixel. Mitigation: bounding-box culling per `sdf/fill` region,
   early-out when alpha saturates.

5. **`each-indexed` in Metal.** Metal supports `for` loops but not dynamic
   dispatch. The loop body must be fully unrollable or bounded by a uniform.
   This is fine for reasonable item counts (< 64).
