# Program generators

A **program** is data that knows how to turn itself into a DGenLisp
instrument: a map with a callable `:generator`. Any eseqlisp script whose last
value is a program can be played and saved like this:

- **In the app:** press C-c C-c in the script's buffer. The generated
  instrument plays on that buffer's draft track (shown with a `draft/` engine
  name). Evaluating again recompiles the track in place and keeps your knob
  values. **Save instrument** in the track's instrument header writes the
  instrument to your library.
- **From the shell:** `eseq generate SCRIPT.lisp [DIR]` writes `dsp.lisp`,
  `ui.lisp` and `instrument.json` to DIR. Without DIR it writes to the user
  instrument folder, under the instrument's name. Then run
  `eseq instrument check DIR`.

`eseq.modal` (tools/modal/README.md) is one generator: `m/patch` returns a
program. [examples/additive.lisp](examples/additive.lisp) is another, about
80 lines, written in the script itself with no Rust and no modal code. The
design is in docs/program-generators-spec.md.

## Writing a generator

A generator is a function from the program to a **result map**:

```lisp
(import eseq.gen :as gen)

(def sine-synth (p)
  (dict :name (get p :name)
        :dsp (str "(def gate (in 1 @name gate))\n"
                  "(out (* gate (sin (* twopi (phasor " (get p :hz) ")))) 1 @name audio)\n")))

(gen/program sine-synth :name "sine" :hz 220)
```

The result map has these keys:

| key | |
|---|---|
| `:name` | required: the folder and engine name |
| `:dsp` | required: the dsp.lisp source |
| `:ui` | the ui.lisp source; leave it out for the stock panel |
| `:voice-mode` | the param the host binds its voice mode to: `(gen/voice-mode voicing)` beside `(gen/voice-mode-params voicing)` |
| `:assets` | absolute paths of files ui.lisp reads; they are copied into the instrument's folder |
| `:kind` | `:instrument`, the default and, for now, the only kind |

A generator can return `(dict :errors (list "..."))` instead. The messages
appear in the editor's status line, or on the terminal for `eseq generate`.

## The library

Build the source as quoted forms (quasiquote) and print it with `pp-forms`.
The modules below give you what eseq.modal is built on:

- **`eseq.gen`**
  - `preamble`: the voice inputs (gate, pitch, velocity, trigger, clock, mod1..4) plus `hit`, `age`, `hit_velocity` and `f0`.
  - `pspec` / `param-form`: param specs and their `(param ...)` forms, grouped by an id.
  - `by-note` / `by-note-forms`: a knob default that follows the played note, worked out only when the note or knob changes.
  - `change-tick-forms`: an event clock for work that should run only when its inputs change.
  - Small helpers: form builders (`tensor-form`, `pick-form`, `pole-form`, …), form analysis (`map-atoms`, `rename-in`, …) and list helpers.
- **`eseq.gen.modulate`**
  - `mod-pass`: makes the knobs modulatable where it costs nothing until they are modulated. A knob that feeds per-block setup stays a plain knob unless you pass `all`.
  - If your forms call library defmacros that keep state, name them with `:stateful`.
- **`eseq.gen.panel`**: sectioned panels in the style of the modal instruments.
  - A section is `(dict :title :tab :params)`.
  - `sections-ui-forms` builds a whole ui.lisp from your sections.
  - `screen-form`, `panel-form` and the `mx-*` helpers are there if you want to arrange the panel yourself.
