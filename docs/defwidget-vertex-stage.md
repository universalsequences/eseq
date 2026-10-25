# defwidget vertex stage (mesh widgets)

Status: built 2026-10-25 (bead eseq-k0m2.28) on Metal. The WGSL lowering is
emitted and validated with naga, but the wgpu renderer does not draw mesh
widgets yet (see "Backends"). Motivation: docs/modalys-gap-analysis.md §4.3.

A `defwidget` with a `:vertex` body is a *mesh widget*. It draws a triangle
mesh: the vertex stage runs once per mesh vertex, and the `:shader` runs per
pixel and reads what the vertex stage passed on. Both are written in the
same Lisp shader DSL as every other `defwidget`. Ordinary widgets are not
affected.

## Example

```lisp
(defwidget mode-mesh-face
  :width 24 :height 14
  :state (scale yaw pitch zoom (amps :floats 256))
  :vertex
  (let ((u (/ (* scale mesh-radius (dot amps (table-row vertex-id)))
              (max table-peak 0.000001)))
        (p (+ position (* normal u))))
    (out :position (orbit-project p (+ yaw orbit-yaw) (+ pitch orbit-pitch) zoom)
         :varyings ((disp u)
                    (view (orbit-rotate p (+ yaw orbit-yaw) (+ pitch orbit-pitch))))))
  :shader
  (let ((n (normalize (cross (dfdx view) (dfdy view))))
        (lit (+ 0.3 (* 0.7 (abs (dot n (normalize (vec3 -0.4 0.6 0.7))))))))
    (* (mix (rgba 0.2 0.4 0.9 1) (rgba 1 0.5 0.2 1) (clamp (+ 0.5 disp) 0 1))
       (rgba lit lit lit 1))))

(mode-mesh-face :src "bell.modes.json" :amps (list 1 0.5) :scale 0.1
                :yaw 0.6 :pitch 0.6 :zoom 1)
```

The working version is `mode-mesh-face` / `mode-mesh-view` in
`content/ui/modal/mesh-view.lisp` (module `eseq.modal.mesh-view`).

## The vertex stage (`:vertex`)

Inputs, all readable anywhere in the body:

| Symbol | Type | Meaning |
|---|---|---|
| `position` | vec3 | the vertex's position, in the geometry's units (metres) |
| `normal` | vec3 | its unit normal |
| `vertex-id` | float | its index in the geometry |
| `mesh-center`, `mesh-radius` | vec3, float | bounding sphere of the geometry |
| `table-cols` | float | width of the per-vertex table (modes of a mode-shape table) |
| `table-peak` | float | the largest absolute table entry |
| `vertex-count` | float | vertices in the geometry |
| `aspect`, `itime` | float | the widget's width/height, shader time |
| `orbit-yaw`, `orbit-pitch` | float | the built-in drag orbit, radians (add them to your camera) |
| scalar `:state` names, instance fields | float | as in any defwidget |

Mesh forms:

- `(out :position clip :varyings ((name value) …))` ends the body (it may sit
  inside `let`s): `clip` is the clip-space position (a vec4; a vec3 gets
  w = 1), and each varying is passed to the fragment stage under its name. At
  most 4 varyings, each a float or vec2/3/4 (the type is inferred; wrap in
  `vec3` to be explicit). `out` is written exactly once.
- `(dot amps (table-row r))`: the sum over k of `amps[k] * table[r][k]`, for k
  below both the array's length and the table width. With mode amplitudes and
  a mode-shape table this is the modal displacement of vertex r along its
  normal. `r` is clamped to the geometry.
- `(table r k)`: one table entry (0 for geometry without a table).
- `(orbit-project p yaw pitch [zoom])`: clip position of mesh point `p` seen
  by an orbit camera 3.2 mesh radii from `mesh-center`, fitted to the widget
  (zoom 1 fits the bounding sphere). The geometry is z-up: yaw turns it about
  z; pitch 0 looks along +y, pi/2 straight down. Depth is perspective-correct.
- `(orbit-rotate v yaw pitch)`: `v` turned into view space (x right, y up,
  z toward the camera), for lighting.

## The fragment stage (`:shader`)

Reads the varyings by name, the scalar state, the array states, `aspect`,
`itime`, `orbit-yaw`/`orbit-pitch`, `mesh-center`/`mesh-radius`, and `x`/`y`
(-1..1 across the widget, y down, as in 2-D widgets). It must return a
colour; alpha below 0.001 discards the pixel. Reading `position`, `normal`,
`vertex-id` or `table` here is a codegen error naming the vertex stage.
`(dfdx v)` / `(dfdy v)` (screen-space derivatives, also available to 2-D
widgets) give the displaced surface's own face normal:
`(normalize (cross (dfdx view) (dfdy view)))`.

## Array states

`:state` entries may be `(name :floats n)`: an array of n floats, read in
either stage as `(name i)` (clamped into the array, never out of bounds) and
`(size name)` (how many the instance gave). An instance passes a list of
numbers under the name (`:amps (list 1 0.5)`); missing entries read 0. At
most 8 arrays and 4096 floats per widget; a mode-amplitude array is `(amps
:floats 256)`.

This is the part of docs/defwidget-array-states-spec.md (eseq-nu3h) that the
mesh needs: the declaration syntax and the indexed read of §3-§4, lowered to
one buffer of arrays per instance. Not built: rank-2 arrays, the `f32-array`
value type and its natives, snapshot natives and sourced (`:source`) arrays,
and array states on ordinary (non-mesh) widgets, which are an error for now.

## Geometry

Geometry is per instance, uploaded to the GPU once per distinct mesh (its id
is a hash of its content) and kept while drawn:

- `:src "path"` names an **eseq-modes** file. A relative path is tried beside
  the file whose code constructs the widget, then beside the file being
  loaded, then the working directory, then a content root. A wrapper function (`mode-mesh-view`)
  constructs it from the wrapper's own file, so for a file beside an
  instrument's ui.lisp prefer a top-level `(def m (mesh-load "x.modes.json"))`.
- `:geometry` takes a handle from `(mesh-load path)` or `(mesh-geometry
  dict)`, or a geometry dict itself: `:points`, `:triangles`, and optionally
  `:normals` (else area-weighted vertex normals), `:shapes`, `:freqs` (or
  `:modes`, the table width) and `:mass`; lists may be flat or nested. A
  handle is `(dict :mesh-id id :points P :modes N :freqs (…) :mass kg
  :radius r)`, so Lisp can read the mode frequencies.
- `eseq.modal.mesh-view` generates the analytic objects: `(string-geometry)`
  (a thin ribbon, harmonics sin(k pi u)), `(plate-geometry)` (simply
  supported, sin(m pi u) sin(n pi v)), `(membrane-geometry)` (clamped disc,
  J_m(j_mn r) cos(m theta)).

The eseq-modes v1 file, as the finite-element solver writes it:

```json
{"format": "eseq-modes", "version": 1,
 "freqs": [N], "points": [P*3, metres], "normals": [P*3],
 "triangles": [T*3 indices], "shapes": [P*N, point-major: shapes[p*N+k] is
 mode k's displacement along the normal at point p], "mass": kg, "spec": {}}
```

`scripts/gen_modes_fixture.py plate|bell OUT.json` writes small fixtures
without the solver (checked in under
`crates/sequencer/ui/capture-fixtures/modes/`). Limits: 2^20 vertices, 256
table columns.

## Rendering

The 2-D renderer positions vertices on the CPU and uses pass-through vertex
shaders (crates/sequencer/docs/gpu-displacement-spec.md); a mesh widget does
not join it. Instead an instance:

1. leaves a draw job (geometry id, scalar uniforms, packed arrays, target
   size, clear colour) in `widget_render/mesh_widget.rs`, and
2. paints an ordinary image primitive whose `src` is
   `eseq-mesh-target:<widget id>`.

When the Metal backend composites that image (`draw_image_primitives`), it
first renders the job if it changed (or every frame for `:animates true`)
into the instance's own target: a 4x MSAA BGRA colour texture plus a
Depth32Float buffer (depth test less, no culling), resolved to a texture the
image pipeline then samples into the widget rect (`:corner-radius` rounds
it). The job renders in its own command buffer, committed while the frame is
encoded, so the queue finishes it before the frame samples it
(`ui/metal_backend/inner/mesh_pass.rs`). `:background` sets the target's clear
colour; without it the target is transparent.

Dragging on a mesh widget orbits it: a drag across the widget's width turns
`orbit-yaw` half a turn, down its height tilts `orbit-pitch` a quarter turn.
Its `:on-drag` and other callbacks still fire.

## Backends

- **Metal** (macOS): vertex + fragment MSL in one library (`mesh_vert`,
  `mesh_frag`, preamble `MESH_PREAMBLE_MSL`), the offscreen pass above.
- **WGSL**: `compile_mesh_to_wgsl` emits the same module (`MESH_PREAMBLE_WGSL`,
  storage buffers at group 0 bindings 0-5) and is what non-macOS builds
  register; it validates with naga. The wgpu renderer does not run it yet: a
  mesh widget's image src names no file, so the wgpu image path draws nothing
  for it (the widget's rect stays empty).

## Code

| Part | Where |
|---|---|
| Lowering of both stages | `crates/eseqlisp/src/lang/sdf_codegen/mesh.rs` |
| `defwidget :vertex`, array entries, `mesh-load`, `mesh-geometry` | `crates/eseqlisp/src/runtime.rs` |
| Geometry, registries, draw jobs, drag orbit | `crates/eseqlisp/src/widget_render/mesh_widget.rs` |
| Offscreen pass | `crates/eseqlisp/src/ui/metal_backend/inner/mesh_pass.rs` |
| Demo widget and generators | `content/ui/modal/mesh-view.lisp` |
| Capture fixture | `crates/sequencer/ui/capture-fixtures/mode-mesh-view.lisp` |
