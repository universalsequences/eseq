# Interactive shapes in defwidget

Author an interactive shape with a stable keyword identity:

```lisp
(sdf/region :handle
  (sdf/translate position 0 (sdf/circle 0.05))
  (material :color (if hit/hover :white :black))
  (sdf/translate position 0 (sdf/circle 0.15)))
```

Arguments are the region keyword, visible signed-distance expression, material,
and optional hit-distance expression. Without the last argument, the visible
shape is also the hit target. Use `sdf/paint` for decoration. Shapes inside
`sdf/layer` and surrounding `let` scopes share the same parameter bindings in
GPU drawing and CPU hit-testing. The topmost matching region wins.

Callbacks retain the existing `(sx sy region)` signature. Coordinates are
normalized to [-1, 1] across the widget, before shader aspect correction.
For `sdf/region`, `region` is its keyword; existing `sdf/fill` shapes return
numeric paint-order indices. A miss is -1. Hover uniforms still use numeric
region indices internally, so existing `hit/hover` material expressions work.

The editor captures the region at pointer-down and keeps it for drag and
mouse-up, even after the bound shape moves or the pointer leaves its hit area.
Capture ends with the editor's normal gesture lifecycle. Both pointer and hover
hit-tests use the actual pixel aspect ratio, including layout cell dimensions.

`exp` is supported by both shader emitters and the CPU distance evaluator.

## Clap example

`content/instruments/Drums/808 Clap/ui.lisp` implements a procedural display
using this API. Burst spacing, sub-bursts and the fast/slow tail follow the DSP
parameters. A fixed procedural texture illustrates noise; it does not claim to
show the exact filtered, saturated audio output. A quadratic time axis gives
short bursts useful editing space while retaining the long tail. The drag
mapping uses the inverse of that same axis and writes through the scoped
instrument parameter setter, including p-lock editing.

Direct shader inputs use reactive bindings. Products and unit conversions are
computed in a scoped Lisp subtree when their inputs change. No samples, audio
render jobs, or additional built-in instrument widget are involved.
