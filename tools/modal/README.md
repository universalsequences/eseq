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

(m/patch :name "my-guitar"
  :objects     (list (m/finger 'fin :depth 0.1)
                     (m/string 'str :material :steel :length 0.65 :modes 60)
                     (m/plate 'body :material :spruce))
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

Materials are keywords: `:uranium :steel :brass :aluminium :granite :nylon
:carbon :diamond :spruce`. `(m/check patch)` lists what is wrong with a patch
by name (`spring bridge: no object bdy`); `eseq modal emit` prints the same
list instead of writing anything.

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
