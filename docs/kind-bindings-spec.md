# Kind bindings

Status: spec rev 3, 2026-10-04. Nothing built. Bead: epic `eseq-0l17` (`bd list --label kind-bindings`).
Rev 3 resolves the open questions (§12 Decisions). Rev 2 dropped the separate `defrecord` form of rev 1: host state and view state
are declared with `def-kind`, which gains keyed and singleton kinds, a `:host`
field group and typed fields.

Typed, schema-checked access to host state and view state, replacing the
string-built reactive keys. One declaration form (`def-kind`), three ways to
use a field: by value (`t.volume`), as a binding (`#'t.volume`), or by handing
the whole instance to a widget (`:track t`).

Builds on `docs/instance-kinds-spec.md` §4 and `docs/jaki-kind-spec.md` §3
(`crates/eseqlisp/src/lang/vm/instances.rs`): `Value::Instance`, dotted
`GetField`/`StoreField` dispatch, one DAG source per (instance, field), schema
errors naming the fields, and field groups that differ only in storage
("the symbol is the API").

## 1. Problem

The mini-DAW experiments (`eseq -noui view.lisp`) are meant to let someone
whip up a custom sequencer view in a few hundred lines of Lisp. Today the
reactive layer fights that:

1. **String-built keys.** The binding store is keyed by `(namespace, field)`
   with at most one index (`ReactiveBindingKey`, `reactive.rs`). Anything with
   two indices is encoded in the name:
   `format!("seq-track-step-active-{track}-{step}")`
   (`crates/sequencer/src/ui/state_values/steps_and_pattern.rs:260`). Authors
   write `(bind-seq (str "seq-track-step-active-" tr "-" s))`.
2. **No schema.** Naming grew organically (`track-{n}-volume` but
   `track-peak-{n}`, `track-color-r-effective` via `bind-seq-nth`). A typo
   binds a slot nobody writes: the widget silently reads 0.
3. **Three access forms, chosen by the author.** `SEQ.x` reads a value and
   records a DAG dependency (re-render on change). `(bind-seq "x")` /
   `(bind-seq-nth "x" i)` return a `Value::ReactiveRef`, a float slot the
   renderer reads per frame (repaint only, `sdf_widget.rs:350`). Nothing tells
   you which one you hold.
4. **Refs misbehave as values, silently.** A `ReactiveRef` reaching Lisp:
   - is truthy: `is_falsey` (`vm.rs:1146`) falls through `_ => false`, so
     `(if (bind-seq "playing") …)` always takes the then-branch;
   - compares unequal to every number: `OpCode::Eq` is derived `==`
     (`vm.rs:8172`);
   - makes arithmetic fail with `IncorrectType` (`vm.rs:8025`), the only loud case;
   - makes most natives return `nil`/defaults: they pattern-match
     `Value::Number` and bail.
   This has bitten real code.
5. **Colors are three floats.** A slot holds one number, so color is
   `tr-r`/`tr-g`/`tr-b`, threaded through every widget state list and every call.
6. **`:bindable` boilerplate.** `defwidget` repeats most of `:state` in
   `:bindable`. For SDF widgets it is only a gate (`prop_accepts_binding`,
   `widgets.rs:490`): every state is already a uniform read through
   `get_f32_prop`, which accepts refs (`sdf_widget.rs:591`).
7. **View state is loose `defstate`s.** Related view state (a context menu's
   open flag, anchor and target) is several globals or a hand-built dict.

## 2. Model

Everything is a **kind** and its **instances** (`Value::Instance`), read and
written with dotted fields. Kinds differ on two axes.

**Where instances come from:**

| Kind | Instances | Example |
|---|---|---|
| created (no `:key`, today's kinds) | created by the host on request (Packages tab, `:on-create`), with lifecycles, tabs, views | `neural`, `jaki` |
| keyed (`:key (index)`) | projections of host things, resolved by key, never created by hand | `track`, `step`, `scene` |
| singleton (`:key ()`) | exactly one, bound to the kind's name | `transport`, `scene-menu` |

**Where each field is stored** (the field group):

| Group | Storage | Writable | Saved |
|---|---|---|---|
| `:state` | local cells | yes | no |
| `:document` | host scene-slot store (jaki-kind §3) | yes | yes, undoable |
| `:host` | host model, pushed to the instance | via `:set` | host's business |

**How a field is used:**

| Form | Is | Use it for | On change |
|---|---|---|---|
| `t` | an instance | passing a thing around; `defwidget` state | n/a |
| `t.volume` | the plain value, read now | anything in Lisp | re-render the reader |
| `#'t.volume` | a binding to one field | any bindable prop | repaint only |
| `:track t` on a `defwidget` | instance state | shaders | repaint only |

The rules an author learns:

- `t.x` is a value.
- `#'t.x` is a binding. Used as a value it reads itself (§8).
- An instance given to a `defwidget` binds whichever fields its shader reads.

## 3. `def-kind` extensions

```lisp
;; Keyed: the host's tracks are the instances
(def-kind track
  :key (index)
  :host ((name     :string)
         (color    :rgb)
         (volume   :number :range (0 1) :set seq-set-track-volume)
         (peak     :number)
         (muted    :bool   :set seq-set-track-mute)
         (armed    :bool   :set seq-set-record-arm)
         (selected :bool)
         (preset   :string)
         (devices  (list-of device))
         (steps    (list-of step))))

;; Keyed under a parent
(def-kind step
  :key (track index)
  :host ((active   :bool :set seq-set-step)
         (playing  :bool)
         (selected :bool)))

;; Singleton, host-backed
(def-kind transport
  :key ()
  :host ((playing   :bool :set seq-set-playing)
         (recording :bool :set seq-set-recording)
         (scene     scene)))

;; Singleton, Lisp-owned view state
(def-kind scene-menu
  :key ()
  :state ((open false)
          (scene scene :default nil)
          (at    :point :default nil)))

;; Created kinds are unchanged
(def-kind neural :sequencer (…) :document (…) :state (…) :view gvr-panel)
```

### 3.1 `:key`

- Absent: a created kind, exactly as today.
- `(index)`: keyed by one integer. `def-kind` defines a constructor named
  after the kind in the defining module: `(track 3)` returns that track's
  instance, or nil when no track 3 exists.
- `(track index)`: keyed under a parent kind. Reached only through the parent
  (`t.steps`, `(nth t.steps 5)`); no constructor (one path per thing).
- `()`: singleton. `def-kind` binds the kind's name, in the defining module,
  to its one instance, so `transport.playing` and
  `(set! scene-menu.open true)` work directly.

Constructors and singleton bindings are ordinary module definitions: a view
gets the host ones by importing them (§3.4). Kind *names* are a separate,
global registry (as for `neural` and `jaki` today), which `defwidget :state`
resolves through, so instance state works without any import.

Keyed and singleton kinds opt out of what only created kinds need: no Packages
tab rows, no `:view`/tabs/buffers, no `:on-create`, no manifest entry. Their
built-in fields are `id` and `kind` only (no `owner`/`label`). Asking to
create one is an error: `track instances come from the project; use (track i)`.

### 3.2 `:host`

Fields whose values the host owns. Entries are typed (§3.3) and may carry:

| Option | Meaning |
|---|---|
| `:set f` | writable; `(set! t.field v)` calls `(f t v)`. Without it the field is read-only. |
| `:range (lo hi)` | metadata, readable as `(field-range 'track 'volume)` so faders scale themselves. |
| `:doc "…"` | shown by `describe-kind`. |

`:host` is allowed only on keyed and singleton kinds. On a created kind it is
an error: `:host fields need :key` (§12 D5).

### 3.3 Field types

| Type | Lisp value | Shader value | Bindable |
|---|---|---|---|
| `:number` | number | float | yes |
| `:int` | number (integral) | float | yes |
| `:bool` | `true`/`false` | 0.0 / 1.0 | yes |
| `:rgb` | `(rgb r g b)` | vec3 (3 uniforms) | yes |
| `:point` | `(dict :col :row)` | n/a | no |
| `:string` | string | n/a | no |
| `:any` | any value | n/a | no |
| `<kind>` | instance | n/a | no |
| `(list-of <type>)` | list | n/a | no |

Numeric types (`:number :int :bool :rgb`) are **slot-backed**: each such field
has an atomic float slot (per component for `:rgb`) as well as its DAG source.
Everything else is **value-only**: a DAG source, no slot.

`:host` entries are always typed: `(name type option…)`.
`:state` and `:document` entries accept either form:

- `(name default)`: today's form. The type is inferred from the default
  (`false` → `:bool`, a number → `:number`, a string → `:string`, a list →
  `(list-of :any)`, anything else → `:any`).
- `(name type :default d)`: explicit. Required when the default is `nil` or
  when inference would pick the wrong type (`(scene scene :default nil)`).

Existing kinds keep working untyped; their numeric `:state` fields become
slot-backed and bindable for free.

### 3.4 Where host kinds are declared

`content/core/kinds.lisp`, the module `eseq.kinds`. It is the one page that
answers "what can I build a view from". A view imports what it uses:

```lisp
(import eseq.kinds :refer (track tracks transport banks selection))
```

so common words (`step`, `bank`, `scene`) are only claimed by files that ask
for them (§12 D1). The host kind names (`track`, `step`, `transport`, `scene`,
`bank`, `device`, `selection`) are reserved in the kind registry. At startup the host checks
itself against it: a field it publishes that a kind lacks, or a declared
`:host` field it never publishes, is a hard error in debug builds and a
warning in release.

## 4. Keyed instances

- **Identity.** Each host thing a keyed kind projects gets an ordinary
  `InstanceId` from the existing registry, stable for that thing's lifetime.
  Reordering tracks changes `(track 3)`'s answer, not the id, so an instance
  a closure captured keeps meaning the same track.
- **Registry.** The host registers and drops keyed instances as its model
  changes (track added or deleted) and keeps the key → id map the
  constructor reads. `t.index` is a `:host` field like any other.
- **Steps are positional and lazy** (§12 D2). A step instance means "step
  slot `s` of track `t`", keyed (track id, step index), not per pattern: a
  scene switch changes no instance, the host pushes new values. Step
  instances are registered on first read (through `t.steps`) and dropped when
  the track is deleted or its length shrinks below their index. Cost scales
  with the steps a view shows, not `MAX_STEPS` (256, `sequencer/data.rs:21`)
  × tracks.
- **Equality** is the existing `Value::Instance` id equality (`vm.rs:490`):
  `(= (track 3) (track 3))` is true, so instances work as dict keys and in
  `=` tests (`(= view.open-slot d)`).
- **Stale instances** keep the existing tombstone behaviour (instance-kinds
  §4): reads answer the field type's default (`0`, `false`, `""`, `()`, nil),
  writes are no-ops.
- **Collections.** `(tracks)`, `(banks)`, `(scenes)` and the nested
  `t.steps`, `t.devices`, `bank.scenes` are value reads (§5), so adding a
  track re-renders whoever iterated `(tracks)`.
- **Printing:** `<track#41 [3]>`, `<step#902 [3 12]>`, `<transport>`.

## 5. Reading by value

`t.field` compiles to load + `GetField`, as today. A new
`FieldSlot::Host(usize)` joins `Id`/`Kind`/`Owner`/`Label`/`State`/`Document`
(`instances.rs:265`):

1. The schema lookup is the existing one; unknown fields keep the existing
   error (`kind 'track' has no field 'volme'; fields: id, kind, name, color, …`).
2. The read records the existing per-(instance, field) DAG dependency under
   `%instance/<id>`.
3. The value is the instance's cell, which the host pushes (§9). It is always
   a plain value, never a ref.

Nested chains (`transport.scene.bank.label`) are repeated `GetField`s, each
recording its own dependency.

## 6. Writing

`(set! t.field v)` compiles to the existing load + `StoreField`, which already
dispatches on `Value::Instance` (`write_instance_field`):

- `:state`: write the cell (today), and the slot when slot-backed.
- `:document`: the host doc-write native (today).
- `:host` with `:set f`: call `(f t v)`. The host updates its model and pushes
  the accepted value back; Lisp never writes host cells directly, so undo and
  project state stay host-side (the same route `label` renames take).
- `:host` without `:set`: error `track.peak is read-only`.

Stdlib (`core/init.lisp`) adds:

```lisp
(defmacro toggle! (place) `(set! ,place (not ,place)))
(defmacro when (c &rest body) `(if ,c (do ,@body) nil))
```

## 7. Bindings

### 7.1 `#'`

`#'` is a reader prefix like `'`: `#'t.volume` reads as `(function t.volume)`,
the Common Lisp / Clojure "the thing itself, not its value" form. (Clojure's
`#'x` yields the Var, a reference that observes later changes, which is exactly
this.)

- Lexer: `#'` becomes a new `Token::HashQuote` (`parser.rs:443` handles the
  other prefixes).
- The operand must be a dotted path `h.f1…fn`. Anything else is a compile
  error: `#' takes a field path like t.volume`.
- Compiles to: evaluate `h.f1…f(n-1)` by value (each step records its
  dependency, so `#'transport.scene.active` rebinds when the scene changes),
  then call `(__field-ref instance "fn")`.
- `__field-ref` checks the schema (unknown field, or a type that is not
  slot-backed, is an error naming the field and type) and returns
  `Value::ReactiveRef { kind: Float, slot, … }` over the field's slot, keyed
  `ReactiveBindingKey::instance(id, field)`. `:rgb` returns a 3-slot ref
  (`BindingKind::Rgb`).
- Paths on singletons (`#'transport.playing`) are checked at compile time.
  Paths through a variable are checked when the ref is built, since the
  instance's kind is only known then.

### 7.2 Built-in widget props

Unchanged. The ~40 built-in widgets (`label :active`, `toggle :value`,
`mixer-meter :level-l`, `context-menu :is-open`, `knob-number`, `adsr-editor`,
…) keep their `bindable_props()` lists and `prop_accepts_binding`. A `#'` ref
is an ordinary `ReactiveRef` to them.

### 7.3 `defwidget` state

```lisp
(defwidget step-cell
  :state (step track seed)
  :shader (… step.active … step.playing … track.color … transport.playing …))

(step-cell :step s :track t :seed 68)
```

- A `:state` name that names a kind (`step`, `track`) declares **instance
  state**. Any other name is scalar state, as today.
- The shader compiler scans the body for `name.field` on instance states and
  for `kind.field` on singleton kinds, and allocates one uniform per
  referenced slot-backed field (three per `:rgb`). Fields the shader never
  mentions cost nothing.
- Budget: 16 floats total (`MAX_SDF_STATE_UNIFORMS`, `sdf_widget.rs:28`).
  Exceeding it is a compile error listing the allocation.
- An unknown field or a non-numeric type in the shader is a compile error:
  `step-cell: kind 'step' has no field 'activ'; fields: active playing selected`.
- At render, an instance prop fills its uniforms from the fields' slots; a
  scalar prop takes a number or any ref (`#'x`, legacy `bind-seq`).
- `:bindable` is deleted: every SDF state accepts refs. It is still parsed and
  ignored during migration, then removed from content.
- `box :background "step-cell" :track t` passes instance props through to the
  background widget like any prop.
- Singleton fields (`transport.playing`) are readable from any shader without
  being passed in.

## 8. Refs used as values

Refs now exist only where `#'` (or legacy `bind`/`bind-seq`) made one. When one
reaches a value position, it **reads itself**: the slot's current value, plus a
DAG dependency on its source. This is exactly what the matching `t.x` read
does, so it costs nothing extra.

Read points:

- `is_falsey` (`if`, `and`, `or`, `not`, `when`);
- `OpCode::Eq`, arithmetic and comparison opcodes;
- the native call boundary: one pass over the args, replacing refs with
  values, skipped for natives registered **ref-aware**: widget constructors,
  `list`, `dict`, `merge`, `reactive-value`, `str` (prints `<bind:…>` for
  debugging).

Ref-aware registration is an explicit flag on `register_native*`, so a native
author opts in and the default is safe. Natives reached through
`register_borrowing_native`'s by-name fast path take the same flag.

## 9. Host side

**Publishing.** A structured push replaces `format!`-built field names,
generalizing `VM::set_instance_host_field` (`instances.rs:536`, today
`owner`/`label` only):

```rust
rt.set_instance_field(track_id, "volume", Value::Number(0.8));
rt.set_instance_field(step_id,  "active", Value::Bool(true));
rt.set_instance_field(track_id, "steps",  Value::List(step_instances));
```

It writes the cell (dirtying that field's readers) and, for slot-backed
types, the slot. A value that does not match the declared type is a debug
assertion.

**Registry.** The host registers keyed instances with their keys
(`rt.register_keyed_instance("track", &[3]) -> InstanceId`), drops them when
the thing goes away, and re-keys them on reorder.

**`:set` natives.** Thin wrappers over the existing commands
(`seq-set-track-volume`, `seq-toggle-track-mute`, …) taking an instance.

**Liveness** (§12 D3). Today some fields stay live only while a buffer with
a particular name is visible (`*sequencer*` for playhead/step fields, `*fx*`
for `SEQ.instrument-panel`/`SEQ.effects`). With kinds every field carries an
**observed** bit: set while its DAG source has readers or one of its slots is
held by a ref (an `Arc` strong count above the store's own). The host skips
computing unobserved fields and pushes only values that changed. Buffer names
stop mattering. A coarse gate comes back only if profiling demands it, and
then only as an internal optimization authors never see.

## 10. Diagnostics

- Every schema error names the kind, the field and the known fields.
- Debug builds log the reason for each subtree re-render:
  `subtree track-3 re-rendered: track#41.muted read at view.lisp:212`.
  This makes an accidental by-value read in a hot view visible.
- `(describe-kind 'track)` prints the fields with group, type, `:set`,
  `:range` and `:doc`.

## 11. Migration

- `SEQ.x`, `bind-seq`, `bind-seq-nth`, `bind`, `reactive-get` keep working.
  The host publishes both the legacy names and the kind fields until content
  is ported, then the legacy names are removed per area.
- Existing `def-kind`s (`neural`, `jaki`) are unaffected; their numeric
  `:state` fields additionally become bindable (§3.3).
- `:bindable` is accepted and ignored (§7.3).
- The §8 read points change behaviour for legacy refs too: code that
  accidentally used a `bind-seq` ref as a value starts getting the right
  answer instead of `true`/`false`/`nil`. Audit content for code that
  depended on the wrong answer (unlikely, but the tests will say).
- Port order: the mini-DAW example (Appendix A) first, as the acceptance
  test, then the factory sequencer grid, mixer and transport.

Small items shipped alongside:

- `when` and `toggle!` in `content/core/init.lisp` (§6).
- `content/ui/effects/index.lisp`: one module importing every module that
  factory instrument/effect UIs call by qualified name (`custom-ui-*`,
  `param-controls`, `drum-surface`, `mnm-surface`, `physical-model-surface`,
  `identified-drum`, …). The module resolver learns that `eseq.effects`
  resolves to `effects/index.lisp`, so a bare `-noui` view needs only
  `(import eseq.effects)`.

## 12. Decisions

Resolved 2026-10-04. The common thread: less implicit behaviour.

**D1. Kind names are not bare globals.** Host kinds are the module
`eseq.kinds`; views import constructors and singletons with `:refer`.
Kind names stay in the global kind registry (reserved for host kinds), which
is what `defwidget :state` uses. (§3.1, §3.4)

**D2. Step instances are positional and lazy.** Keyed (track id, step
index), values follow the current pattern, registered on first read, dropped
on track delete or length shrink. (§4)

**D3. Liveness is per field.** An observed bit (readers or held slots) gates
host computation; only changed values are pushed. No buffer-name gates.
(§9)

**D4. Events stay dicts, with one documented shape.** Events are transient
input, never bound, so they are not instances. Dotted access already works on
dicts (`e.sx` compiles to `GetField`). Pointer events gain consistent fields:

| Field | Meaning |
|---|---|
| `e.u`, `e.v` | 0–1 position within the widget (today faders compute `(* 0.5 (+ e.sx 1))`) |
| `e.sx`, `e.sy` | -1–1 position, kept |
| `e.at` | grid point `(dict :col :row)`, what menus anchor to |
| `e.col`, `e.row` | kept |
| `e.shift`, `e.cmd`, `e.alt` | modifiers |

**D5. No `:host` on created kinds in v1.** It is an error (`:host fields
need :key`). Package instances exposing host-maintained fields can come later
with a real user; the field group and storage already exist. (§3.2)

**D6. Bindings are one-way.** `#'` only reads. Widgets write through their
handlers: `(hslider :value #'t.volume :on-change (lambda (v) (set! t.volume v)))`.
A later opt-in (`:value-write #'t.volume`) stays possible, since a ref knows
its instance and field.

## 13. Stages

1. **Singleton kinds and typed fields.** `:key ()`, the kind-name binding,
   typed `:state` entries with inference, `toggle!`/`when`. Tests: a
   singleton's fields read/write and dirty only their readers; inferred and
   explicit types; existing `neural`/`jaki` kinds unchanged.
2. **`#'` and self-reading refs.** Lexer token, compile rule, `__field-ref`,
   slots for slot-backed `:state` fields, the §8 read points with the
   ref-aware native flag. Tests: `(if #'k.flag …)` follows the value, `=` and
   arithmetic work, a ref passed to `label :active` still binds.
3. **Keyed kinds and `:host` in eseqlisp.** `:key (index)` /
   `(parent index)`, `FieldSlot::Host`, keyed registration and key lookup,
   constructors, `:set` dispatch, read-only errors, the created-kind opt-outs.
   Tested with a fake host.
4. **Host kinds.** `eseq.kinds` (`content/core/kinds.lisp`),
   `set_instance_field`, keyed registration from the project model, lazy
   step instances (D2), the observed bit (D3), the startup schema check, and
   `track`/`step`/`transport`/`scene`/`bank`/`device`/`selection` published
   alongside the legacy names, plus collections. Pointer event fields (D4).
5. **`defwidget` instance state.** Shader field scan, uniform allocation and
   budget error, singleton fields in shaders, `:rgb` as vec3, `:bindable`
   ignored.
6. **Example port.** Rewrite the mini-DAW view (Appendix A) against kinds,
   plus `eseq.effects` index resolution. Acceptance: it renders and plays like
   the string-key version, with no `bind-seq`, `str`-built keys or color
   triples.
7. **Factory kind inventory.** Stage 4 publishes the seven kinds the
   mini-DAW needs. The factory UI reaches further: ~1,600 legacy accesses
   (`bind-seq`, `bind-seq-nth`, `(bind "SEQV" …)`, `(bind "GRAPH" …)`,
   `SEQ.x`, `SEQV.x`, `reactive-get`, `:bindable`) across ~80 content files,
   heaviest in `ui/sequencer.lisp`, `ui/mixer.lisp`, `ui/arrangement.lisp`,
   `ui/browser.lisp` and the effect panels. Before porting, extend
   `eseq.kinds` with what those areas read, at least: `bus`, `param` (device
   parameters including p-lock variants, today's `SEQV`), `lane` (process
   lanes), `clip`/`region` (arrangement), `note` (piano roll), `graph-node`
   (`GRAPH`), browser entries. Each new kind is a §3 declaration plus host
   publishing; this spec's rules do not change.
8. **Factory port**, one area at a time, each removing that area's legacy
   field names: sequencer grid and step editing; transport, scenes and
   banks; mixer; effect and instrument panels (incl. custom-ui runtime,
   physical-model/drum/MnM surfaces, param controls); arrangement; piano
   roll; browser, sample import and resample; patching and macros; packages
   (`alez.tracker`, `alez.neural`, `alez.jaki`) and the sequencer demo
   scripts; factory instrument/effect `ui.lisp` files. Then delete
   `bind-seq`/`bind-seq-nth`/`:bindable` support and the legacy publishers.
9. **Diagnostics.** Re-render reason log, `describe-kind`. Useful from
   stage 6 on; can run in parallel with the ports.

Stages 1–3 touch only eseqlisp and can land before any host work.

## Appendix A. Target example (abridged)

```lisp
(import eseq.effects)
(import eseq.kinds :refer (tracks transport banks scenes selection))
(import eseq.step-grid :as grid)

(def-kind view
  :key ()
  :state ((bank -1)                         ; -1 = follow the playing scene
          (open-slot device :default nil)))

(def-kind scene-menu
  :key ()
  :state ((open false)
          (scene scene :default nil)
          (at    :point :default nil)))

(defwidget fader
  :width 24 :height 4 :paint-margin 0.5
  :state (track)
  :shader
  (sdf/layer
    (sdf/fill bar (vgrad (gray 0.07)))
    (sdf/fill (clip-x bar track.volume) (* (if track.muted 0.35 1) (vgrad track.color)))
    (outline bar)
    (sdf/fill (clip-x meter track.peak) (vgrad (rgb 0.45 0.9 0.4)))))

(defmacro pill-box (text lit &rest props)
  `(box :background "pill" :lit ,lit :width (sc 10) :height (sc 4)
        :h-align :center :v-align :center ,@props
     (label ,text :bg :transparent :color :white :active ,lit :active-color :black)))

(def step-view (s t)
  (box :width (sc 8) :height (sc 4)
    :on-mouse-down   (lambda (e) (grid/down s e))
    :on-drag         (lambda (e) (grid/drag s e))
    :on-mouse-up     (lambda (e) (grid/up s e))
    :on-double-click (lambda (e) (grid/double-click s e))
    (step-cell :step s :track t :seed (~slider 68.457 :min 0 :max 100))))

(def track-view (t)
  (box :on-mouse-down (lambda (e) (set! selection.track t))
    (h-stack :v-align :top :gap (sc 3)
      (label (substring t.name 0 3)
        :font-size (sc 32) :color :dim
        :active #'t.selected :active-color :white)
      (grid :columns 8 :cell-width (sc 8) :cell-height (sc 4)
        (each t.steps |s| (step-view s t)))
      (v-stack :gap (sc 0.5)
        (h-stack :gap (sc 1)
          (box :background "mute-button" :track t :on-click (lambda (e) (toggle! t.muted)))
          (box :background "arm-button" :armed #'t.armed
            :on-click (lambda (e) (toggle! t.armed)))
          (box :background "fader" :track t
            :on-drag (lambda (e) (set! t.volume e.u))))
        (slot-row t)))))

(def scenes-view ()
  (let ((shown (if (< view.bank 0) transport.scene.bank (nth (banks) view.bank))))
    (v-stack :gap (sc 1)
      (h-stack :gap (sc 1)
        (each (banks) |b|
          (pill-box b.label (= b shown)
            :pulsing (and b.playing (not (= b shown)))
            :on-click (lambda (e) (set! view.bank b.index)))))
      (h-stack :gap (sc 1)
        (each shown.scenes |s|
          (pill-box (str s.number) #'s.active
            :pulsing #'s.queued
            :on-click (lambda (e) (launch! s))
            :on-right-click (lambda (e)
              (do (set! scene-menu.scene s)
                  (set! scene-menu.at e.at)
                  (set! scene-menu.open true))))))
      (context-menu :is-open #'scene-menu.open :anchor scene-menu.at
        :on-close (lambda () (set! scene-menu.open false))
        (menu-item "Clone scene"  :on-select (lambda (e) (clone! scene-menu.scene)))
        (menu-item "Delete scene" :disabled (<= (len (scenes)) 1)
                                  :on-select (lambda (e) (delete! scene-menu.scene)))))))

(effect-buffer "*sequencer*"
  (v-stack :padding (sc 1) :gap (sc 1)
    (each (tracks) |t| (track-view t))))
```

Note the scene pills (`#'s.active`, repaint on launch) and bank pills
(`(= b shown)`, re-render) share `pill-box`; only the argument form differs.
