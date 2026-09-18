# eseqlisp

eseqlisp is the Lisp that runs inside eseq. Every panel you see is an
eseqlisp function returning a widget tree. MIDI effects, step processes, and
generative sequencers are eseqlisp. Themes, key bindings, the customize
dialog, and a package that replaces the mixer are eseqlisp. The Rust host
publishes sequencer state as reactive values and accepts commands back; the
Lisp decides what to draw and what to do.

The language is small: `def`, `if`, `let`, `lambda`, lists, maps, keywords,
and a reactive layer that re-renders exactly the widgets whose inputs changed.
It hot-reloads on save, keeps state across reloads, and never brings the
editor down on an error.

This guide is for people and agents writing eseqlisp: a custom instrument
panel, a MIDI effect, a process, a package. The canonical sources are the
compiler at `crates/eseqlisp/src/lang/compiler.rs`, the native table in
`crates/eseqlisp/src/lang/vm.rs`, and the widget list in
`crates/eseqlisp/src/widgets.rs`. When this guide and the source disagree,
the source wins.

## Hello, Buffer

```lisp
(defstate clicks 0)

(effect-buffer "*hello*"
  (v-stack :gap 0.5 :padding 1
    (label (fmt "Clicked {} times" clicks) :font-size 12)
    (button "Click me" :on-click (lambda (e) (set! clicks (+ clicks 1))))))
```

Paste that into any editor buffer and press `C-x C-b` (evaluate buffer), or
save it as `~/.eseq.d/init.lisp` and relaunch. Then `C-x b` and switch a tile
to `*hello*`. The label re-renders when `clicks` changes because it read
`clicks` while rendering; nothing else does.

Three things carry the whole language:

- `defstate` makes a reactive cell. Reading it is a bare symbol; writing it is
  `set!`.
- `effect-buffer` binds a widget tree to a named buffer and reruns it when a
  cell it read changes.
- Widgets are functions: a name, an optional positional text, `:keyword value`
  props, and children.

## Core Language

### Literals and values

```lisp
42  1.5  -.5  1e3         ; numbers are all f64
"a string"                ; NO escape sequences; a \" ends the string
:keyword                  ; :foo
'sym  '(1 2 3)            ; quote
true false nil
|a b| (+ a b)             ; shorthand for (lambda (a b) (+ a b))
```

Comments run from `;` to end of line. There is no `[…]` or `{…}` syntax;
brackets are ordinary symbol characters.

Strings cannot contain a literal quote. Build such strings with `str` or
`fmt`:

```lisp
(str "say " (source "hi"))
(fmt "{} of {:.2}" n total)      ; Rust-style placeholders
```

### Truthiness

`false`, `nil`, `0`, `""`, and `()` are all false. Everything else is true.

```lisp
(if 0 "yes" "no")          ; "no"
(if (= x nil) "unset" x)   ; the test to use when 0 is a legal value
```

`=` is structural equality with no numeric coercion.

### Lists and maps

```lisp
(list 1 2 3)  '(1 2 3)
(first xs) (rest xs) (nth xs 0) (len xs) (append xs ys) (cons x xs)
(empty? xs) (reverse xs) (range 4) (range 2 6) (zip xs ys) (chunks xs 2)

(dict :name "kick" :vel 0.8)
(get m :name)                      ; also works on (:a 1 :b 2) lists
(merge m :vel 1.0 :pan 0.5)        ; keyword args, NOT (merge m other-map)
(keys m)
m.name                             ; dotted read
(set! m.name "snare")              ; dotted write
```

`(find-by-key xs :id 3)` finds the first map with that key. There is no
`find`; use `filter` and `first`.

### Higher-order functions

The callback comes first, except in `each`, where the list comes first:

```lisp
(map |x| (* x 2) xs)
(filter |x| (> x 1) xs)
(reduce |acc x| (+ acc x) 0 xs)
(for-each |x| (status (str x)) xs)
(each xs |x i| (label (str i ": " x)))    ; widget children; see below
```

An error inside a callback is logged and the element becomes `nil`. Set
`ESEQLISP_DEBUG_LISP_ERRORS=1` to see those logs.

### Control flow and binding

```lisp
(if c then else)                   ; else defaults to nil
(match kind :scroll-view (…) :click (…) _ (…))
(and a b) (or a b) (not a)
(do e1 e2 e3)
(let ((a 1) (b (+ a 1))) body…)    ; sequential: b sees a
(-> x (f 1) g)  (->> x f)          ; threading
(set! name value)
```

There is no `cond`, `when`, `unless`, `case`, `loop`, `while`, or `let*`.
Nest `if`, and `let` is already sequential. There is no `try`/`catch`; a
native error aborts the current evaluation and shows in `*lisp-reload*` or
the status line.

A list pattern in `let` or an argument list destructures a map by keyword,
not a list by position:

```lisp
(let (((note vel) event)) …)       ; note = (get event :note), vel = (get event :vel)
```

### Functions

```lisp
(def area (w h) (* w h))           ; named function
(def total 0)                      ; global value
(lambda (x) (* x x))
|x| (* x x)
```

Arity is fixed. There are no optional, keyword, or rest arguments for
functions; calling with the wrong count is an error. Pass a map when you
want options. Closures capture their environment and recursion works through
the global name.

### Macros

```lisp
(defmacro circle (r)
  `(- (length (vec2 x y)) ,r))

(defmacro sig (name &rest spec)
  (let ((h (gensym)))
    `(def-process ,h …)))
```

Exactly one body form. Quasiquote, `,x`, and `,@xs` work only inside a
`defmacro` body; elsewhere a backtick is a plain quote. `&rest` is allowed in
macros only. Inside a module, a macro is interned as `module/name`, which is
why the shader stdlib is called as `(sdf/circle r)`.

## Reactive State

### Cells, views, and observers

```lisp
(defstate volume 0.8)              ; reactive cell; read bare, write with set!
(def db (derived (* 20 (log volume))))   ; memoized
(observe (status (fmt "vol {}" volume)))  ; side effect, reruns on change
(effect-buffer "*vol*" (label (str volume)))  ; render root
```

Dependencies are recorded by reading. A view that reads a cell reruns when
the cell changes and only then. `defstate` values survive hot reload.

Inside a `(module m)` the cell is registered as `m/volume`, so two modules
can each have a `volume`.

### The host namespaces

The sequencer publishes its state as dotted reactive reads:

```lisp
SEQ.current-track  SEQ.num-tracks  SEQ.track-names  SEQ.selected-steps
(nth SEQ.steps 4)  (len SEQ.track-ids)
```

Read `(nth SEQ.steps i)`, never bind `SEQ.steps` whole. Indexed reads
register a per-index dependency; a whole-list read makes the buffer rerun on
every edit anywhere in the list. Other namespaces are `SEQV` (Lisp-writable
scratch), `THEME`, `APP`, `INPUT`, `MIDI`, and `GRAPH`.

Writes go through host commands, not `set!`:

```lisp
(host-command "toggle-step" (dict :track 0 :step 4))
(seq-set-step-param 4 :velocity 0.9)
(seq-set-effect-param slot param-index 800)   ; on the current track
```

Roughly three hundred host commands exist; the names are the `COMMANDS`
arrays under `crates/sequencer/src/ui/host_commands/`. An unknown name shows
"Unknown host command" in the status line and does nothing.

### Float bindings for hot values

A meter or a knob that follows audio should not rerun a view sixty times a
second. Bind the prop to a float reference instead:

```lisp
(meter :level (bind-seq "master-peak-l"))
(slider :value (bind-nth "SEQV" "track-gain" i))
```

Only props a widget lists as bindable accept references. The
`eseq.bindings` module wraps this with `scope`, `bound`, `write!`, and
`one-hot!` over the `SEQV` namespace.

### `subtree`

```lisp
(subtree :key (str "strip-" track)
  (track-strip track))
```

The body reruns on its own when its dependencies change; the parent does
not. Use it around every repeated row. The subtree key replaces the root
child's own `:key` in the layout, so tests and `reveal-widget` look for the
subtree key.

## UI

### Widgets

A widget call is `(name [text] :prop value … children…)`. Children may be
maps, lists of maps, or `nil`; lists are flattened.

```lisp
(v-stack :gap 0.4 :padding 0.6
  (label "Filter" :font-size 11 :color :dim)
  (h-stack :gap 0.3 :align :center
    (knob :value cutoff :min 20 :max 20000 :on-change (lambda (v) (set! cutoff v)))
    (number-picker :value cutoff :min 20 :max 20000 :decimals 0 :unit "Hz"
      :on-change (lambda (v) (set! cutoff v)))
    (toggle :value on? :on-change (lambda (b) (set! on? b)))
    (dropdown :options (list "LP" "BP" "HP") :value-index mode
      :on-change (lambda (s) (set! mode (if (= s "LP") 0 (if (= s "BP") 1 2)))))))
```

Sizes are in cell units, or `:fill`. Layout containers are `v-stack`,
`h-stack`, `wrap`, `grid`, `responsive-grid`, `scroll`, `virtual-v-stack`,
and `box`. `:flex N` on a child takes a share of leftover space. `h-stack`
defaults to gap 1, `v-stack` to gap 0.

`:bind cell` is shorthand for `:value cell :on-change (lambda (v) (set! cell v))`.

The full list of widget names is `BUILTIN_WIDGET_NAMES` in
`crates/eseqlisp/src/widgets.rs`. The props each accepts are the
`completion_props` list in the matching `crates/eseqlisp/src/widget_render/*`
file. Do not name a `def` or a parameter after a widget: a local called
`label` shadows the widget and every `(label …)` below it fails.

### Handler arguments

| widget | handler | receives |
|---|---|---|
| `box` `:on-click :on-drag :on-mouse-down …` | one event map | `phase x y col row shift ctrl alt super` |
| `button :on-click` | one map | click info |
| `slider knob number-picker tabs :on-change` | one number | |
| `toggle :on-change` | one bool | |
| `dropdown :on-change` | the option string | use `:value-index` for enums |
| `text-input :on-change :on-submit` | string, none | |
| `xy-pad :on-change` | `x y` | |
| shader widgets `:on-drag` | `sx sy region` | normalized -1..1 |

### Children come from `each`

```lisp
(v-stack
  (each rows |row i|
    (subtree :key (str "row-" i) (row-view row))))
```

`each` records which list and index produced each child so pickers,
dropdowns, and drag state stay attached to the right item. `map` produces the
same tree in a layout test and blank widgets in the live renderer. The
source should be a named symbol, not an inline expression.

### Themes

A theme file sets slots; widgets refer to slots by keyword:

```lisp
(def my-theme ()
  (apply-theme (dict
    :buffer-bg '(0.08 0.08 0.1 1.0)
    :clock-fg  '(0.9 0.9 0.8 1.0))))

(box :background-color :buffer-bg (label "12:00" :color :clock-fg))
```

Only slots present in the map change. `:primary`, `:dim`, `:transparent`,
and a few other aliases are always available. A slot name that does not
exist in the Rust `Theme` struct falls back to the widget default with no
warning; adding a slot means adding it to the struct, to `theme_slots!`, and
to every complete theme.

`ui/style` describes press and hover transitions:

```lisp
(button "Play" :style (ui/style :pressed (dict :scale 1.08) :hover (dict :brightness 1.1)))
```

### Shader widgets

`defwidget` does not define a component. It defines a GPU-painted widget
whose body is an SDF expression:

```lisp
(defwidget transport-btn-bg
  :width 1 :height 1 :paint-margin 0.3
  :state (active)
  :shader
  (sdf/layer
    (sdf/fill (sdf/rounded-rect width height 0.4)
      (material :color (if active :mixer-strip-selected-bg :mixer-strip-bg)))))

(transport-btn-bg :active playing?)
(box :background "transport-btn-bg" :active playing? (label "Play"))
```

`x y width height aspect` and the state names are free variables in the
shader; `hit/hover` and `hit/active` report pointer state. The shapes live in
`content/core/sdf-stdlib.lisp` as `sdf/circle`, `sdf/rect`, `sdf/translate`,
`sdf/union`, and so on. `(sdf/region :name shape material)` makes a region
that pointer handlers can identify.

A reusable component is a plain function that returns a widget.

### Modes and keys

```lisp
(def my-key (key) (do (status key) true))     ; return true to consume
(bind-key "C-c C-k" "compile-current")           ; global chord
(define-mode "piano-mode" :read-only true :live-keys true
  :inherit "eseq.sequencer-keys/sequencer-keys" :on-key "my-key")
(mode-bind-key "piano-mode" "s-p" "place-pattern")
(set-buffer-mode-for "*piano-roll*" "piano-mode")
```

Handlers are named by string. Modifiers are `C-` control, `M-` option, `S-`
shift, `s-` command, in that order; named keys are `RET BS ESC UP DOWN LEFT
RIGHT`. Precedence is the mode's `on-key`, then its keymap and ancestors,
then global bindings. `M-x name` runs any global function by name.

## Modules and Packages

```lisp
(module alec.acid.panel)

(import eseq.effects.param-controls :as pc)
(import eseq.track-collapse :refer (collapsed?))

(export acid-panel)

(defstate open? false)                 ; private, registered as alec.acid.panel/open?
(def acid-panel (track) …)             ; public
```

One `module` per file, before the imports. Names are private unless
exported; `pc/param-set-control-value` reaches an export through its alias.
A file with no `module` form belongs to `eseq.vanilla` and exports
everything. Module `a.b.c` is the file `a/b/c.lisp` on the load path:
`content/ui/` for factory code, then `~/.eseq.d/packages/<pkg>/src/`, then
your user directory.

`(load "@/ui/themes.lisp")` re-evaluates a file as a side effect. Use
`import` for code you call and `load` for files whose evaluation is the point.

### Changing what ships

The ladder, from least to most invasive:

1. **Customize.** A module declares a knob and users set it in the Customize
   dialog or in `init.lisp`:

   ```lisp
   (defcustom strip-width 6 :type :number :min 3 :max 12 :step 0.5
     :doc "Mixer strip width in cells")
   (setopt eseq.mixer/strip-width 8)
   ```

   `:type` and `:doc` are required. Types are `:number`, `:bool`, `:string`,
   and `:choices`.

2. **Extend.** Add a hook, a buffer, a key.

   ```lisp
   (add-hook "macro-mapping-changed" "my-key" (lambda (m) …))
   ```

3. **Override.** Replace one exported function and keep the original in reach:

   ```lisp
   (override eseq.mixer/track-strip :around (original i)
     (v-stack (label "★") (original i)))
   ```

   Overrides survive the owner's hot reload. One that throws is quarantined
   and the factory version comes back. Users switch a package's overrides
   off in Customize with no reload.

4. **Shadow.** Put a same-named module earlier on the load path.

### A package

```
~/.eseq.d/packages/alec.acid/
  manifest.json      {"name": "alec/acid", "version": "1.0.0", "entry": "alec.acid.panel"}
  src/panel.lisp     (module alec.acid.panel) …
  instruments/…      dsp.lisp folders, see the DGenLisp guide
  effects/  midi-fx/  presets/  samples/  themes/
```

`alec/acid` owns every module under `alec.acid.*`. `eseq.*` and one-segment
names are reserved. Installed instruments get `pkg:alec.acid/<name>` ids so
they never collide with library or factory sounds. Export with **File >
Export Package**; the `.eseqpack` is a zip.

`~/.eseq.d/init.lisp` runs last. The Customize dialog owns a managed block at
its end and leaves the rest alone.

## The Musical Roles

The same language runs in two virtual machines. The UI VM draws and handles
input. The scheduler VM runs on the transport tick. Bodies of `def-process`,
`def-midi-fx`, and `def-sequencer` are quoted and shipped to the scheduler,
so they cannot close over UI values; they see only their own inlets, params,
and the natives listed below.

### An instrument panel

`content/instruments/<Name>/ui.lisp` next to the `dsp.lisp`:

```lisp
(def my-page ()
  (v-stack :gap 0.3
    (h-stack :gap 0.3
      (eseq.effects.mnm-surface/mnm-num "cutoff" "Cutoff Hz" 0)
      (eseq.effects.mnm-surface/mnm-num "resonance" "Res" 2))))

(defsynth-ui (my-page))
```

Params are named by their `(param …)` name, or `group.name` when grouped.
The lego helpers under `eseq.effects.custom-ui-lego` give knobs, number
fields, options, and ADSR editors that already handle p-lock display:

```lisp
(defeffect-ui
  (h-stack :gap 0.35 :align :stretch
    (eseq.effects.custom-ui-lego/ui-lego-column-full
      (eseq.effects.custom-ui-lego/ui-control-block-medium "CHORUS"
        (eseq.effects.custom-ui-lego/ui-accent-cyan)
        (h-stack :gap 0.32
          (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "rate" "rate" 4.8
            (eseq.effects.custom-ui-lego/ui-accent-blue) 2))))))
```

Exactly one `defsynth-ui` per instrument, one `defeffect-ui` per effect, one
`def-midi-fx-ui` per MIDI effect, each with a single root.

### A MIDI effect

`content/midi-fx/<name>/dsp.lisp`:

```lisp
(midi-fx-param "rate" :default 4 :min 0 :max 12 :role :clock-rate
  :enum "1" "1/2" "1/4" "1/8" "1/16" "1/32" "1/64")
(midi-fx-param "gate" :default 0.9 :min 0.05 :max 1)

(def-midi-fx "beat-repeat"
  (let ((rate (fx-param "rate"))
        (gate (fx-param "gate")))
    (do
      (fx-suppress)
      (for-each |i|
        (fx-emit :beats (fx-note-start i)
          :note (fx-note i)
          :vel (fx-velocity)
          :dur (* gate (/ (fx-time rate) (fx-source-time))))
        (range 0 (fx-note-count))))))
```

`midi-fx-param` lines attach to the next `def-midi-fx`. The body runs once
per incoming event with no arguments. Read with `fx-note-count`, `fx-note`,
`fx-note-start`, `fx-notes`, `fx-velocity`, `fx-param`, `fx-time`; keep
state with `fx-state-get` and `fx-state-set`; `fx-suppress` drops the
original note and `fx-emit` adds one. Helpers shared by the factory effects
are plain functions in `content/midi-fx/_lib/dsp.lisp`.

### A process

A process runs just before a step plays and can change that step:

```lisp
(def-process prob-mask
  :doc "Veto the current step when a roll exceeds the probability inlet."
  :in ((prob :float 0 1 :default 1 :lane true))
  :seed :locked
  :run (if (> (rand) (in :prob))
         (veto!)
         nil))
```

Inlets are `(name :float|:int|:gate|:track|:field [min max] :default v
[:lane true])`; `:lane true` gives the inlet a paintable step lane. `:state
((x 0))` declares state, `:out ((value :float))` an outlet, `:every` a
period, `:target (step-param :transpose)` where writes land. In `:run`,
`transpose!`, `veto!`, `roll!`, and `ratchet!` act on the step; `out` and
`send` publish; `(read (track n :transpose :trigs-ago 1))` and
`(track n :note :pattern)` read other tracks; `hear` and `suggest` exchange
typed fields between processes. A `set!` of state must sit at the top level
of `:run`, not inside a nested `let`.

Every `def-process` is also a constructor:

```lisp
(def h (prob-mask :prob (lane 1 0.5 1 0.2)))
(start h)
(connect! h :value (inlet other :amount))
(ps)
```

`(defchan name 0)`, `(send name v)`, and `(chan-get "name")` carry values
between processes, sequencers, and the UI.

### A generative sequencer

```lisp
(def-sequencer "walker"
  :resolution :16
  :tick (seq-emit
          :track (mod (gen-tick) 4)
          :at :now
          :note (- (gen-rand 12) 6)
          :vel 0.8
          :dur 0.25))
```

`:tick` is evaluated on every grid tick. `gen-tick`, `gen-beat`, `gen-bar`,
`gen-phase`, `gen-rand`, `state-get`, `state-set!`, and `chan-get` are the
vocabulary; `seq-emit` takes `:track :at :note :vel :dur :chord :quantize`.
A `defmacro` at the top level can stamp out sequencers with parameters, as
`content/scripts/sequencers/test-seq.lisp` does.

Attach any of these to a project by adding an `(import your.module)` line to
the project's scratch buffer.

## Gotchas

- **`0`, `""`, and `()` are false.** Compare with `nil` explicitly.
- **Functions have fixed arity.** No optional args; pass a map.
- **`merge` takes keyword pairs**, not a second map.
- **No `\"` in strings.** Build the string with `str` or `fmt`.
- **`each` for widget children, never `map`.** And give `each` a named list.
- **`map`, `filter`, `reduce` take the function first.** `each` takes the
  list first.
- **`let` destructures maps**, not lists.
- **No `cond`, `when`, `unless`, `let*`, `find`, `try`.**
- **`defwidget` is a shader**, not a component.
- **A `def` or parameter named like a widget shadows it.** `label` and
  `value` are the usual victims.
- **Read `(nth SEQ.list i)`**, not `SEQ.list`.
- **Do not echo host values into a `defstate` from a drag handler.** Every
  reader re-renders per event. Read the host value directly or use a float
  binding.
- **`subtree :key` replaces the child's key.**
- **`wrap` inside a flexed `h-stack` sibling overlaps its rows.** Put the
  flex on the wrap itself.
- **`:padding-left` and friends are ignored.** Only `:padding` exists.
- **`dropdown :on-change` gives the string.** Map it back yourself, or use
  `:value-index`.
- **A theme slot that does not exist is silently ignored.**
- **`midi-fx-param` binds to the next `def-midi-fx`.**
- **Scheduler bodies are data.** No closures over UI state in `:run`,
  `:tick`, or a MIDI effect.
- **Process `set!` at top level of `:run` only.**
- **`import` at top level, after `module`.** `export` needs a named module.
- **Errors do not throw.** Watch `*lisp-reload*` and the status line; set
  `ESEQLISP_DEBUG_LISP_ERRORS=1` for callback errors.

## Tooling

Run the app and edit `content/ui/*.lisp` or your package on disk; the
watcher reloads the changed module. Inside the app, `C-x C-e` evaluates the
form at the cursor, `C-x C-b` the buffer, `C-c C-c` the editor code buffer,
and `M-x` runs a command by name. A failed reload keeps the last good UI and
opens `*lisp-reload*` with the error.

Render a panel headlessly and look at it:

```sh
cargo run -q -p sequencer --bin metal_seq -- capture \
  --script fixture.lisp --buffer fx --track 0 --width 2400 --height 900 --out /tmp/fx.png
```

A fixture is one `(capture-project (track :instrument "Synths/Hello"))` form
followed by ordinary Lisp. Host commands are not drained inside a fixture, so
step edits there do nothing. The `render-panel` skill wraps this.

A layout test drives the same runtime from Rust; see
`crates/sequencer/src/ui/state_values/customize_ui_tests.rs` for the pattern
of `eval_str`, `run_reactive_cycle`, and walking `widget_layout()`. Run one
test with:

```sh
cargo nextest run -p sequencer -E 'test(/customize_lists_the_mixer_clip_knob/)'
```

## Syntax Reference

### Reader

```
42 1.5 -.5 1e3      "text"      :kw      sym      'x  '(…)
`(… ,x ,@xs)        ; quasiquote, defmacro bodies only
|a b| body          ; lambda shorthand
; comment
```

### Definitions

```lisp
(def name value)  (def name (args…) body…)  (lambda (args…) body…)
(defmacro name (args… [&rest r]) body)
(defstate name init)  (def name (state init))  (def name (derived body…))
(defscene name default)                 ; per-pattern persisted slot
(defcustom name default :type t :doc "…" [:min :max :step :choices])
(setopt name v)  (setopt-by-name "mod/name" v)
(defhook "n")  (add-hook "n" "key" fn)  (remove-hook "n" "key")  (run-hook "n" args…)
(module a.b)  (import a.b [:as x | :refer (s…)])  (export s…)  (load "@/path.lisp")
(override m/f (args) body…)  (override m/f :around (original args) body…)
(remove-override m/f)  (disable-module-overrides m)  (enable-module-overrides m)
```

### Control

```lisp
(if c a [b])  (match v p1 e1 … _ default)  (and …)  (or …)  (not x)
(do …)  (let ((a 1) (b 2)) …)  (set! target v)  (-> x f…)  (->> x f…)
(eval "string")  (gensym)  (macroexpand form)  (source value)
```

### Data

```lisp
+ - * / min max  = < > <= >=
abs sqrt sin cos floor ceil round fract log exp pow atan2 mod rand-int
list first rest cons nth set-nth len append empty? reverse range zip chunks find-by-key
dict get merge keys  number? string?
str fmt substring str-contains? string-split string-trim string-downcase
string-starts-with? string-ends-with?
map filter reduce for-each each
```

### Reactive and UI

```lisp
(effect-buffer "*name*" body)  (effect body…)  (observe body…)
(subtree :key k body)
(bind "NS" "field")  (bind-seq "field")  (bind-nth "NS" "field" i)  (bind-seq-nth "field" i)
(reactive-get "NS" "f")  (reactive-set "NS" "f" v)  (reactive-value ref)
(host-command "name" payload)  (status "text")
(apply-theme (dict :slot '(r g b a) …))  (ui/style :pressed (dict …) :hover (dict …))
(defwidget name :width w :height h [:paint-margin m] [:animates b]
  :state (s…) :bindable (p…) :shader sdf-expr)
(define-mode "m" [:read-only b] [:live-keys b] [:inherit "p"] [:on-enter "f"] [:on-key "f"])
(bind-key "K" "f")  (mode-bind-key "m" "K" "f")  (set-buffer-mode-for "*b*" "m")
```

Layout: `v-stack h-stack wrap grid responsive-grid scroll virtual-v-stack box
modal context-menu tabs`. Controls: `label button badge toggle slider hslider
vslider knob knob-number number-picker number-label dropdown select
menu-button text-input textbox xy-pad`. Displays: `meter mixer-meter scope
xy-scope spectrogram waveform linegraph matrix adsr-editor lfo-curve
automation-lane timeline piano-keyboard image tree`. Instrument panel roots:
`defsynth-ui defeffect-ui def-midi-fx-ui`.

### Scheduler

```lisp
(midi-fx-param "n" :default d :min a :max b [:role r] [:enum "…"…])
(def-midi-fx "name" body)
  fx-param fx-note-count fx-note fx-note-start fx-note-end fx-notes fx-velocity fx-track
  fx-time fx-source-time fx-state-get fx-state-set fx-suppress fx-emit

(def-process name :doc "…" :in ((n :type [lo hi] :default d [:lane true])…)
  [:out ((n :type)…)] [:state ((n init)…)] [:every dur] [:seed :locked]
  [:target t] [:listen (…)] [:init body] :run body)
  in out send transpose! veto! roll! ratchet! rand clip wrap now-beats
  step-note current-note read track hear suggest
(def-accumulator name :target t :amount (n :float lo hi :default d :lane true) …)
(defchan name [init])  (send name v)  (chan-get "name" [default])
(start h) (stop h) (lane! h :in v…) (connect! h :out (inlet other :in)) (ps)

(def-sequencer "name" :resolution :16 [:init body] :tick body …)
  gen-tick gen-beat gen-bar gen-phase gen-rand state-get state-set! chan-get
  (seq-emit :track t :at :now :note n :vel v :dur d [:chord (…)] [:quantize g])
```

## Further Reading

- `docs/module-system-spec.md` and `docs/module-export-spec.md`: modules, imports, override.
- `docs/content-tiers-spec.md`: the customize, extend, override, shadow ladder.
- `docs/manual/customization.md` and `docs/manual/packages.md`: the user-facing view.
- `crates/sequencer/docs/process-channels-spec.md`: processes and channels in depth.
- `crates/sequencer/docs/lisp-sequencer-spec.md`: `def-sequencer`.
- `docs/defwidget-interactive-regions.md` and `crates/eseqlisp/docs/lisp-shader-widgets-spec.md`: shader widgets.
- `docs/metal-seq-ui-capture.md`: headless rendering.
- Exemplars: `content/ui/track-collapse.lisp` (a small module),
  `content/ui/customize.lisp` (a modal panel), `content/ui/transport.lisp`
  (shader widgets), `content/midi-fx/beat-repeat/`, `content/processes/builtin.lisp`,
  `content/packages/alez.sig/` (a package with a macro surface).
- The DGenLisp guide, `docs/dgenlisp/GUIDE.md`, for the `dsp.lisp` side.
