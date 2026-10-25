# Modal patches

Build Modalys-style physical-model instruments from objects and connections,
written as eseqlisp data with the `eseq.modal` module
(`content/ui/modal.lisp`) and compiled to an ordinary DGenLisp instrument.

```sh
eseq modal emit tools/modal/examples/cello.lisp [DIR]
```

This writes `dsp.lisp`, `ui.lisp` and `instrument.json` into DIR, by default
the user instrument folder under the patch's `:name`
(`.local/instruments/modal-cello` in a dev checkout). Check the result with
`eseq instrument check DIR`.

## A patch script

A script imports the module and ends with the patch:

```lisp
(import eseq.modal :as m)
(import eseq.modal.materials :as mat)

(m/patch :name "my-guitar"
  :objects     (list (m/finger 'fin :depth 0.1)
                     (m/string 'str :made-of mat/steel :length 0.65 :modes 60)
                     (m/plate 'body :made-of mat/spruce))
  :connections (list (m/pluck 'pick 'fin 'str 0.8)
                     (m/spring 'bridge 'str 0.96 'body (list 0.55 0.4)))
  :listens     (list (m/listen 'str 0.2 :gain 0.6)
                     (m/listen 'body (list 0.3 0.65) :gain 0.7)))
```

It is ordinary Lisp, so a patch can be computed: `examples/sympathetic.lisp`
bridges any number of tuned strings onto one body.

| Objects | Access point |
|---|---|
| `m/string` (stiff string, follows the note, `:tune` ratio) | `x` in 0..1 |
| `m/plate` (simply supported, 8 x 8 modes) | `(x y)` |
| `m/membrane` (circular, 120 Bessel modes, `:tune`) | `(r theta)` |
| `m/finger` (Modalys' two-mass: hand on a stroke + soft tip) | lumped |
| `m/mallet` (thrown mass) | lumped |

| Connections | Between |
|---|---|
| `(m/pluck id finger obj at)` | catch, hold, release |
| `(m/felt id mallet obj at)` | one-sided, hardening felt |
| `(m/spring id a a-at b b-at)` | bridge / adhere |
| `(m/bow id obj at)` | stick-slip friction, sustains under the gate |

Materials are data. `eseq.modal.materials` (`content/ui/modal/materials.lisp`)
holds a library (`mat/steel`, `mat/nylon`, `mat/spruce`, `mat/bronze`,
`mat/glass`, ...), each a dict of name, density, Young's modulus and
Poisson's ratio. A string or plate is `:made-of` one, or a list of them to
pick from on the panel (the first is the default):

```lisp
(m/plate 'body :made-of (list mat/spruce mat/maple mat/rosewood))
(m/string 'str :made-of (mat/material :name "gut" :density 1300 :young 5e9))
(m/string 'str :made-of (merge mat/steel :name "spring steel" :young 2.1e11))
```

(The argument is not called `:material`: every call quotes the value after
`:material`, which carries shader code for widgets.)

`(m/check patch)` lists what is wrong with a patch by name (`spring bridge:
no object bdy`); `eseq modal emit` prints the same list instead of writing
anything.

Every param is named after its object or connection (`str_length`,
`bridge_stiff`) and grouped by it (`@group str`), so the auto-generated panel
shows one section per object, and the generated `ui.lisp` (addressing params
as `str.str_length`) gives each a knob panel around a screen that draws the
patch and lists the selected section's params.

## What the generated DSP does

Each sample follows the protocol of the `content/defmacros/modal-*` macros:
predict every object's free motion; solve the exciters (pluck, felt) in turn;
solve springs and bows together by Gauss-Seidel sweeps; commit every object.
All forces are solved inside the sample from both ends' compliance, so any
stiffness is stable.

## Rules learned the hard way

- Library macros take and return state values; the patch owns its
  `make-history` / `read-history` / `write-history`. A macro in
  `content/defmacros` that reads or writes a history passed in by name
  compiles, but the patch editor rejects it and that breaks the whole patcher.
- Contacts must not create energy: a pluck engages its spring at the gap it
  caught at, and a bow reads its point's true previous position.
- Connections sharing an object are solved jointly; one sequential pass let
  stiff springs on a light body, and a bow next to a bridge, pump energy in.
- Bowing needs harmonic modes: use nylon/gut-like materials at low notes and
  put bridges at the string's end (0.99), not part-way in.
- Masses are relative to one modal mass; a 120-mode head's point is ~60x
  lighter, so useful mallets are 0.002..0.5.

## Finite-element solids (mesh → modes)

`eseq modal solve SPEC.json OUT.modes.json` meshes a 3-D elastic solid and
computes its lowest vibration modes (`crates/sequencer/src/modal_fe/`).
Samples: `fe/bar.json` (free aluminium bar, voxel mesh) and `fe/bell.json`
(bronze bell revolved from its wall profile, mapped mesh).

A spec names a `solid` (`box`, `cylinder`, `tube`, `sphere`, `shell`,
`revolve` from `[r, z]` / `[r, z, wall]` profile points, `union`,
`difference`, or an `obj` file), a `resolution` (`{"cells": N}` along the
largest extent or `{"element_size": metres}`), a `material`
(`density`, `young`, `poisson`), an optional `block` region (`box` min/max
or `sphere` center/radius) whose nodes are clamped, and the number of
`modes`. A lone revolve with a wall, tube, shell or cylinder gets a
body-fitted mesh (`"segments": {"along", "around", "through"}` override its
counts); anything else is voxelized.

The output is the "eseq-modes" v1 JSON document: `freqs` (Hz, ascending,
rigid-body modes excluded), the boundary surface as `points` / `normals`
(P×3) and `triangles` (T×3), `shapes` (P×N, point-major: mode k's
mass-normalized displacement along the normal at point p is
`shapes[p*N+k]`), `mass` (kg) and the canonical `spec`.
