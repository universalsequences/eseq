# Ellipses and pixel-width strokes

Use these shader forms for crisp vector-style outlines:

```lisp
(sdf/stroke-px
  (sdf/ellipse 3.5 0.3)
  1.5
  (rgba 0.16 0.06 0.11 1))
```

`sdf/ellipse` takes horizontal and vertical radii in the shader's isotropic
drawing coordinates. It returns Euclidean signed distance: negative inside,
zero on the contour, positive outside. Stretching its radii changes the shape
without stretching the stroke. Circles, axis points, the center, and radii
collapsed to a line or point are supported. Negative radii use their magnitude.
CPU hit testing uses the same geometry for `sdf/fill` and `sdf/region`.

`sdf/stroke-px` takes a distance expression, the **full** stroke width in
render-target pixels, and a color. It integrates the two stroke edges across
one pixel, with solid coverage inside and antialiasing confined to the edge.
Subpixel widths preserve partial coverage. Zero and negative widths draw
nothing. This operation does not add smoothstep softness or fade dense detail.
Keep separate contours far enough apart for their strokes and gaps to fit.

The distance must be Euclidean for uniform width. Circle, ellipse, rectangle,
and line distances satisfy this contract. Put geometry transforms inside the
distance argument:

```lisp
(sdf/stroke-px
  (sdf/translate 0.2 0.1
    (sdf/rotate 0.3 (sdf/ellipse 2 0.4)))
  1
  :white)
```

The existing `sdf/stroke` takes a **half-width in drawing units**; the new
operation explicitly specifies a pixel width. Both Metal and WGSL emitters
support the new forms.

## Distance calculation

The closest-point construction follows [David Eberly's ellipse-distance
derivation](https://www.geometrictools.com/Documentation/DistancePointEllipseEllipsoid.pdf),
sections 2.5–2.8. The implementation solves for the dimensionless quantity
`k = (t + minor_radius²) / (minor_radius * abs(y))`, whose bracket starts at
one, using geometric bisection. This preserves relative precision near an
axis without tiny squared intermediates. Axis and degenerate cases have direct solutions.
The bounded solver exits when its floating-point bracket stops changing.

Focused checks cover normal offsets around the contour, high aspect ratios,
axis continuity, transformed hit regions, both shader emitters, and numeric
execution of the emitted Metal geometry against the CPU distance calculation.
The PM Hi-Hat, PM Crash and PM Ride diagrams use these primitives for the rim,
bell and four individually spaced grooves.
