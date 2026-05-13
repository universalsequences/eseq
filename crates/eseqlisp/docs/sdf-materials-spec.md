# SDF Materials for `defwidget` — Focused Specification

## Scope

This document specifies the next layer of the Lisp SDF widget system:

- `defwidget`-defined SDF widgets only
- material-driven appearance for `sdf/fill`
- soft shadows and paint overflow
- Metal rendering behavior for these materials

Out of scope for this document:

- built-in Rust widgets
- CPU hit-testing changes beyond current `sdf/fill` region behavior
- TUI fallback rendering
- migration/parity work for existing widgets

This is a focused follow-on to [lisp-shader-widgets-spec.md](/Users/alecresende/code/learning/anthropic/eseqlisp/docs/lisp-shader-widgets-spec.md).

## Motivation

The current SDF path is strong at geometry and anti-aliased masking, but still
too flat stylistically:

- `sdf/fill` effectively means “mask with one flat color”
- shadows cannot extend beyond the widget's measured layout bounds
- expressive appearance tied to the SDF value itself (`d`) is awkward

We want a system where:

- the SDF defines form
- a material defines appearance
- `x`, `y`, and `d` are available implicitly inside material expressions
- shadows are first-class and can extend beyond layout bounds

## Design Principles

1. **Form and appearance are separate.** SDF expressions define the shape; materials define how that shape looks.
2. **`material` is declarative data.** It describes appearance and is interpreted by the renderer; it is not a bag of imperative paint calls.
3. **`x`, `y`, and `d` are implicit inside materials.**
   - `x`, `y`: local normalized coordinates in widget space
   - `d`: signed distance to the current `sdf/fill` shape
4. **Shadows are part of the material model.** They are derived from the same distance field and may render outside the layout bounds.
5. **`sdf/fill` remains the hit-region primitive.** Material richness must not blur the meaning of interactive region ownership.

## Proposed Syntax

### `material`

`material` is the new appearance object passed to `sdf/fill`.

```lisp
(material
  :color :accent)
```

The initial supported fields are:

- `:color` — required base color expression
- `:alpha` — optional scalar multiplier applied to the final fill alpha
- `:edge-color` — optional color expression for a rim/border contribution
- `:edge-width` — optional scalar width for edge emphasis, interpreted in SDF distance space
- `:shadow` — optional shadow spec created with `shadow`

Example:

```lisp
(material
  :color (mix :surface-bright :surface
              (smoothstep -0.9 0.7 y))
  :edge-color (rgba 1 1 1 0.35)
  :edge-width 0.03
  :shadow (shadow :color (rgba 0 0 0 0.18)
                  :blur 0.22
                  :offset (vec2 0 0.06)))
```

### `shadow`

`shadow` returns a shadow description consumed by `material`.

```lisp
(shadow
  :color (rgba 0 0 0 0.22)
  :blur 0.18
  :offset (vec2 0 0.05))
```

The initial supported fields are:

- `:color` — required color expression
- `:blur` — required blur radius in normalized widget-space terms
- `:offset` — optional `vec2`, default `(vec2 0 0)`
- `:spread` — optional scalar to expand or contract the effective shadow shape

## Revised `sdf/fill`

Current:

```lisp
(sdf/fill shape :accent)
```

Proposed:

```lisp
(sdf/fill shape
  (material
    :color :accent))
```

Flat theme fills remain valid through sugar. The compiler/runtime should treat:

```lisp
(sdf/fill shape :accent)
```

as equivalent to:

```lisp
(sdf/fill shape (material :color :accent))
```

This keeps the current syntax ergonomic while opening the richer path.

## Implicit Variables Inside Materials

When evaluating a material for a given `sdf/fill`, the following names are
implicitly in scope:

- `x`
- `y`
- `d`
- `hit/hover`
- `hit/active`
- `hit/region`

`d` is the signed distance produced by the shape expression passed as the first
argument to `sdf/fill`.

Example:

```lisp
(let ((shape (sdf/circle 0.8)))
  (sdf/fill shape
    (material
      :color (mix :accent :highlight
                  (smoothstep -0.08 0.02 d))
      :shadow (shadow :color (rgba 0 0 0 0.24)
                      :blur 0.18
                      :offset (vec2 0 0.05)))))
```

This should be read as:

- use `shape` for region ownership and masking
- shade the interior using a distance-aware color
- derive a soft shadow from the same shape

## Color Expressions

Material color fields accept arbitrary expressions that compile to `float4`.

This includes:

- theme keywords such as `:accent`, `:surface`, `:primary`
- `rgba`
- conditionals
- math-based color mixing
- expressions depending on `x`, `y`, and `d`

### `rgba`

`rgba` is a low-level color constructor:

```lisp
(rgba r g b a)
```

Each channel is a scalar expression. The output is a `float4`.

Example:

```lisp
(rgba 1.0 0.9 0.2 0.8)
```

### Examples

Vertical gradient:

```lisp
(material
  :color (mix :surface-bright :surface
              (smoothstep -1.0 1.0 y)))
```

Distance-driven rim tint:

```lisp
(material
  :color :accent
  :edge-color (rgba 1 1 1 0.35)
  :edge-width 0.03)
```

Hover-reactive fill:

```lisp
(material
  :color (if hit/hover :highlight :accent))
```

## Shadow Semantics

Shadows are evaluated from the current fill's SDF and rendered as a paint-only
contribution. A shadow must not create a hit region.

Conceptually:

1. Evaluate the fill SDF to obtain `d`
2. Offset the sample position by the shadow offset
3. Re-evaluate or transform the effective distance for shadow purposes
4. Convert that distance to a blurred alpha falloff
5. Composite the shadow beneath the fill contribution

The initial implementation may approximate this directly in generated Metal
code using `smoothstep` and blur/spread shaping derived from `d`.

## Paint Overflow

Materials may paint outside the widget's measured layout size, primarily for
shadows and glow.

This introduces a distinction:

- **layout bounds** — the size reserved by layout and used for interaction
- **paint bounds** — the expanded area the renderer may draw into

### Widget Definition

`defwidget` gains optional paint overflow metadata:

```lisp
(defwidget glow-dot
  :width 2
  :height 2
  :paint-margin 1
  :shader ...)
```

The initial form is:

- `:paint-margin <number>` — expands paint bounds uniformly on all sides

This should be interpreted as:

- layout still reserves `2 x 2`
- rendering may draw into `(width + 2 * paint-margin) x (height + 2 * paint-margin)`

Future extensions may add per-edge margins, but that is not required now.

### Clipping

Paint overflow must not be clipped away by default widget rendering if the
parent/container does not explicitly require clipping.

If a parent introduces scissoring for correctness, that clipping behavior must
be deliberate and documented. The default expectation for SDF widget materials
with shadows is visible overflow.

## Metal Codegen Requirements

The SDF codegen path must evolve from “flat color fill” to “shape + material”.

### Required additions

- Recognize `material`
- Recognize `shadow`
- Recognize `rgba`
- Evaluate the fill SDF once and bind it to `d`
- Evaluate material fields in the scope where `x`, `y`, `d`, and hit vars exist
- Generate:
  - fill color contribution
  - optional edge/rim contribution
  - optional shadow contribution
  - final premultiplied alpha composition

### Composition order

For one `sdf/fill`, the renderer should conceptually composite:

1. shadow
2. fill/base color
3. optional edge/rim highlight

Then the result participates in the existing `sdf/layer` compositing order.

## `defwidget` Semantics

`defwidget` continues to compile its `:shader` once at registration time.

New expectations:

- `:shader` may use `material` and `shadow`
- `:paint-margin` is stored alongside the widget definition
- the Metal primitive for the widget uses paint bounds, not just layout bounds
- region ownership still comes from `sdf/fill`, not from shadow pixels

## Examples

### Soft circular button

```lisp
(defwidget soft-dot
  :width 2
  :height 2
  :paint-margin 1
  :shader
  (let ((shape (sdf/circle 0.7)))
    (sdf/layer
      (sdf/fill shape
        (material
          :color (mix :accent :highlight
                      (smoothstep -0.08 0.02 d))
          :shadow (shadow :color (rgba 0 0 0 0.22)
                          :blur 0.18
                          :offset (vec2 0 0.05)))))))
```

### Rounded badge with edge light

```lisp
(defwidget lit-badge
  :width 6
  :height 2
  :paint-margin 1
  :shader
  (let ((shape (sdf/rounded-rect 0.9 0.6 0.14)))
    (sdf/layer
      (sdf/fill shape
        (material
          :color (mix :surface-bright :surface
                      (smoothstep -1.0 1.0 y))
          :edge-color (rgba 1 1 1 0.30)
          :edge-width 0.03
          :shadow (shadow :color (rgba 0 0 0 0.16)
                          :blur 0.20
                          :offset (vec2 0 0.05)))))))
```

## Validation

The implementation should be considered complete for this scope when the
following work:

- A `defwidget` can use `(material :color :accent)` and render identically to
  today's flat `sdf/fill ... :accent` behavior
- `rgba`-based color expressions compile and render correctly
- A material can use `d` inside `:color`
- A shadow can render beyond the layout bounds when `:paint-margin` is present
- A widget with shadow still uses only the base fill shape for hit regions
- Two adjacent widgets with paint overflow blend correctly without corrupting
  z-order

## Non-Goals

These are intentionally deferred:

- physically based lighting
- automatic normal estimation and specular terms
- generalized material reuse/inheritance
- applying materials to built-in widgets
- TUI rendering parity for materials and shadows

## Future Extensions

The chosen syntax leaves room for additional material fields later, for example:

- `:glow`
- `:inner-shadow`
- `:specular`
- `:light-dir`
- `:normal-strength`

These should remain additive extensions to `material`, not reasons to collapse
form and appearance back into one primitive.
