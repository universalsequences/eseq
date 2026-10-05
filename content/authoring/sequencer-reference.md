Sequencer package rules. A custom sequencer is a Lisp module in eseq's own
Lisp (eseqlisp, not DGenLisp) that defines an **instance kind** with
`def-kind`. Each instance of the kind gets its own tab beside **Seq**, its own
saved settings (per scene, undoable), and a tick that the scheduler runs to
emit notes. A Drum Rack can own an instance. No Rust and no rebuild are
involved: the module is loaded from disk.

## Files and names

- Write personal modules under the Local packages folder (see AGENTS.md for
  the path). The module name is the path with dots: `euclid/rings.lisp` is
  `euclid.rings`, `my/pulse.lisp` is `my.pulse`. Local needs no manifest.
- Every file starts with `(module NAME)` matching its path, then `(export …)`
  (private by default), then `(import …)` lines.
- Split the kind into two modules:
  - `NAME.core` (or similar): pure data and the tick. The scheduler runs the
    tick in a separate, headless Lisp VM that imports ONLY the modules named
    in `:requires`. Keep UI forms (`defwidget`, widgets, `bind-seq`, `SEQ.…`)
    out of it.
  - `NAME.view` / `NAME.rings`: imports the core, draws the panel, declares
    the `def-kind`. The user attaches this one.
- A distributable package is a folder with `manifest.json` and `src/`; every
  module is renamed into the `author.name.` namespace, and the manifest lists
  the kind so the Packages tab and the rack menu can offer it before the
  module is loaded:

  ```json
  { "name": "alec/euclid", "version": "0.1.0", "entry": "alec.euclid.rings",
    "kinds": [ { "name": "euclid", "module": "alec.euclid.rings" } ] }
  ```

  A Local module has no manifest, so its kind only appears (in the rack
  menu's "New <kind> in rack", and as a working "New <kind>" item) once the
  module has been loaded in the session.

## def-kind

```lisp
(def-kind pulse
  :generator (:resolution :16
              :requires (my.pulse-core)
              :tick (my.pulse-core/tick self.cells self.track))
  :document ((cells (list 1 0 0 0 1 0 0 0 1 0 0 0 1 0 1 0))
             (track 0))
  :state ((sel -1))
  :view pulse-panel)
```

Slots (all optional except that a sequencer needs `:generator` or
`:sequencer`, never both):

- `:generator (:resolution TB :requires (MODULE …) :tick EXPR)`: the tick
  body, evaluated once per step of the timebase (`:16`, `:8`, `:16t`, `:32`,
  …) while the transport plays. `self.FIELD` reads document fields. `:requires`
  names every module the tick calls into; a function the tick calls must be
  reachable through them (qualified: `my.pulse-core/tick`).
- `:sequencer (…)`: a graph sequencer body (nodes and edges), as
  alez/neural uses. Most custom sequencers want `:generator`.
- `:document ((field default) …)`: the instance's saved settings. Stored per
  scene/pattern, saved with the project, undoable, copied on duplicate, and
  read by the tick as `self.field`. Values must be portable literals:
  numbers, strings, keywords, booleans, nil, lists and dicts of those. No
  functions, no reactive refs.
- `:state ((field default) …)`: per-instance view state (selection, open
  menus). Not saved, not seen by the tick.
- Field types: `(field default)` infers the type from the default (a number,
  `true`/`false`, a string, a list, else any); `(field :number :default nil)`
  declares it (`:number :int :bool :rgb :point :string :any`, a kind name,
  `(list-of type)`), and is required for a nil default. Writing a value of
  another type is an error.
- `:view FN`: `(FN self)` returns the panel widget tree for the instance's
  tab.
- `:on-create FN`, `:keymap MODE`: optional.

In the view, `self.field` reads and `(set! self.field value)` writes (one
undoable edit per write). `self.id` is the instance id, `self.owner` is
`:project` or the owning rack's group id.

`#'self.field` is a binding to a `:state` field of type `:number`, `:int`,
`:bool` or `:rgb`: pass it to a bindable widget prop (`(label "x" :active
#'self.open)`) and a write repaints that widget without re-running the view.
Used as a value (`if`, `=`, arithmetic, `str`, most natives) a binding reads
the field, like `self.field`: `(str "Vol " #'self.vol)` is `"Vol 0.8"`.

## The tick

Natives available inside a tick:

| Form | Meaning |
|---|---|
| `(gen-tick)` | 0-based count of this instance's steps since the transport started |
| `(gen-beat)`, `(gen-bar)`, `(gen-phase)` | position in quarter-note beats, bar index, beats into the bar |
| `(gen-rand)` | deterministic random float in [0, 1) |
| `(state-get "key" default)` / `(state-set! "key" n)` | numeric per-instance memory across ticks (counters) |
| `(seq-emit :track T :at :now :note N :vel V …)` | emit one note |
| `(gen-mark value [key] [at-beats])` | publish a number to the panel at the note's audio time |

`seq-emit` keys: `:track` (0-based track; when a rack owns the instance, the
rack's k-th pad), `:at` (`:now` or an offset in beats from this step, for
ratchets and swing), `:note` (transpose in semitones from the track's base
note, 0 = the track's own pitch), `:vel` (0..1), `:dur`, `:pan`, `:chord`
(list of transposes), `:quantize`, `:params` (a flat `name value …` list of
parameter locks). One step at `:16` is 0.25 beats.

Rules:

- Detect a transport restart yourself if you keep counters: `gen-tick` starts
  at 0 again, so reset when the tick is not greater than the last one seen
  (`state-get "last-tick" -1`).
- A tick that throws is reported once and then parked (silent) until the
  module is reloaded. Guard `nil` and empty lists.
- Keep the tick cheap: it runs on every step for every instance.

## Showing the playhead

In the tick, `(gen-mark (+ (gen-tick) 1))`. In the view,
`(bind-seq (str "generator-mark-" self.id))` is a binding that reads the
latest sounded mark (0 when stopped). Pass the binding straight into a
widget property (a `defwidget` `:bindable` field, a sexp-slot `:lit`): it
repaints on every step without re-running your view function. Do not do
arithmetic on it in Lisp; do the arithmetic in the shader. Keyed marks,
`(gen-mark v "k3")`, read as `generator-mark-<id>-k3`.

## Panels

- Build from the standard widgets: `box`, `h-stack`, `v-stack` (`:gap`,
  `:align :center|:start`), `label`, `button` (`:on-click (lambda (event)
  …)`), `number-picker` (`:value :min :max :step :decimals :on-change (lambda
  (v) …)`), `dropdown` (`:options` list of strings, `:value-index`,
  `:on-change (lambda (label) …)`), `sexp-slot` (below), and your own
  `defwidget` shaders.
- Children of a stack are generated with `(each LIST |i| body)`, never `map`.
- Every interactive widget needs a unique `:key` within the panel, such as
  `(str "pulse-cell-" i)`.
- Sizes are layout cells. A cell is about 2.2 times taller than it is wide,
  so a visually square widget is about twice as many columns as rows (33 x 15).
- Track names: `SEQ.track-names`, colours `SEQ.track-colors`. When
  `self.owner` is a number the instance belongs to a rack; its pads are
  `(eseq.drum-rack-v2/members (eseq.drum-rack-v2/group-index-by-id self.owner))`.
- Offer "Off" in track pickers (store -1, skip the emit), so a voice can be
  silenced without losing its settings.

### sexp-slot: editable lists

A `sexp-slot` edits a small piece of data: numbers, words, lists, forms. A
row schema makes the value a list of items shown without outer parens, so
the user can type `0 12 4 5 9` or nest `(3 5)`:

```lisp
(sexp-slot :key (str "note-" i)
  :schema (list "forms" (list "num" :min -48 :max 48 :step 1 :decimals 0 :default 0))
  :value (get ring :note)            ; a list
  :height 1.3 :font-size 11 :wrap false
  :lit (bind-seq (str "generator-mark-" self.id "-n" i))   ; bitmask of the item playing
  :on-change (lambda (v) (set-ring-field self i :note v)))
```

Decide what advances a list (per hit, per cycle) and cycle through it in the
tick: item `count mod len`; a nested list advances each time its parent
item comes round. Light the playing item with `(gen-mark (bit item) key)`.
Give each slot a fixed-width `box` wrapper, and put the column most likely
to grow last in the row.

### defwidget shaders

```lisp
(defwidget pulse-strip
  :width 32 :height 1
  :state (cells mark)
  :bindable (cells mark)
  :shader
  (let ((c (clamp (floor (* (+ (/ x aspect) 1.0) 8.0)) 0.0 15.0))
        (on (mod (floor (/ cells (pow 2.0 c))) 2.0))
        (here (if (= c (mod (- mark 1.0) 16.0)) 1.0 0.0)))
    (sdf/paint (let ((x (- x (* aspect (- (/ (+ c 0.5) 8.0) 1.0))))) (sdf/circle 0.5))
               (rgba 1.0 (- 1.0 (* 0.5 here)) 0.4 (+ 0.25 (* 0.75 (max on here)))))))
```

- `:state` is capped at **16 scalar uniforms**; extra names are dropped
  silently. Pack integers into one float (`a + 64 b + 4096 c`, keep it below
  2^24) and unpack with `floor`/`mod`. Lists cannot be passed.
- Coordinates: the SHORT axis spans [-1, 1], the long axis
  [-aspect, aspect] (or [-1/aspect, 1/aspect] when taller than wide); `y`
  points DOWN. `aspect` is width/height in pixels.
- Available: `+ - * / min max abs clamp mix smoothstep floor ceil fract
  round mod pow exp sqrt sin cos atan2 length vec2 rgba`, `let`, `if`,
  `sdf/circle sdf/rect sdf/rounded-rect sdf/line sdf/translate sdf/rotate`,
  `sdf/layer` (later children on top), `sdf/paint dist color` (straight
  alpha), `sdf/fill dist (material :color …)`, `sdf/region :key visible
  material [hit-shape]` (pointer target), `hit/hover`.
- Macros used inside `:shader` must be written module-qualified
  (`my.pulse/dot`), and exported, even in the module that defines them.
- Pointer callbacks: `:on-mouse-down` / `:on-drag` / `:on-mouse-up`
  `(lambda (sx sy region) …)`. `sx`, `sy` are [-1, 1] per axis with no
  aspect correction; `region` is the `sdf/region` keyword under the press.
- Each pixel should only evaluate what is near it (for example the nearest
  step), not loop over everything.

## eseqlisp gotchas

- `0` is falsy. Test with `(= x 0)` / `(= x nil)`, never `(if count …)`.
- Plain argument lists are fixed-arity; `def`/`lambda` also take
  `&optional`, `&rest` and `&key` (`(def f (a &key (b 1) c) …)`,
  called `(f 0 :c 2)`). Macros take `&rest` only.
- `(merge dict :k v :k2 v2)` returns an updated dict; `(get dict :k)` reads.
  `(dict :a 1)` builds one. `(nth list i)`, `(len list)`, `(range a b)`,
  `(append list (list x))`, `(reduce f init list)`, `(map f list)`,
  `(filter f list)`.
- Strings have no escape sequences; `str` concatenates anything.
- Replace a list element by rebuilding the list:
  `(map (lambda (j) (if (= j i) new (nth xs j))) (range 0 (len xs)))`.
- Qualified calls across modules: `my.pulse-core/tick`. Only exported names
  are visible.
- A module is loaded once per session; eseq reloads a changed Local file
  when it is saved. Save a matching pair of files (core and view) together,
  or a reload can see one new file and one old.

## Making it a package

When the user wants to share a sequencer (identity `author/name`, for
example `alec/euclid`):

1. Build the package folder OUTSIDE the packages folder (the importer refuses
   a folder already inside it), for example `~/Desktop/alec.euclid/`.
2. Copy each module to `src/`, renaming it into the namespace
   `author.name.`: `euclid/core.lisp` becomes `src/core.lisp` declaring
   `(module alec.euclid.core)`. Rewrite every qualified reference to match,
   including `:requires`, `(import …)`, calls like `euclid.core/tick`, and
   module-qualified macro names inside `:shader` bodies.
3. Write `manifest.json` with `name`, `version`, `entry` (the view module) and
   `kinds` (see "Files and names").
4. Install it with `eseq package import ~/Desktop/alec.euclid`, then check
   the installed module: `eseq sequencer check alec.euclid.rings`.
5. Tell the user: the package now appears under **Installed** in the
   Packages tab, and **File > Import Package…** installs the same folder
   (or a zip of it) on another machine. The kind id changes with the
   namespace (`alec/euclid:euclid` instead of `euclid.rings:euclid`), so
   instances in existing projects still use the Local copy; keep it until
   those projects are moved over.

## Checking

```sh
eseq sequencer check MODULE [--eval FORM] [--no-render]
```

Give it the module that declares the kind (the view module). Stages:

| Stage | Fails when |
|---|---|
| module | no file for the name, or the file does not begin with `(module NAME)` |
| parse | an unbalanced bracket in the module or any Local/installed module it imports (file and line) |
| def-kind | the module declares no `(def-kind …)` |
| panel | loading the module, creating an instance, or drawing its tab fails |

On success it prints a PNG path per kind. Open it and look at it: overlaps,
clipping, unreadable text, empty space. `--eval FORM` runs after the
instance exists, to fill the document with an example or pin a preview:

```sh
eseq sequencer check my.pulse --eval '(let ((e (instance-ref 1))) (set! e.cells (list 1 1 0 1)))'
```

To see the playing state in the PNG, give the view a preview hook, such as
`(defstate pulse-preview-mark 0)` used in place of the `bind-seq` mark when
above 0, and set it with `--eval '(set! my.pulse/pulse-preview-mark 37)'`.

The check does not run the tick. When the panel is right, ask the user to
press Play and listen, and to report any error in the status line. A tick
error reads `Sequencer '<label>' tick failed (generator parked until its
source changes): <error>`; the sequencer stays silent until a file changes.
