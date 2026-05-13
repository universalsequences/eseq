# SDF Materials Implementation Plan

This plan implements [sdf-materials-spec.md](/Users/alecresende/code/learning/anthropic/eseqlisp/docs/sdf-materials-spec.md) in scoped, testable steps.

## Goals

- Preserve current `defwidget` and `sdf/fill ... :accent` behavior
- Add `material`, `shadow`, and `rgba` for SDF widgets only
- Allow material color expressions to reference implicit `x`, `y`, and `d`
- Support paint overflow via `:paint-margin`
- Keep hit regions tied to `sdf/fill`, not shadows

## Non-Goals

- Built-in widget styling/materials
- TUI materials/shadows
- CPU hit-testing expansion to shadow pixels
- Generalized lighting/specular work

## Phase 0 — Lock Down Current Behavior

Outcome: a safety net before changing codegen semantics.

### Tasks

- Add focused tests for current `sdf/fill` behavior:
  - flat theme keyword fill
  - `hit/hover` / `hit/active` behavior
  - `sdf/fill` region counting within `sdf/layer`
- Add a regression test proving current `defwidget` compilation still works with flat fills

### Files

- [src/editor/tests.rs](/Users/alecresende/code/learning/anthropic/eseqlisp/src/editor/tests.rs)
- [src/lang/sdf_codegen.rs](/Users/alecresende/code/learning/anthropic/eseqlisp/src/lang/sdf_codegen.rs)

### Exit Criteria

- Existing SDF tests pass
- New regression tests pass without behavior changes

## Phase 1 — Add Material Syntax as Sugar-Preserving Data

Outcome: the Lisp surface supports `material`, `shadow`, and `rgba` without breaking old forms.

### Tasks

- Define the parsing/codegen contract for:
  - `(material :color ... :alpha ... :edge-color ... :edge-width ... :shadow ...)`
  - `(shadow :color ... :blur ... :offset ... :spread ...)`
  - `(rgba r g b a)`
- Decide whether `material` and `shadow` are compiled as special forms in `sdf_codegen` or normalized helper expressions
- Make flat fills sugar:
  - `(sdf/fill shape :accent)` => equivalent to `(sdf/fill shape (material :color :accent))`
- Keep `sdf/paint` behavior unchanged for now unless material support falls out cleanly

### Recommended Approach

- Treat `material` and `shadow` as codegen-recognized forms, not runtime VM values
- Treat `rgba` as a color constructor that compiles to `float4`
- Normalize legacy `sdf/fill shape color-expr` inside the emitter into a material with `:color`

### Files

- [src/lang/sdf_codegen.rs](/Users/alecresende/code/learning/anthropic/eseqlisp/src/lang/sdf_codegen.rs)
- [docs/sdf-materials-spec.md](/Users/alecresende/code/learning/anthropic/eseqlisp/docs/sdf-materials-spec.md)

### Tests

- `rgba` compiles to `float4(...)`
- `(material :color :accent)` compiles
- legacy `(sdf/fill shape :accent)` still compiles identically

### Exit Criteria

- Old fill syntax still works
- New material syntax parses and codegens

## Phase 2 — Introduce Implicit `d` for Materials

Outcome: material color expressions can use `d` naturally.

### Tasks

- Refactor the `sdf/fill` codegen path so the shape SDF is always evaluated once and bound to a generated local
- Expose that local as `d` while emitting material field expressions
- Ensure `x`, `y`, and hit vars continue to resolve exactly as before
- Decide whether `d` should only exist inside material evaluation scope or also inside nested shadow field evaluation

### Recommended Approach

- Replace the current “shape + color” helper with a “shape + material” helper
- Push a temporary scope mapping `d` to the generated distance variable before emitting material expressions
- Reuse the same scope mechanism already used for symbol rebinding in the Metal emitter

### Files

- [src/lang/sdf_codegen.rs](/Users/alecresende/code/learning/anthropic/eseqlisp/src/lang/sdf_codegen.rs)

### Tests

- Material `:color` using `d` compiles correctly
- Material `:color` using `x`, `y`, and `d` together compiles correctly
- Existing AA behavior remains unchanged

### Exit Criteria

- A widget can shade by distance without repeating the shape expression

## Phase 3 — Add Edge/Rim Material Fields

Outcome: materials can do more than flat fills before shadows are added.

### Tasks

- Implement `:alpha`
- Implement `:edge-color`
- Implement `:edge-width`
- Define a simple, predictable edge contribution:
  - band centered near `d = 0`
  - width controlled by `:edge-width`
  - blended above base fill

### Recommended Approach

- Keep the first implementation minimal:
  - edge band derived from `abs(d)`
  - AA-aware width using `fwidth(d)`
- Avoid introducing nested material sub-objects yet

### Files

- [src/lang/sdf_codegen.rs](/Users/alecresende/code/learning/anthropic/eseqlisp/src/lang/sdf_codegen.rs)

### Tests

- Base color plus alpha multiplier compiles
- Edge band compiles with correct use of `d`
- Materials without edge fields keep current output behavior

### Exit Criteria

- Richer non-shadow styling works with no layout/rendering changes yet

## Phase 4 — Implement Shadows in Material Codegen

Outcome: shadows render as paint-only contributions derived from the same SDF.

### Tasks

- Implement `shadow` parsing inside the material path
- Support:
  - `:color`
  - `:blur`
  - `:offset`
  - `:spread`
- Generate a shadow alpha from a transformed distance function
- Composite shadow beneath fill and edge contributions
- Ensure shadow pixels do not affect region ownership

### Recommended Approach

- Start with one shadow only per material
- Compute a shadow distance from offset coordinates:
  - either by re-emitting the shape with temporary shifted `x`/`y`
  - or by evaluating the same expression under a rebinding scope
- Keep the first shadow model soft and monotonic; avoid physically-based lighting here

### Files

- [src/lang/sdf_codegen.rs](/Users/alecresende/code/learning/anthropic/eseqlisp/src/lang/sdf_codegen.rs)

### Tests

- Shadow form compiles with default offset handling
- Shadow compiles beneath fill, not above it
- Shadow does not increment region count
- A widget with shadow still reports the same region count as without shadow

### Exit Criteria

- A simple soft shadow widget renders correctly in Metal

## Phase 5 — Add Paint Overflow to `defwidget`

Outcome: shadows can extend beyond layout bounds without changing the widget’s logical size.

### Tasks

- Extend `defwidget` to accept `:paint-margin`
- Store `paint_margin` in `SdfWidgetDef`
- Keep measurement based on `:width` and `:height` only
- Expand the rendered Metal quad by `paint_margin`
- Preserve widget positioning and hit-testing relative to logical layout bounds

### Recommended Approach

- Use a single uniform margin first, matching the spec
- Expand the visual rect in the Metal primitive builder, not in layout measurement
- Thread both logical size and paint rect through rendering only where needed

### Files

- [src/runtime.rs](/Users/alecresende/code/learning/anthropic/eseqlisp/src/runtime.rs)
- [src/widget_render/sdf_widget.rs](/Users/alecresende/code/learning/anthropic/eseqlisp/src/widget_render/sdf_widget.rs)
- [src/widget_render/mod.rs](/Users/alecresende/code/learning/anthropic/eseqlisp/src/widget_render/mod.rs)
- Any Metal backend code that assumes widget quad == layout rect

### Tests

- `defwidget` accepts `:paint-margin`
- Measure still returns the logical width/height
- Metal primitive bounds expand when `:paint-margin` is present

### Exit Criteria

- A `2x2` widget can paint like `4x4` while still laying out as `2x2`

## Phase 6 — Audit Clipping and Compositing

Outcome: overflow shadows remain visible and blend correctly.

### Tasks

- Audit Metal scissoring/clipping paths that may trim widget overflow
- Decide default clipping policy for SDF widgets inside containers
- Verify overlapping neighboring SDF widgets compose in stable z-order
- Verify focus/highlight/background rects do not accidentally occlude shadow pixels

### Files

- [src/ui/metal_backend.rs](/Users/alecresende/code/learning/anthropic/eseqlisp/src/ui/metal_backend.rs)
- [src/widget_render/mod.rs](/Users/alecresende/code/learning/anthropic/eseqlisp/src/widget_render/mod.rs)
- Any hit/layout scissor integration points

### Tests

- Visual/manual demo with adjacent shadowed widgets
- No shadow clipping in the default non-clipped container case

### Exit Criteria

- Shadows visibly overflow and blend without surprising clipping

## Phase 7 — Demo and Polish

Outcome: the feature is easy to evaluate visually and easy to keep stable.

### Tasks

- Update or replace [sdf-demo.lisp](/Users/alecresende/code/learning/anthropic/eseqlisp/sdf-demo.lisp) with material-based examples
- Include at least:
  - one flat fill using material sugar
  - one gradient-driven fill using `y`
  - one distance-driven fill using `d`
  - one soft shadow using `:paint-margin`
- Add a small `sdf->metal` inspection example for materials/shadows

### Files

- [sdf-demo.lisp](/Users/alecresende/code/learning/anthropic/eseqlisp/sdf-demo.lisp)
- [shadertest.lisp](/Users/alecresende/code/learning/anthropic/eseqlisp/shadertest.lisp)

### Exit Criteria

- The demo clearly shows why materials are more powerful than flat fills

## Suggested Delivery Order

1. Phase 0
2. Phase 1
3. Phase 2
4. Phase 4
5. Phase 5
6. Phase 6
7. Phase 3
8. Phase 7

Rationale:

- get syntax and `d` working first
- get shadows working early to prove the feature
- defer edge polish slightly if it slows shadow delivery

## Risks

- Shadow codegen may require re-evaluating the shape expression under shifted coordinates; doing that cleanly without duplicating too much emitter logic is the main technical risk
- Paint overflow may be partially blocked by existing clipping assumptions in the Metal path
- Material expression support may push the emitter toward ad hoc typing; resist adding too many one-off rules too early

## First Slice Recommendation

If the goal is to get the fastest convincing result:

1. Add `rgba`
2. Add `material :color`
3. Add implicit `d`
4. Add one `shadow`
5. Add `:paint-margin`
6. Build one compelling demo widget

That sequence gives a strong visual payoff without overbuilding the material system up front.
