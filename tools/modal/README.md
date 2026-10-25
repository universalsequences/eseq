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
| `m/plate` (simply supported, `:grid` x `:grid` modes, default 8) | `(x y)` |
| `m/membrane` (circular, 120 Bessel modes, `:tune`) | `(r theta)` |
| `m/rect-membrane` (fixed edges, `:aspect`, follows the note) | `(x y)` |
| `m/circ-plate` (`:edge :free` or `:clamped`; cymbals, gongs; modes solved from the Bessel equations, pitch from material and size) | `(r theta)` |
| `m/bar` (free bar, follows the note; `:undercut` 1 tunes modes 2 and 3 to 4x and 10x, marimba-style) | `x` in 0..1 |
| `m/oscillator` (one mode at the note: a reed, a lip, a resonance) | lumped-like, any |
| `m/finger` (Modalys' two-mass: hand on a stroke + soft tip) | lumped |
| `m/mallet` (thrown mass) | lumped |
| `m/modal-data` (modes from data: frequencies, losses or bandwidths, shapes over points; `:root` makes it follow the note) | point index, or `(x y z)` for the nearest point |

`(m/solid 'bell :shape (dict :revolve ...) :made-of mat/bronze)` is a
finite-element solid: any 3-D shape (the spec's "solid", below), solved for
its modes by `(modal-solve spec)` the first time it is evaluated (a few
seconds), then read from a cache keyed by the spec. Its shapes are scaled to
the same unit modal mass as every other object. `examples/fe-bell.lisp`
strikes a solved bronze bell; `:root` makes it follow the note.

`(modal-read-data "file")` reads data for `m/modal-data :data`, relative to
the script: an IRCAM resonance model (text: index, Hz, dB, bandwidth Hz per
line) or an `eseq-modes` `.json` from `eseq modal solve`.
`examples/bell-data.lisp` strikes a resonance-model bell. A data object with
more than 64 points keeps only the points its connections and listens use.

| Connections | Between |
|---|---|
| `(m/pluck id finger obj at)` | catch, hold, release |
| `(m/felt id mallet obj at)` | one-sided, hardening felt |
| `(m/spring id a a-at b b-at)` | bridge / adhere |
| `(m/bow id obj at)` | stick-slip friction, sustains under the gate |
| `(m/force id obj at :signal s)` | pushes the point with gain x signal |
| `(m/position id obj at :signal s)` | holds the point at the signal (a constant 0 clamps it) |
| `(m/speed id obj at :signal s)` | moves the point at gain x signal |

| `(m/strike id a a-at b b-at)` | stiff one-sided contact between two points (`:offset`) |
| `(m/stop id obj at :height h)` | a rigid floor under a point: sitar jawari, buzz |
| `(m/fingerboard id string at :press s)` | clamps the string while pressed, plus a board to buzz on |

`m/felt` takes `:exponent :hysteresis :tau-ms` for Stulov's hysteretic
felt (`examples/piano-hammer.lisp`).

**Directions.** `(m/string 'str :planes 2)` adds a second transverse
direction, `(m/plane 'str 1)`, with the same params (`split` detunes it
slightly). A bow with `:hair d` on such a string presses the second
direction down, and its friction pressure follows that contact's force
(`examples/bowed-bi-string.lisp`).

**Feedback taps.** Signals that read last sample: `(m/tap-position obj at)`,
`(m/tap-speed obj at)`, `(m/tap-force conn)`, `(m/tap-contact conn)`. A
force's `:saturate` bounds a self-driven loop (`examples/feedback-gong.lisp`).

**Hybrids.** `(m/melt 'h objA objB :morph x)` blends two objects' modal data
(equal mode counts; positions on it are `(list atA atB)`):
`examples/melt.lisp` morphs a string into a membrane. `(m/mix 'h 'a 'b)`
shares connections and listens between two objects in the patch
(`examples/mix.lisp`).

`(m/spectral obj ...)` adds Modalys' pseudo-physical controls to any object:
`bend` (cents) and `diffraction` (rho: 1 leaves the partials, below 1 pulls
them toward the lowest, above 1 spreads them) params, `:follow-note` (the
lowest mode on the played note, for plates), and per-mode lists
`:freq-scales`, `:loss-scales`, `:gains` (a mode equalizer on what listens
hear). `(m/listen obj at :modes (list 0 2))` hears only those modes.

Every connection has a `weight` param (0..1) that scales its force (a bow's
pressure), to fade it in or out or couple partly. A driver's signal is data:
`(m/envelope :attack-ms :decay-ms :sustain :release-ms)`, `(m/noise)`,
`(m/sine :ratio r)` (at r times the note), `(m/constant v)`, or any
DGenLisp form `(m/signal '(...))`; a list of signals is their product.
`examples/ebow.lisp` sustains a string with a sine force at the note.

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
