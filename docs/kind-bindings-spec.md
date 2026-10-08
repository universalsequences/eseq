# Kind bindings

Status: spec rev 3, 2026-10-04. Stages 1–6 built, stage 7 in part (§14; 7, 7b, 7b-2, 7b-3, 7c, 7d, 7e, 7f, 7g, 7h and 7i built), stage 8 in part (§13, §13.1: .12, .13, .15, .16, .17, .18, .19, .21, .65, .66, .67, .76 and .82 ported, .11, .14 (groups A–D: .14, .61, .74), .20 and .64 in part) (§3.1, §3.2, §3.3, §3.4, §4, §7.1, §7.3, §8, §9 notes). Bead: epic `eseq-0l17` (`bd list --label kind-bindings`).
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
| view-local (`:key (a b …)`, no `:host`) | created by Lisp, one per key, by the constructor (§3.1) | `adsr-gesture` |

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
- `((p1 p2 …) index)`: keyed under any one of several parent kinds (built
  in 7b-2 for `device`, under a track or a bus). The first key part is a
  live instance of one of them; each parent drops exactly its own children.
  The parent list is a set: naming a parent twice is an error (at compile
  time and in the VM's own `:key` parser), and a re-definition that lists
  the same parents in another order keeps the key's shape.
- `()`: singleton. `def-kind` binds the kind's name, in the defining module,
  to its one instance, so `transport.playing` and
  `(set! scene-menu.open true)` work directly.
- `(a b …)` with no `:host` group: view-local (eseq-0l17.62, below). Lisp
  owns the instances: `(adsr-gesture scope section)` returns the instance
  for that key, creating it on first call.

Constructors and singleton bindings are ordinary module definitions: a view
gets the host ones by importing them (§3.4). Kind *names* are a separate,
global registry (as for `neural` and `jaki` today), which `defwidget :state`
resolves through, so instance state works without any import.

Because a singleton or an index-keyed kind binds its name, that name may
not be a widget constructor's (eseq-0l17.24): `(def-kind knob :key (index)
…)` is a compile error, `def-kind knob: 'knob' is a built-in widget; a :key
() or :key (index) kind binds its name, which would shadow the widget (and
`(knob …)` calls get widget source props); rename the kind`, and likewise
`… is a defwidget` for a registered `defwidget` (checked again when the form
runs, for a `defwidget` earlier in the same unit). It used to define the
kind and then fail at the first `(knob 0)` with `takes one non-negative
integer`, since source annotation adds widget source props to every call
whose head is a widget name. A parent-keyed kind binds nothing and may take
any name (`compiler::kind_name_widget_collision`).

Keyed and singleton kinds opt out of what only created kinds need: no Packages
tab rows, no `:view`/tabs/buffers, no `:on-create`, no manifest entry. Their
built-in fields are `id` and `kind` only (no `owner`/`label`); a keyed
kind adds `key`. Asking to
create one is an error: `track instances come from the project; use (track i)`.
Declaring a field with a built-in field's name (`key` on a keyed kind, `id`/
`kind` on any, `owner`/`label` on a created kind) in `:host`, `:state` or
`:document` is a compile error naming the field and the kind
(eseq-0l17.60): `def-kind generator-mark: :host field 'key' is a built-in
field of a keyed kind (id, kind, key); rename it`. The schema check
(`InstanceKindSchema::validate`) still rejects the same at run time for
schemas built in Rust.

Built (stage 1, singletons): `(def-kind k :key () :state …)` compiles to
`(def k (__def-singleton-kind 'k :state …))` and never calls the host's
`def-kind` native, so the host registry, manifest check, project instance
list, tabs and kits never see it; `VM::live_instances` and
`instance_kind_ids` leave singletons out, and `eseq sequencer check` skips
`:key` kinds. A singleton takes only `:state` (`:document` is saved
per-instance project state, which a singleton is not). Its kind id is
`<module>:<name>` (`scratch:<name>` headerless); its instance id comes from
`SINGLETON_INSTANCE_ID_BASE` (2^48) up, clear of host ids. Re-evaluating the
`def-kind` keeps the instance and its values (§3.3 reload rule). A kind
cannot switch between created and singleton without a restart.

Built (stage 3, keyed kinds):

- Every `:key` form compiles to `__def-keyed-kind` (no host `def-kind`
  call). `:key (index)` binds the name to the constructor (a native closing
  over the kind id); `:key (parent index)` binds nothing and the form's
  value is the kind id. Keys longer than two names are a compile error.
  Kinds with a `:key` take `:host` and `:state`, singletons included, in any
  mix (`transport` can carry a local `:state` field); `:view`, `:document`,
  `:on-create` and the rest are errors. `:key` stays fixed in shape
  (created / singleton / `(index)` / `(parent index)` under the same parent
  name; index names may change) until restart. The schema holds it as
  `KindKey::{Created, Singleton, Indexed, Under}`; the compiler is the one
  place the "at most two names" rule is checked.
- Keys are `&[u64]`. For `(parent index)` the first part is the parent
  instance's **id** (D2: re-keying a track leaves its steps alone), so
  `<step#902 [41 12]>` prints the track id. The VM keeps a children index
  (parent id → child ids), maintained by register, re-key (a step moved to
  another track belongs to the new one) and drop; dropping a parent drops
  exactly its current children.
- Several parents (7b-2): `:key ((track bus) did)` is `KindKey::Under {
  parents, index }` with every name; each resolves as below, and a key's
  first part must be a live instance of any of them (the error names them
  all, `not a live 'm:track' or 'm:bus' instance`). The parent list is part
  of the key's shape (a hot reload may not change it); messages read
  `reach them through their track or bus`.
- The parent named in `:key` is resolved when the kind's first instance
  registers, not at `def-kind`, so `eseq.kinds` can declare `step` before
  `track`: the parent's own module first (`m:track` for `m:step`), else the
  one keyed kind with that name. The resolved parent kind id is then fixed
  for the VM's life, so a kind defined later that makes the name ambiguous
  never re-parents (or orphans) live children. A parent that is not keyed,
  or a key whose first part is not a live parent instance, is a
  registration error.
- Built-ins of a keyed kind are `id`, `kind` and **`key`** (the key as a
  list, `(3)`, tracked and re-published on re-key; read-only). `t.index`
  is not built in: the host declares and publishes `index` as a `:host`
  field where views want it (§4).
- Keyed instance ids are VM-allocated from `KEYED_INSTANCE_ID_BASE` (2^32)
  up, never reused (an eval rollback keeps the allocation counter):
  ordinary instance ids (equality, tombstones, `#'`), in a range clear of
  the ids the host saves for created instances; `create_instance` with an
  id from that range (or the singleton range) is an error
  (`InstanceError::ReservedId`). Dropping a keyed instance frees its field
  sources nothing reads any more.
- `(track i)` records a dependency on the source `(%keys/<kind id>, "i")`,
  one per key, advanced on register, drop and re-key; a non-integer or
  negative argument is an error. Only index-keyed kinds (which have a
  constructor) publish key sources.
- Host API (VM, mirrored on `Runtime`): `register_keyed_instance(kind,
  &[u64]) -> Result<InstanceId>` (kind id or a unique bare name;
  idempotent), `keyed_instance(kind, key)`, `drop_keyed_instance(kind, key)`
  (or `drop_instance(id)`), `rekey_instance(id, key)` and
  `rekey_instances(&[(id, key)])` (atomic, so a reorder can swap keys; a
  conflicting move, or a list naming one id twice, changes nothing),
  `instance_key(id)`.
  `create_instance` on a keyed kind is an error
  (`track instances come from the project; use (track i)`, or `…; reach
  them through their track` for a parent-keyed kind).
- Printing: `VM::format_value` / `Runtime::format_value` (used by `str`,
  `fmt` and the minibuffer echo) print `<track#41 [3]>`, `<step#902 [41 12]>`,
  `<transport>`, and a dropped keyed instance as `<track#41>`;
  `format_lisp_value` without a VM still prints `<instance:id>`.

Built (eseq-0l17.62, view-local keyed state): a `:key` of names on a kind
with no `:host` group is **view-local** (`KindKey::Local`). Such a kind has
nothing for a host to publish, so Lisp creates its instances; the rule is
the declaration's, not a flag (adding a `:host` group makes it a host kind
and is a key-shape change, "restart to change its :key"):

```lisp
(def-kind adsr-gesture
  :key (scope section)
  :state ((attack false) (decay false) (sustain false) (release false)))

(let ((g (adsr-gesture "core" -1)))   ; created on first call, then the same
  (set! g.attack true))
(drop-instance g)                      ; true; readers of its key re-run
```

- Any number of key names (one or more, no duplicates, no parent lists:
  `((a b) c)` is a compile error). The compiler emits the key as
  `:local-key` to `__def-keyed-kind`; the name is bound to the
  constructor, so the widget-name check applies (§3.1 above) and hot
  reload counts it as a definition.
- Key values (`LocalKeyPart`): numbers (`-0` is `0`), strings, keywords,
  symbols, booleans and instances; nil, lists, maps and functions are an
  error naming the call (`(adsr-gesture scope section) takes 2 key values
  …`), as is the wrong count. Equal arguments return the same instance.
- Instances are ordinary keyed ones: ids from `KEYED_INSTANCE_ID_BASE`,
  built-in fields `id`, `kind` and `key` (the key values as a list), typed
  `:state` cells, `#'` bindings, printing `<adsr-gesture#41 ["core" -1]>`,
  tombstones once dropped. Re-evaluating the `def-kind` keeps every
  instance and its values; the number of key names is fixed until restart.
- A constructor call records a dependency on that key's source
  (`%keys/<kind id>`, field = the key's text), so a reader re-runs when its
  instance is dropped and gets a fresh one at the fields' defaults.
- `(drop-instance x)` drops a view-local instance (true when it was live);
  any other value, a host or singleton instance included, is an error.
  An instance key part makes the view-local instance that instance's
  child: dropping the parent (a track deleted) drops it, and a constructor
  call naming a dropped instance answers nil instead of creating one.
- The host never creates them: `register_keyed_instance` and
  `create_instance` on a view-local kind are errors
  (`InstanceError::LocalKind`, `adsr-gesture instances are view-local;
  create them with (adsr-gesture key …)`). `VM::local_instances(kind)`
  lists the live ones.
- First user (eseq-0l17.73): `eseq.effects.custom-ui-sections`' ADSR stage
  flags, one `(adsr-gesture scope-name section)` per custom-UI envelope
  editor (§13 stage 8, .14 group A). A readout's render calls the
  constructor (creating the instance before any drag) and binds
  `#'g.attack` …; it reads no field by value, so a drag repaints only the
  readouts it lights. Nothing drops these instances: a scope is a stable
  name and the module hears of no scope going away (the same as
  `section-choice`'s entries).

### 3.2 `:host`

Fields whose values the host owns. Entries are typed (§3.3) and may carry:

| Option | Meaning |
|---|---|
| `:set f` | writable; `(set! t.field v)` calls `(f t v)`. Without it the field is read-only. |
| `:range (lo hi)` | metadata, readable as `(field-range 'track 'volume)` so faders scale themselves. |
| `:doc "…"` | shown by `describe-kind`. |

`:host` is allowed only on keyed and singleton kinds. On a created kind it is
an error: `:host fields need :key` (§12 D5).

Built (stage 3): a `:host` field (`HostField`) starts at its type's default
(`0`, `false`, `""`, `(rgb 0 0 0)`, `()`, nil) until the host pushes a
value with `set_instance_field`, which type-checks the push (an error, as
for every write, §3.3) and writes the cell, the readers' source and a bound
slot. `:set` is evaluated when `def-kind` runs (like `:view`), so the setter
must be defined first, and must be something `invoke` can call (a closure,
a native, a host handle). The created kind's `owner`/`label` are *built-in*
fields (`InstanceBuiltinField`, `set_instance_builtin_field`), not `:host`
ones. A Lisp write checks the
value's type, then calls `(f t v)` and writes nothing; without `:set` it is
`track.peak is read-only` (the kind name part of the kind id). Stale
instances ignore both. `:range` takes two literal numbers; `:range`/`:doc`
are schema metadata (`InstanceKindSchema::host`) until `field-range`/
`describe-kind` land. `:host` fields bind with `#'` like `:state` ones.
In `def-kind` field groups (`:key`, `:host`, `:state`, `:document`) the
field names and types are read as data, so a field named like a widget
(`(label :string)`) is not annotated as a widget call; defaults,
`:default` and `:set` expressions convert as ordinary code.

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

Built (stage 1), decisions the above left open:

- Types are checked on every write (`set!` and host `set_instance_field`,
  `:state` and `:document`), always, as an error
  `field 'n' of kind 'm:k' is :number; got "x"`. nil is accepted only by a
  field whose default is nil (an optional field) and by `(list-of …)` (the
  empty list). A typed default its type rejects is an error at `def-kind`.
- A nil default with no type (`f`, `(f)`, `(f nil)`) is a compile error, for
  created kinds too; a default that evaluates to nil errors when `def-kind`
  runs.
- `:rgb` values are the tagged list `(rgb r g b)` from the core `rgb` native
  (the shape widget color parsing already accepts); `:point` is a map with
  numeric `:col`/`:row`. A bare kind type (`scene`) matches an instance
  whose kind id's name part is the name (`pkg:scene`, `kind_name_of`); a
  qualified one (`pkg:scene`) must equal the kind id exactly.
- Hot reload keeps a value by field name only while the new type admits it;
  otherwise the field restarts at its default (instance-kinds §5 otherwise).

### 3.4 Where host kinds are declared

`content/core/modules/kinds.lisp`, the module `eseq.kinds`. It is the one page that
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

Built (stage 4):

- **Load.** `content/core/modules/kinds.lisp` declares `(module
  eseq.kinds)`. The module resolver tries `core/modules/<name>.lisp`
  (`modules::CORE_MODULES_DIR`) last for `eseq.` names
  (`module_relative_file_candidates`, and `@/core/modules/<name>.lisp` in
  the rootless `module_file_candidates` fallback), so `ui/` modules shadow
  core ones, package modules never reach `core/`, and the startup scripts
  beside the modules (`core/init.lisp`, `themes.lisp`, `sdf-stdlib.lisp`)
  never resolve as modules (`(import eseq.init)` finds nothing). Both roots (`ui/main.lisp` and the
  `-noui` root `ui/noui.lisp`) `(import eseq.kinds)` without `:refer`, so the
  kinds exist for the host to publish while no name lands in `eseq.vanilla`;
  a view refers what it uses. An `import`'s `:refer` covers the source it
  heads (a REPL eval needs its own). A `def-kind` with `:key ()` or
  `:key (index)` counts as a definition of its name for `(export …)`
  validation and hot reload (`extract_defined_symbols_from_source`, which
  reads `:key` with the compiler's `def_kind_slots`).
- **Exports:** `track`, `scene`, `bank` (constructors), `transport`,
  `selection`, `project` (singletons), `tracks`, `scenes`, `banks`
  (collections) and `launch!`. `step` and `device` are reached through
  their track.
- **Reserved names.** `VM::reserve_kind_names(module, names)` /
  `Runtime::reserve_kind_names`: defining a kind with a reserved name in any
  other module is an error (`kind name 'track' is reserved for the host
  kinds of eseq.kinds; …`). The host reserves every kind in `PUBLISHED`
  (`host_kind_names`: `track step device scene bank transport selection
  project`, and since stage 7 `send bus group master engine`, since 7b `param`, since 7i `route`, since 7d `song region scene-span clip cell`, since 7h `pad rack-clip groove pad-groove pool-groove library-groove`, since 7b-3 `mod-target tensor variant macro rack-macro macro-mapping`, since 7c `process-class process-library process lane inlet port fanout state-cell`, since 7f `browser preset-file slot-presets sound sound-palette editor editor-macro editor-asset asset-info learn learn-plan-param learn-epoch-param learn-delta retro retro-lane retro-item song-export settings midi-device agent`, since 7e `note piano-roll`, since 7e-2 `focus-step`, since 7d-2 `pending-lane pending-scene pending-launch`) before
  evaluating the root.
- **Schema check.** `host_kinds::PUBLISHED`
  (`crates/sequencer/src/ui/host_kinds/mod.rs`) lists every field the host
  publishes as a `FieldKey` (kind id, field; the `host_kinds::f`
  constants every push site uses), its type and its feed; `check_schema`
  compares it with the loaded kinds in both directions (a published field
  missing or of another type, a declared `:host` field never published, a
  kind not declared). `create_editor_with_root` runs it after the root
  loads: `panic!` in debug, an `eprintln!` warning in release. After
  startup `HostKinds::sync` re-runs it whenever
  `Runtime::instance_kind_schema_generation` moves (any `def-kind`, or a
  rollback, e.g. a hot reload of `eseq.kinds`): a mismatch warns once per
  distinct set and its fields are skipped (no push, the reader answers
  `None`) until fixed; it never panics.
- **Fields as built:**

  | Kind | Key | `:host` fields (`:set` in brackets) |
  |---|---|---|
  | `track` | `(index)` | `index :int`, `name :string`, `color :rgb`, `volume :number` [`seq-set-track-volume`], `peak :number`, `muted :bool` [`seq-set-track-mute`], `audible :bool`, `armed :bool` [`seq-set-record-arm`], `selected :bool`, `preset :string`, `num-steps :int`, `steps (list-of step)`, `devices (list-of device)` |
  | `step` | `(track index)` | `index :int`, `track track`, `active :bool` [`seq-set-track-step`], `playing :bool`, `selected :bool` |
  | `device` | `(track slot)`, slot part = slot + 1 (since 7b `(track did)`, §14.2b; since 7b-2 `((track bus) did)`, §14.2f) | `track track`, `slot :int` (-1 = instrument), `name :string`, `enabled :bool` |
  | `scene` | `(index)` | `index :int`, `number :int` (1-based in its bank), `name :string`, `active :bool`, `queued :bool`, `bank bank` |
  | `bank` | `(index)` | `index :int`, `label :string`, `scenes (list-of scene)`, `playing :bool`; since stage 8 (.12) `bid :int` (the stable bank id the scene-bank commands take), `name :string` (its own name, "" for none) |
  | `transport` | `()` | `playing :bool` [`seq-set-playing`; since .12 it compares against this flag, the raw transport, so a Play / Stop click is never a no-op], `recording :bool` [`seq-set-recording`], `scene scene`, `queued scene` (nil when none), `launch-quantize :string` (since .12 [host command `set-scene-launch-quantize`]) |
  | `selection` | `()` | `track track` [`seq-set-track`] |
  | `project` | `()` | `tracks (list-of track)`, `scenes (list-of scene)`, `banks (list-of bank)` |

  The `:set` functions are Lisp wrappers in `eseq.kinds` over those
  natives, all absolute: `seq-set-track-mute` (slice-3 op `set-mute`),
  `seq-set-track-step` (`toggle-step` with `:active`) and `seq-set-playing`
  (`song-transport-set-playing`) toggle only when the model differs when
  the command lands; `seq-set-record-arm` and `seq-set-recording` compare
  with the shared flag at once. So two `set!`s or `toggle!`s in one frame
  never undo each other through a cell that has not caught up. `(launch!
  s)` is an action (`switch-pattern` with `transport.launch-quantize`), not
  a `:set`. `(tracks)`, `(scenes)`, `(banks)` read the `project`
  singleton, so adding a track re-renders whoever iterated them. Model
  identity: tracks by `app.track_registry` `TrackId` paired with the
  registry's `generation()` (ids restart with every project, so a new
  generation drops every track instance: a project load replaces them; a
  rebuilt track shell keeps its instance); scenes and banks by their ids.
  Identity never falls back to position: a frame whose scene or bank ids
  are not distinct, or whose registry is out of step with the track list,
  skips that part of the sync. A reorder re-keys in one `rekey_instances`,
  a delete drops (with the steps and devices under it). `reconcile`
  returns one `Option` per model entry, so a failed registration leaves a
  hole instead of shifting later indices.
- **Sources.** Track name/color/preset/devices come from the same `App`
  data as `SEQ.track-names`, `track_display_color`
  (`track-color-*-effective` without the mute dimming),
  `track_loaded_presets` (shared with `SEQ.track-loaded-presets`) and
  `track_device_chain` (shared with `SEQ.track-device-chains`);
  volume/mute/steps/num-steps from `SequencerState` track params and
  patterns; `armed` from the shared record-arm vector;
  `selected`/`selection.track` from the current track; `step.selected`
  from the step selection on the current track or, while a rack-wide
  `ActiveDeleteTarget::TrackSteps` is armed, on each of its tracks, clipped
  to the track's length (like the legacy step-selection publish);
  `step.playing` from `track_active_playhead_step` while the transport
  plays; `peak` from the reactive tick's track meter cache
  (`MeterCache::cached_track_peak_levels`, polled at the meter cadence,
  also while only a host-kinds `peak` is observed: `HostKinds::wants_peaks`);
  scenes and banks from `with_project_scenes` (labels from
  `scene_bank_label`, shared with `SEQ.scene-banks`), the queued scene from
  `queued_transport_scene` (shared with `SEQ.queued-scene`),
  `launch-quantize` from `SEQ.scene-launch-quantize`. `muted` is the track's own mute (what the
  setter toggles), not the solo-effective mute; `audible` is the effective
  one (not muted and not silenced by a solo, the legacy
  `track-muted-effective` negated; the tick copies the `App`'s solo state
  for it).
- **Publishing.** `HostKinds::sync` runs at the end of every
  `sync_reactive_tick` (state in `FrameDiffState::host_kinds`): re-check the
  schema when its generation moved, reconcile the registry and push model
  fields when the model revision moved (§9), compare the queued scene and
  launch quantization, push observed live fields, and run one reactive
  cycle when anything changed.
  Every push compares with the cell first (`Runtime::instance_field`), so
  only changed values reach readers and slots. Legacy `SEQ` fields and their
  buffer-name gates are untouched.

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

A view-local kind's constructor (§3.1, eseq-0l17.62) is a read too: it
depends on its key's source, which moves only when the instance is dropped
(or created), so a view that keeps per-scope state in `(adsr-gesture scope
section)` re-renders on its fields, never on the lookup.

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

with `unless` beside `when`, and (eseq-0l17.46) `cond`: `(cond (test a b)
... (else c))` runs the first clause whose test is truthy, every form of its
body, and yields the last; a final `else` (or `true`) clause always matches;
no match yields nil. Like `when` / `unless` it is an init.lisp macro, absent
from a bare runtime. `eseq.kinds/mod-in-level` dispatches with it.

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

Built (stage 2), decisions the above left open:

- The reader form is `(function path)` (`parser::FUNCTION_FORM`); `function`
  was free (no native, no content use), so it is a special form now, and
  `__field-ref` is the native it compiles to. Only `#` immediately followed
  by `'` is the prefix; `#` elsewhere is still a symbol character.
- Every path is checked when the ref is built, singletons included (no
  compile-time schema check yet): the singleton's field types are only known
  once its `def-kind` has evaluated.
- `#'NS.field` on a reactive namespace (`#'SEQ.playing`) compiles to the
  legacy ref `(bind "SEQ" "playing")`, as a migration aid; the compiler
  knows the reactive namespaces statically. `#'SEQ.a.b` is a compile error.
- Errors: `#': kind 'm:k' has no field 'x'; bindable fields: a, b` and
  `#': field 'name' of kind 'm:k' is :string, which is not bindable;
  bindable fields: …` (also for built-in fields). A non-instance head (a
  dict, nil, a string) is an error too. The check is
  `InstanceStore::field_binding`, shared with `defwidget` (§7.3), whose
  errors carry the widget's name in place of `#'`.
- An instance ref's kind names its instance: `BindingKind::InstanceFloat(id)`
  (`:number :int :bool`) or `BindingKind::InstanceRgb(id)`; legacy refs are
  `BindingKind::Float`. The `ReactiveRef` shape is otherwise unchanged, and
  the namespace is the field's `%instance/<id>` DAG source.
- `:rgb`: the ref's `slot` is the r component, and the three component slots
  are keyed as elements 0..3 of the field (`ReactiveBindingStore::rgb_slots`).
  Read as a value it is `(rgb r g b)`. Widget consumers come in stage 5; a
  built-in float prop given one reads r.
- Slots are created by the first `#'` on a field, seeded from its value, and
  then written by every change of the field (Lisp `set!`, host
  `set_instance_field`, kind re-registration, rollback); a write that changes
  a slot queues a repaint of the widgets bound to it
  (`VM::take_pending_binding_repaints`; the runtime flushes it lazily when
  the dirty widget ids are read), never a re-render. The bookkeeping (bound
  fields, pending repaints) lives on the VM beside the slots, outside eval
  snapshots. Dropping an instance writes the defaults into its slots (held
  refs read the stale default) and removes them from the store. A stale
  instance's `#'` gets a detached slot.
- `:document` fields bind only while they are local cells; with the host's
  document natives (`__instance-doc-read`) they are an error until the host
  publishes their slots (stage 4).

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
  `step-cell: kind 'step' has no field 'activ'; bindable fields: active, playing,
  selected`.
- At render, an instance prop fills its uniforms from the fields' slots; a
  scalar prop takes a number or any ref (`#'x`, legacy `bind-seq`).
- `:bindable` is deleted: every SDF state accepts refs. It is still parsed and
  ignored during migration, then removed from content.
- `box :background "step-cell" :track t` passes instance props through to the
  background widget like any prop.
- Singleton fields (`transport.playing`) are readable from any shader without
  being passed in.

Built (stage 5):

- **Which names are instance state.** A `:state` name is instance state when
  the shader reads it dotted (`step.active`); a name read bare stays scalar
  even when a kind has that name, so legacy widgets with `:state (scene)`
  (`ui/transport.lisp`) or `(track)` (`effects/identified-drum.lisp`) keep
  working. Reading one name both ways is an error (`step-cell: :state 'step'
  is read both as a number (step) and as an instance (step.active)`).
  Dotted symbols are found after macro expansion, skipping qualified names
  (`m/f`) and heads a `let` binds (one walker, `sdf_codegen::
  walk_free_symbols`, serves this and `collect_state_symbols`); a nested
  path (`transport.scene.active`) is an error.
- **Resolution** happens when `defwidget` runs (`VM::plan_sdf_widget_state`,
  `lang/vm/widget_state.rs`): the kind with that exact id (`:state
  (alpha:dup)`, read `alpha:dup.on`), else the one kind of any key shape
  whose name part matches (`InstanceStore::kinds_named`, shared with keyed
  kind lookup). None is an error (`… reads a field of :state 'knob', but no
  kind is named 'knob' (define the kind before the widget)`); several list
  the candidates (`… names several kinds (alpha:dup, beta:dup); use its
  kind id`). Host kinds load with the root, before any view. A dotted head
  that is not a `:state` name resolves the same way and must be a singleton
  (`transport.playing`); a keyed kind there is an error (`… which is not a
  singleton; add track to :state and pass the instance`), and a head naming
  no kind is left to the shader compiler, as before.
- **Field errors** are `#'`'s (§7.1, the same check) with the widget's name
  in front: `step-cell: kind 'eseq.kinds:step' has no field 'activ';
  bindable fields: index, active, playing, selected`, `step-cell: field
  'name' of kind 'eseq.kinds:track' is :string, which is not bindable;
  bindable fields: …` (also built-ins, and `:document` fields while the
  host stores them).
- **Errors.** The plan's, the budget's, a name collision and a shader
  codegen error (`<widget>: shader error: …`) are evaluation errors
  (`VMError::Instance`); the widget is not registered (eseq-0l17.25,
  `a_shader_that_does_not_compile_is_a_defwidget_error`). The usual cause is
  a material macro whose module is not loaded yet (`eseq.materials/color`):
  `:shader` and `:material` bodies expand outside their module, so a file
  that calls another module's macro must `(import …)` it.
  `ui/sequencer.lisp`, `ui/step-grid.lisp` and `ui/legacy/mixer.lisp`
  (and `ui/effects/param-grid.lisp`, eseq-0l17.61) import `eseq.materials`, so
  `rec-arm-dot` and `seqv-rec-arm-dot` compile in any boot order
  (`legacy_mixer_definitions_are_top_level_and_source_loads`,
  `metal_seq_main_import_block_boots_in_reverse_order`).
  `content_shader_corpus_emits_valid_wgsl` checks every content file that
  calls a corpus macro of another module imports it (its
  `PENDING_MACRO_IMPORTS` lists the `:material`-only stragglers; empty
  since eseq-0l17.74: its last entry, `ui/effects/track-panels.lisp`, calls
  no `eseq.materials` macro any more, so it needs no import).
- **Uniform layout.** `SdfWidgetDef::state_uniforms` stays one float name per
  slot, in slot order: scalar states first (as `collect_state_symbols`
  finds them, captured `defstate`s included), then fields in first-read
  order, each once: `step.active`, and for `:rgb` three names
  `track.color|r`, `|g`, `|b` (`|` never appears in a symbol;
  `sdf_codegen::rgb_uniform_name`, read back by `rgb_uniform_base`).
  Shader identifiers map `.` to `__` (`sdf_state_step__active`); the codegen
  declares each `:rgb` field as a `float3 sdf_state_track__color` over its
  components, typed `float3` for inference. Two states that map to one
  identifier are an error naming both (`clash: state 'lane__color' and
  'lane.color' name the same shader uniform; rename one`; also
  `lane.color|r` against a `lane.color-r`). `SdfWidgetDef::state`
  (`SdfWidgetState`) holds the `:state` names, the plan (`SdfStatePlan`:
  scalars, and `SdfInstanceField`s — source `Prop(state)` or
  `Singleton(kind id)`, kind, field, uniform, rgb, and the field's
  `shader-state-*` prop names), the kind schema generation it was planned
  at, and the `shader-state-*` prop name of every uniform (filled at
  registration, so per-frame packing and hit testing format nothing).
- **`:rgb` in the shader.** `track.color` is a vec3: arithmetic works on it
  (`(* 0.5 track.color)`), `(rgba track.color a)` adds an alpha (MSL/WGSL
  `float4(float3, a)`), and in a color position (`sdf/fill`, `sdf/paint`,
  `sdf/stroke`, `sdf/stroke-px`, material and shadow `:color`) a `float3`
  is widened to an opaque color. Both `if` branches must agree, so mix it
  with a theme keyword through `rgba`.
- **Budget.** More than 16 floats (scalars plus fields) is an error listing
  the allocation: `too-many: shader state needs 17 floats, over the budget
  of 16: s0 1, …, lane.color 3`. No silent truncation is left: `defwidget`,
  `:material` sliders (error prefix `material`, printed, the slider drawn
  without its material, as for any material error) and `sdf->metal`
  (prefix `sdf->metal`, returned as its `"error: …"` string) all plan
  through `plan_sdf_widget_state` and check the budget, so singleton reads
  (`transport.playing`) work in materials and `sdf->metal` too. A
  material's state names are the slider's props; its cache key includes
  the plan, so a kind reload that changes the fields it reads recompiles
  it. `content_shader_corpus_emits_valid_wgsl` plans every content
  `defwidget` this way with the host kinds (`core/modules/kinds.lisp`)
  loaded, plus an instance-state fixture (`step.active`, `track.color`,
  `transport.playing`).
- **Render.** The constructor (`defwidget`'s, a `:material` slider's, and
  `box` for its `:background`) calls `VM::bind_sdf_widget_instance_fields`
  with the registered definition: per field, the instance (the prop named
  like the state, or the singleton; each source looked up and kind-checked
  once) gets `#'`'s bindings (`instance_field_refs`: one ref per slot, r, g
  and b for `:rgb`, with one store lock; slots created and seeded, cold
  `:host` fields read, observer epoch bumped; a field already bound skips
  the read) stored under the field's precomputed `shader-state-*` props.
  A stale instance gets detached slots, so nothing enters the store under
  a dead namespace. The widget holding the refs makes the fields observed
  (§9), so the host computes `step.playing` only while such a widget
  exists; the binding table maps the refs to the widget, so a field write
  repaints it and never re-runs the view. A missing or nil instance prop,
  or a dropped instance, leaves its uniforms at 0; an instance of another
  kind is an error (`step-cell: :step takes an instance of kind
  'eseq.kinds:step'; got <track#…>`). Scalar props are unchanged: a
  number, or any ref.
- **Kind hot reload.** The definition records
  `instance_kind_schema_generation()`; when it has moved, construction
  re-plans the widget against the current kinds. The same fields go on as
  before; any change to them (`lane.color` turned from `:rgb` to `:number`,
  a kind no longer a singleton) is an error, `step-cell: kind
  'scratch:lane' changed since defwidget; re-evaluate it`, never silent
  zeros. Re-evaluating the `defwidget` recompiles against the new kind.
- **`:bindable`** is parsed and ignored. `prop_accepts_binding` accepts a
  ref on a scalar state of an SDF widget, directly or as a `box
  :background`: a declared `:state` name that is not an instance head,
  read by the shader or not (`ui/mixer.lisp` binds `assigned` and
  `override`, which only the host reads), or a captured `defstate` the
  shader reads. An instance state takes an instance, so `(step-cell :cell
  #'x)` gets the widget diagnostic `step-cell: :cell does not accept
  reactive bindings`.
- **A ref on an undeclared prop** (eseq-0l17.68). A `defwidget` call that
  binds a ref to a prop that is neither a `:state` name nor a captured
  `defstate` the shader reads (a uniform name such as `:cell.active`
  included) is an evaluation error (`VMError::Instance`), also logged as
  `[lisp-error][<widget>]` (a view re-run's error is otherwise only
  traced): `lane-patch-port: :pending-port is bound to a ref, but
  'pending-port' is not in the defwidget's :state (active, output,
  selected); declare it there or pass a plain value`
  (`widgets::undeclared_sdf_binding_error`). It used to replace the widget
  with a diagnostic label that a port layout hid, so the widget silently
  vanished (every patch out port, until `pending-port` was declared). A
  plain value on an undeclared prop is still an ordinary prop.
- **`box :background`.** The `box` constructor binds the background widget's
  instance fields the same way, so `(box :background "step-cell" :step s
  :track t)` works; scalar states pass through as props, as before.
- Not typed: uniforms stay a `Vec<String>` with the `|r` convention rather
  than an enum through the codegen API (every `compile_sdf_*` entry point
  and the per-frame packing take names).

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

Built (stage 2):

- A ref reads itself through `VM::read_binding_ref`, which `reactive-value`
  also uses: an instance ref reads like `t.x` (typed, so a `:bool` binding is
  `true`/`false` and `:rgb` is `(rgb r g b)`) and records that field's
  dependency; a legacy ref reads its float slot and records the
  `(namespace, field)` dependency, as `reactive-value` always did.
- Read points: `JumpIfFalse` (so `if`/`and`/`or`/`when`), `Eq`, `Lt`/`Gt`/
  `Lte`/`Gte`, `Add`/`Sub`/`Mul`/`Div`/`Min`/`Max` (only on the non-number
  path), the target of `GetField` (a ref held in a local, then `.field`),
  the index of `LoadReactiveNth`, `filter`'s predicate result, and the
  native call boundary: the opcode call (owned and borrowing fast path) and
  `VM::invoke` (`map`, callbacks). Closures, `HostHandle`s and override
  dispatch pass refs through; the boundary is the native's.
- Ref-aware: `VM::register_ref_aware_native_with_vm` or
  `VM::mark_natives_ref_aware` (`Runtime::mark_natives_ref_aware` for natives
  registered through the runtime's wrappers); re-registering a native clears
  the flag. Marked: every built-in widget constructor, `defwidget`-generated
  and material slider constructors, `~slider`/`~knob`/`~toggle`/`~scope`/
  `~lane` and the inline target binder, `dict`, `ui/style`, `list`, `merge`,
  `cons`, `append`, `set-nth`, `bind`, `bind-seq`, `bind-nth`,
  `bind-seq-nth`, `bind-view-buffer`, `reactive-value`, `__field-ref`.
  Accessors (`get`, `nth`, `first`) are not: the boundary only looks at
  top-level arguments, so a ref inside a collection survives any native.
- `str` is not ref-aware (the list above had it print `<bind:…>`): used as a
  value a binding reads itself, so `(str "Vol " #'t.vol)` formats the
  value, as `fmt` does. The raw ref prints as `<bind:namespace.field>` only
  where the printer sees it without the call boundary (REPL echo,
  `source`).

## 9. Host side

**Publishing.** A structured push replaces `format!`-built field names,
generalizing `VM::set_instance_builtin_field` (formerly
`set_instance_host_field`; `owner`/`label` only):

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

Built (stage 4):

- **Observed bit:** `VM::host_field_observed(id, field)` /
  `Runtime::host_field_observed`: the field's `%instance/<id>` DAG source
  has dependents, or its bound slot's `Arc` strong count exceeds the store's
  own (`ReactiveBindingStore::binding_slots_held`). A hidden buffer's
  deferred effect still counts as a reader. The batched
  `host_fields_observed(id, &[fields]) -> ObservedMask` (bit `i` =
  `fields[i]`; `ObservedMask = u64` since eseq-0l17.71, so at most
  `MAX_OBSERVED_FIELDS` = 64 fields per call, asserted) formats the
  namespace once and locks the slot store once; the tick asks it once per
  instance. The host's `LiveFields`, `ObservedList` entries, observer
  caches and per-kind bit tables are `ObservedMask`s too, and
  `LiveFields::of` asserts a kind has at most 64 live fields. `instance_observer_epoch` moves whenever a field
  may have gained an observer (a tracked read, a handed-out `#'`), so a host
  can cache "nothing observes these" until it moves.
- **Cold reads.** The host skips unobserved fields, so a by-value read of
  one (a REPL `t.volume`, an event handler's `toggle!`, the first read that
  makes a field observed) would see a stale cell. `VM::set_host_field_reader`
  installs a `HostFieldReader` (`Fn(&mut VM, InstanceId, &str) ->
  Option<Value>`), called by `GetField` and by `#'` (seeding the slot) on a
  live instance's `:host` field nothing observes; a `Some` equal to the
  cell is not written, any other is type-checked and stored like a push.
  The reader may register keyed instances (lazy steps) but must not read
  fields through Lisp. The host's reader returns `None` at once for any
  field name that is not a live field, and reads `LiveSources` built once
  when it is installed.
- **Two feeds.** *Live* fields (the `Feed::Live` entries of `PUBLISHED`:
  track `volume muted audible armed selected peak num-steps steps`, step `active
  playing selected`, transport `playing recording`, `selection.track`) read
  shared state the UI thread reaches without the `App`: the tick computes
  them only while observed (`Pusher::push_live`: one batched observed query
  per instance, then compute and push the observed ones), the reader
  answers cold reads, and `KindsShared::computed` (keyed by `FieldKey`)
  counts every computation (an unobserved field never counts). Steps are
  dropped only when a track's length shrinks; their fields are diffed in
  place per track (the active mask every tick, the selected mask only when
  the current track, step selection or rack-wide target changed, the
  playing step) and only for tracks with step instances whose `steps` or
  some step field is observed (the latter cached per
  `instance_observer_epoch`); only observed changed fields are pushed.
  Playheads and meters cost no per-step work while nothing observes them.
  *Model* fields (names, colors, presets, device chains, indices, scenes,
  banks, transport scene, collections) need the `App`, which the reader
  cannot reach. The tick re-derives them for every registered instance
  only when a `ModelRevision` moves: UI, FX and FX-value epochs, the
  pattern epoch, song-row mirror and sound-binding epochs, the history
  revision (renames and every recorded edit), the scene revision and
  current scene, the track count, the registry generation and order, and
  the theme's track tint (mirroring `capture_param_sync_revision`; the
  legacy publishers are epoch-gated the same way). It also runs after the
  reader is installed, after a schema change, and when an instance it
  produced is gone (a hot reload). The transport's queued scene and launch
  quantization are compared every tick (a quantized launch moves no
  counter). `KindsShared::model_syncs` counts model runs.
- **Lazy steps (D2).** Step instances are keyed (track instance id, step
  index) and registered by the reader on a cold `t.steps` read, or by the
  tick while `steps` is observed (then the list follows `num-steps`). Every
  sync drops a track's step instances at or past its length; dropping a
  track drops its steps. A track nobody read `steps` of has none.

## 10. Diagnostics

- Every schema error names the kind, the field and the known fields.
- A failed `(import …)` / `(load …)` is logged as `[lisp-error][import]` /
  `[lisp-error][load]` with the errors the module queued (a compile
  error's message), besides its string value and the load-error queue
  (eseq-0l17.60). A host that evaluates its root with `eval_str` (the
  distro boot, `editor_setup.rs`) never drains that queue, so before this a
  module that failed to compile left only its missing definitions behind.
- Debug builds log the reason for each subtree re-render:
  `subtree track-3 re-rendered: track#41.muted read at view.lisp:212`.
  This makes an accidental by-value read in a hot view visible.
- `(describe-kind 'track)` prints the fields with group, type, `:set`,
  `:range` and `:doc`.

Built (eseq-0l17.23):

- **Re-render reason log.** `ESEQ_RERENDER_LOG=1` (any build) turns it on
  at startup and prints to stderr; `(rerender-log! true)` / `false` turns
  it on or off from Lisp and `(rerender-reasons)` returns the lines kept
  since the last call (the newest 512), oldest first. Each effect or
  subtree a changed source dirties logs one line, the source an instance
  field (`<track#41 [3]>.muted`), a constructor key (`(track 3)`) or a
  `namespace.field`, and where the effect read it (the running function,
  and its file when it has one):
  `[rerender] subtree :row-name (*row*): <track#41 [0]>.name read in row-label (view.lisp)`.
  Chunks carry no line table, so the site is the function, not a line. A
  read is located when it happens: with the log turned on from Lisp, an
  effect's reads before then are logged without a site until it re-runs.
  `#'` bindings repaint without re-running, so they never log (which is
  the point: a line in a hot view is a by-value read to turn into a
  binding). `VM::set_rerender_log` / `take_rerender_reasons` are the Rust
  side; the existing `ESEQ_SCENE_TRACE` `[mark-dirty]` trace is unchanged.
- **`(describe-kind 'k)`** returns the kind as text (the REPL echoes it):
  `k` is a kind id, or a bare name (the current module's kind first, else
  the one kind with that name; ambiguous or unknown names are errors). A
  first line with the kind id, how its instances come (`created (no
  :key)`, `as a singleton (:key ())`, `keyed (:key (index))`, `view-local
  (:key (scope section))`) and its built-in fields; `:keymap`, `:view` and
  `:on-create` when present; then one line per field, `:host` then
  `:state` then `:document`, with its type and options: `:set` (the
  setter's name), `:range`, `:doc` on `:host` fields, `:default` on the
  others:

  ```
  kind eseq.kinds:track: keyed (:key (index)); built-in fields: id, kind, key
    :host     volume         :number  :set seq-set-track-volume  :range (0 1)
    :state    open           :bool  :default false
  ```

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
4. **Host kinds.** `eseq.kinds` (`content/core/modules/kinds.lisp`),
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
   Built (stage 6): `docs/examples/mini-daw.lisp`, the reference example
   (the user's exp4 view ported; `host_kinds::tests::views::mini_daw_*` load it
   under the `-noui` root, scan it for legacy forms, string-built keys and
   re-bound step keys, check a playhead write repaints its step cell
   without re-rendering, and open an instrument panel in its top tile). The
   view holds layout and shaders only; the plumbing moved into libraries:
   - `eseq.step-grid-interactions`: `down` / `drag` / `up` / `double-click`
     (the main grid's step gestures over a step instance; each selects the
     step's track) and `(bind-step-keys)` (ESC / C-a / s-a / BS in widget
     views, rack-aware). Importing it binds no keys: the DAW root
     (`ui/main.lisp`) binds C-a and `.` itself, and the DAW-only calls are
     guarded with `(module-loaded? …)` (a new native) for `-noui`.
   - `eseq.kinds`: `clone-scene!`, `delete-scene!` (the host's
     `clone-pattern` / `delete-pattern` take an explicit scene `:idx`; the
     host makes it current itself, and a delete returns to the scene that
     was playing) and `step-preset!`; `track.audible` (a live field: false
     while muted or silenced by another track's or a bus's solo).
   - `eseq.effects`: `device-panel` (a device instance's panel data),
     `device-panel-body` (the factory body: instrument synth, a rack's
     selected slot, audio or MIDI effect, at `panel-height`),
     `rack-slot-select` and `panel-buffer` (the host publishes panels only
     while "*fx*" is visible).
   - eseqlisp: `subtree :key` takes an instance (`#<id>`) or a list of
     parts (`(list :preset t)`); `context-menu :anchor` takes a point
     (`e.at`; nil keeps the menu hidden); `label` / `number-label` `:active`
     accept a Lisp bool.
   - eseqlisp (eseq-0l17.26): `grid` takes `:gap`, and `:col-gap` /
     `:row-gap` overriding it per axis (default 0): a gap sits between
     slots only, so a grid measures `cols * col-width + (cols - 1) *
     col-gap` wide (likewise in rows), and an auto-`:cols` grid fits
     columns with their gaps. The example's step grid is `(grid :cols 8
     :col-width (sc 8) :col-gap 1 :row-height (sc 4) …)` and a `(sc 3)`
     spacer, not a `(+ (sc 8) 1)` column and a `(- (sc 3) 1)` spacer; its
     layout is the same cell for cell (only the grid's own rect loses the
     trailing gap, which the spacer now holds). `:col-gap`, not `:gap`: a
     `:gap 1` would also part its two rows, which touched. No factory view
     outside `ui/effects/*` hand-compensated a grid gap (`ui/mixer.lisp`'s
     pattern cells are inset in their columns, not gapped).
   - eseq-0l17.27: the scene menu's missing focus highlight (the capture
     opens it from `capture-after-sync`, then makes `*sequencer*` the
     active tile) was not the subtree: only the active tile's widget focus
     followed relayouts, so once the menu's tile (`*fx*`) went inactive and
     was laid out afresh, its leaf kept a widget id the new layout gave
     another node (800024 vs 800019) and no item painted focused. An
     inactive tile's relayout now remaps the leaf's focus the way the
     active tile's does (`widget_focus::remap_leaf_focus_to_layout`: stable
     widget id, stable key and type, subtree root and type, else the same
     id with the same identity; a widget that is gone leaves it unfocused).
   - `metal_seq capture --noui` (bare root) with a host-kinds sync
     (`HostKinds::sync_with` over `KindsHandles`), so kinds views render
     headlessly.
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
   Built (stage 7, eseq-0l17.10): the inventory (§14), and `bus`, `group`,
   `send`, `master`, `engine` plus the new `track`, `step`, `transport`,
   `selection` and `project` fields (§14.2). The rest of the kinds (`param`,
   `lane`, `clip`, `note`, `graph-node`, browser and editor singletons, rack
   pads/clips, track settings and routing) are follow-up beads
   eseq-0l17.28–.35 (§14.3), which the port beads depend on.
   Built (stage 7b, eseq-0l17.28): `param` under `device`, the step
   p-lock render and send lock flags (§14.2, "Built (7b)").
   Built (stage 7d, eseq-0l17.30): the arrangement: `song` and `region`
   singletons, `scene-span`, `clip`, `cell`, `track.governed` / `latched`
   (§14.2d).
   Built (stage 7i, eseq-0l17.35): track settings, routing (`route`, bus
   outputs, mod port levels), option constants, selection, transport and
   engine extras (§14.2c).
   Built (stage 7h, eseq-0l17.34): drum racks: `pad`, `rack-clip`,
   `groove`, `pad-groove`, `pool-groove`, `library-groove`, the group's
   rack fields and `track.pad` (§14.2e).
   Built (stage 7b-2, eseq-0l17.36): devices beyond the track chain (MIDI
   effects, bus effects, drum rack slots and their effects) with their
   params, `device.voices` and `device.delete-target` (§14.2f).
   Built (stage 7b-3, eseq-0l17.37): the device panel extras: param
   placement, modulation lanes and display, process mapping, key locks,
   the base note, tensors, p-lock variants, project and rack macros, the
   neural selection's override (§14.2g).
   Built (stage 7c, eseq-0l17.29): process lanes: a track's processes
   (`process`), their lanes, inlets, ports, fan-out entries and state
   cells, the process library (`process-class`, `process-library`) and
   `track.processes` / `track.lanes` (§14.2h).
   Built (stage 7f, eseq-0l17.32): the browser, the sound palette, the
   editor and the app's views: `browser` (with `preset-file`,
   `slot-presets`), `sound-palette` and `sound`, `editor` (with
   `editor-macro`, `editor-asset`, `asset-info`), `learn` (with its plan,
   epoch and result rows), `retro` (with `retro-lane`, `retro-item`),
   `song-export`, `settings` (with `midi-device`), `agent`, `project.name`,
   `project.audio-workers-options` and `track.instrument-id` (§14.2i).
   Built (stage 7e, eseq-0l17.31): the piano roll: the `piano-roll`
   singleton (the current track's edit focus, its pinned clip and loop
   window, its playhead) and its `note`s, with note setters, script note
   drags and `add-note!` / `delete-notes!`; the tracker's lock cells
   (`param.step-locks`, `rack-macro.step-locks`) (§14.2j).
   Built (stage 7e-2, eseq-0l17.47): the piano roll's focus steps
   (`piano-roll.steps` → `focus-step`, the source's steps on its axis with
   their step params and setters), so the automation lane is a view
   (§14.2r).
8. **Factory port**, one area at a time, each removing that area's legacy
   field names: sequencer grid and step editing; transport, scenes and
   banks; mixer; effect and instrument panels (incl. custom-ui runtime,
   physical-model/drum/MnM surfaces, param controls); arrangement; piano
   roll; browser, sample import and resample; patching and macros; packages
   (`alez.tracker`, `alez.neural`, `alez.jaki`) and the sequencer demo
   scripts; factory instrument/effect `ui.lisp` files. Then delete
   `bind-seq`/`bind-seq-nth`/`:bindable` support and the legacy publishers.
   Each port follows the playbook (§13.1).
   Built (stage 8, eseq-0l17.12): transport, scenes and banks
   (`ui/transport.lisp`, `ui/scene-banks.lisp`) and the MIDI capture
   (`ui/retrospective.lisp`), the first port, which set the playbook:
   - **Kinds.** `bank.bid` and `bank.name` (the bank commands' id and the
     rename draft), a `:set` on `transport.launch-quantize` (host command
     `set-scene-launch-quantize`). A project load (a new track registry
     generation) now drops scene and bank instances with the tracks: their
     ids restart with the project, and a view holding the bank it shows
     (`scene-bank-view.bank`) sees it go stale and shows the loaded
     project's playing bank again. That replaces
     `SEQ.scene-bank-view-generation`.
   - **View state** (all `:key ()` `:state` singletons): `scene-bank-view`
     (`eseq.scene-banks`: the shown bank instance, its last `index` and
     an `other` bank listed beside it; `pending` = the bank count when
     New bank was picked). A shown bank no longer listed falls back to the
     bank at its last index, clamped (scene-banks spec §4: a delete or an
     undo), while `other` is still listed; when `other` went too (a project
     load replaces every bank instance) or nothing was shown yet, to the
     playing scene's bank. In `eseq.transport`: `bank-ops-menu`,
     `bank-rename` (`bank`, non-nil while renaming, and the draft),
     `scene-bank-menu` (its scene is an instance), `transpose-menu` (its
     bank an instance), `app-menu` (the open menu's id and its point) and
     `scene-push` (the push gesture; the target stays a scene index, the
     commands' address; `reset-scene-push!` ends it). Menus open with
     `(open-menu! m event)` at `event.at` (every pointer event carries its
     grid point as `:at`) and render with `(menu-of m items…)`.
     `eseq.retrospective`: `retro-crop` (open, start, end, bars, bpm,
     requested-bars), `retro-view` (the roll's scroll and zoom) and
     `roll-rows` (the timeline's rows with the `retro.lanes` /
     `retro.items` lists they were built from: rebuilt only when those
     change, never on scroll or zoom). `scene-transpose` stays a
     `defscene` (saved per scene, not view state). Fixtures and Rust tests
     reach a singleton through a local: `(let ((p
     eseq.transport/scene-push)) (set! p.target 0))` (a qualified name
     takes no dotted field).
   - **Bindings.** Icons and pills bind (`:active #'transport.playing`,
     `#'master.recording`, `#'song.manual-latch`, `#'s.active` per scene
     pill), the clock binds `#'transport.position` and `#'song.cursor` /
     `#'song.position`, the meters `#'master.peak-l` / `peak-r`, the
     readouts `#'engine.cpu-load` / `latency-ms`, and the MET / ROLL / WAV
     / cpu labels `:active` (a `label`'s `:active-color` replaces the
     value-chosen `:color`; a scene pill's number too, valued only while
     queued, so a launch only repaints). The push gesture binds
     `#'scene-push.value` on the pills and the strip. The capture's roll
     binds `#'retro.playhead`. Values stay where Lisp decides (the pill's
     background by `s.queued`, the roll label by `transport.roll-mode`).
     Clicks write through setters: `(toggle! transport.playing)`,
     `recording`, `master.recording`, `metronome`, `roll-mode`, `(set!
     transport.bpm (floor v))`, `(set! song.manual-latch false)`; a pill
     launches with `(launch! s)` (eseq.kinds; quantized by
     `transport.launch-quantize`, "off" until published) and `-` is
     `(delete-scene! transport.scene)`; the other scene and bank commands
     stay host commands addressed by `s.index` / `b.bid`, sent only while
     the held scene or bank is still listed (`listed?`, exported by
     `eseq.scene-banks`). `eseq.kinds` exports `launch-quantize-options`
     and `record-quantize-options`; content/core/init.lisp gains `unless`
     beside `when`. The dead `toggle-metronome` host command is gone
     (`set-metronome` is `transport.metronome`'s setter).
   - **Legacy removed:** `SEQ.cpu-load-pct`, `cpu-overloaded`,
     `output-latency-ms`, `master-recording`, `metronome`,
     `record-quantize`, `roll-mode`, `roll-rate`, `sequence-rolling`,
     `queued-scene`, `transport-playhead`, `song-cursor-beats`,
     `song-manual-latch`, `scene-banks`, `scene-bank-view-generation` and
     all of `RETRO` (registration, the mirror, the live audition fields;
     the capture area is `unmirrored`). Kept, still read by unported
     areas (eseq-0l17.22's list): `SEQ.playing`, `recording`, `bpm`,
     `current-pattern`, `num-patterns`, `master-peak-l` / `-r` (removed by .13),
     `scene-launch-quantize` (also the host kinds' source for
     `transport.launch-quantize`), `song-mode`, `song-position-beats`.
   - **Tests.** `host_kinds::tests::transport` (Distro root): the ported
     files use no legacy form (`views::legacy_forms`, the mini-DAW
     scanner extended), the transport binds its host state through kinds
     and only repaints on playback and on a scene launch, a loaded project
     shows its playing scene's bank (twice, the same file), and an undo
     that removes the viewed bank shows the previous one. The capture's
     roll zooms to the duration `open` is passed (asserted with no tick). The editor tests without
     a host (`full_grid_editor_for_scroll_tests`) seed kinds with
     `set_kind_field` / `seed_kind_scene_banks` (state_values tests) and
     `sync_retro_kind` (the capture tests) instead of `SEQ` fields.
   Built (stage 8, eseq-0l17.13): the mixer (`ui/mixer.lisp`), its
   unloaded predecessor (`ui/legacy/mixer.lisp`) and the MIDImix map
   (`ui/midi-midimix.lisp`):
   - **Kinds.** `track.in-selection :bool` (L): the current track or one
     of `selection.tracks`, the strip highlight (legacy `track-selected-N`);
     `group.delete-target :bool` (L) [`set-group-delete-target`: true
     arms the group, false clears it when it is armed]; `send.process-mapped
     :bool` and `send.process-value :number` (L): an enabled process slot of
     the track writes the send, and the level it last wrote (the displayed
     level until one has), the send twin of `param.process-mapped` /
     `process-value` (the bound set cached per track under its `PlockKey`,
     next to `process_bound` in `host_kinds/panel.rs`; the scheduler's
     writes copied into `KindsShared` only when
     `process_effective_params_version` moves). `send.has-locks` reads a
     per-track set of locked buses cached under the `PlockKey` and step
     count; the tick reads `selection.tracks` and each track's display step
     once (`TickMemo`, for `in-selection`, `display`, `locked`,
     `process-value`) and pushes `group.delete-target` only when the
     delete target's version (or the observers) moved.
   - **Strips take instances.** `track-strip`, `track-collapsed-strip`,
     `bus-strip`, `group-container`, `patch-mixer-strip`, the menus and the
     renames take track / bus / group / cell / rack-clip instances; the
     drop handlers keep the host's drop meta (track and bus positions, the
     add/drop commands' address). `render-order` lists `(dict :kind "loose"
     :track t)` and `(dict :kind "group" :group g)`; `display-buses` the
     buses with the main mix last; `group-bus?` and `main-bus` replace the
     bus-id helpers (the MIDImix map follows them). `track-color-r/g/b` and
     `track-collapsed-label` keep a position argument for the unported track
     panel header.
   - **View state** (`:key ()` singletons, exported): `strip-menu` (open,
     at, the track or group), `strip-rename` (the track or group, the
     draft: one rename at a time), `strip-select` (the range anchor, a
     track) and `mod-patch` (the cable's source track). Menus open with
     `eseq.view-kit`'s `open-menu!` / `menu-of`; held instances (a menu's
     target, the range anchor) are checked `listed?` before use. Starting
     a rename commits one in progress first (its input blurs only after
     the rebuild).
   - **Shape.** One frame for the three track strips (`track-frame`, with
     `track-strip-props` for the mixer's drop target and clicks), one
     click handler (`track-click event t plain`), one strip menu and
     rename (`open-strip-menu` / `begin-rename` / `finish-rename`, track
     or group), `strip-button` with its unlit look (`:bg`,
     `:idle-color`; the inverted bus mute), `main-bus?` (bus id 0) for
     every main mix check (`display-buses` puts it last wherever it sits),
     the clip grid and the mod port rows in keyed subtrees (a launch or a
     patch re-renders them alone), the output dropdown naming its buses
     from one shared-name read.
   - **Bindings.** Faders `#'t.volume` / `#'b.volume`, pan `#'t.pan`,
     meters `#'t.peak`, `#'b.peak`, `#'master.peak-l/-r` (the main mix),
     mute / solo / arm `#'t.audible`, `#'t.soloed`, `#'t.armed`,
     `#'b.muted`, `#'b.soloed`, `#'g.armed`, highlights
     `#'t.in-selection`, `#'t.delete-target`, `#'g.delete-target`, mod
     ports `#'t.mod-out-level` and `(mod-in-level x i)`, sends
     `#'s.display` / `#'s.locked` / `#'s.amount` (and `#'s.process-value`
     while mapped), clip cells `#'c.active` / `assigned` / `override` /
     `selected`, rack clip glyphs `#'rc.active`. Values stay where Lisp
     decides: the clip grid (`t.cells`, `c.banks`, `c.queued` for the
     blinking background), output dropdowns (`t.output`, `b.output`,
     `project.output-options`), routes (`(routes)`, `r.selected` for the
     cable lists), the rack clip column's follow row (`g.rack-clip.index`,
     a subtree of its own, replacing the `SEQV.rack-clip-center-*`
     observer). Setters: `toggle!` mute / solo / arm, `set!` volume, pan,
     outputs, `selection.track`, `r.selected`, `g.delete-target`,
     `g.collapsed`, `d.voices` (a rack slot's, in the patch mixer);
     actions `launch-cell!`, `launch-rack-clip!`, `save-rack-clip-as!`,
     `convert-rack-to-clips!`. The send knob keeps `set-track-bus-send`
     (it p-locks the selected steps of the current track, which `set!
     s.amount` never does). A cell launch arms the cell with `set!
     c.selected true`: the delete-target setters (`cell.selected`,
     `route.selected`, `group.delete-target`, through `eseq.kinds`'
     `set-delete-target`) touch only UI-thread state and apply at once, so
     BS or clone right after the click act on the cell (the `set-cell`
     host command is gone). `eseq.track-collapse`
     gains `instrument-icon` and `replaceable-type?` (by `t.instrument-type`);
     `eseq.scene-banks/clip-in-viewed-bank?` takes a cell and a bank.
   - **`eseq.view-kit`** (`ui/view-kit.lisp`, new): the side-effect-free
     helpers the views shared by copy: `open-menu!`, `menu-of`, `nothing`
     (a zero box), `listed?`, `index-of`, `prop-if` (`(k v)` to splice
     into props when v is set). The transport, the retrospective view,
     the mixer and the drum rack lookups use it; `eseq.scene-banks` keeps
     `listed?` as a compat re-export. Not `ui/menus.lisp`: importing that
     registers menus.
   - **Legacy removed:** `SEQ.master-peak-l/-r`, `mod-routes`,
     `selected-mod-routes`, `track-mod-output-available`, `track-outputs`,
     `track-output-options`, `bus-output-routes`, `track-bus-sends`,
     `queued-track-clips`, `graph-sequencers`, `track-mixer-pans`,
     `track-N-pan`, `track-N-bus-M-send` with its `-plock-any/-active/
     -default` and `-proc-mapped/-proc-value`, `mixer-track-delete-target-N`,
     `track-pattern-cell-assigned/override/selected-*`, `mod-in-level-*`,
     `mod-out-level-*`, `bus-mod-in-level-*`, and `SEQV.rack-clip-center-*`
     (with their builders, frame state and the legacy-only tests). Kept,
     still read by unported areas (eseq-0l17.22): `SEQ.groups`,
     `bus-names`, `bus-ids`, `bus-mutes/solos/volumes`, `bus-peak-*`,
     `track-peak-*`, `track-N-volume`, `track-volumes`, `track-mutes`,
     `track-solos`, `track-muted-effective`, `record-armed`,
     `track-selected-*`, `selected-tracks`, `track-names`, `track-colors`,
     `track-instrument-types`, `track-ids`, `num-tracks`, `current-track`,
     `track-pattern-cells`, `track-pattern-cell-active-*`, `rack-clips`,
     `rack-clip-active-*`, `armed-rack-id`, `tp-*` (incl. `tp-bus-M-send`),
     `delete-target-version`, `instances`, `scene-launch-quantize`,
     `current-pattern`.
   - **Tests.** `host_kinds::tests::mixer_view` (Distro root): the ported
     files use no legacy form, the mixer binds its host state through kinds
     and only repaints while mixing and metering, the selection highlight
     and the group delete target, a send's process value. The host-less
     editor (`full_grid_editor_for_scroll_tests`) publishes its tracks,
     buses and groups as kinds (`seed_kind_tracks` with `KindTrack`,
     `seed_kind_buses`, `seed_kind_groups`, `seed_kind_rack_clips`,
     `kind_cell`; `set_full_grid_track_count` and `apply_groups_bindings`
     seed them too; `kind_track`, `select_kind_tracks`, `kind_singleton_rt`
     and `instance_or_nil` read and write them; `KindTrack` builds with
     `.instrument()` / `.collapsed()`); the Distro-root view tests share
     `instance_bindings` and `distro()` (`host_kinds::tests::views`) and
     the process fixtures `one_slot_chain` (`host_kinds::tests`); `solo_binding_tests::mute_and_solo_only_repaint_through_kinds` (since .11 also the sequencer grid and the arrangement headers)
     asserts the mixer and patch mixer repaint without re-running for
     mute, solo, audibility, arm, selection, delete target, faders and
     meters; the MIDImix tests publish their topology as kinds.
   Built (stage 8, eseq-0l17.15): the arrangement (`ui/arrangement.lisp`):
   - **Kinds read.** The song is `song` (`end`, `mode`, `position`,
     `scene-latched`, `edit-error`, `region` → `region`, `bound-clip`,
     `spans` → `scene-span`, `pending` and `pending-head` / `-lanes` /
     `-scenes` / `-launches`), each lane `t.clips` (`clip`: `cid`, `start`,
     `end`, `cell.pid`, `take`, `offset`, `num-steps`, `length`,
     `note-dots`), `t.color`, `t.latched`, `t.cells` (placement, with
     each `c.active` read for invalidation), `(scenes)` / `s.name`,
     `transport.scene` and `selection.track`. One field was added:
     `clip.note-dots`, the clip's notes flattened to the timeline's dots
     (the view's `clip-content` flattening, moved to the host:
     `host_kinds::arrangement::clip_note_dots`, a pattern clip's built
     once per (track, source) with the source cache, a take clip's kept
     per clip under its window), so a lane re-render (a recording lane
     re-renders every frame) flattens no committed clip; the provisional
     items still flatten in the view (`windowed-dots`, the same
     arithmetic).
     Lanes stay addressed by track position (the timeline commands', the
     sequencer header's and `visible-track-indices`' address; `(track i)`
     is the lane's track); rows are subtrees keyed `arr-track-<tid>` (the
     header), each lane a subtree nested in its row (`arr-lane-<tid>`), so
     a lane re-render leaves the header alone; the first visible track is
     found once by the rows' parent and passed in. The scene row's placement
     toolbar, starting scene, hint and scene lane, the lane sync and the song
     end (`arr-view.content-length`, which moves with playback past the end)
     are subtrees of their own.
   - **Bindings.** Every playhead `#'song.position`; the time axis
     `#'arr-view.start` / `duration` / `content-length`, the cursor
     `#'arr-view.cursor-time` (the scene lane's marker included, no longer a
     by-value read). The per-lane channels (`SEQV.arr-<channel>-<i>`, 11
     floats × 128 lanes seeded at load) are four view singletons every
     track lane binds the same fields of: `arr-select` (`lane`, `clip`: the
     click selection), `arr-lanes` (`bound-lane` / `bound-clip`,
     `region-on` / `region-lane-a` / `-b` / `region-a` / `-b`, written by
     the `sync-lanes` subtree from `song.bound-clip` and `song.region`) and
     `arr-drag` (`ghost` (the kind), `clip`, `time`, `region-a` / `-b`,
     `lane-a` / `-b`). The timeline widget gained **lane ownership**
     (`crates/eseqlisp/src/widget_render/timeline.rs`, `lane_owns`): a lane
     passes `:lane-key` (its position) and per channel the owner —
     `:cursor-lane`, `:selected-lane`, `:bound-lane`, the ranges
     `:ghost-lane-a` / `-b` and `:region-lane-a` / `-b` (inclusive, either
     order) — and draws the channel only on the owning lane(s); without
     `:lane-key` or the owner props nothing changes (the scene lane, the
     piano roll). A write repaints every bound lane (the legacy channels
     repainted only the written lanes; the region and cursor publishes
     already wrote every visible lane), never re-runs one.
   - **View state** (`:key ()` singletons, exported): `arr-view` (axis and
     cursor), `arr-select` (also `scenes`, `rect`, `clips`, the
     defstate / def selection), `arr-lanes`, `arr-drag` (also the scene
     ghost `scene`, the track drag `track` and the sweep `region`, read by
     no render), `arr-placement` (`active`, and `choice`: the cell the
     dropdown picked, replacing the (track id, pattern id) pair) and the
     menus `arr-scene-menu` (`open`, `at`, `time`, `span`) and
     `arr-pattern-menu` (`open`, `at`, `track`, `source`, `time`, `clip`),
     through `eseq.view-kit`'s `open-menu!` / `menu-of` (a menu's items
     spread with `apply`: `menu-of` passes one list, and a list nested in
     it is not flattened). `kind` and `id` are built-in fields: the ghost
     kind is `arr-drag.ghost`, its clip `arr-drag.clip`. Held instances are
     checked `listed?` before use (a menu's span, clip and track, the
     placement's track; the scene menu also keeps the span's start beat,
     `span-start`, and acts only while the span still starts there, since
     spans are positional). `lane-ghost` joins `lane-region-rect` and
     `lane-selection` as the test surface over the bound fields.
   - **Setters.** A clip click is `(set! song.bound-clip c)` (binds and
     selects the clip's span as the region, as `seq-song-select-clip`
     did, also when the clip is already bound: a binding from a capture
     commit or with the region cleared still gets its region; the status
     line reports "Bound: …"), a release `(set! song.bound-clip nil)`; regions `select-region!`
     / `clear-region!` (tracks as instances); Change Pattern `(set! c.cell
     cell)` (was `arrangement-clip-set-source`). Kept host commands: the
     `seq-arrangement-action` translator (every gesture, one primitive),
     `seq-song-set-arr-cursor` (`song.cursor` has no track), the region
     clipboard natives, the scene commands, `arrangement-pattern-place`,
     `seq-arrangement-empty-take-create` and the `seq-arrangement-pattern`
     preview query.
   - **Fixes found on the way.** `arrangement-pattern-place` checked
     `:track-id` against the track's pan node id (`SEQ.track-ids`), while
     the mixer's pattern drags carry `t.tid` since .13, so a mixer →
     arrangement pattern drop never matched the lane's `track-pattern-<id>`
     drop type and placement failed with "The target track changed": the
     command now takes the `TrackId` (`live_track_index`), the lane's drop
     type is `track-pattern-<tid>` (this fixed the known failure
     `metal_seq_arrangement_pattern_placement_controls_and_dispatch`, whose
     last assertion is corrected: placement is sticky). The capture setup
     applies the arrangement kinds' selection setters
     (`apply_capture_selection_command`: `set-song-region` and `set-song`'s
     non-edit fields); the legacy `song-select-clip` / `song-set-region`
     were silently dropped there (transport-class commands), so a fixture
     that selected a clip or a region rendered without it. Three capture
     fixtures set view state that never reached the widgets (the
     `view-start` / `view-duration` defstates without the SEQV publish, and
     a track-resize ghost in the scene ghost): they now set `arr-view` and
     drive `track-action` (`arrangement-resize-offset`,
     `arrangement-zoomed-out-grid`), and the menu fixtures pass `:at`.
   - **Legacy removed:** `SEQ.song-lanes`, `scene-spans`,
     `song-lane-events`, `song-clip-sounds`, `song-pending` (with
     `sync_song_pending` and `build_song_pending_value`; §14.2o),
     `scene-names`, `song-mode`, `song-end-beat`, `song-position-beats`,
     `song-edit-error`, `song-track-latched`, `song-scene-latched`,
     `song-bound-clip`, `song-region`, `track-pattern-cells`,
     `track-pattern-cell-active-*` and the unread `track-active-pattern-ids`
     (with their builders, `SongFrameState`'s lane, scene-name, event and
     pending caches, `sync_song_state`'s visibility argument and
     `sync_sound_palette`'s runtime and visibility arguments), every
     `SEQV.arr-*` channel, the alias rows of the removed names. The
     arrangement-region Backspace path (`input.rs`) reads `song.region` /
     `song.bound-clip` from the singleton's cells. Kept, still read by unported areas (eseq-0l17.22):
     `SEQ.current-pattern` (macros, scripts), `track-colors` (sequencer +),
     `track-ids` (sequencer, tracker), `current-track` (many),
     `song-track-governed` (sequencer), `THEME.scene_clip_bg` (theme); and
     the orphan song scalars no content reads (`song-exists`,
     `song-recording-kind`, `song-current-row(-id)`, `song-row-count`,
     `song-loop-enabled`, `song-capture-failed` / `-error`); the natives
     `seq-song-select-clip`, `-deselect-clip`, `-set-region`,
     `-clear-region` and `seq-arrangement-clip-set-source` (script API, no
     factory caller now).
   - **Tests.** `host_kinds::tests::arrangement_view` (Distro root): the
     file uses no legacy form; the lanes bind `song.position` and the four
     view singletons; a title-bar click binds the clip and lights only its
     lane, a scene click releases both; a click on the bound clip with no
     region selects its span; `note-dots` match the view's flattening and
     follow a step edit; a capture in flight draws inert
     provisional items that go with the capture; the edit-error banner.
     The host-less editor tests seed the song as kinds (`KindClip`,
     `seed_kind_clips` / `cells` / `spans` / `song`, `set_kind_region` /
     `bound_clip`) and read the setters' commands (`bound_clip_writes`,
     `region_writes`); the legacy publisher tests became model checks
     (`pending_surface`) or kinds tests
     (`a_step_edit_refreshes_clip_events_without_a_song_edit`), the
     legacy-parity halves of `host_kinds::tests::arrangement` / `pending`
     compare with the model. The timeline's lane ownership is in
     `bound_ghost_channels_transform_items_like_the_lisp_projection`. The
     baseline and after captures (every arrangement fixture, plus scratch
     ones for regions, a bound clip, track and scene selection, cursor,
     loop, zoom and scroll, move / resize / marquee ghosts, scene ghosts,
     placement and the pattern menu) are byte-identical but for the two
     fixtures whose view state now reaches the widgets (identical to the
     intended state rendered on the legacy view) and the track-select
     capture, which now shows the clip's region (the capture applies the
     selection).
   Built (stage 8, eseq-0l17.17): the browser (`ui/browser.lisp`), the
   sample import modal (`ui/sample-import.lisp`), resample
   (`ui/resample.lisp`) and the sound palette (`ui/sound-palette.lisp`):
   - **Kinds.** `project.instances (list-of :any)`: the package instances
     as the Packages tree's `seq-package-tree` takes them (dicts `:id :kind
     :label :owner-label :owner-rack :registered?`, the legacy
     `SEQ.instances` rows), pushed when their fingerprint moves
     (`instances_value_if_changed`, compared in
     `host_kinds/presentation.rs` like `project.name`, but only while
     observed: the fingerprint resets while nothing reads the field, so
     the first observing tick pushes and a first render of the Packages
     tab may show the previous rows for one tick). Everything else was
     built by 7f: `browser.*` (the sidebar, `rack-slots` → `slot-presets`,
     `sound-presets` / `kit-presets` → `preset-file`, `engines`,
     `library-epoch`, `preview-playing` / `preview-position`), `editor.*`,
     `sound-palette` / `sound`, `project.name`, `track.instrument-id`,
     `song.bound-clip` and `clip.take` / `clip.cell` (open a clip's sound).
     `present-fixture` is unchanged (the edit session republishes the
     editor, so editor states are covered by tests, not captures).
   - **View state** (all `:key ()` singletons; no `defstate` or `(state …)`
     is left in the four files): in `eseq.browser`, `browser-view` (tab,
     mode, the tab's and the preset list's search), `sample-pick` (tag and
     origin filters, the selected and auditioned sample, the shown track
     instance and sample the filters were last reset for),
     `sample-preview` (path, buffer, auto), `instrument-pick` (tier filter,
     favorites, the highlighted instrument and custom effect, the
     instrument loading), `editor-draft` (Save-as name), `kit-save` (open,
     name, group id, scene indices: the save command's address),
     `preset-save`, `package-menu` / `instrument-menu` (open, at, the row),
     `package-draft` and `instance-rename` (its target is the instance id
     the rename command takes). `eseq.sample-import`: `import-view` (open,
     generation, the drafts) and `import-preview`. `eseq.resample`:
     `resample-view`, which also holds the print the host hands `open`
     (buffer and duration, passed as arguments: the legacy `RESAMPLE`
     namespace, which the §14 inventory missed, is gone) and the last error
     (`show-error`, which the host invokes on a failed command).
     `eseq.sound-palette`: `sound-rename` (the sound, the draft). The
     former `eseq.vanilla` host protocol (`sbrowser-tab`,
     `sbrowser-loading-instrument-name`, `sbrowser-editor-name`, written by
     name from Rust) is the singletons now: the host calls
     `(eseq.browser/show-loading! name)`, `(eseq.browser/clear-editor-name!)`,
     `(eseq.browser/show-browser-tab! name)` (the tab, its searches kept:
     `select-tab` clears them) and `(eseq.browser/mark-auditioned! path)`;
     `ui/mixer.lisp` and `ui/sequencer.lisp` call `show-loading!` too.
   - **Bindings.** The preview strips (browser, sample import) and the
     resample waveform bind `#'browser.preview-position`; resample's play
     icon `#'browser.preview-playing`. Values stay where Lisp decides
     (`browser.preview-playing` gates the resample playhead and the stop
     calls; `editor.*` picks the editor header; the sidebar's fields pick
     the tab and the presets). Tracks: drop handlers keep the host's drop
     meta (a track position) and look the track up (`track-at`) for its
     `instrument-type`; the current track is `selection.track` (`t.index`
     addresses the commands). The selected rack slot's presets come from
     `browser.rack-slots`, the slot whose `device.delete-target` is set
     (replacing `seq-delete-target?` + `SEQ.delete-target-version`). Kit
     export lists `(scenes)`; rack menus `(groups)` (`g.rack`, `g.gid`).
     The rack check (`rack-panel-open?`: the Layer action and a modified
     activation add the sample as a layer of the shown rack) reads
     `browser.track.rack` alone: `browser.instrument-kind` is never "rack"
     (the legacy `SEQ.sidebar-kind` never was either, so the check was
     dead before the port). The Instrument Rack builtin always adds a new
     track (`add-new-layer-rack-track`). The preview strips share one
     implementation taking the preview singleton (`sync-preview!`,
     `toggle-preview!`, `preview-strip`, `stop-preview` and the headphone
     widget, in `eseq.preview-strip` (`ui/preview-strip.lisp`: importing
     the browser from the import modal would load it into harnesses that
     only load the sequencer); the import modal passes `import-preview`).
     Setters: `(set! editor.run-mode …)` replaces the run-mode command;
     the palette's rename is `(set! s.name draft)`, apply / fork / open /
     close are `apply-sound!`, `fork-sound!`, `open-sound-palette!` (a clip's
     take or pattern: `c.take`, `c.cell.pid`), `close-sound-palette!`.
     `eseq.view-kit` gains `rgb-part` and `color-rgba` (moved from the
     mixer, which imports them).
   - **Fixes found on the way.** `SEQ.current-pNroject-name` (a typo:
     unregistered, so the Projects tree never marked the open project) is
     `project.name`. A number 0 is falsy in eseqlisp: guard a position with
     `(= i nil)`, never `(and i …)`, or track 0 reads as missing.
   - **Legacy removed:** `SEQ.browser-preview-playing` / `-playhead` (with
     the tick's mirror and `prev_browser_preview_playing`),
     `content-library-epoch` (the watcher only bumps the counter the kinds
     compare), `sidebar-kind`, `sidebar-track-index`,
     `sidebar-selected-sample`, `sidebar-presets`, `sidebar-user-presets`,
     `sidebar-loaded-preset`, `sidebar-instrument-display-name`,
     `sidebar-rack-slot-presets`, `sidebar-preset-tree` (unread),
     `project-instrument-engines`, `sound-presets`, `kit-presets` (the
     listings only record into `presented` now: `record_sound_presets` /
     `record_kit_presets`), `sound-palette`, `track-instrument-ids`,
     `editor-surface`, `editor-buffer-name`, `editor-error`,
     `editor-canceling`, `editor-instrument-run-mode`,
     `editor-active-macro-name`, `editor-active-macro-action` (dropped from
     the `presented/legacy.rs` mirror), all of `RESAMPLE`, and the
     compat alias rows of the removed browser state. Kept, still read by
     unported areas (eseq-0l17.22): `SEQ.current-project-name`
     (application menus), `sidebar-instrument-name` (application menus,
     the panel frame), `editor-mode` / `editor-active` / `editor-open-macro`
     (seq-panels, scale editor, patch macros), `instances` (alez.neural),
     `scene-names`, `song-lanes`, `song-bound-clip` (arrangement), `groups`,
     `num-tracks`, `current-track`, `instrument-panel`,
     `delete-target-version`, `track-loaded-presets` (no reader; not this
     area's).
   - **Tests.** `host_kinds::tests::browser_view` (Distro root): the four
     files (and `ui/preview-strip.lisp`) use no legacy form and refer
     their kinds; the preview strip binds
     `browser.preview-position` and an idle sync re-renders nothing;
     `project.instances` follows the `App`'s instances once the Packages
     tab observes it (not before); resample's error and
     print land in `resample-view`. `host_kinds::tests::browser` checks the
     kinds against the presented record (sidebar, palette, editor, rack
     slots) instead of the removed legacy names. The host-less browser
     harness (`browser_editor_on_instrument_tab`) seeds kinds
     (`seed_browser_kinds`: a sampler sidebar, one Sound, one kit, three
     scenes; `seed_browser_tracks`, `show_browser_track(_sample)`,
     `push_presented_sidebar`, `set_kind_instrument_types`,
     `seed_kind_scene_names`, `seed_open_sound_palette`); tests reach view
     state through `set_browser_view_field` / `browser_view_field`. A
     rack track shown in the sidebar takes the Layer action and a modified
     activation as a layer, the Instrument Rack builtin as a new track. The
     ported-file scanners share `assert_ported` (`host_kinds::tests::views`).
     A track and its sample pushed together (one
     cycle, as one sync) keep the browser's sample-search reset rule.
   Built (stage 8, eseq-0l17.18): patching, macros, menus and settings
   (`ui/patch-learn.lisp`, `ui/patch-macros.lisp`, `ui/macros.lisp`,
   `ui/macro-state.lisp`, `ui/application-menus.lisp`, `ui/settings.lisp`;
   `ui/bindings.lisp` stays, below):
   - **Kinds.** `macro-mapping.path` and `param-label :string` (M): the
     mapping table's Path and Name columns (the display metadata the legacy
     dicts' `path-label` / `param-label` came from; `label` stays `path ·
     param`). `device.builtin :bool` (M): an audio effect built into eseq (a
     track, rack slot or bus effect `EffectDescriptor::builtin_insert` or
     the dgen builtins know, the legacy effect dicts' `:builtin`), which the
     effect editor cannot open. Everything else was built by 7b-3, 7b-4 and
     7f: `learn` and its rows, `editor` with `editor-macro`, `editor-asset`
     and `asset-info`, `settings` and `midi-device`,
     `project.audio-workers-options`, `macro`, `rack-macro`,
     `macro-mapping`, `browser.instrument`, `device.delete-target`.
   - **View state** (`:key ()` singletons): `eseq.settings`'s
     `settings-view` (`open`; `open-settings` / `close-settings` stay the
     host's entry points), `eseq.patch-macros`'s `macro-sidebar` (the search
     `filter`), `eseq.macro-state`'s `macro-arm` (`open`; `mid`, the armed
     project macro's id; `rack-index`, the armed rack macro's index: ids,
     the map commands' address; `rack-armed?` and `arm-macro!` are its
     shared reads and arm), replacing `mapping-open`,
     `mapping-selected` and `rack-mapping-selected`; their readers in
     `effects/param-controls.lisp` and `effects/instrument-panel.lisp`
     refer `macro-arm` (param-controls' rack checks are `rack-armed?`);
     fixtures and tests arm through `arm-macro!` or reach it through a
     local.
   - **Reads and setters.** Patch Learn reads `learn.*` (rows as
     `row.name`, which the fixtures' dicts answer too: `result-panel` keeps
     its arguments, `training-panel` takes the instance) and sets each training setting
     with `(set! learn.x v)` (`set-learn` under the value rule: a value it
     rejects is now an error in the status line, where `configure-learn`
     clamped and dropped it silently; `configure-learn` is deleted, and the
     population picker steps over 1–3 itself, to 4 going up and to 0, auto,
     going down); `training-panel` takes the `learn` instance (a fixture a
     dict of its fields) and its readouts an epoch moves (the epoch / loss
     header with its graph, the params list) are subtrees reading it, so an
     epoch re-renders those and not the target picker (a label's text and a
     list field cannot bind, so the subtrees read by value); the choices
     are `learn-method-options` / `learn-refine-mode-options`. The patch
     macros sidebar builds its tree from `editor.patch-macros` /
     `library-macros` (`m.name`, `calls`, `used`) and `editor.assets` (each
     row the dict the legacy mirror built, from `a.reference`, `tier`,
     `source-path`), selects `editor.open-macro` and inspects
     `editor.selected-asset` (an undeclared label reads empty, a count 0).
     Settings reads `settings.*` and `project.audio-workers-options`; the
     dropdown is `(set! settings.audio-workers-choice v)` (strict), a
     device's switch `(toggle! d.enabled)`. The macro controls find a
     script's macro by `m.script-key` among `(macros)`; the knob binds
     `#'m.value` and sets it (`set-macro`, a performance control; the
     legacy sent `macro-set-value`), hold sets 1, release stays
     `macro-release`. The mapping table lists the armed rack macro's
     mappings (`selection.track`'s instrument device's `macros` by
     `macro-arm.rack-index`) or every project macro's: ranges in the
     target's display units (`mm.min` / `max`; the pickers ranged by the
     target param's `min` / `max`, decimals and unit from its `type` and
     `unit`; a mapping that drives no device param, a rack slot's gain or a
     suspended target, by its own range with 2 decimals), edits through
     `(set! mm.min v)`, `max`, `curve` (a rack mapping's now the recorded
     `set-macro-mapping`, eseq-0l17.44), unmapping still `macro-unmap` /
     `unmap-rack-macro-param`. Scene macro controls read `m.target-scene`
     (labelled `Scene N` by `s.index`), `morph-params`, `steal-patterns`,
     `quantize`, `tracks` (the mask lists `(tracks)`) and `diff-count`, and
     set them with `set!` (was `macro-scene-config`); hold is a subtree, so
     the knob's binding is all a value change repaints elsewhere. The
     application menus read `(tracks)`, `selection.track` (its
     `instrument-type` and `index`), `browser.instrument`, `project.name`
     (read so Open Recent re-lists) and the armed effect: a device with
     `delete-target` and not `builtin` among the current track's `effect`
     devices and each bus's `devices`, addressed by `d.name`, `d.slot` and
     `d.bus.index`. Menu predicates answer booleans (a menu's `:enabled`
     must be one, so `(and … instance)` would break the boot). Widget lists
     are built with `each`.
   - **Fixes found on the way.** A reactive cycle skipped an observer (an
     `observe` effect) that only a kind field write had dirtied:
     `run_reactive_cycle`'s idle gate (`VM::has_visible_deferred_effects`)
     counted dirty named-buffer effects alone, so the application menus (an
     observer) kept a kind-only change (`selection.track`'s instrument type)
     until some legacy write ran a cycle; any dirty effect on a visible
     target counts now (observers and unnamed buffer effects too), and an
     effect whose run fails clears its dirty bit (it re-runs on its next
     input change instead of forcing every later cycle). The
     capture fixtures `patch-learn-training` (the training panel's
     `optimization-losses` argument) and `rack-macro-mapping-sidebar` (its
     sample path) failed at HEAD and are fixed.
   - **Kept.** `eseq.macro-state/macro-name`, the files' one COMPAT read:
     the rack panel's and the track panels' macro dicts name a live
     `:name-field`; it goes with the effects port (eseq-0l17.61), which
     reads `rm.name`. `ui/bindings.lisp` (`eseq.bindings`, SEQV channels
     with per-element float bindings): alez.tracker (eseq-0l17.20) is its
     only reader, and a kind field cannot hold a list of bindable slots, so
     the tracker's port replaces it and eseq-0l17.22 deletes the file (and
     its imports in `ui/main.lisp` / `ui/noui.lisp`).
   - **Legacy removed:** all of `AUDIO` (its registration), `MIDI.devices`
     / `error` / `persistent` (`MIDI.ports` stays: the dispatch's port
     identities), every `SEQ.learn-*`, `SEQ.editor-patch-macros` /
     `library-macros` / `assets` / `selected-asset` / `open-macro` (the
     record's learn, sidebar and settings areas are unmirrored;
     `presented/legacy.rs` keeps `editor-active` / `editor-mode`, `EXPORT`
     and `AGENT`), `SEQ.macros` (`sync_macro_state`, `build_macros_value`
     and its helpers), `SEQ.current-project-name` (`sync_project_state`,
     left listing presets only, is `record_preset_listings`),
     `SEQ.sidebar-instrument-name`, `SEQ.num-patterns`,
     `SEQ.delete-target-version` (the tick's, the invalidation apply's and
     the registration's; `UiInvalidationApplyCtx` loses the version), and
     the compat alias rows of the removed names (`macro-mapping-open`,
     `macro-mapping-selected`, `rack-macro-mapping-selected`,
     `patch-macros-filter`, the mapping table's and scene macro surface's
     internal helpers, no longer exported). Kept for eseq-0l17.22 (other
     readers): `SEQ.current-track`, `track-instrument-types`,
     `track-names`, `num-tracks`, `effects`, `bus-effects`,
     `instrument-panel`, `editor-mode` / `editor-active`, the rack macro
     name fields behind `macro-name`.
   - **Tests.** `host_kinds::tests::patching_view` (Distro root): the five
     files use no legacy form (macro-state none but its COMPAT
     `reactive-get`); the project and rack mapping tables read their
     mappings and edit them through the setters; player controls resolve a
     scripted macro and bind `m.value`; scene macro controls read and set
     the config; `device.builtin`; a built-in armed effect offers no
     editor. The host-less editor tests seed the kinds from the presented
     record (`push_presented_settings`, `present_learn_kinds`,
     `present_editor_sidebar_kinds`) and the project macros
     (`seed_project_macros`); Edit > Edit Selected Effect opens an armed
     custom effect on a track or a bus. `presented::tests` check the
     unmirrored areas; the legacy macro parity checks compare with the
     model. `present-fixture` gains `learn` and `editor` areas, so captures
     show Patch Learn's phases and the macro sidebar.
   - **Captures.** 42 baseline renders (the Patch Learn, settings, menu,
     macro player, scene macro, macro map mode and mapping sidebar fixtures,
     plus scratch states: every Patch Learn phase and stage, the macro
     sidebar's tree, search and asset inspector, project and rack mapping
     tables, a mapped player surface, a masked scene macro, a settings error,
     the Edit menu over a custom instrument and effect): 41 byte-identical.
     The one difference is a scratch scene macro whose target scene does
     not exist (its fixture's `clone-pattern` is not applied at capture):
     the legacy view printed `SCENE 2` and a `nil` option, the kinds read a
     nil `target-scene` and show neither; with the scene there (`(scenes 2)`)
     it reads `Scene 2`. The two fixtures fixed above render for the first
     time.
   Built (stage 8, eseq-0l17.21): the factory device UIs
   (`content/instruments/**/ui.lisp`, `content/effects/**/ui.lisp`,
   `content/midi-fx/**/ui.lisp`; 18 files used legacy forms: 808 Clap,
   909 Open Hat, PM Ride Kit, PM Tabla, PM Bongos, PM Ride, PM Milagre
   Brass, Digi Syn (and its `versions/1`, Digi Drift), Digi Wave, Digi FM,
   Grit, Heat, Melt, Poseidon, Revsynt, Vox, spatial-harmonic-delay):
   - **Shape.** These files speak the `eseq.effects` custom-UI vocabulary
     (`custom-ui-current-param`, `custom-ui-param-binding`, the lego and
     surface helpers), which resolves a param from the panel the host
     builds and is .14's to port to `param` / `device`; they hold no field
     names of their own. So no kind field was needed: a value Lisp decides
     on is `custom-ui-param-value p` / `ui-param-value name fallback` (the
     existing value twins of `custom-ui-param-binding` /
     `ui-param-bound-value`), a binding a widget draws stays the binding,
     and a binding held in a local outside a `subtree` and read inside it
     reads itself (§8: `(round mode)`, `(* first-cutoff …)`), so the read
     still records its dependency in the subtree. Small per-file value
     helpers sit beside the binding ones (`idclap-value`, `idhat-value`,
     `melt-value`, `heat-value`, `idclap-scope-value` for a drag handler's
     resolved scope). `:bindable` is deleted (ignored since stage 5).
     spatial-harmonic-delay's tap count keeps one COMPAT
     `(reactive-get "SEQ" (get p :value-field))`: `fx-param-value-for` shows
     mod depth while mods are open, and the tap count must read the stored
     value. It moves to `param.value` with the custom-UI layer (.14).
   - **Kept:** the per-scope `defstate` lists of VILLAIN Kick / Hat /
     Snare and Digi Wave (Lisp view state keyed by panel scope, not a
     reactive binding; the scanner allows them).
   - **Legacy removed:** none: the files read no host field by name; the
     legacy param fields they reach (through `fx-param-value-for`'s
     `bind-seq` of the panel's `:value-field`) belong to the custom-UI
     runtime and go with .14.
   - **Tests.** `host_kinds::tests::views::factory_device_uis_use_no_legacy_binding_forms`
     walks every `ui.lisp` under the three directories (versions included)
     and asserts no `legacy_forms` but `defstate`. The baseline and after
     captures (every fixture of these instruments, sections 0-5 of the
     ones without fixtures, the factory synth mods view, the MIDI effect)
     are byte-identical.
   Built in part (stage 8, eseq-0l17.14, group A of A–D): the shared
   plumbing of the factory device panels (`ui/effects/param-controls.lisp`,
   the custom-UI runtime `custom-ui-runtime` / `-sections` / `-controls` /
   `-lego`, `custom-effect-ui`, `param-grid`, `state`, `panel-frame`,
   `panel-widgets`, `panel-bodies`, and the new `ui/effects/devices.lisp`).
   Groups B (instrument, sampler and modulator panels, instrument
   modulation), C (effect panels, effect modulation, `builtin/*`, process
   panel, scale editor, buffers) and D (the surfaces, track panels,
   `ui/materials.lisp`) remain:
   - **Shape.** The panels still lay out from the host's panel dicts
     (`SEQ.instrument-panel`, `SEQ.effects`; they go with B/C), but every
     value a control shows comes from the kinds: `eseq.effects.devices`
     (side-effect free) finds the device a dict stands for by its own
     address (`inst-device`, `fx-device`: track, chain slot, bus, rack slot,
     MIDI chain) and a param by descriptor index (`param-of fx p`). A
     control binds `#'prm.value` (the base note control
     `#'d.base-note-display`), its lock state reads `prm.locked` /
     `has-locks` / `printing` / `base`, the modulation lanes are the
     param's `mod-targets` (`mod-target-source-slot`, `mod-target-depth` =
     `#'mt.depth.value`, `mt.depth-min` / `-max` / `unit`), the live dot
     `#'prm.mod-offset` / `mod-scale` / `mod-value` (bound for every param:
     an unmodulated one reads 0, which draws no dot), the process overlay
     `prm.process-mapped` / `#'prm.process-value`, key locks
     `prm.key-locks`, rack and project macro ownership `d.macros` /
     `(macros)` matched by `mm.target`, tensors `tz.values`, the key-lock
     chips `d.variants` / `d.key-locked-notes` (stamped with
     `stamp-key-variant!`), the keys piano's notes `t.active-notes`, an
     effect's delete-target highlight `d.delete-target`. The one-argument
     forms (`param-mod-offset p`, `fx-param-value p`, …) resolve a param
     through its owner (`param-owner-fx`: a custom UI's scoped param, or a
     built-in panel's, which `builtin-audio-fx-ui` tags with
     `with-param-owners`); the `-for` forms take fx. The knob commands stay
     the host commands (they latch prints, record neural overrides and
     p-lock the selected steps, which the base setter does not).
   - **Units.** The track, bus, MIDI and drum rack slot effect panel dicts
     now carry display units too (value, min, max: `stored_to_user`), so a
     fraction % param reads 0–100 everywhere;
     `eseq.effects.devices/param-stored-value` converts a control's value
     back for the effect commands, which store a fraction (`param.percent`,
     new). A rack effect's macro mapping range now reaches
     `map-rack-macro-param` in the display units it converts from (before,
     a % param's stored range was divided by 100 again). Built-in percent knobs show
     `pc/percent-scale` (1 when the shown unit is %, else 100) instead of a
     literal 100; a visualizer drawn on the 0–1 scale reads
     `param-effective-ratio` (by value for a fraction % param).
     `ParamDescriptor::is_percent` now also requires a ratio range (within
     ±10: 0–1, the OTT time's 0.1–10); the Filterbank's 0–100 % params are
     already display units (before, the kinds read them as 3000 %). Since
     eseq-0l17.63 that is an explicit `ParamDescriptor::percent_ratio`
     flag: built-in descriptors declare it (a literal, or a builder's
     per-effect rule: every % param of the effect is a ratio, the
     Filterbank's none, a % mod depth a ratio as its target), and only
     sources with no such flag (DGen manifests and their mod depths,
     `midi-fx-param`) infer it by the range rule
     (`ParamDescriptor::percent_ratio_by_range`); a test holds every
     built-in param's flag to the retired rule.
   - **View state** (`:key ()` singletons): `eseq.effects.state`'s
     `instrument-view` (tab, source-tab, mods-open, mod-slot),
     `key-lock-view` (octave, octave-count, anchor, notes, audition),
     `effect-mods` (open, chain, track, slot, rack-slot, bus, mod-slot),
     `process-panel-view`, `rack-panel-view` (views by the host's track id;
     the host calls `eseq.effects.state/reset-rack-panel-views!` on a
     project replacement, replacing `SEQ.rack-panel-view-generation`);
     `param-controls`' `process-map`, `plock-menu` (open, at, host, its
     clear command's target) and `plock-color` (the current step variant's
     color, three bindable components, one effect follows
     `selection.track.variants`); `custom-ui-sections`' `section-view` and
     `adsr-gesture` (view-local, keyed by the editor's scope name and
     section, one bound flag per stage: a drag only repaints its readouts);
     `param-grid`'s `grid-sections`. Fixtures and tests reach them through a
     local (`(let ((v eseq.effects.state/effect-mods)) (set! v.open true))`).
     An `effect-mods.track` of a dict naming no track (a MIDI effect's) is -1
     (`pc/effect-mods-track`): a typed field takes no nil.
   - **Moved, not ported:** the track-level and rack-target p-lock
     projection (`*plock-sync*`, `*plock-any-sync*`: timebase / swing rows,
     rack macro and slot control targets, which no kind field covers yet)
     lives in `track-panels.lisp` (D) with `target-plock-any?`.
   - **Legacy removed:** `SEQ.track-plock-printing` (publisher, registration,
     its row test, the `*plock-print-sync*` projection),
     `SEQ.rack-panel-view-generation`, spatial-harmonic-delay's COMPAT
     (`param-base-value`) and its scanner exemption, the compat alias rows of
     the removed state names. Kept for B–D and eseq-0l17.22: the param value
     publishers (`track-N-fx-*`, bus and MIDI fields, the instrument and rack
     value fields: track-panels' lock rows, the rack macro fields, Rust
     tests), `SEQ.track-plocks` / `-plock-any` / `-plock-variants`,
     `SEQ.instrument-panel` / `effects` / `midi-effects` / `bus-effects`.
   - **Tests.** `host_kinds::tests::panels_view` (the ported files use no
     legacy form; a Distro sampler + Filter panel binds its params'
     `value`; a % param reads display units and sets stored ones). The
     host-less panel harnesses seed the kinds from the dicts they publish
     (`seed_panel_kinds`: track, devices, params with their lanes, rack
     slots and macros, bus effects, tensors, key locks, the legacy lock
     lists) and drive values with `set_seeded_field`; `bound_field` names
     the legacy field a binding replaced.
   - **Review round.** Built-in percent knobs take `pc/percent-scale`: their
     own value x 100 unless its unit is % (display units); a lane depth x 100
     only under a % lane whose depth param is a plain fraction (Chorus mix,
     Slowdown MIX; a `percent` depth such as Str8 Delay wet's reads x 100
     already). Effect writes convert through their param and skip an
     unpublished one (its units are unknown): `fx-set-effect-value`, the
     batch helper `effect-param-updates` (Reverb curves, the filter curves,
     EQ8's rack batch, the param grid's ADSR, the custom-UI envelopes);
     preset tables written in stored units (Multiverb) go through
     `fx-set-effect-stored-value`. `fx-device` checks the device at the
     dict's address is its effect (`d.type`). A panel resolves each param
     once: `with-param-owners`, the param grid's rows and the custom-UI
     params carry it (`:prm`, `dv/with-prm`). Visualizers drawn on 0–1 bind
     `param.mod-ratio` (new) instead of reading a % param by value. View
     state: one section choice per scope (`eseq.effects.state`'s
     `section-of` / `select-section!`, for the param grid and the custom
     UIs); an ADSR drag's stage flags live in a view-local
     `(def-kind adsr-gesture :key (scope section) :state …)` instance per
     editor (§3.1; eseq-0l17.73 replaced the eight per-section singletons
     `adsr-gesture-0` … `-7`, which lit the readouts of two custom UIs on
     screen sharing a section number), so a readout binds its flags and
     reads nothing by value (no first-drag re-render; its render creates
     the instance). Test: `host_kinds::tests::panels_view::
     adsr_gesture_flags_are_per_scope_and_a_drag_only_repaints`. The keys tab resolves the instrument's
     key locks once per render (`key-locks-of`). The p-lock projections
     publish only the rows their COMPAT readers use (track-level `-on` /
     `-def`, rack macro and slot-control `-any`). The tick no longer
     publishes per-param modulation fields (`fx-mod-*`, `fx-instrument-mod-*`,
     `instrument-mod-*`, `rack-slot-mod-*`; the delta syncs publish only the
     slot phases the source editors read) nor the track, MIDI and bus effect
     value fields in the bulk binding sync (the print latch and the eval
     natives still write theirs; the dict builders still carry the
     `*-field` names, which the host-less test seeds key on, until
     eseq-0l17.22). `ParamDescriptor::is_percent` kept its range heuristic
     here; eseq-0l17.63 replaced it with the explicit `percent_ratio` flag
     (above).
   - **Captures.** Of 135 baseline renders (the fixtures plus scratch
     states: mods views, p-locks, rack, bus, MIDI effects, process lanes),
     all but three are byte-identical: the Phaser-Flanger notch display
     (its LFO sweep animates with wall-clock time, so two runs differ),
     PM Electric Bass's Slow % knob (its value was unresolved and drew "0";
     it now reads the param, "0.0" at the knob's one decimal), and a
     multi-track selection's sampler knobs (stray mod dots from the old
     fields are gone).
   Built (stage 8, eseq-0l17.61): groups B–D of the factory device panels
   (`ui/effects/instrument-panel`, `sampler-panel`, `modulator-panel`,
   `instrument-modulation`, `instrument-sources`, `effect-panels`,
   `effect-modulation`, every `builtin/*`, `process-panel`, `scale-editor`,
   `buffers`, `step-buffer`, `drum-surface`, `mnm-surface`,
   `identified-drum`, `physical-model-surface`, `track-panels`;
   `ui/materials.lisp`):
   - **Shape.** As group A: the panels lay out from the host's panel dicts
     and read every value from the kinds through `eseq.effects.devices`
     (new: `rack-slot-device`). The rack panel's macros are its instrument
     device's `rack-macro`s (`#'rm.value`, `rm.locked`, `rm.base`,
     `rm.has-locks`, `(len rm.mappings)`; the name field reads `rm.name` and
     renames with `(set! rm.name …)`, so `eseq.macro-state/macro-name` and
     its COMPAT read are gone); a slot row's strip binds the slot device's
     `*-display` fields and `#'sd.delete-target`, its p-lock dot reads
     `sd.strip-locks`, its macro dot the rack dict's own `:macros`. The
     sampler reads `d.sample-buffer`, `sample-duration`, `slices`,
     `slice-active`, `#'d.start-time` / `end-time` / `playhead`; the
     modulator `#'d.modulator-phase` / `-level`; a modulation source editor's
     LFO curve `#'prm.mod-phase` of the section's type param
     (`im/section-phase`). The Filter Table reads `d.table-*` and its
     response editor the `table-editor` singleton (shown while `te.open` and
     `(= te.device d)`; every button is a `table-editor-…!` action); the
     Convolution Reverb `d.ir-name`; EQ8's spectrum `d.meter`; the
     Phaser-Flanger `#'transport.bpm`. The process panel lists
     `selection.track.processes` (its edits `set-process-enabled!`,
     `remove-process!`, `move-process!`, `clear-port!`, `(set! i.value v)`;
     the process-map arm keeps its dict addresses: `sequencer.lisp` shares
     it). The scale editor reads `t.tuning` (`tn.root`, `mode`, `morph`, the
     degrees' fields) and edits through `set!` / `toggle!` and the tuning
     actions (`import-scl` stays `seq-tuning`). The track settings strip
     reads the track's own settings and their p-locks at the displayed step
     (`track.setting-locks`); the voices follow the selected drum rack
     slot's `voices` (`tp/poly?`, `tp/voices`); the edits stay the
     `seq-set-track-param` natives (they p-lock a selection and batch a
     multi-track one). The step panel binds `selection.edit-step`'s fields,
     counts `selection.steps` and shows `selection.cursor-step`; its track
     chip binds `#'t.audible` (the silenced look on the plain props). The
     surfaces' `bind` / `value` (`mnm-`, `drum-` too) are thin calls to
     custom-ui-controls' `ui-param-bound-value` / `ui-param-value` (0 for a
     missing param); the legacy-forms scanner flags the generic `(bind "NS"
     field)` alone, not a surface's one-argument `bind`. `:bindable` is
     deleted throughout.
   - **Kinds.** `param.mod-phase :number` (L: a source param's modulation
     source's cycle position, device.mod-phases' entry for its mod-slot; -1
     otherwise; observing it keeps the modulation sample polled),
     `device.strip-locks (list-of :string)` (L: a rack slot's strip controls
     some step of its track's pattern locks, in `RackSlotParam` order,
     cached per slot under the track's `PlockKey` and step count; the last
     free bit of the device's then-`u32` observed mask: 31 live fields and
     `params`, 32 of 32; eseq-0l17.71 widened it to a 64-bit
     `ObservedMask`), `track.setting-locks
     (list-of :any)` (L: `(dict :name :value)` per timebase / swing / swing
     resolution lock at the displayed step). `HostKinds::wants_sampler_playhead`
     keeps the current track's sampler voices on the watchlist while its
     `device.playhead` is observed (it replaced the `SEQ.sampler-playhead`
     consumer check). `param.mod-phase` keeps the modulation sample polled
     only on a modulation source's setting (a `mod-slot` in 1–4). The
     print latch shows on the controls it holds while the transport plays
     and records (`KindsHandles::print_latch`), as `param.printing` reads
     it: `step.*` of the edit step (the step panel's pickers),
     `rack-macro.value` (after a take's override) and a rack slot's
     `device.*-display`; `step_print`'s writers of the legacy
     `fx-step-value-*` and rack slot value fields are gone (the rack macro
     value field stays: the p-lock table's rows bind it). New action:
     `(clear-degree! dg)` (`set-tuning` `clear`: its own undo entry, never
     merged into a drag on the degree, as the legacy `ClearDegree`).
   - **View state** (`:key ()` singletons): `sampler-view` (start,
     duration, cursor-time, active-marker, selected-slice),
     `polyphony-menu`, `plock-table` (the selected row), `scale-view` (open,
     selected-degree, rand-amount, stretch-amount; `eseq.seq-layout` sizes
     the *track* tile by `editor-open?` when it re-lays out), `eq8-view`
     (the device whose band is picked, and the band), `roar-view`,
     `chorus-view`. A view singleton is never read
     through a qualified dotted path (`tp/plock-table.row` is an unknown
     variable): the owning module exports a function (`clear-plock-row!`,
     `editor-open?`).
   - **Fixes.** An effect's modulation source type read its dict's
     `:options` by value, which leave out `env`: `rand` showed as `drift`
     (group A); `source-type` and the effect source dropdowns read
     `prm.text`. `param-grid` imports `eseq.materials` (eseq-0l17.25; the
     legacy mixer half was .13): `sequencer.lisp` and `step-grid.lisp` (the
     song lane's) still call `eseq.materials/` in shaders without importing
     it, so `DefwidgetError::Shader` stays non-fatal. Capture settles the
     kind fields computed only while observed (a sampler's media): it syncs
     the host kinds again after the first frame and rebuilds it when that
     sync pushed anything.
   - **Kept** (eseq-0l17.22): `SEQ.instrument-panel` / `effects` /
     `midi-effects` / `bus-effects` as the panels' structure (read by
     `buffers` and `eseq.effects/device-panel` alone; every test harness and
     capture fixture publishes them); the p-lock table (`SEQ.track-plocks`
     rows bound to their `:value-field`, `SEQ.track-plock-variants` chips:
     no kind holds a lock row, the def chip or a preview); the dicts'
     `*-field` strings and the param value publishers (the table rows, the
     test seeds); the rack macro name fields (main's alez.tracker until
     eseq-0l17.65 lands); `SEQ.tp-num-steps` / `tp-timebase`
     (seq-grid-mode, seq-core-state, step-grid); the `fx-step-*` fields
     (seq-core-state); `SEQ.compiling` (unread now); the pad-grid COMPAT
     shims `buffers` calls in `sequencer.lisp`.
   - **Legacy removed:** the track settings' `SEQ.tp-*` fields but
     `tp-num-steps` / `tp-timebase` (`tp-poly`, `-is-rack`,
     `-rack-slot-idx`, `-max-polyphony`, `-gate`, `-swing`,
     `-swing-resolution`, `-voice-priority`, `-mono-trigger`,
     `-supports-mono-trigger`, `-mute-group`, `-fts`, `-accumulator`,
     `-accum-*`, `-attack`, `-release`, `-send`, `-output`, `-bus-sends`,
     `tp-bus-N-send`, every `tp-tuning-*`) and the option lists
     (`fts-options`, `accumulator-options`, `mute-group-options`,
     `accum-mode-options`, `tuning-root-options`): publishers
     (`sync_track_polyphony_fields`, the send syncs), registration;
     `SEQ.track-plock-any` (publisher, registration, the
     `*plock-any-sync*` and `*plock-sync*` projections, `target-plock-any?`);
     `SEQ.sampler-playhead`; `SEQ.process-slots`; the
     `rack-slot-delete-target-*` fields; the `modulator-phase-N` /
     `-level-N` fields and the source editors' slot phase fields
     (`fx-mod-slot-phase-*`, `instrument-mod-slot-phase-*`, the rack slot
     ones); `eseq.macro-state/macro-name`; the compat alias rows of the
     removed state names; the dead `track-bus-send-control` and
     `track-accumulator-panel`.
   - **Tests.** `host_kinds::tests::panels_view` scans the ported files
     (with kinds, without, every `builtin/*`) and pins the COMPAT reads
     left (`buffers` and `index`: `SEQ.`; `track-panels`: `bind-seq`,
     `SEQ.`); `settings` covers `track.setting-locks`, `devices`
     `device.strip-locks`. The host-less seed (`seed_panel_kinds`) now also
     mirrors the published `tp-*` / tuning fields, the rack macros and slot
     strips, the sampler media, the effect tables and the editor session,
     and the track-level lock rows; the panel tests publish, then seed.
     Ported: the polyphony header, step panel, track settings, scale
     editor, sampler waveform, rack macro typing (renames through
     `set-rack-macro`), rack slot indicators and rack view tests; deleted:
     the pure parity checks of removed fields.
   - **Captures.** 268 renders (the port14 and port21 sets: every panel,
     effect, rack, process, track, step, sequencer and mixer fixture, every
     factory device UI and `pm-*` fixture, plus scratch states: six built-in
     effect chains, a Roar stage, a selected scale degree; 4 fixtures that
     load a missing IR fail at HEAD too): 264 byte-identical. The
     differences: the Phaser-Flanger's notch display (it animates with
     wall-clock time; three renders), and the modulator panel's envelope at
     rest: the legacy `modulator-level-N` field was registered at 1, the
     kinds read the meter cache (0 with no DSP), so the capture draws a flat
     envelope; live, both read the meter. A first after-render showed the
     rack slot sampler's waveform at its defaults (media are computed only
     while observed): capture now settles them (above) and they match.
   Built (stage 8, eseq-0l17.74, in part): the step panel's p-lock table
   (`ui/effects/track-panels.lisp`), the print latch on `param.value`:
   - **Kinds.** `(def-kind plock-row :key (index) …)` (M; `host_kinds::
     plock_rows`): `target`, `domain` (inst, seq, fx, neural), `source`
     (step, neuron, preview), `name`, `value`, `text`, `default`,
     `default-text`, `min`, `max`, `options`, `step`, `param`, `rack-macro`,
     `address` (the dict the table's host commands take). `selection.
     plock-rows (list-of plock-row)` (M): the locks at the first selected
     step (a selected neuron's overrides instead; a previewed variant's locks
     with no step selected), built while observed (an unobserved table
     costs a query; by value it reads its last build) and only when the
     `ModelRevision` (epochs, history, scenes, song row mirror, sound
     binding), the track's `PlockKey`, the selection, the neuron selection
     or the preview moved, each row keyed by what it locks (`KeyedRows`), so
     an edit keeps the instances. A lock whose param or rack macro has no
     instance yet is built again next tick (no row keeps the legacy units). `selection.plock-variant :string` (M): the
     chip the table lights (`plock_variant_chip`). The preview is gesture
     state: the tick hands it over (`HostKinds::set_plock_preview`). A device
     param's lock (instrument, effect, MIDI effect, rack slot effect) carries
     its param and step and speaks the param's display units (the legacy
     effect rows showed stored units: a % effect param reads 0-100 in the
     table now, as on its knob); a rack macro's carries its `rack-macro`.
   - **Shape.** The table lists `selection.plock-rows`; a row's LOCK binds
     `#'r.param.value`, `#'r.rack-macro.value`, else `#'r.value`; edits go
     through `lock-param!` / `unlock-param!` / `lock-rack-macro!` /
     `unlock-rack-macro!` (an option row's label → its index), the other
     rows through `set-track-plock-entry` / `-option` /
     `clear-track-plock-entry` with `r.address`. The chips are a def chip and
     `selection.track.variants` (`plock-chip` takes the variant, nil for
     def; colors through view-kit's `color-rgba`); a click stamps through
     `stamp-variant!` (with `selection.steps`) or previews
     (`preview-plock-variant`). `track-panels` uses no legacy form now
     (`panels_view` PORTED).
   - **Print latch.** `param.value` (and `text`) shows the latched value
     while a print latch holds the param (play + record), every tick, so the
     knob follows the hand, not the step the playhead last printed (the
     step pickers, rack macros and strips since .61). The legacy display
     writer (`sync_print_latch_display`, `print_latch_display_updates`) is
     gone.
   - **Legacy removed:** `SEQ.track-plocks` / `track-plock-variants`
     (publishers in `sync_track_params` (now the step grid fields alone, no
     `App`), `sync_track_params_with_neural_selection`,
     `sync_track_plocks_for_neural_selection`,
     `sync_track_plock_variant_preview`, the rack row republishes, the
     registration, `build_track_plock_variants_value(_with_preview)`), the
     rows' `:value-field` / `:name-field`, `SEQ.compiling` (publishers,
     registration, test seeds; `engine.compiling` covers it).
     `PENDING_MACRO_IMPORTS` is empty: track-panels calls no
     `eseq.materials` macro, so it takes no import.
   - **Remains (eseq-0l17.22, after eseq-0l17.19):** the panels still lay
     out from `SEQ.instrument-panel` / `effects` / `midi-effects` /
     `bus-effects`: `buffers.lisp`, their reader, is the drum rack lane's
     (.19), and `eseq.effects/device-panel` shares its dicts; the param
     value publishers (`sync_fx_param_binding_fields*`, the rack value
     field syncs), unread by content now but read by the host-less seeds
     (`seed_panel_kinds` keys on the dicts' `*-field` strings) and Rust
     tests.
   - **Tests.** `host_kinds::tests::panel::plock_rows_*` (rows, units,
     handles, identity across an edit, no rebuild while nothing moves; the
     preview and its chip; a neuron override edit, the song row mirror and
     a sound binding loan refresh them; nothing builds while hidden),
     `a_rack_macro_row_carries_its_macro_before_the_rack_panel_shows`,
     `a_param_held_by_the_print_latch_shows_the_latched_value`; the host-less table tests seed `plock-row`s
     (`seed_plock_table`, from rows in the legacy shape) and assert the LOCK
     binds `r.value`; the chip tests read `plock_variant_chip`; the print
     command tests (`device_param_commands_print_while_recording`, the rack
     EQ8 band and macro take) assert the latch (or the take override)
     instead of the removed display fields.
   - **Captures.** 278 renders (port61's set plus scratch p-lock states:
     step params and two instrument locks at the selected step, a selected
     table row, a step with no lock, each in *step* and *fx*; 4 fixtures that
     load a missing IR fail at HEAD too): 271 of 274 byte-identical; the
     Phaser-Flanger notch (wall-clock animation) and the two EQ8 fixtures
     (their spectrum differs run to run at HEAD and after alike).
   Built (stage 8, eseq-0l17.16): the piano roll (`ui/piano-roll.lisp`;
   `ui/seq-panels.lisp` sets its entry mode):
   - **Kinds.** `note.item :int` (M): the timeline's id for the note
     (`piano_roll_item_id`: its step and voice), what the host's piano roll
     editor (`seq-piano-roll-action`) addresses. The note grid keeps that
     editor for its gestures (select, marquee, move, resize, nudge, copy,
     paste, create, delete): one engine with one undo entry per gesture,
     where a nudge or paste over `set!`s would land note by note (one entry
     each, a moved note replacing a neighbour not yet moved). So the items
     are `(dict :id n.item :lane (- pitch-max n.pitch) :start n.start :end
     (+ n.start n.length) :selected n.selected :label n.label)` (the
     timeline reads the selection from `:selected`). Everything else was built by 7e
     and 7e-2: `piano-roll.*`, `note`, `focus-step`, `param.step-locks`,
     `rack-macro.step-locks`.
   - **View state** (`:key ()` singletons, exported): `piano-roll-view`
     (start, duration, lane-scroll, lane-height, cursor, the marquee being
     drawn, the new-note length, `arrangement`: entered from arrangement
     clip gestures, and `fit`: the track whose notes it fits once shown) and
     `lane-view` (the lane's step param by name, or the device param or rack
     macro held while still on the piano roll's track, and the edit value).
     The pinned `eseq.vanilla/piano-roll-fit-pending` and
     `piano-roll-automation-param` and the reactive
     `SEQV.piano-roll-arrangement-mode` are gone: seq-panels calls
     `(eseq.piano-roll/set-arrangement-mode! on)`; a fit for a track the
     piano roll does not show yet waits in `fit`, and the host kinds invoke
     `eseq.piano-roll/piano-roll-apply-pending-fit` after each note sync
     (`apply_pending_piano_roll_fit`, from `HostKinds::sync` when
     `NoteShared::syncs` moved). Lanes are a constant list from `pitch-min`
     / `pitch-max`.
   - **Bindings.** The timeline's playhead binds `#'piano-roll.playhead`
     (playback only repaints), its cursor and view axis
     `#'piano-roll-view.cursor`, `.start`, `.duration`, and the automation
     lane its view axis too (`automation-lane` declares `view-start` /
     `view-duration` bindable): a scroll, zoom or cursor move repaints and
     re-renders nothing. The clip panel, the note grid, the automation row
     and the lane's value readout are subtrees (`clip-panel`, `note-grid`,
     `automation-row`, `automation-axis-readout`), so a note edit leaves the
     panel alone and a drag's readout re-renders by itself. The timeline
     takes no `:selection` (its items carry `:selected`). A zoom of the
     lanes (`lane-height`) and the lane scroll stay values. Values stay
     where Lisp decides: the notes, the focus fields
     (`piano-roll.focus-kind`, `clip-kind`, `focus-label`,
     `focus-num-steps`, the window), the clip panel (`piano-roll.clip`'s
     start / end / offset), the track's name, color and instrument type
     (`piano-roll.track`).
   - **The automation lane** is a view: a step param's points are the
     active focus steps (start, end, `focus-step-value`), a device param's or
     rack macro's (live focus only) each step holding a lock in its
     `step-locks` and each other active step at its `base` (gray); the
     scale is the param's (`min`, `max`, `base`, an increment of 1 unless
     continuous) or 0–1. The picker lists `(focus-step-params)` then the
     track's devices' params and rack macros with `has-locks` (labelled
     `inst lp_freq`, `Filter cutoff`, `rack Macro 1`); a held param or macro
     no longer on the track (stale, or another track) falls back to the step
     param. `lane-target` is what the lane edits (kind, target, scale,
     editable), `current-lane` adds its points; the edits read the target
     alone. Edits: a step param `(set-focus-step! fs name v)` (a drag's
     frames join one undo entry, as the legacy gesture), a clear its
     default; a device param `lock-param!` / `unlock-param!`, a rack macro
     `lock-rack-macro!` / `unlock-rack-macro!` on the live step. A drag's
     locks of one step join one undo entry until the release and a lock of
     another step starts the next, as the legacy `set-track-plock-entry`
     coalesced per step (eseq-0l17.58, below); a clear is an entry of its
     own.
   - **Kept:** the clip panel's and loop bar's focus host commands
     (`focus-clip-resize`, `focus-set-offset`, `focus-set-num-steps`,
     `focus-finish-num-steps`, `focus-take-set-length`, `focus-slide-band`:
     `clip.start`'s setter moves a clip, where the panel's Start trims it),
     `seq-set-track-param :num-steps` (`track.num-steps` has no setter) and
     `seq-piano-roll-action` with its history commands.
   - **Legacy removed:** `SEQ.piano-roll-items`, `piano-roll-selection`,
     `piano-roll-lanes`, `piano-roll-playhead`, `piano-roll-automation`,
     `piano-roll-automation-params`, `focus-num-steps`, `focus-label`,
     `focus-live` (unread), `focus-kind`, `focus-clip-kind`,
     `focus-window-marker` / `-span` / `-repeat`, `focus-clip-start` /
     `-end` / `-offset`, `SEQV.piano-roll-arrangement-mode`, with their
     publishers (`sync_piano_roll_state`, `sync_piano_roll_note_state`,
     `sync_piano_roll_playhead`, `sync_piano_roll_automation_state`,
     `build_piano_roll_items_value`, `-selection_value`, `-lanes_value`,
     `build_piano_roll_automation_value`, `-params_value`), the tick's
     clip-surface diff (`prev_focus_clip_surface`), the legacy lane's
     `update-` / `finish-automation-step-param` gesture and
     `PianoRollDragKind::Automation`, and the alias rows of the four view
     `defstate`s. The callers of `sync_piano_roll_state` now call
     `sync_track_automation_state` (the tracker's columns, which rode on
     it). Kept for the tracker (`alez.tracker`, eseq-0l17.20 /
     eseq-0l17.22), all since removed by its port (.65, with
     `sync_track_automation_state`): the `piano-roll-automation-refresh`
     host command (it republished `SEQ.track-automation`), the
     `set-automation-step-param` action of `piano-roll-history-action`, the
     pinned `eseq.vanilla/track-automation-wanted` (defined in this file),
     `SEQ.track-automation`, `track-lock-targets`, `tracker-rows`,
     `track-grid-playhead-*`.
   - **Captures.** Every piano roll fixture (notes, velocity, step param,
     instrument param, effect param and rack macro lanes, selection,
     cursor and marquee, zoom and scroll, the arrangement entry) matches
     but for two lags the legacy capture had: its publishers ran before
     `capture-after-sync`, so it never drew the live focus's playhead (the
     live app does, at the track's playhead) nor a selection made in the
     hook; the kinds sync after it and draw both. The review pass (the
     bindings and subtrees above) renders every one byte-identical.
   - **Tests.** `host_kinds::tests::piano_roll_view` (Distro root): the
     file uses no legacy form; the grid draws the notes by item, the lanes
     and the track color, and binds the playhead (playback only repaints);
     scroll, zoom, cursor, new-note length and fit; scrolling and the
     cursor bind (the buffer tree keeps its revision); a fit for another track
     applies once the host shows its notes; the lane over step params,
     device param locks and a rack macro, and its edits; a pinned take's
     lane edits its chunk in one entry; the clip panel of a pinned clip; the
     arrangement entry's empty state. The legacy parity checks
     (`host_kinds::tests::piano_roll`, `focus_steps`) assert the kinds'
     values directly; the bare-VM loads of the file
     (`metal_seq_piano_roll_lisp_loads`,
     `sync_piano_roll_state_applies_pending_track_fit_after_items_update`)
     and the legacy lane builder tests moved to the view tests.
   Built (eseq-0l17.58): script drags of device p-locks join one undo
   entry. `lock-param!` / `unlock-param!` (`set-device-param-locks` /
   `clear-device-param-locks`) and the `lock_steps` actions
   (`lock-rack-macro!`, `lock-strip!` and their clears) end their
   `ScriptEdit` as continuous: while the pointer is down the entry stays
   open, and the history's device p-lock coalescing
   (`apply_coalesced_device_plock_command`, merge key
   `device-plock:<track>:<pattern>:<param or macro>:steps:<steps>`) joins
   the frames that lock the same param or macro on the same steps; a lock
   of other steps (or another param) seals it and opens the next, the
   release seals the last. A clear (`Clear…PlockMulti`, `Record`) is an
   entry of its own, and with the pointer up each call is one. Tests:
   `host_kinds::tests::params::a_script_drag_of_param_locks_joins_one_entry_per_step`,
   `panel::a_script_drag_of_rack_macro_locks_joins_one_entry_per_step`.
   Built (stage 8, eseq-0l17.20, in part): the packages and demo scripts
   that read no GRAPH namespace: `alez.jaki`
   (`packages/alez.jaki/src/kind.lisp`), `scripts/processes/process-ui-control-demo.lisp`,
   and in `scripts/sequencers/` `band-coupling-matrix-demo.lisp`,
   `jaki-builder-demo.lisp` and `neural-8x8-track-router.lisp`:
   - **Kinds.** No new field: 7c, 7g-3, 7g-4 and 7g-5 built every one they
     read. New in `eseq.kinds`: `(generator-mark-of x key)` (nil-safe
     `generator-of` + `generator-mark-named`) and `(set-neural-thresholds!
     nw v)` (`set-neural` `"thresholds"`, `NeuralSlot::Thresholds`: every
     neuron's threshold as one edit, so a drag joins one entry and undo
     restores each neuron's own).
   - **alez.jaki** (§14.2t). The panel reads its generator's marks:
     `jk-mark-value` (`(generator-mark-of self key)`'s value, 0 before the
     key's first stamp) and `jk-mark-binding` (`#'m.value`, 0 then; a row
     takes `(generator-of self)` once and passes it down, with its route
     tracks and track). The hit strip's playhead reads the `""` mark and
     the Chords row's label the `"chord"` mark, each in its own subtree; a
     sounding row's sexp-slot binds `:lit` to its route slot's mark and
     `:lit-values` to the slot's `"<slot>.<k>"` marks (the slot reads an
     instance binding as it read the legacy float). Reading `g.marks`
     re-renders the panel once, when a key gets its first stamp. Routes are
     `(tracks)` or the owning rack's `group.tracks` (the group whose `gid`
     is `self.owner`); a row's track is a track instance (`:dyn-context
     t.index`, the plock reason names `t.name`), its color `t.color`. The
     plock hover reason is the `:key ()` singleton `jk-plock-hover` (row,
     reason; kind names are one global registry, so a package's are
     prefixed).
   - **The event-view demos** (process-ui, band, §14.2s): `:events
     transport.track-events` in a subtree of its own (whose key the event
     view takes), `:current-beat #'transport.track-events-beat`, the palette
     `(map (lambda (t) t.color) (tracks))`. Their view state is a singleton
     each (`process-ui`, `band-ui`).
   - **Band** (§14.2h): the panel reads each track's inlets back through
     `t.processes` / `p.inlets` (`band-inlet` of a process; the panel takes
     the four `band-ear` and four `band-voice` processes once a render and
     passes them down): a scene's chain shows its
     values, a chain without the band processes the panel's own edits. The
     legacy render copied the chains' values into its defstates; the edits
     no longer pick up the chains' values (only the display falls back to
     them).
   - **jaki-builder:** the header's scene is `transport.scene.index`, the
     bake the singleton `jaki-builder`.
   - **The neural router** (§14.2q), rewritten over `network` and `neuron`:
     the panel finds its network by name in `(networks)` (without it, a
     message and a Create button that runs `router-ensure`; the render calls
     no native); every control reads its field and edits with `set!` (one
     undo entry each, where the legacy natives recorded nothing); the
     threshold control sets every neuron's with `set-neural-thresholds!`
     (one entry, a drag's frames joining it); a row's selection
     binds `#'nr.selected` and its number `set!`s it; the playback matrices
     are subtrees over `nr.trigger`, `nr.energy` and `nr.dampening`, sized
     by `nw.neuron-count`. Route options are the project's tracks ("Track
     n") and Off (legacy: a fixed sixteen); a new network routes neuron i to
     track i whatever the project's tracks (a route past them shows Off
     until the track exists; creation cannot read `(tracks)`, which at load
     can predate the host's next push). The max-poly picker offers
     `graph-max-poly-selection-options` (legacy: three; the native engine
     plays the graph-only ones as deterministic). Creating the network stays the natives (`neural-create`,
     `neural-set`, `neural-neuron`, which answer at once); the 48
     per-neuron defstates and their load-from-model mirror are gone.
   - **Learned:** `when` / `unless` are `core/init.lisp` macros, absent from
     a bare runtime: a script that uses them is tested on a Harness (the
     router's tests moved there).
   - **Legacy removed:** `SEQ.generator-mark-*`
     (`sync_generator_mark_fields`, its liveness map, the parity checks);
     `SEQ.neural-networks`, `neural-energy-matrix`, `neural-trigger-matrix`,
     `neural-dampening-matrix`, `selected-neural-neurons` and
     `neural-neuron-selected-*` (their registration, `sync_pattern_state`'s
     pushes, `build_neural_*_value`, `neural_network_value` /
     `neural_neuron_value`, `sync_selected_neural_neuron_bindings`,
     `SequencerState::has_neural_visualization`;
     `sync_neural_visualization_fields` is now `sync_visualization_fields`);
     the docs naming them (`content/authoring/sequencer-reference.md`, the
     jaki, harmony and lisp-sequencer specs).
   - **Kept** (read elsewhere; eseq-0l17.22): `SEQ.track-events` and
     `track-event-current-beat` (graph-neural-8x8-demo),
     `track-process-slots` (sequencer, seqv-track-params), `current-pattern`,
     `track-names`, `track-colors`, `groups`.
   - **Not ported (the rest of .20; the graph demos since .64, alez.neural
     since .67):** the GRAPH namespace's consumers,
     `packages/alez.neural/src/variable-reset.lisp` and the seven graph
     demos (`graph-markov-8x8`, `graph-neural-16`, `-16-cycle`, `-8x8`,
     `-8x8-reset`, `-group-matrix`, `-variable-reset`): the kinds cover them
     (§14.2k, m, n, s), but each brings bare-runtime layout tests
     (`lisp_host::tests`, `state_values::tests`, the graph visualization and
     rack restore tests) that move to Harness tests with it. `alez.tracker`:
     its data maps (§14.2j), but its playhead (`track-grid-playhead-*`: which
     repeat of a shorter track lights, per row) and cursor highlights (the
     `eseq.bindings` SEQV channels) are per-row bindings no kind field
     gives; a Lisp derivation re-renders every row each step. It needs a
     live per-track field (the grid row the playhead lights) compared in a
     row `defwidget`, or the like (built and ported by .65, below).
   - **Tests.** `host_kinds::tests::packages_view` (bare root): the files
     use no legacy form; the jaki panel binds a sounding row's `:lit` to its
     mark, and a later hit repaints without re-rendering; the event views
     bind the beat and read the events; the band panel reads inlets back
     and shows its edits where a chain lacks them; the router is
     idempotent, reuses its network, follows an edit from elsewhere with
     nothing written back, lines its rows up with its matrices, binds and
     sets the selection, edits a route and a delay, sets every threshold in one entry (a drag included,
     undone whole) and, without its network, offers a Create button that
     makes it. `host_kinds::tests::neural::set_neural_thresholds_is_one_edit_a_drag_one_entry_and_undo_restores_each`. The router's
     bare-runtime tests moved there; the remaining lisp_host demo tests
     drop their `SEQ` stubs.
   - **Captures.** Byte-identical: the six jaki fixtures, jaki-builder,
     process-ui, band and the router. A router fixture that edits the
     network and selects a neuron in `capture-after-sync` differs as
     expected: the legacy panel showed none of it (its mirrors loaded at
     render, its publishers ran before the hook), the kinds show the edits
     and the lit row. The untouched graph demos and neural panel differ run
     to run in their auto-rotating event views only.
   Built (stage 8, eseq-0l17.64, in part): the seven graph demo scripts in
   `scripts/sequencers/` (`graph-markov-8x8`, `graph-neural-16`,
   `-16-cycle`, `-8x8`, `-8x8-reset`, `-group-matrix`, `-variable-reset`),
   over a shared, side-effect-free `eseq.graph-kit` (`ui/graph-kit.lisp`):
   - **Kinds.** No new field: 7g (§14.2k) and 7g-4 (§14.2s) cover them. A
     panel finds its graph with `(graph-of <prefix>-name)` (the handle
     `def-sequencer` returns is the sequencer id); until the host publishes
     it (the next sync) the panel is an empty box, re-rendered when
     `project.graphs` lists it.
   - **Controls.** Node fields and params bind (`#'n.delay`, `#'p.value` of
     `(graph-param-named n "transpose")`, `#'g.reset-bars`, `#'g.max-poly`,
     `#'g.node-count`, the group knobs' `#'g.group-trace-decay` …) and edit
     with `set!`: one undo entry each, a drag's frames joining one (the
     legacy natives recorded nothing). Labels (resolution, quantize, the
     max-poly selection, a node's group) are values: a dropdown's `:value`
     is the label (a bound string would not draw). 0 / 1 params and
     `seed-route` are toggles bound to the field; `seed-on-reset` (a
     number) passes `(> n.seed-on-reset 0)`. Each row is a subtree of its
     own (`<key>-row-<n>`), so a route or label edit re-runs one row. The
     group-matrix and variable rows are a highlight box (lit by the pressed
     weight column) around a controls subtree (`<key>-row-controls-<n>`):
     a press re-runs the boxes, which reuse the controls.
   - **Routes** (graph-kit `route-options`, `node-route-label`,
     `set-route-label!`): a track of the graph's owner, the project's
     ("Track n") or a rack's members (`g.owner.tracks`, "n name"), then
     Off; `set-route-label!` sets `n.route` to the track a label names,
     which stores a rack-owned graph's member index. A panel builds the
     options once and passes them to its rows. Legacy: a fixed
     sixteen "Track n" on a project-owned graph. The route color strips
     (`ggm-` / `gvr-route-color-strip`, no longer `:bindable`) take
     `n.route.color`.
   - **Weights:** `weight-rows` builds the matrix from the nodes' edges and
     their weight params (in the matrix's subtree), `set-weight!` sets one
     edge's; the group cells are `(chunks g.group-gain 4)` and
     `set-group-gain!` / `set-group-coupling!`.
   - **Playback**, each in a subtree: `g.events` and `#'g.beat` (the event
     views), `(column g.triggers)`, `(column g.energy)`, `g.dampening`,
     `(column g.group-activity)` / `g.group-suppression`; the 8x8's track
     heatmap `transport.track-events` and `#'transport.track-events-beat`,
     colored by the tracks; the keyboards each route track's
     `t.active-notes` with its color (legacy: the project's notes with a
     rack's colors).
   - **Batch edits** (global transpose, dur x, swing, the threshold up to
     the capacity, delay x, res/q x) stay the `graph-*` natives (unrecorded
     until eseq-0l17.53, now one undo entry per batch: §14.2k; since
     eseq-0l17.67 a param on every node is one recorded edit,
     `set-graph-params!`; delay x and res/q x stay natives)
     (graph-kit `set-param-on-nodes!`, `scale-delays!`,
     `shift-timebases!`, reading the current values from the kinds): a
     setter per node would record an entry per node, and the threshold
     writes dormant nodes no setter addresses. res/q x moves a resolution
     and a quantize alike, within its family (straight 1–64, triplets
     2T–64T), off and Prh kept. The factor pickers are one-shot (they show
     1). The explicit inits (`script-init-fn`) stay natives too: the graph
     is no kind instance until the next sync.
   - **View state** is a singleton per demo (`g8-view`: the keys' press
     depth; `ggm-view`, `gvr-view`: the pressed weight column's neuron;
     `g16c-view`: the cycle text typed, a `(dict :node :field :text :cycle)`
     per field). A 16-cycle field shows the text typed into it while the
     node's cycle is still the one that text set (or, a resolution naming
     no label, left), else the cycle, so an undo, a scene switch or an
     init shows the real cycle; it sets `n.resolution-cycle` /
     `quantize-cycle` to the labels it names: tokens among
     `graph-timebase-options` in any case, or the words the `graph-*`
     natives take (`parse_timebase_arg`: `whole` … `sixty-fourth`,
     `half-triplet` / `halftriplet` … `sixty-fourth-triplet`,
     `polyrhythm`), as their label; the rest (`off`, `none` included)
     dropped; a quantize field naming none is off. Each field is a subtree
     of its own, so a keystroke re-runs the fields, never the rows.
     Legacy: the native's lenient parse, the strings re-read on a pattern
     switch.
   - **Capture fixtures:** `graph-neural-8x8-piano-panel.lisp` renders
     `(g8-panel :notes …)` (the keyboard's notes in place of the tracks');
     `graph-homeostat-panel.lisp` sets `gvr-view.selected-neuron`.
   - **Legacy removed:** `SEQ.track-events` and
     `track-event-current-beat` (their registration, the pattern-sync and
     visualization-poll publishers, `build_track_output_events_value`,
     `build_track_output_current_beat_value`, `track_output_event_value`,
     `SequencerState::track_output_events` / `has_track_output_events`, the
     replay probe's hidden fields), the parity checks against them (now the
     rows themselves), the docs naming them.
   - **Kept** (eseq-0l17.22): `SEQ.graph-visualizations` and
     `track-active-notes` (alez.neural's panel; `panel_kinds_seed` reads
     the latter), the GRAPH namespace and `bind-graph*` / `graph-*-value`
     reads (alez.neural; the demos' explicit inits read
     `graph-config-value` once), `SEQ.groups`, `track-names`,
     `track-colors`, `instances`, `process-library` (alez.neural and
     others). `SEQ.current-pattern` has no `content/` reader left; its
     publisher and Rust tests remain.
   - **Not ported:** `packages/alez.neural/src/variable-reset.lisp`. Its
     expanded node editor is `eseq.sequencer`'s lane patchbay and the
     *processes* dock, which read a node's chain through the
     `graph-node-process-*` natives, and its bare-runtime tests (the
     package's graph visualization, node notes, processes dock and expr
     card tests) move to Harness tests with it. eseq-0l17.67 ported it
     (below).
   - **Tests.** `host_kinds::tests::graph_demos_view` (bare root; the
     owners test on the Distro root, whose step tabs it reads): the files
     and graph-kit use no legacy form; every demo keeps its handle, writes
     no override when loaded, registers its step tab, routes to the
     project's tracks, and under a rack owner publishes a rack-owned
     instance named for the rack whose routes are its members (a later
     member included) and stores a member index; the 8x8 panel's widgets,
     keyboard and bindings (an edit from elsewhere only repaints), its init
     ring (which propagates a seed through the engine's update), its
     control and cell edits (one entry each; a zero matrix is silent), its
     saved overrides and each scene's; the reset fork's batch and toggle
     edits; the variable graph's node count (dormant overrides kept), row
     lighting, route colors, seeds, threshold and selection; the group
     cells; res/q x keeping a triplet and Prh; the Markov init; the
     16-node controls and ring; the cycle fields' text and cycles (any
     case, the natives' words, an undo showing the restored cycle, a
     keystroke re-running only the fields); a weight-cell press re-running
     only the row highlights; playback re-running only the playback
     subtrees (four) and notes only the keyboard; a rack's graph tab
     restored from an empty scratch (its tab click showing the panel), and
     its rows whole through member churn (legacy and clip rack). They replace the bare-runtime demo tests
     in `lisp_host::tests`, `rack_sequencer_restore_tests` and the demo
     half of `graph_visualization_ui_tests`.
   - **Captures.** Every demo fixture, with and without its init, and the
     piano fixture render byte-identical outside the auto-rotating event
     views (which differ run to run). The homeostat fixture now lights
     row 3: its hook's selection shows (the legacy capture rendered
     before it). The untouched alez.neural fixtures differ in their event
     views only.
   Built (stage 8, eseq-0l17.11, part 1): the sequencer's compact grid
   (`ui/sequencer.lisp`: track rows, step cells, playhead bars, track and
   group headers, the rack clip run), `ui/track-collapse.lisp` and
   `ui/sequencer-keys.lisp`:
   - **Kinds.** `track.playhead-page :int` (L): the playhead's 16-step page,
     -1 while stopped (the expanded editor's page follow reads it; the grid's
     row lamps and playhead bars derive the page from `track.playhead` in
     the shader, so a page turn only repaints);
     `track.length-step :int` (L): the step a length lane (`length!`) last
     set the pattern length to, while playing, -1 otherwise
     (`track_process_length_step`; the legacy row field had no kind
     twin). `track.expanded` is a `:state` field of the host `track` kind
     (view state, not saved): one track's toggle re-renders that row alone,
     where a list in a view singleton re-rendered every row (and left the
     retained rows' stable widget ids stale, which
     `metal_seq_non_first_track_partial_layout_matches_full_layout_after_collapse`
     catches). The track kind is keyed by position, so the expanded tracks'
     tids are kept in `seq-view.expanded` (read by no render) and the inert
     `*seq-expand-sync*` projection re-applies them to `t.expanded` when
     tracks move: like legacy, an expansion follows its track through a
     delete, its undo and a reorder. eseqlisp: a badge's `muted-track-r/-g/-b` color its glyph
     while `:muted` (the header binds `:muted #'t.audible`, so the plain
     `track-r/-g/-b` carry the silenced, dimmed color).
   - **Shape.** `step-cell` takes the step and its track: the cursor frame
     is `seqv-step-cursor` (`step.index`, `track.num-steps`,
     `track.in-selection`, and `cursor` bound to the view singleton's
     `#'grid-cursor.step`), the shell `seqv-step-shell` (`step.active`, `lock-kind`,
     `held` (the duration span), `variant-color`, `track.color`, `audible`,
     `governed`; `selected` stays a scalar state bound to `#'s.selected`, so
     the renderer's selected rim follows it; 15 floats). The expanded
     editor's `seqv-slot-shell` shares its layers (the `step-shell-shader`
     macro, without the span). The playhead bar takes its track and row
     (`track.playhead`, `length-step`); each grid row's background is a
     `seqv-row-lamp` (its track and row) lighting the row number while the
     playhead plays in it. Shader colors: the take-governed dim and the
     silenced dim (the `silenced` macro, `eseq.view-kit/dimmed` in Lisp) are
     computed in the shaders from `track.governed` / `track.audible`,
     replacing the six `*-color-*-effective` channels. Headers bind `#'t.armed`, `muted`,
     `soloed`, `audible`, `volume`, `peak`, `governed`; group headers
     `#'b.muted` / `soloed` / `volume` / `peak` of `g.bus`, `#'g.armed`,
     `#'rc.active` per rack clip (each cell a subtree, so a rename re-runs
     one cell), the playing clip's number by value in a subtree of its own
     that alone reads the bank and the playing clip, the member activity
     dots `#'m.pad.triggered`. Render order (`grid-items`) is the mixer's
     with collapsed loose tracks left out; group blocks draw `g.tracks`
     (collapsed members hidden) and `g.racks`, and the shift-click range
     follows that order (`visible-tracks`, flattened from `grid-items`). Public functions take instances (`select-track-for-edit`,
     `track-click`, `track-menu-click`, `set-track-expanded`,
     `set-track-param-mode`, `set-track-cursor`, `track-cursor`,
     `track-param-mode`, `open-piano-roll-for-track`, `show-fx-for-group`,
     `track-header`, `grid-step-pointer-down` / `-up` (a step)); the capture
     fixtures and Rust tests pass `(eseq.kinds/track i)`.
   - **View state** (`:key ()` singletons, exported): `grid-cursor` (`step`,
     the shared step cursor mirrored by the cursor hook), `grid-select` (the
     range anchor, a track), `seq-view` (the expanded tracks' tids, and per
     track its param mode and step cursor as `(dict :tid tid …)` lists, like
     legacy keyed by tid so they survive a delete and its undo, entries of
     tracks no longer listed dropped on the next write; read by handlers
     only), `clip-menu` and `clip-rename` (the rack clip menu and rename,
     held clips checked `clip-listed?`).
   - **Behaviour changes.** The step cursor frame shows on the selected
     tracks' cells at the shared cursor step (legacy: each selected track's
     own stored cursor; they agree after any click); the stopped playhead
     bar draws nothing (the legacy row field showed column 0 after a full
     sync until the first stop); a track row stays highlighted while a
     group's bus owns the fx panel (`track.in-selection`, as the mixer's
     strips; legacy gated it off through `*sel-sync*`); the playing row's
     number shows on a blue lamp (a repaint) where legacy re-rendered it
     white.
   - **Not ported yet** (COMPAT(eseq-0l17.11)): the expanded step editor
     (the host's slot projection, keyed by `t.tid` now), its process lanes,
     lane strip and patchbay, the node bay (§14.2n), the pad grid (gidx
     based, for `effects/buffers.lisp`), `seqv-track-params.lisp`,
     `seq-grid-mode.lisp`, `step-grid-interactions.lisp`, `seq-panels.lisp`,
     `step-grid.lisp` (unloaded), `effects/step-buffer.lisp`, and the bus /
     group selection projection of `seq-core-state.lisp` (`*sel-sync*`,
     which the mixer's bus strips and the group blocks bind until the bus
     selection is a kind field). eseq-0l17.66 ported all of these but
     `step-grid.lisp`, `effects/step-buffer.lisp` and the bus / group
     selection projection (below).
   - **Legacy removed:** `SEQ.step-color-{r,g,b}-effective`,
     `track-color-{r,g,b}-effective`, `song-track-governed`,
     `track-playhead-page-*`, `track-playhead-active-*-*`,
     `track-playhead-row-*`, `track-playhead-row-active-*`,
     `track-length-row-*`, `track-selected-*`, `rack-clip-active-*`,
     `rack-clip-index-*`, `SEQV.sel-track-vis-*`, `seqv-track-cursor-*`,
     `cursor-field-*`, `cursor-step-*`, `track-expanded-*` (Lisp-written),
     with their builders and the tests that pinned them; the alias rows of
     the removed `track-collapse` and `track-selected-binding` names. Kept
     (eseq-0l17.22's list): unread since .11, `SEQ.track-mutes`,
     `track-solos`, `track-volumes`, `track-N-volume`, `track-peak-*`,
     `bus-mutes`, `bus-solos`, `bus-volumes`, `bus-peak-*`,
     `track-timebases`, `track-collapsed`, the per-step `seq-track-step-*`
     fields; still read,
     `track-muted-effective` (track panels), `record-armed`,
     `track-names`, `track-colors`, `track-ids`, `track-num-steps`,
     `num-tracks`, `current-track`, `selected-tracks`, `groups`,
     `bus-ids`, `armed-rack-id`, `rack-clips`, `rack-clip-banks`,
     `track-instrument-types`, `sync-labels`,
     `tp-num-steps`, `process-lanes`, and the expanded editor's and lanes'
     fields.
   - **Tests.** `host_kinds::tests::sequencer_view` (Distro root): the
     ported files and the grid half of `ui/sequencer.lisp` use no legacy
     form; the grid binds its host state and only repaints on playback;
     `playhead-page` / `length-step` follow the transport; a page turn
     re-renders no row; an expand re-renders one row; an expansion and its
     param mode follow their track through a delete and its undo. The rack
     clip test asserts a launch re-runs the number picker's subtree alone. The host-less editor seeds steps
     (`seed_kind_steps`, `set_kind_track_steps`, `kind_step`,
     `KindTrack::steps`) and the current track
     (`select_kind_current_track`); `layout_instance_field` and
     `binds_kind_field` read the bound instances. The known failures
     `metal_seq_sequencer_ellipsis_toggles_expanded_track_editor` (the
     test invoked an event handler with `(x y r)`) and
     `metal_seq_sequencer_hides_step_shells_beyond_pattern_length` (the
     grid renders spacers past the length, not hidden shells) pass with
     corrected tests. Captures: every sequencer fixture, the arrangement
     timeline, the node bays and scratch ones (lengths, solo, p-locks,
     selections, groups and racks, armed and muted, expanded editors in
     several modes) match the baseline but for the stopped playhead bar
     (above), the cursor frame on a range-selected track that never held
     its own cursor (`track-selection`; above) and the node bay's animated
     cube (nondeterministic: two captures of one build differ the same way).
   Built (stage 8, eseq-0l17.66, part 2 of the sequencer): the expanded
   step editor, its process lanes, lane strip and patchbay, the node bay,
   the drum rack pad grid and map (`ui/sequencer.lisp`, now whole),
   `ui/seqv-track-params.lisp`, `ui/step-grid-interactions.lisp`,
   `ui/seq-grid-mode.lisp`, `ui/seq-panels.lisp` and the cursor / page half
   of `ui/seq-core-state.lisp`:
   - **Kinds.** `track`'s `:state` gains `param-mode`, `cursor` and `page`
     (view state beside `expanded`; the inert `*seq-expand-sync*`
     projection writes all four from `seq-view`, keyed by tid, so they
     follow their track through a delete, its undo and a reorder). Action
     `set-fanout! fo bound v &key all` (a fan-out entry's `lo` / `hi`, the
     lane strip's range pickers). Nothing else was missing: the slots read
     `step.*`, the lanes `lane.values` / `min` / `max` / `position`, the
     strip and patchbay `process` / `port` / `fanout` / `inlet`, the pads
     `group.pads` / `pad.note` / `role` / `triggered`.
   - **Shape.** A slot is its step: the slider and toggle bind
     `#'s.velocity` (… by param mode, `tp/seqv-step-ref`, beside
     `seqv-step-value` and `seqv-set-step-value!` in one field table) and
     `#'s.selected`,
     the shell `seqv-slot-shell` takes the step and track, the length mark
     the track and index; a curved slider's and a lane slot's value are
     computed, so each such slot is a subtree of its own. The page shown
     is `t.page` (the playhead's page, `t.playhead-page`, while following);
     the cursor frame binds `#'t.cursor`; `seqv-step-cursor` takes
     `(index track cursor)` (the grid passes `:index s.index`). Lane edits
     go through `set-lane-steps!`, bypass through `set-process-enabled!`,
     inlets through `set-inlet!`, wiring through `bind-port!` /
     `add-fanout!` / `clear-port!` / `remove-fanout!`, cards through
     `move-process!` / `remove-process!`, the + box through `add-process!`
     (`lane-add` holds the pending class; `*lane-add-sync*` selects the
     new lane once the host lists it). The patchbay renders legacy-shaped
     entry dicts from one renderer: a track bay builds them from the kinds
     (`track-bay-entries`), a node bay from `graph-node-lane-patch`
     (COMPAT(eseq-0l17.20), ported by .67: node structure stays on the node natives;
     its run errors and scopes read the kinds' node processes,
     `lane-patch-run-error` / `process-scope-cells-for` → `p.error` /
     `p.cells`). The pad grid takes the group (`pad-grid g`, `pad-map g`):
     cells are MIDI-note positions, each binds `#'p.triggered`; a drop
     sets `p.note`, the role menu `p.role`, an empty cell queues
     `add-track-sample` for its note. View singletons: `lane-edit`,
     `patch-view` (pending cable, selected cable, node bay targets),
     `card-menu`, `lane-add`, `pad-view` (page per group, focused pad),
     `pad-menu`.
   - **COMPAT(eseq-0l17.14)**, for `ui/effects/*` (not edited here):
     `rack-pad-grid` / `rack-pad-map` / `selected-pad` /
     `open-pad-member-fx` take the group position `effects/buffers.lisp`
     passes; `map-slot` / `map-port` hand `eseq.effects.param-controls`'
     process map its legacy dict shapes and `armed-port` reads it back.
     (`seq-core-state/set-cursor-step-value` echoed the `fx-step-*` fields
     the *step* panel bound; the panel reads the kinds since .61, so it only
     moves the cursor.)
   - **Behaviour changes.** A following editor shows the playhead's page
     without moving the cursor (legacy moved the cursor frame with it); a
     focused pad is the pad instance, not its note (a pad moved to another
     note stays focused); the process-lane strip's title names the process
     (legacy showed `nil`, `process-lane-edit` capture); bypass, inlet and
     lane edits apply `:all` only to a project lane; a curved slider's
     slot re-runs its own subtree on an edit, a lane edit re-runs all
     sixteen lane slot subtrees (each reads `lane.values`), a linear one
     repaints; the row picker shows 0 in a lane mode with no lane; a card
     delete that leaves no lane puts the editor on transpose; a page click
     on another track's editor moves that track's cursor only (the
     cursor hook named the old current track until the host pushed the
     new one). The tabs, the strip, the patchbay, its card menu and the
     OTHER LANES row are subtrees of their own (a map, cable, card or
     inlet edit re-runs one, never an expanded row), and arming an out
     port only repaints: the port binds `patch-view.pending` as
     `:pending-port`, which the patch machinery compares with the port's
     id (eseqlisp `patch_port_pending`), and its shader compares the
     pending port's bay and slot (`pending-bay` / `pending-slot`), not the
     whole id, which a float rounds in a node bay.
   - **Legacy removed:** the host's expanded-step slot projection
     (`expanded_step.rs`, now `track_steps.rs` with what other code still
     uses; the viewport registry, `seqv-sync-expanded-step-slots` /
     `seqv-clear-expanded-step-slots`, `UiInvalidation::ExpandedStepViewport`)
     and its fields (`<slot-field>`, `<slot-param-field>`,
     `<slot-page-active-field>`, `<slot-bar-transpose-field>`,
     `<slot-bar-transpose-set-field>`, `seqv-cursor-param-value-*`,
     `seqv-cursor-sync-index-*`, `seqv-slot-length-active-*`); the
     per-step `seq-track-step-{active,duration,plocked,selected}-*-*` and
     `seq-track-step-param-{haptic,slider}-*` fields and the duration-span
     and p-lock render publishers; `SEQ.track-volumes`, `track-N-volume`,
     `track-mutes`, `track-solos`, `track-collapsed`, `bus-volumes`,
     `bus-mutes`, `bus-solos`, `track-timebases`, `track-lane-patch`,
     `process-run-errors`, `track-process-scopes`, `process-scope-cells`,
     `rack-pad-trigger-*` (the flags still feed `pad.triggered`); the alias
     rows `seq-set-process-lane-from-step`, `seqv-current-page`,
     `seqv-current-step`, `seqv-param-value-at`, `seqv-track-param-values`,
     `seqv-track-process-lanes`; the Rust tests that pinned them (ported to
     the kinds or deleted as parity checks). Kept (eseq-0l17.22's list):
     unread, `track-peak-*` and `bus-peak-*` (their publishers thread
     through the meter paths), the per-track step lists `track-velocities`,
     `track-durations`, `track-auxas`, `track-transposes`, `track-pans`,
     `track-syncs`, `track-delays`, `track-retrigs`, `track-retrig-rates`
     (one publisher with the current-track lists), `syncs`, `delays`,
     `auxas`, `track-step-plock-kinds`, `track-step-variant-{r,g,b}`,
     `step-plock-kinds`, `step-variant-{r,g,b}`, `track-muted-by-solo`,
     `playhead-page`; still read, `velocities`, `durations`, `transposes`,
     `pans`, `retrigs`, `retrig-rates`, `fx-step-*` (the *step* panel,
     .14), `tp-num-steps`, `tp-timebase`, `auto-follow` (track panels),
     `process-slots` (process panel), `track-process-slots` (a script
     demo), `track-process-lanes`, `track-process-lane-values`,
     `process-lanes` (alez.tracker, .20), `process-library`
     (alez.neural), `selected-steps`, `step-has-plocks`,
     `playhead-active-*`, `sync-labels`, `steps` (the unloaded
     `step-grid.lisp`).
   - **Tests.** `host_kinds::tests::sequencer_editor` (Distro root): the
     slots bind their steps and an edit only repaints; a curved slot's
     subtree; a cursor page turn re-runs the slots, not the row; a
     following editor's page; bar transposes; slot, lane and row-picker
     edits (selection, cursor step, UI step quantize, symmetric origin);
     the lane selector; a lane mode's strip and patchbay; wiring, fan-out,
     `set-fanout!` and cable delete; the + box; the strip's map onto a
     param tab; the enable dot's scope; card move and delete; the pad grid
     and map (note positions, drops, roles, the compat shims); node bay
     errors and scopes; multi-track select-all; the global cursor wrapped
     per track; two expanded rows' own tab and page; an inactive row's
     controls making its track current. `sequencer_view`'s legacy-forms
     test covers all seven ported files. Captures (the sequencer, lane,
     node bay and rack fixtures plus scratch expanded editors in every
     mode, lane strips and collapsed lanes) match the baseline but for the
     strip title above and the node bay's animated cube.
   Built (stage 8, eseq-0l17.67): alez.neural's `neural` panel
   (`packages/alez.neural/src/variable-reset.lisp`), the node bay of
   `ui/sequencer.lisp` and the chain reads of `ui/processes-buffer.lisp`:
   - **Kinds.** `process-class.node-label :string` (the class's name on a
     node: `graph_node_process_label`) and `node-hidden :bool` (does nothing
     on a node fire: `GRAPH_NODE_HIDDEN_PROCESS_CLASSES`), so the node bay's
     + box is a view derivation from `process-library.classes` (it was the
     `graph-node-process-classes` native). Action `(set-graph-params! g
     count name v)`: param `name` of nodes 0 to count - 1 at once, up to
     `g.max-nodes` (a dormant node keeps it), one undo entry (a drag's set!s
     join it): `set-graph` field `node-params` (`:param`, `:count`), a
     `GraphOverrideSlot::NodeParams` that undo restores node by node.
     graph-kit's `set-param-on-nodes!` uses it, so the demos' batch params
     are recorded too.
   - **Shape.** The panel is the demo's (§13 .64) over the instance's graph
     (`(graph-of self)`; an empty box until the host publishes it): config
     controls bind `#'g.node-count` / `reset-bars` / `max-poly`, the poly mode
     is a label; rows are subtrees (`graph-variable-reset-row-<n>` lit by
     `self.selected-neuron`, around `-row-controls-<n>`) binding `#'n.delay`,
     `#'n.seed-route`, `#'p.value` of node params, labels for route, group,
     resolution and quantize; the batch params bind node 0's and set with
     `set-param-on-nodes!`; the matrices read `weight-rows`, `column
     g.triggers` / `g.energy` and `g.dampening`, the event view `g.events` /
     `#'g.beat`, the piano the route tracks' `t.active-notes` and colors.
     A node's sounding readout is `n.sounding` in a subtree of its own
     (legacy: `bind-graph-node-notes` element bindings). The route menu is
     the graph's route tracks (graph-kit), then its owner's jakis (the
     generators of `g.owner` whose instance is an `alez/jaki:jaki`, labelled
     by `(instance-ref gen.gid).label`, " #id" on a shared label) gated, then
     restarted, then Off: `gate-generator!` / `set-route-label!`; the label
     shows `n.generator` / `n.restart`. The expanded editor's edge strips
     read `weight-rows` (a subtree), its patch is the sequencer's node bay;
     the process card reads `p.name`, `enabled`, `inlets` (`i.type`,
     `options`, `value`, `min`, `max`), `ports` (`mappable`,
     `target-step-param`), `expr`, `expr-source`, `promoted-expr`,
     `as-expr-reason`, `error`, `compile-error`, and edits with
     `set-process-enabled!`, `set-inlet!` (number pickers bind `#'i.value`),
     `bind-port!` to a payload field (map arming), `clear-port!` (unmap),
     `remove-process!`, `add-process!`; a track inlet is a dropdown of the
     route tracks then "nrn k" (stored -(k+1)), an enum one of its labels.
     The harmony meter takes the process. Expr actions (commit, promote,
     as expr, presets) stay eseq.expr-buffer's natives (§14.2m).
   - **The node bay** (`ui/sequencer.lisp`) builds its entries from the
     node's `n.processes` with the track bay's builder (`bay-entries` over
     `bay-processes`; each entry carries its `:process`) and edits through
     the process setters (bypass, wire, fan-out, cable delete, card move and
     delete); the COMPAT(eseq-0l17.20) natives path, `node-edit!`,
     `node-process`, `patch-view.node-version` and its touch are gone, and
     so are the COMPAT shims `process-scope-cells-for` /
     `lane-patch-run-error` (an expr card's run error is its entry's
     `p.error`). The *processes* dock finds the inspected card among
     `n.processes` and deletes it with `remove-process!` (its `defstate`s
     stay: eseq-0l17.22).
   - **View state** stays the instance's `:state` (`expanded-node`,
     `selected-neuron`, `map-slot` (a proc-id) / `map-port`, `piano-depth`).
     The on-create ring stays the natives (the graph is no kind instance
     until the next sync).
   - **Behaviour changes.** Project-owned routes are the project's tracks
     (legacy: a fixed sixteen "Track n"); a track inlet's options are the
     route tracks too. Every control edit is a recorded entry (the legacy
     natives recorded nothing but process edits), the batch params one
     entry for every node. A process card's number picker ranges are the
     inlet's (`i.min` / `max`: the class's, else a hint around the value;
     legacy 0 to 1). The node bay and dock show a native edit (an expr
     preset, a promote) at the host's next sync, a setter's when it lands
     (next frame). A sounding change re-runs one row's readout subtree
     (legacy: repainted bound elements). The piano colors and notes are the
     route tracks' (legacy: the project's notes with a rack's colors). The
     reset-seed toggle shows on for any `n.seed-on-reset` above 0
     (`(> n.seed-on-reset 0)`, as the .64 demos'; legacy: at 1 or above, so
     a value between 0 and 1 showed off).
   - **Legacy removed:** `SEQ.graph-visualizations` (its registration, the
     pattern-sync push, the visualization poll and its liveness state
     `VisualizationLiveness` / `last_visualization_poll_at`,
     `build_graph_visualizations_value` and its map builders,
     `SequencerState::has_graph_visualizations`); `SEQ.track-active-notes`
     (its registration and per-tick publisher, `prev_track_active_notes`,
     the replay probe's hidden field; `panel_kinds_seed` seeds
     `track.active-notes` from the panel dict's `:active-notes`); the GRAPH
     namespace: `bind-graph`, `bind-graph-edge`, `bind-graph-config`,
     `bind-graph-node-notes`, `graph-key`, `graph-edge-key`,
     `graph-config-key`, the `GRAPH` reactive namespace and the writes' echo
     into it; the per-graph `SEQ.graph-node-notes-<id>` publisher
     (`sync_graph_node_notes_fields`, `node_sounding_field`,
     `NODE_SOUNDING_STRIDE`, `SequencerState::graph_node_sounding_at`); the
     `graph-node-lane-patch` native and its builder; the Rust tests that
     pinned them (ported to value reads or deleted as parity checks).
   - **Kept** (eseq-0l17.22): the tracked value reads `graph-node-value`,
     `graph-param-value`, `graph-edge-value` (no content reader; Rust tests
     and scripts) and `graph-config-value` (the demos' and the kind's
     on-create inits); the node-process natives no content calls any more,
     only the Rust tests (their fixtures build and read node patches with
     them): `graph-node-process-chain`, `graph-node-process-classes`,
     `graph-node-process-enable`, `graph-node-process-remove`,
     `graph-node-process-move`, `graph-node-process-wire`,
     `graph-node-process-unwire`, `graph-node-process-fanout-add`,
     `graph-node-process-fanout-remove`, `graph-node-process-map`, and
     `graph-route-tracks`; unread now, `SEQ.process-library` and
     `SEQ.instances` (the Packages host commands still parse the latter);
     `SEQ.groups`, `track-names`, `track-colors` (read elsewhere); the
     expr-buffer natives (§14.2m, not covered); the `defstate`s of
     `ui/processes-buffer.lisp` and `ui/expr-buffer.lisp`.
   - **Tests.** `host_kinds::tests::neural_panel` (Distro root, a real
     instance): the file uses no legacy form and none of the node natives'
     reads; the kind registers on import; the panel binds its graph, edits
     through the setters (one entry each; a route off and its undo), lights
     a pressed column's row and edits a weight; the batch params are one
     entry (a drag one) and reach a dormant node; the route menu's tracks
     and jakis (two sharing a label) gate, restart and route back; a
     rack-owned instance's member routes, chip and piano; playback re-runs
     the four playback subtrees, notes the keyboard alone; sounding re-runs
     one readout; two instances' rings, matrices and view state stay apart;
     the node bay wires, fans out and deletes a selected cable (× chip and
     Backspace), bypasses, moves and deletes cards and adds by node label;
     the card's gate, enum and track inlets; the dock (its column, the tab
     strip, a source reload, the inspector, the code tile, promote and its
     undo, delete / enable / map / per-inlet drag undo, an expr commit's
     undo); the expr cards (edit buffer, error dot, overflow badge,
     presets, completion across commits, reloads and hide / show). They
     replace the bare-runtime `graph_visualization_ui_tests`,
     `graph_node_notes_ui_tests`, `processes_buffer_ui_tests` and
     `expr_card_ui_tests`, and the lisp_host route menu test.
     `host_kinds::tests::graph`: the event stream rows are checked as values
     (no legacy entry); `sequencer_editor`'s node bay test reads the
     process's error through the bay.
   - **Captures.** The node bay fixtures, the dock, and scratch panels
     (edits, a rack owner, jaki routes, inspectors of a gate / enum / track
     card, a mapped port, a bypassed card, the last node expanded, the dock
     on three cards) render byte-identical outside the auto-rotating event
     view (which differs run to run); the sequencer's lane and bay fixtures
     match .66's.
   Built (stage 8, eseq-0l17.65): the tracker package, `alez.tracker`
   (`packages/alez.tracker/src/ui.lisp`):
   - **Kinds.** `track.playhead-row` (live, `:int`): the row the playhead
     lights on a grid as tall as the longest pattern, where a shorter
     track repeats down its column: the transport's sixteenth modulo the
     grid's height when that repeat is the one playing (its row modulo the
     track's length is the track's playing step), else the track's own
     step (another timebase, an off-grid launch); -1 while stopped
     (`KindsHandles::playhead_row`, the legacy
     `sync_tracker_grid_playhead_fields` derivation), computed per tick
     only while observed, the grid's height once per tick (`TickMemo`);
     `selection.playhead-row` (live, `:int`): the current track's (-1
     without one), a scalar a view binds without reading
     `selection.track`; `track.step-params-in-use` (live,
     `(list-of :string)`): the step params (`focus-step-params`' names, in
     their order) some active step holds off its default, a scan of the
     active steps per tick while observed, pushed only when the set moved
     (`sync_step_params_in_use`, compared as a bit mask). The cells are
     `t.steps`' fields, the columns `param.step-locks` /
     `rack-macro.step-locks` / `has-locks` (7e), `lane.values` and
     `project.focus-step-params`. `eseq.kinds` gained
     `(set-step-param! s name v &key activate)`: a step's param on its
     track's live pattern (the `set-step` host command: the value rule,
     one undo entry, `set-step-param-history`'s recorded step mutation;
     `:activate true` turns the step on in that entry).
   - **Per-row live state.** A track row's box takes the `tracker-row-lamp`
     background (`:track t :row r`), whose shader compares
     `track.playhead-row` with its row; the row numbers'
     `tracker-gutter-lamp` compares the current track's
     (`:playhead-row #'selection.playhead-row`) and the cursor's row. The
     cursor is the view singleton `tracker-cursor` (track position, row,
     sub-column): every cell's `tracker-cursor-lamp` binds its fields to
     scalar states (`:cursor-track #'tracker-cursor.track`, …) and compares
     them with its own (`:cell-track :cell-row :cell-col`). So playback
     and a cursor move within a track repaint and re-render nothing (a
     cursor move repaints every cell, which parses nothing); a move to
     another track (the host's current track follows the cursor)
     re-renders the two track headers (`t.selected`, their highlight) and
     nothing else. The scroll follows `#'selection.playhead-row` while
     playing, `#'tracker-cursor.row` while stopped (only play and stop
     re-run the body). A key first clamps the cursor to the tracks, the
     grid's rows and the track's sub-columns and writes it back, so its
     lamp always shows the cell the key edits; placing the cursor
     (`set-cursor`) clamps the same way and drops a pending hex digit.
   - **Columns** are dicts over instances (`:kind` step, param, macro or
     lane; `:src` the step param's name, the param, the rack macro or the
     lane); the view computes each track's shown columns once a render
     (`track-layout`, from `has-locks` and `t.step-params-in-use`: it
     reads no step) and hands them to the header and row subtrees. Each
     grid row is a subtree holding one subtree per track (keyed as the
     track's row box, `tracker-row-<t>-<r>`), so a step edit re-renders
     that track's cells on the rows showing the step (a ghost row's too),
     a lock or lane edit that track's cells; the view re-runs only when a
     track's set of columns moves. Shown: the step params an active step
     holds off their default, the locked instrument and effect params,
     rack macros and MIDI effect params, in the legacy order, then the
     picker's added ones. Header spellings (`compact-label`, the legacy
     host's `compact_param_label`, now Lisp) are computed for the shown
     columns only (`track-layout`); the picker builds its items only while
     open, listing every param of the track's devices (`t.devices`,
     `t.midi-devices`, `device.macros`) and its lanes, checked against one
     `track-columns` read. The grid is as tall as the longest pattern, up
     to 256 rows (`MAX_STEPS`): the grid `track.playhead-row` lights.
     Keys: `step:<name>`,
     `param:<role>:<did>:<index>`, `macro:<index>`, `lane:<proc-id>:<inlet>`.
   - **Writes**, one undo entry each: a step's active, transpose and
     velocity through the step setters (`toggle!`, `set!`); a note through
     `set-step-param!` (`transpose`, clamped to its range, `:activate
     true`: a note typed into an empty step is one entry); a step param
     column through `set-step-param!` on the live step the cell shows (the
     legacy `set-automation-step-param` wrote the track's edit focus: a
     pinned take's or pattern's step, not the cell's); a device param
     `lock-param!` / `unlock-param!`, a rack macro `lock-rack-macro!` /
     `unlock-rack-macro!`, a lane `set-lane-steps!`. Vol is 00–7F, as the
     cells print it (`hex2` of `127 × velocity`): typing stores
     `(min 1 (/ n 127))`.
   - **View state:** `tracker-cursor`; `tracker-view` (added and hidden
     columns by track id, collapsed track ids, the pending hex digit, the
     octave and step advance); `tracker-menu` (the picker: open, at, its
     track, checked against `(tracks)` before it lists anything).
   - **Behaviour changes.** An added column shows its values (legacy: an
     added column was empty until it was a locked one); the gutter's
     playing row lights by its lamp (legacy: white text); an edit leaves
     the step selection alone (the step setters, where
     `seq-set-step-param` cleared it); a playing row's lamp tops the row's
     tint up to the playhead's (exact over the white bar and beat tints,
     near over a ghost row's color); a step param column writes the live
     step even with a take or pattern pinned (legacy: the edit focus's);
     typed Vol is n/127, as the cells print it (legacy: n/255, so typing
     `40` showed `1F`); the grid runs to the longest pattern, up to 256
     rows (legacy: at most 64); a note typed into an empty step is one
     undo entry (legacy: two); the column picker builds its items only
     while open.
   - **Learned:**
     - an `sdf/fill` of a translucent color shows at its alpha squared
       (the fill's output is premultiplied and the widget pipeline blends
       by source alpha): a wash returns its color with the shape's
       coverage as alpha;
     - a box draws its `:background` widget over its `background-color`,
       so a lamp lit only sometimes leaves the box's own fill
       pixel-exact; a box's rounded fill sits inside its one-pixel border
       (inset a lamp's shape by `(fwidth y)`);
     - the first key after a buffer is activated re-renders it, so a
       repaint-only test measures after one;
     - a shader reads a view's own singleton through a scalar state bound
       to it (`#'`), never by name: the WGSL corpus test
       (`content_shader_corpus_emits_valid_wgsl`) plans content shaders
       with only the host kinds loaded;
     - a parent subtree's re-run reuses its keyed children's renders but
       splices them in anew (a replacement is deep-copied), so a choice
       that must leave the rows alone binds a scalar
       (`selection.playhead-row`) rather than wrapping them in a subtree;
       a test tells a re-rendered widget from a kept one by its prop
       cells' identity (the global re-run counters count every buffer).
   - **Legacy removed:** `SEQ.track-automation`, `track-lock-targets`,
     `tracker-rows`, `track-grid-playhead-<t>`, `-row-<t>`, `-current` and
     `-row-current`, and `track-<t>-rack-macro-<k>-short-name`, with their
     publishers (`sync_track_automation_state` and its callers in the
     tick, pattern switch, step and piano roll invalidation paths,
     `sync_tracker_grid_playhead_fields`, `build_track_automation_value`,
     `build_tracker_rows_value`, `build_track_lock_targets_value`, the
     `PianoRollAutomationTarget` column model, `compact_param_label`); the
     `piano-roll-automation-refresh` command; the `set-automation-step-param`
     piano roll action; the pinned `eseq.vanilla/track-automation-wanted`
     (piano-roll.lisp); the tracker's SEQV channels; the perf probes'
     piano phase timing. The legacy tests (the publisher and package tests
     in `state_values::tests`, the tracker half of the rack macro name
     test, the piano roll step-locks parity check) moved to the Harness
     tests.
   - **Kept** (eseq-0l17.22): `eseq.bindings` (no content reader left; the
     roots import it, its own test stands; deleted by eseq-0l17.77),
     `SEQ.track-num-steps` and
     `track-ids` (sequencer), `track-process-lanes` and
     `track-process-lane-values` (seqv-track-params), `track-names`,
     `track-colors`, `current-track`, `playing` (many), the
     `set-track-plock-entry` / `clear-track-plock-entry` commands
     (effects/track-panels), `seq-set-process-lane-step`
     (step-grid-interactions) and the `track-<t>-rack-macro-<k>-name`
     fields (macro-state, track-panels).
   - **Tests.** `host_kinds::tests::tracker_view` (Distro root): the file
     uses no legacy form and no SEQV channel; the import installs and
     selects the tab, the cells read the steps (a ghost row its real step)
     and hide drops it; `track.playhead-row` (the repeat that plays, the
     real row when the clock disagrees, -1 stopped), which the row lamps
     bind and the gutter's and the scroll through
     `selection.playhead-row`, playback only repainting; the keys (a real
     key through the mode) edit the cursor's step, one entry each (a note
     into an empty step included), Vol as 00–7F, a pending digit dropped
     by a move or a click, a cursor left off the grid clamped back, and a
     cursor move only repaints; the columns (a step param off its default,
     a lock, added targets, hiding, typing, a clear, a nudge from the
     base, a collapse, no picker item while closed); a lane column; a rack
     macro column titled by its renamed macro; a step param column
     writing the live step while a take is pinned (one entry, the take
     untouched, the value rule); what re-renders (a toggle: that track's
     cells on that row; a lock: that track's cells; a new column: the
     view; a cross-track move: the two headers); the compact labels (the
     Rust test's cases).
   - **Captures.** `tracker`, `tracker-lengths` and scratch fixtures (the
     cursor, an added step param column, a collapsed track, a pending
     digit at octave 6) match but for the antialiased edges of the cursor
     cell and the cursor row's number (about 190 pixels: the shader's
     rounded rect against the box's), and the added Pan column, which now
     shows each active step's value (a dim 0, its default) where the
     legacy column showed `..`. The Seq tab renders byte-identical. The
     repo fixture `tracker.lisp` addresses tracks as instances
     (`(eseq.kinds/track 3)`, its first `t.lanes`) and opens the picker
     with an event's `:at`.
   Built (stage 8, eseq-0l17.76, legacy removal C of eseq-0l17.22): the
   song export (`ui/export-song.lisp`), Agent Mode (`ui/agent.lisp`) and
   Promote to factory (`ui/factory-promote.lisp`), and the end of the
   presented record's legacy mirror:
   - **Kinds.** `factory-promote` (`:key ()`, all M; the stage 7 inventory
     missed the `FACTORY_PROMOTE` namespace): `target` (what is promoted;
     `kind` is built in), `destination`, `skipped (list-of :string)`,
     `blocking`, `error`, `taken`. It is a presented area (`promote`,
     `present_promote`, pushed like the others when its generation moves)
     and a `present-fixture` area. The Lisp writes none of these: the open
     and commit stay host commands (`factory-promote-open` /
     `-commit`), whose errors and taken name the host presents. `song-export`
     and `agent` were built by 7f.
   - **View state** (`:key ()` singletons): `eseq.export-song`'s
     `export-draft` (`open`, the settings being typed: `name`, `range`,
     `start`, `end`, `rate`, `tail`), `eseq.agent`'s `agent-chat` (`conv`,
     the open conversation; `prompt`; `finalize-name`),
     `eseq.factory-promote`'s `promote-form` (`open`, `name`). The modules
     export the singletons in place of the `defstate`s (`open?`,
     `name-draft`, `range-draft`, `agent-current-conv`, `name`).
     `eseq.export-song/reset` takes the suggested name and end beat: the
     host calls it right after presenting the export, before the next tick
     pushes `song-export` (§13.1). Agent Mode's two buffers read
     `agent.generation`, so they re-render when an agent session changes;
     `ui/agent.lisp` gains its one import (`eseq.kinds`), which resolves in
     the bare runtimes its tests load it into.
   - **Legacy removed:** `presented/legacy.rs` (the mirror, its `Sink`
     impls and the `seq_registration` / `export_registration` /
     `agent_registration` re-exports), the `EXPORT` and `AGENT`
     registrations, `SEQ.editor-active` / `editor-mode` (the last reader,
     Shift+Tab in an instrument editor, asks the record:
     `presented::instrument_editor_open`), the `FACTORY_PROMOTE` namespace
     (`factory_promote::register_state`), the record's legacy-only
     `EditorView::active` and `LearnView::checkpoint_wav`, and the compat
     alias row `agent-current-conv`. The mutators' `rt` argument stays
     (unused) so their ~100 call sites did not change. `legacy_forms`
     flags `FACTORY_PROMOTE.` too.
   - **Tests.** `host_kinds::tests::modals_view` (Distro root): the three
     files use no legacy form; the export modal follows a job's
     transitions through the kinds with no reopening; Agent Mode re-renders
     on a new generation and not on an idle tick; the promote modal shows
     the presented promotion, turns Promote into Replace on a taken name
     and commits nothing while blocked. `host_kinds::tests::browser`:
     `song-export` through a job's completion, `factory-promote` from the
     record and a command's error. `presented::tests` lose the mirror and
     registration checks (`instrument_editor_open`, the promote area and
     its fixture added). The host-less tests seed `song-export` and
     `factory-promote` as the tick pushes them (`sync_export_kind`,
     `set_kind_field`) and address the view singletons; the agent tests
     drop their `AGENT` registrations.
   - **Captures.** 12 renders (`export-song` over the sequencer and the
     arrangement, `export-song-progress`, and scratch states: preparing,
     done, failed, three promotions (skips, blocked with an error, a taken
     name), a promotion over the arrangement, Agent Mode and its artifact
     panel): all 12 byte-identical. The repo fixture `export-song.lisp`
     passes `reset` its name and end and sets the range through
     `export-draft`.
   Built (stage 8, eseq-0l17.19): the drum rack lookups
   (`ui/drum-rack-v2.lisp`) and the *groove* buffer
   (`ui/rack-groove-buffer.lisp`), with their callers (`ui/sequencer.lisp`,
   `ui/mixer.lisp`, `ui/sequencer-keys.lisp`, `ui/browser.lisp`,
   `ui/step-grid-interactions.lisp`, `ui/effects/buffers.lisp`,
   `ui/effects/track-panels.lisp`):
   - **Kinds.** The buffer reads the playing groove's fields, its shares
     (`gr.pads`), `project.groove-pool` and `project.groove-library`; the
     lanes' hits read `q.pad.track.steps` (§14.2e). Added: the clip-addressed
     groove setter `(set-clip-groove! g rc field v :pad p)` and
     `use-library-groove!`'s `:clip`, so an edit to a clip that follows the
     rack's groove (it has no groove instance) can still be sent (below).
   - **Lookups.** `eseq.drum-rack-v2` is the group topology over the
     kinds, taking instances: `rack-of-bus` (the rack behind a bus
     position, or nil) and `selected-bus-rack` (the one behind the selected
     bus: the *fx* and *groove* buffers, the browser, Cmd+A),
     `rack-of-track`, `render-items` (the grid's and, with collapsed
     tracks, the mixer's top-level rows; both views' copies are gone),
     `visible-track-order` / `mixer-visible-track-order` (tracks) and
     `track-relative` (UP / DOWN, a track or nil), `scene-plays-clip?`
     (the kit export's default scenes), the pad grid geometry, the role
     menu's labelled `pad-role-options`, and the groove helpers
     `playing-groove` (`(or (and rc rc.groove) g.groove)`),
     `groove-of-track` (the track panel's swing hint) and
     `pool-groove-labels` (the picker's unique labels, which never
     depend on the library rows after them). The gidx-addressed lookups,
     the pad, clip and arm commands (`nudge-pad-note`, `set-pad-choke`,
     `launch-clip`, `toggle-armed`, …) and the groove commands went: the
     views use the kind setters and actions.
   - **View state.** `groove-extract` (the modal: open, the rack it
     extracts from, name, bars, resolution, quantize), `groove-rename` (the
     pool groove being renamed and its draft), both `:key ()` singletons
     in `eseq.rack-groove-buffer`. A held rack or pool
     groove is checked `listed?` before its command goes.
   - **Copy-on-first-edit.** A playing clip that follows the rack's groove
     gets its own on its first edit, and the edit lands on the copy, in one
     undo entry as the legacy commands did: `edit-groove!` sends the edit
     to the playing clip (`set-clip-groove!`, `set-groove` with the clip's
     id), and the host compares it with the groove the clip plays and
     applies it through `groove_target_mut`, which forks the copy inside
     the edit's entry (an amount that forks lands as a structure edit: the
     clip gains a groove instance). A drag forks on its first step.
   - **Bindings.** The amounts and shares bind (`#'gr.timing`, `#'q.amount`)
     and so do the on/off switch (`#'gr.enabled`) and the selected track's
     lane light (`:selected #'t.selected`), so a drag repaints only; the
     header, the amounts and the lanes are subtrees, and each pad row its
     own.
   - **COMPAT.** The pad grid shims for `ui/effects/buffers.lisp`
     (`rack-pad-grid`, `rack-pad-map`, `selected-pad`,
     `open-pad-member-fx`, COMPAT(eseq-0l17.14)) are gone: the rack panel
     takes the rack (`pad-grid g`, `pad-map g`), and its unused pad focus
     column (`rack-panel-controls`) was dropped.
   - **Legacy publishers removed:** `SEQ.groups` and `SEQ.group-collapsed`
     (`sync_groups_bindings` and its 20 calls, `build_groups_value`),
     `SEQ.rack-grooves`, `SEQ.groove-pool`, `SEQ.groove-library` and the
     `rack-groove-*` amount fields (`rack_groove_fields.rs` keeps only what
     the kinds share: lanes, grid labels, the library listing),
     `SEQ.rack-clips` and `SEQ.rack-clip-banks` (`sync_rack_clip_state`),
     `SEQ.armed-rack-id` (`prev_armed_rack`) and `SEQ.bus-ids`, with their
     registrations. Rust tests that asserted them moved to the model or the
     kinds (`host_kinds::tests::racks`), or went (the `build_groups_value`
     parity tests).
   - **Kept** (eseq-0l17.22): `SEQ.bus-names` and `SEQ.track-steps` (no
     content reader left; published with the mixer and per-track list
     families), `SEQ.track-names` (the param words' track list rides it),
     `SEQ.scene-launch-quantize` (the host kinds' launch-quantize source),
     `track-colors`, `current-track`, `num-tracks` (step-grid, unloaded), and
     the legacy pad map and groove host commands. Capture applies the kind
     setters too (`rack_kinds::apply_command`), so these have no production
     sender left (content, scripts, tools or capture fixtures):
     `set-rack-pad-note`, `set-rack-pad-choke-group`, `set-rack-pad-role`,
     `rename-rack-clip`, `set-rack-groove-amount`,
     `set-rack-groove-enabled`, `set-rack-groove-scale`,
     `set-rack-clip-own-groove`, `set-rack-groove-pad-amount`,
     `set-rack-groove-pad-enabled`, `rename-rack-groove`,
     `rename-library-groove` and `delete-library-groove`; the groove
     actions still send `set-rack-groove` (`use-library-groove!`),
     `extract-rack-groove`, `apply-rack-groove-to-all-clips`,
     `duplicate-pool-groove`, `delete-rack-groove` and
     `save-groove-to-library`.
   - **Tests.** `host_kinds::tests::rack_view` (Distro root): the files use
     no legacy form; the end-to-end groove test (extract, pick through the
     picker's own `:on-change`, amounts as one drag entry, pad shares and
     the include dot, the switch, the menu, Scale, a pad role's badge, and
     clips: the first edit forking in one undo entry that undo takes back
     whole, a drag forking once, Apply to All) drives the production host path and the scheduler, moved from
     `host_commands::rack_grooves` tests; the buffer binds its amounts,
     shares, switch and lane light, and an amount edit keeps its tree. The
     kit save, rack clip run, select-all, mixer range and visual order tests
     seed the kinds.
   - **Captures.** The repo `rack-groove-buffer` and `rack-pad-roles`
     fixtures address the rack as `(first (eseq.kinds/groups))` (the pad
     roles fixture lays its pads out with `(set! p.note …)` and
     `(set! p.role …)`); with them
     and scratch fixtures (straight, edited and bypassed grooves, a member
     selected, the track panel's hint, pads with roles and choke, nested
     groups and racks expanded and collapsed in the grid, mixer and *fx*,
     the kit save panel, the rack slot panels) every capture matches but
     the straight groove buffer (legacy: empty, since capture publishes the
     groups only after a hook's host command; now its pads' hits) and the
     mixer's pattern-cell play glyphs (nondeterministic: two captures of
     one build differ the same way).
   Built (stage 8, eseq-0l17.77, legacy removal B):
   - **Bus / group selection.** The selection stays the Lisp `defstate`
     `selected-bus` (the host reads and clears it by name, so no host
     field holds it); its highlight is a view-local kind in
     `eseq.seq-core-state`, `(def-kind bus-highlight :key (bus) :state
     ((selected false)))`, keyed by the bus instance (dropped with it).
     `*sel-sync*` stays the one reader of `selected-bus` and sets each
     bus's `selected` (only when it changes); the mixer's bus strips bind
     `(bus-selected-ref b)` and the mixer's group containers and the
     grid's group blocks `(group-selected-ref g)` (its bus's highlight;
     false without a bus, or for a bus dropped under a render). The SEQV
     `sel-bus-vis-*` / `sel-group-vis-*` fields,
     `bus-/group-selected-vis-binding`, `sel-*-vis-field` and the
     grid's `group-selected-binding` are gone.
   - **Deleted:** `ui/bindings.lisp` (`eseq.bindings`; its imports in
     `ui/main.lisp` and `ui/noui.lisp` and its state_values test), the
     unloaded `ui/step-grid.lisp` (its parse test and two ignored `*metal*`
     tests, the parse-gate list entry, `eseq.materials`'
     `slider-track-material` / `slider-track-muted-material`, which only it
     used, and the alias rows `metal-track-r/g/b`,
     `aqua-slider-track-material`, `aqua-slider-track-muted-material`), and
     the dead `*metal*` branches: the read-only key check, the soft step
     param and number picker arms and `current_metal_param_mode`
     (`input.rs`), the scroll reset and the visible checks
     (`reactive_tick.rs`), the step-selection list sync
     (`reactive_sync.rs`: `sync_step_selection_bindings` keeps the cursor
     fields; its `SEQ.selected-steps` writer, only `*metal*`'s, went with
     `UiInvalidationApplyCtx::active_delete_target`), and the content
     checks (`seq-step-tabs`, `step-grid-interactions`).
   - **Kept:** the mixer's positional shims `track-color-r/g/b` and
     `track-collapsed-label` (callers in `ui/effects/track-panels.lisp`,
     .74's), `eseq.browser/list-contains?` (the alias table's
     `sbrowser-list-contains?` target), the process map COMPAT(.14) shapes;
     the `SEQV` registration (`natives.rs`) and `SEQ.selected-steps`
     (registration and the redraw gate) for chunk A, and
     `SEQ.step-has-plocks`, which has no content reader left (the step grid
     read it): its publishers (`sync_shared_panel_state` and the fx sync)
     are dead and go in chunk A. `eseq.scene-banks`'
     `listed?` re-export had no referrer and is gone.
   - **Tests.** `host_kinds::tests::mixer_view::selecting_a_bus_or_group_lights_only_its_highlight_and_only_repaints`
     (Distro root, a group): `seq-core-state.lisp` uses no legacy form but
     its `defstate`s; the mixer binds both highlights and the grid the
     group's, neither binds SEQV; a group selection, a bus selection and
     none light exactly the selected bus's highlight, and neither view
     re-renders. The group block test and the drift probe read the
     highlight.
   - **Captures.** Scratch fixtures (a group and a rack, nothing, the main
     bus, the group and the rack selected; mixer and grid) match before
     and after.
   Built (stage 8, eseq-0l17.82): the device panels' layout from kinds.
   - **Adapter.** `ui/effects/panel-data.lisp` (`eseq.effects.panel-data`)
     builds the dicts the renderers take (synth, sampler, rack and its slot
     rows, track / bus / MIDI / rack slot effects) from the instrument
     device, track.devices, track.midi-devices, (buses) b.devices and the
     rack's slot devices. Each param dict carries `:prm`; controls bind
     `#'prm.value`, and a dict reads only structure by value (names,
     ranges, options, section, `visible`), never a param's value or text.
     The *fx* buffer builds each effect's dict inside that effect's subtree
     (`buffers`: each MIDI, chain and bus effect; `rack-selected-fx-panel`:
     the selected rack slot's effects, the only ones shown), so a
     structural change re-renders that panel alone. The instrument's is
     built inline: a custom instrument UI sets its render scope
     (`synth-ui-current-inst`, the selected section) as the tree evaluates,
     which a re-render of its subtree alone loses (fm-formant's voice page
     went missing); its dict reads no value or text, only which source
     settings show.
     `eseq.effects/device-panel` uses `instrument-panel-of` /
     `fx-panel-of`; the capture fixtures read `current-instrument-panel` /
     `current-effect-panels`. A node id comes from `devices/fx-node-id`
     (device.node-id). A bus effect's source type offers no `env`
     (`bus-source-param`, as the legacy builder dropped it); every
     instrument panel lists its base-note row first, as before; a drum
     rack slot holding any instrument (a param-less custom one too) shows
     its panel, an empty slot the drop panel.
   - **Option labels.** A dropdown binds its option to the param's value
     (`:value-index`, `pc/param-option-index`) when the value indexes its
     options (an enum's own, or the synced Delay's divisions,
     `:index-options`); otherwise (the keys tab, a dict that leaves options
     out) it shows `pc/fx-param-text-value-for` by value. So an option
     edit, or a p-lock under the playhead, repaints the dropdown and
     re-renders no panel (the generic grid, the sampler's and the built-in
     effects' plain dropdowns; a built-in that draws from an option, the
     Filter's curve or a choice button row, still reads it by value).
   - **New fields.** param.host-modulatable (model), device.instrument-name
     (model), device.strip-macros (model, pushed by the rack macro sync:
     the slot strip controls a rack macro maps, which replace
     `rack-slot-param-macro-owned?`), device.node-id (live, read only by
     the rack's selected chain and at event time). `PanelSection::of_device`
     places an effect's params (sidechain main, routing and host-only
     hidden, voice modulation sources `source`, no mod split) for
     param.section and visible.
   - **Epochs.** A param edit bumps `fx_epoch` only when its value
     redefines model data (`param_change_needs_fx_rebuild`: the sampler's
     `sens` and `slice`); option and toggle edits, their p-locks and a
     tensor cell bump none (`text`, `visible` and a tensor's values are
     live fields).
   - **Deleted:** the `SEQ.instrument-panel`, `SEQ.effects`,
     `SEQ.midi-effects` and `SEQ.bus-effects` publishers, registrations and
     builders (`build_*_value*`, the rack panel and rack slot builders,
     `filter_table_editor_value`, `EffectTableFields::insert_into`; the
     struct stays, the host's `device_table_fields` reads it), the dicts'
     `*-field` strings, every param value-field publisher
     (`sync_*_value_field*`, `sync_fx_param_binding_fields*`, rack macro
     name / value and slot strip syncs, the process effective syncs, the
     learn preview field writes) and their field-name helpers, the rack
     slot selection fields (`sync_all_rack_slot_selection_binding_fields`,
     unread), the reactive tick's shared and fx panel syncs, their epochs
     and timers, `fx_value_epoch` (the scene launch value patch) and
     `prev_selected_neural_neurons`, `VariantChip::legacy_map` (unused: the
     keys-tab chips build from d.variants), `rack_slot_type_name`; the
     mixer shims `track-color-r/g/b` and `track-collapsed-label`
     (track-panels reads view-kit's `track-color-part`) and their four
     alias rows. `refresh_instrument_panel_reactive` is the `ui_epoch` bump.
   - **Kept:** `SEQ.step-has-plocks` and
     `sync_instrument_plock_presence_fields` (eseq-0l17.78 deletes them),
     the step inspector's `fx-step-value-*` fields, `fx_epoch` (the kinds'
     structure gate), and the host-less test seeds' handle maps (test-local
     names). Follow-ups: the Delay's sync labels, the rack strip ranges and
     the bus `env` rule are duplicated in Lisp (`panel-data`).
   - **Tests.** `host_kinds::tests::panels_view`:
     `panel_layout_reads_the_kinds` (legacy-forms scan of buffers,
     panel-data and index, non-vacuous),
     `editing_a_param_repaints_without_rebuilding_the_panels` (a knob's
     value; fails if a dict reads `prm.value` by value),
     `a_p_locked_instrument_option_steps_under_the_playhead_without_rebuilding`
     and its drum rack slot effect twin (the *fx* revision holds while the
     playhead steps between two locked options),
     `a_synced_delay_time_edit_rebuilds_no_panel`,
     `adding_or_removing_an_effect_rebuilds_the_panels`; `devices`:
     `panel_device_fields_follow_the_host` (strip-macros on mapping and
     unmapping, node-id, host-modulatable),
     `a_rack_slot_eq8_drag_reads_one_coherent_band_through_the_kinds`;
     `params`: `an_option_or_toggle_edit_bumps_no_epoch_and_its_text_and_visible_follow`;
     `panel`: the sampler's placement and lanes pinned literally. The
     host-less suites seed the kinds from the App (`seed_app_panels`,
     through the host's `seed_device_params`, `strip_macros` and
     `EffectTableFields`) or a `PanelSeed`; pure parity tests of the
     deleted publishers are gone.
   - **Captures.** 340 jobs: all byte-identical but phaser-flanger-panel
     (wall-clock animation) and the two EQ8 spectra (x-f14a/x-f14b-eq8),
     which differ between two runs of one build too; four rack-sampler
     fixtures fail before and after alike (a missing IR).
9. **Diagnostics.** Re-render reason log, `describe-kind`. Useful from
   stage 6 on; can run in parallel with the ports.

Stages 1–3 touch only eseqlisp and can land before any host work.

### 13.1 Port playbook (stage 8)

How a factory area moves to the kinds (set by eseq-0l17.12, the transport;
the other port beads follow it):

1. **Baseline first.** Before touching the files, render every state the
   area's capture fixtures show (`metal_seq capture --script
   crates/sequencer/ui/capture-fixtures/<f>.lisp --buffer <b>`, the full
   DAW root, not `--noui`), plus scratch fixtures (in the scratchpad, never
   the repo) for states no fixture covers (menus open, a second bank, a
   rename in progress). The same script renders `after/`; compare pixel
   for pixel, then look at the pairs. Live state a capture cannot set (the
   transport playing: no tick runs) is covered by tests instead.
2. **Inventory.** `bd show` the port bead: its notes list every family
   the files read with its kind target (§14.4). Missing fields are added
   to `eseq.kinds` the host_kinds way (the `f::` constant, the `PUBLISHED`
   row, the push, a test), or split into a bead the port depends on.
3. **Rewrite, one rule per access:**
   - a value Lisp decides on (`if`, `str`, a list to iterate) → `t.x`;
   - a built-in widget prop or a `defwidget` scalar state the widget only
     draws → `#'t.x` (repaint only; a label's colour by state becomes
     `:active #'… :active-color …`); a widget that draws several fields of
     one thing takes the instance (`:track t`, §7.3);
   - a prop a widget does not declare takes no binding (a box's `:active`
     is forwarded to its `:background` widget; one without that state
     rejects a ref): pass a value there, or declare it in the `defwidget`'s
     `:state`; a ref on an undeclared `defwidget` prop is an error naming
     the widget and the prop (§7.3, eseq-0l17.68), no longer a vanished
     widget;
   - `defstate` / `(state …)` view state → a `:key ()` `:state` singleton
     per concern in the view's module (a menu: `open`, `at :point`, its
     target as an instance); keep instances, not indices, unless the index
     is the host command's address;
   - a raw host command that a kind setter covers → `set!` / `toggle!`
     (absolute setters: `toggle!` reads the cell, the setter compares when
     it lands); actions with no field stay host commands, addressed by
     stable ids (`b.bid`, `t.tid`) or by `s.index` where the command takes
     one;
   - `:bindable` is deleted; `(import eseq.kinds :refer (…))` heads the
     file; `&key` helpers (`(pill-label text on &key (font 9))`) where the
     same widget repeats with a few varying props;
   - string widget `:key`s that tests and `--key` captures address stay
     (they are layout identity, not bindings);
   - view state set synchronously right after a `present_*` or host edit
     (the host invoking a Lisp `open`) must not read the kind field the
     edit changes: the host kinds push it on the next tick, so the read
     sees the old value. Pass the value as an argument (the capture's
     duration to `eseq.retrospective/open`);
   - a held instance (a menu's target, the bank being renamed) can go
     stale while held (a project load, an undo, an edit from elsewhere):
     check it is still listed (`listed?`) before sending its id (never
     send a dropped instance's id, which can read 0).
   The authoring rules: no `\"` escapes inside Lisp strings, and
   (originally) no `cond` (`match` / `if` / `when` / `unless`); since
   eseq-0l17.46 `cond` is a `core/init.lisp` macro beside `when` / `unless`
   (§6), with multi-form clause bodies and an `else` / `true` final clause.
   Learned by the mixer (.13):
   - a binding cannot be negated: bind the positive field and put the lit
     look on the bound state (`:muted #'t.audible` with the silenced look
     on the plain props, a mute button lit by `:active #'t.audible`);
   - `select` is a special form: a local named `select` is called as the
     form (an `ArityMismatch`);
   - dotted event reads (`event.shift`) need a map: tests that invoke a
     handler pass a map event, with `:at` for anything opening a menu;
   - a setter that touches only UI-thread state (the delete target, the
     selection) applies at once through a native; only model edits, which
     need the `App`, are host commands that land next frame;
   - shared view helpers (menus, `listed?`, `nothing`, `prop-if`) come from
     `eseq.view-kit`, never a copy in the view; a helper module a view
     imports must have no import side effects;
   - host-less tests seed kinds before loading a bare-runtime view, or the
     last invalidation trace still shows the initial re-render; shared
     state modules that read `SEQ` at load need an (empty) `SEQ` namespace;
   - a capture may differ where a legacy publisher lagged a sync (live
     kind fields read the host at once): compare the pair, not the bytes.
   Learned by the piano roll (.16):
   - a host editor addressed by its own ids (the timeline's item ids)
     keeps them as a kind field (`note.item`) rather than being rebuilt
     over setters: setters that land one by one can collide (a nudge moving
     a note onto a neighbour not yet moved);
   - view state the host must act on after a sync (a fit waiting for a
     track's notes) is a singleton field plus an exported function the host
     invokes after the push, never a flat def Rust reads by name;
   - a capture's legacy publishers ran before `capture-after-sync` and the
     kinds after it: a hook's state (a selection, the playhead) shows only
     in the after capture;
   - `min` / `max` compile to an opcode in call position only: `(apply
     min xs)` is an unknown variable in a module (reduce over `min`);
   - a field of nil is an error: a chained read through a possibly stale
     handle (`x.device.track`) guards the middle step.
   Learned by the browser (.17):
   - 0 is falsy: a position guard is `(= i nil)`, never `(and i …)` (track
     0 read as missing);
   - a parameter named like a module function shadows it (resample's
     `open` took `duration` and broke `(duration)`): name arguments for
     what they hold;
   - a view that resets on "this field changed" (the browser's
     sample-search reset on a new track or sample) needs related fields
     pushed in one cycle: host-less tests push them together, as one sync;
   - host protocol state written from Rust by name becomes singleton
     fields reached through a local or a small exported helper
     (`show-loading!`), never a flat `defstate`.
   Learned by the arrangement (.15):
   - per-item view state a widget draws (one channel per lane) is one
     singleton the items all bind, with the owner as a bound field the
     widget compares to its own key (the timeline's `:lane-key`); a
     by-value owner check would re-run every item on each change;
   - `kind` and `id` are built-in instance fields (and `key` on a keyed
     kind): a field of any of these names is a compile error naming the
     field and the kind (§3.1, eseq-0l17.60). Before, it failed when the
     form ran, and a failed `(import eseq.kinds)` under the boot's
     `eval_str` was silent: the whole module's kinds were just missing;
   - `menu-of` passes its items as one list, and a list nested in it is
     dropped: build conditional items as one list and `apply` it;
   - a subtree's key replaces its root widget's (qualified) `:key`: a
     keyed widget that tests or captures find stays below an unkeyed root
     (the lane subtrees wrap their timeline in a flex `h-stack`);
   - a channel every lane binds repaints every lane: the timeline keeps
     each widget's parsed `:items` while the items list is the same
     (`cached_items`), so the repaint parses nothing;
   - headless capture drops the transport-class song commands (the clip
     and region selection): fixtures that select go through the kind
     setters, which capture applies.
   Learned by the sequencer grid (.11):
   - per-item view state that picks an item's structure (a track row's
     expanded editor) lives on the item: a `:state` field of the host kind
     (`track.expanded`) re-renders that item's subtree alone, where a list
     in a view singleton re-renders every reader;
   - a `defwidget` state the renderer itself reads (`selected` picks a
     box's `:selected-color`, the shader's `input-color`) stays a scalar
     state bound with `#'`, not an instance field the shader reads;
   - per-cell highlights of one moving thing (the step cursor) bind a
     singleton field into each cell and compare it with the cell's own
     instance in the shader (`:cursor #'grid-cursor.step` against
     `step.index`), with no per-cell field; a view singleton is bound, not
     read in the shader, since the shader corpus plans widgets with the host
     kinds alone;
   - a box with a `:background` widget clips its children to its rect: a
     lamp behind a label that overflows its own box goes on an enclosing
     box (the grid row), not the label's;
   - a `:state` field of a position-keyed host kind (`track`) stays with the
     position: view state that must follow the item through a delete or a
     reorder keeps the stable ids in a view singleton no render reads, and
     an inert projection writes the field from them (`*seq-expand-sync*`).
   Learned by the effect panels (.61):
   - a module's view singleton is not read through a qualified dotted path
     from another module (`tp/plock-table.row` compiles to an unknown
     variable): the owner exports a function (`clear-plock-row!`); a
     test reaches it with `(let ((v mod/singleton)) v.field)`;
   - a `list-of` field cannot be bound per element: a value a widget draws
     from one entry (an LFO's phase among `device.mod-phases`) needs a
     scalar field (`param.mod-phase`) or a by-value read in a small subtree;
     mind the observed-mask budget: a device's holds 31 live fields and
     `params` (32 of the 64 bits of `ObservedMask` since eseq-0l17.71;
     `LiveFields::of` and `DEVICE_PARAMS_BIT` assert the limit);
   - fields computed only while observed (a sampler's media) read their
     defaults on the frame that first observes them: capture syncs the host
     kinds again after its first frame;
   - a text input reads its own `:value` prop on every key: a host field it
     shows must be pushed (the frame's sync) before the next key, as the
     legacy synchronous field was; a host-less test sets the field and runs
     a cycle and a side-effect refresh after each edit lands;
   - a panel dict whose `:options` leave entries out is no index by value
     (an effect's source types without `env`): read the param's `text`.
   Learned by the expanded editor and lanes (.66):
   - a `defwidget` that takes `id` or `kind` as a singleton `:state` field
     fails the schema (the card menu's target is `proc-id`), and a name a
     module defines that is also a compat alias key (`page-size`) logs a
     migration warning: name new helpers apart (`slots-per-page`);
   - dotted access applies to a symbol, not a call: `(nth xs i).f` is
     invalid, bind it first (`(let ((x (nth xs i))) x.f)`);
   - an empty optional row (a scope with no cells) still takes a gap slot
     in its stack: group it with its neighbour in a gap-0 stack, or render
     a zero-height spacer, to keep the layout byte-identical;
   - a view shared by two hosts of one shape (a track's chain and a graph
     node's) can keep one renderer over plain entry dicts and build the
     entries per host, so one host ports without the other;
   - a COMPAT shim for a caller outside the bead (`effects/*`) takes the
     caller's old address (`gidx`) and resolves the instance, tagged with
     the bead that ports the caller;
   - a `#'` binding on a prop a `defwidget` does not declare drops the
     widget without an error (the patchbay's out ports vanished): declare
     the prop as a state even when the shader does not read it;
   - a shader compares floats: an id past 2^24 (a node bay's port ids)
     rounds onto its neighbours, so compare small parts of it;
   - `sgi/set-track-cursor-step` hooks the cursor to the current track,
     which reads stale right after `select-track-for-edit`: a gesture that
     knows its track names it, `sgi/set-cursor-step-for-track` (since
     eseq-0l17.69 the shared gesture paths do: `step-pointer-down-for-track`,
     the drag-over paths and `step-select-drag-start-for-track` pass their
     track, so a press, shift-click or duration-edge drag on another track's
     row moves that track's cursor and page only; `set-track-cursor-step`
     and `step-select-drag-start` remain the current-track forms the keyboard
     uses; the legacy `step-grid.lisp` used them too until eseq-0l17.77
     deleted it).
   Learned by alez.neural (.67):
   - a Harness test's `drain` takes every queued host command, the
     editor's own (a `set-layout`, a buffer switch) included: run the
     reactive cycle and `refresh_runtime_side_effects` before draining;
   - a tile remembers its selected tab per tab set, and a layout spec's
     `:buf` does not override it: a test switches tabs through the tab
     strip (a click), as the user does, not by setting
     `step-panel-buffer`;
   - a host-created instance's step tab is registered and opened by the
     host: a Harness test needs no `seq-register-instance-tab`;
   - numbers a kind stores as f32 (an inlet's value) read back rounded
     (0.4 is 0.4000000059604645): compare them with a tolerance;
   - `(instance-ref gen.gid)` is a created instance's record (its `kind`
     and `label`) for an instance's generator, nil for a script's: the way
     to filter `(generators)` by kind;
   - a view that edits through setters and through natives that answer at
     once (expr presets, promote) reads the native's result as a value and
     the kinds' list at the next sync: select by id, and let the render
     find it when it lands.
   Learned by the drum rack (.19):
   - an edit whose target instance does not exist yet (a clip's own
     groove, which its first edit creates) is addressed by its owner (the
     clip: `set-clip-groove!`) and resolved by the host when it lands, in
     one entry; never two edits chained through view state;
   - headless capture applies a kind setter only where its host module
     exposes one (`rack_kinds::apply_command`, beside the legacy routes in
     `capture.rs`): add one before a fixture needs it;
   - a capture whose hook sends no host command never published the legacy
     group fields: a legacy view of them rendered empty where the kinds
     show the state.
   Learned by the device panels (.82):
   - renderers over a legacy dict shape port whole when one module
     (`eseq.effects.panel-data`) builds the same dicts from the kinds and
     each param dict carries its instance (`:prm`): the renderers bind
     `#'prm.value` and read only structure by value (names, ranges,
     options, section, visible);
   - a dict built by value inside the subtree that draws it re-renders
     only that subtree when its device's structure changes; built above
     it (the *fx* root), every tick a read field moves re-renders the
     whole tile. A custom UI that sets render-scope globals as it
     evaluates (the custom instrument UIs) cannot move into a subtree of
     its own: keep its by-value reads structural instead;
   - a binding read as a value is its value: testing `#'prm.value` for
     truth (`(if (index-of p) ...)`) reads the param and subscribes the
     subtree; test a predicate, return the binding;
   - a dropdown's label follows a value without a re-render through
     `:value-index` bound to it (the widget rounds and clamps); a label
     read by value (`param.text`) re-renders the dropdown's subtree on
     every change, a p-lock under the playhead included;
   - the panel's placement rules are the host's (`PanelSection::of_device`:
     an effect's sidechain params are main, routing and host-only params
     hidden, voice modulation sources `source`): publish them as
     `param.section` / `visible`, never re-derive them in Lisp;
   - a host-less test that renders panels seeds the kinds from the App
     (`seed_app_panels`: descriptors, racks, macros, buses) through the
     host's own functions where it can (`seed_device_params`,
     `strip_macros`, `EffectTableFields`); what the seed computes itself
     (live values at the displayed step, media) is tested on a Harness,
     against the real sync;
   - `ESEQ_RERENDER_LOG=1` names the field a subtree re-rendered on: the
     quickest way to find a stray by-value read.
4. **Legacy publishers.** For each family the area read, grep every
   reader and mention: `content/` Lisp, all of `crates/` Rust (tests and
   capture fixtures included), `tools/` and `docs/` (the compat alias
   table `tools/module-compat-aliases.tsv`, and specs naming the old
   names). Unread now → delete its registration (`natives.rs`), every
   publisher, the host commands and frame fields only it used, the alias
   rows of names the port removed, and port the Rust tests that asserted
   it to the kind field (or delete a pure parity check); fix the docs that
   describe it. Still read elsewhere → keep it and list it on
   eseq-0l17.22. Mark the §14.4 rows.
5. **Tests.** Port the area's Rust tests: tests on the host-less editor
   (`full_grid_editor_for_scroll_tests`) push kind fields with
   `set_kind_field` / `seed_kind_scene_banks` (they mirror the host's
   push); behaviour that needs the host (a project load, live fields)
   moves to a `host_kinds::tests` Harness (`UiRoot::Distro` for the
   factory). Add the area's files to a `legacy_forms` regression test and
   assert its key widgets bind through kinds.
6. **Verify:** the area's tests, `cargo nextest run -p eseqlisp`, the full
   `-p sequencer` run against the known failures, `cargo check -p
   sequencer`, and the after captures.

## 14. Factory inventory (stage 7)

Every legacy reactive access in `content/` (`ui/**`, `packages/**`,
`scripts/**`, and the factory `instruments/`, `effects/`, `midi-fx/` UIs),
mapped to a kind field, a view-local singleton kind, or removal. Taken
2026-10-05 by scanning for `SEQ.x` / `NS.x`, `bind-seq`, `bind-seq-nth`,
`(bind "NS" …)`, `bind-nth`, `reactive-get`, `reactive-set`, `bind-graph`,
`reactive-value` and `:bindable`, with string-built names (`(str
"seq-track-step-active-" t "-" s)`, `(slot-field "active" t s)`, the panel
data's `:value-field` strings) normalised to their field family.

**Counts.** 357 families: `SEQ` 306, `SEQV` 18, `EXPORT` 10, `RETRO` 7,
`AUDIO` 3, `MIDI` 3, `THEME` 3, `GRAPH` 2 (plus 103 `bind-graph` /
`bind-graph-config` calls), `AGENT` 1, and `:bindable` / `reactive-value` /
generic `bind` helpers 4. Mapped to kinds built in this stage: 88. Mapped to
kinds of the follow-up beads: 238 (`param` and p-locks 18, lanes 10,
arrangement 22, piano roll 22, browser/editor/learn/retro/export/settings
singletons 89, graph and neural 13, drum rack 10, track settings / routing /
selection extras 54). View-local singleton kinds: 17. Removed: 10. Kept
(`THEME`, package instances): 4.

### 14.1 Decisions

- **`SEQV` is view state, not host state.** No Rust code writes `SEQV`; the
  views `reactive-set` it themselves (arrangement view range, the grid
  cursor, expanded tracks, the p-lock menu target and variant color, the
  piano roll's arrangement mode, rack clip centering, selection visuals).
  Each becomes a `:key ()` `:state` singleton in its view module (`(def-kind
  arrangement-view :key () :state ((start 0) (duration 16) …))`), so it needs
  no host publishing. `SEQV.<sel-track-vis>` is `track.selected`.
- **Device parameters are `param`, keyed `(device index)`** under the
  existing `device` kind and registered lazily on the first read of
  `d.params`, like steps. `param.value` is the displayed value (the p-lock at
  the selected or playing step, else the base), as today's
  `track-N-…-param-…` fields; `base`, `locked`, `has-locks` carry the p-lock
  state. Bus effects, MIDI effects and rack slots get devices too. Bead
  eseq-0l17.28 (7b).
- **Sends split base and display the same way.** `send.amount` is the
  track's own send level (the base, like `param.base`): its `:set` changes
  that level (host command `set-track-send-base`, addressed by bus id) and
  never p-locks. `send.display` is the level shown (like `param.value`: on
  the selected track the p-lock at the selected or playing step, else
  `amount`), and `send.locked` says a p-lock supplies it. Both are
  read-only; locking a send is a p-lock edit (7b), not a `set!` of
  `amount`.
- **List fields become per-instance fields.** `SEQ.track-volumes`,
  `track-mutes`, `velocities`, `track-colors`, … (lists indexed by position)
  map to the field of each instance; the `…-effective` color channels map to
  `track.color` dimmed by `track.audible` in the shader.
- **Derived values move into views.** Playhead page and row, length rows,
  expanded-step slot projections, slider/haptic normalisations: a view
  computes them from `track.playhead`, `track.num-steps` and the step
  fields. `num-tracks`, `num-patterns`, `track-ids`, `bus-ids`,
  `delete-target-version` and the `*-view-generation` counters are removed:
  collections and instance identity replace them.
- **Option lists.** Lists the host owns or derives (the scales, the step
  sync resolutions, the accumulators, the track outputs, the step params
  a process port writes) are `project` fields the host publishes
  (`project.fts-options`, `sync-options`, `accumulator-options`,
  `output-options`, since 7c `step-param-options`); short fixed enums
  (`mute-group-options`, `accum-mode-options`, `tuning-root-options`, …)
  are `eseq.kinds` constants the schema tests hold to the host's (7i).
- **`THEME` stays** (theme namespace, not host model state).

### 14.2 Built in stage 7

`eseq.kinds` gains the kinds `bus`, `group`, `send`, `master`, `engine` and
fields on `track`, `step`, `transport`, `selection` and `project`; the
schema check covers them all. (L) = `Feed::Live`, observed-gated; others
Model.

| Kind | Key | New `:host` fields (`:set` in brackets) |
|---|---|---|
| `track` | `(index)` | `pan :number` (L) [`seq-set-track-pan`], `soloed :bool` (L) [`seq-set-track-solo`], `collapsed :bool` (L) [`seq-set-track-collapsed`], `playhead :int` (L, -1 stopped), `timebase :string` (L), `instrument-type :string`, `rack :bool`, `group group`, `sends (list-of send)` |
| `step` | `(track index)` | `held :bool` (L: inside an active step's duration, that step included), `velocity`, `duration`, `transpose`, `delay`, `retrig`, `retrig-rate`, `pan`, `sync`, `aux-a` (all `:number` (L) [`seq-set-track-step-param`]) |
| `send` | `(track bus-id)` | `track track`, `bus bus`, `amount :number` (L, the base) [host command `set-track-send-base`], `display :number` (L, the shown level), `locked :bool` (L) |
| `bus` | `(index)` | `index :int`, `bid :int` (the stable `BusId`), `name :string`, `volume :number` [`seq-set-bus-volume`], `muted :bool` [`seq-set-bus-mute`], `soloed :bool` [`seq-set-bus-solo`], `peak :number` (L) |
| `group` | `(index)` | `index :int`, `gid :int`, `name :string`, `color :rgb`, `collapsed :bool` [`seq-set-group-collapsed`], `rack :bool`, `tracks (list-of track)`, `bus bus` |
| `transport` | `()` | `bpm :int` (L) [`seq-set-bpm`], `position :int` (L), `metronome :bool` (L) [host command `set-metronome`], `roll-mode :bool` (L) [`set-roll-mode`], `record-quantize :string` (L) [`set-record-quantize`] |
| `master` | `()` | `peak-l`, `peak-r :number` (L), `recording :bool` (L) [`seq-set-master-recording`] |
| `engine` | `()` | `cpu-load :number` (L), `latency-ms :number` (L) |
| `selection` | `()` | `tracks (list-of track)` (L) |
| `project` | `()` | `buses (list-of bus)`, `groups (list-of group)`; `(buses)`, `(groups)` |

- **New absolute natives** (the `:set` rule of stage 4: compared when the
  command lands, so two `set!`s never undo each other):
  `seq-set-track-solo` (slice-3 op `set-solo`), `seq-set-track-collapsed`,
  `seq-set-bus-mute` / `seq-set-bus-solo` (bus mixer ops `set-mute` /
  `set-solo`), `seq-set-group-collapsed`, `seq-set-master-recording`,
  `seq-set-track-step-param` (any track, in the param's range (since
  eseq-0l17.38 the value rule, §14.2c: out of range is an error), one undo entry, no
  selection side effect), host commands `set-metronome`, `set-roll-mode`
  and `set-track-send-base` (`:track`, `:bus-id`, `:amount`: the base
  level, never a p-lock; the bus by id, so a reorder before the command
  lands cannot retarget it). `seq-set-bpm`, `seq-set-bus-volume`,
  `seq-set-track-pan` and `set-record-quantize` were absolute already. The
  legacy `set-track-bus-send` (mixer, MIDI Mix, track panel) p-locks the
  selected steps only when its track is the current track; on any other
  track it sets the base.
- **Identity.** Buses by `BusId`, groups by group id (`reconcile`, so a bus
  reorder re-keys); both are dropped and re-registered on a project load
  (the track registry's generation moves), like tracks: once, at the top
  of the model sync, so a registry that lags the track list for a few
  ticks retries the track model without re-registering buses or groups.
  Non-distinct bus or group ids leave the model sync pending (retried next
  tick), like scene and bank ids. Sends are keyed
  (track instance id, bus id), one per bus but the main mix, registered
  with their track and dropped with it or with their bus. `project.buses`
  / `project.groups` follow.
- **Feeds.** Bus volume/mute/solo are `App` state a fader drag changes
  without moving a model counter; the tick compares them every tick (a
  handful of buses), like the queued scene. A bus id list or group change
  (`app.groups`, which the tick pulls from the natives' shared copy) forces
  a model sync; the tick pushes them only when they differ from the last
  push. `send.display` is the legacy displayed send level
  (`track-N-bus-M-send`, `tp-bus-M-send`; one helper,
  `displayed_track_send_amount`, serves both). The send and bus live loops
  run only while some instance observes a field (a union cached per
  observer epoch, like steps'); `selection.tracks` is pushed only when the
  sorted selection changes. `step.held` and the legacy
  `seq-track-step-duration-*` share one scan (`track_held_steps`).
  Step value fields (`held` and the parameters) are diffed per tick in
  place, like `active`, but only the fields some step of the track
  observes: the per-track observer cache is now the union mask of the step
  instances' observed fields (step-parameter edits bump no epoch). Meters:
  `KindsMeters` (track, bus, master levels and the CPU load) replaces the
  track-peak slice; `wants_bus_peaks` / `wants_master_peaks` keep the meter
  cache polled while only a kind field observes them.

### 14.2b Built in stage 7b (eseq-0l17.28)

| Kind | Key | New `:host` fields (`:set` in brackets) |
|---|---|---|
| `param` | `(device index)` | `device device`, `index :int`, `name :string`, `min`, `max`, `default :number`, `type :string` (`continuous`, `enum`, `boolean`), `options (list-of :string)`, `unit :string`, `percent :bool` (since .14: a fraction % param, display = 100 × stored), `value :number` (L, shown), `base :number` (L) [host command `set-device-param`], `locked :bool` (L), `has-locks :bool` (L), `text :string` (L), `printing :bool` (L) |
| `device` | `(track did)` (was `(track slot)`) | `did :int`, `type :string`, `params (list-of param)` (lazy; then Model), `playhead :number` (L) |
| `track` | `(index)` | `tid :int` (the stable `TrackId`) |
| `step` | `(track index)` | `plocked :bool`, `lock-kind :int` (`lock-none`, `lock-seq`, `lock-variant`), `variant-color :rgb` (all L) |
| `send` | `(track bus-id)` | `has-locks :bool` (L) |

Built (7b):

- **Names.** As for sends (§14.1): `param.base` is the device's own
  value, what its `:set` changes (never a p-lock); `param.value` is the
  value shown: on the current track the p-lock in force at the selected
  (else, while playing, the playing) step, with the off-step hold of
  `held_plock_value`, else the base under any engaged macro override;
  `locked` says a p-lock supplies `value`; `has-locks` says some step of
  the pattern (the first `num-steps`, like `send.has-locks`) locks the
  param (the knob's automation dot, legacy `track-plock-any` /
  `plk-…-any`); `printing` says a live print latch holds the param while
  playing and recording (legacy `track-plock-printing` / `plk-…-print`).
  `text` is the option an enum value selects (rounded, clamped:
  `ParamDescriptor::option_label`, which the effect panels use too),
  `on`/`off` for a boolean, empty for a continuous param. `type` replaces
  a `boolean` flag (`kind` is a built-in instance field). The legacy
  `plk-…-def` projection is `param.base`; `plk-…-on` is `param.locked`
  (also true while playing a locked step, like `send.locked`).
  `step.lock-kind` is an `:int` (bindable) named by the exported
  constants `lock-none` (0), `lock-seq` (1, sequencer-only locks) and
  `lock-variant` (2, a p-lock variant).
- **One unit convention.** Every param's `value`, `base`, `min`, `max`
  and `default`, and every setter, speak display units, for instrument
  and effect params alike: a percent param reads and takes 0–100
  (`stored_to_user` / `user_input_to_stored` through
  `DeviceSlot::to_user` / `from_user_clamped`). Effect params' legacy
  `track-N-fx-*` fields stay in stored units; they agree with `param` for
  every non-percent param.
- **Identity.** Devices are keyed (track instance id, `did`): 0 for the
  instrument, else the effect's stable instance id
  (`DeviceIdentityRegistry`, bound by recorded chain edits and project
  loads; an effect slot with none bound yet uses 2^52 + slot until one
  is). A reorder keeps a device's instance and its params (only `slot`
  moves). Params are keyed (device instance id, descriptor index) and
  registered lazily, on the first read of `d.params` (the reader hook,
  or the tick once `params` is observed), each registration pushing
  `index`, `device` and the descriptor fields; `d.params` is then a model
  field. A descriptor change in a device (an effect replaced in its slot,
  which adopts the replaced instance id; an instrument swap or a preset
  load that changes the descriptor) drops its params and, when they were
  registered, registers fresh ones and re-pushes `d.params`: old handles
  go stale. Deleting the device (or its track, or a project load) drops
  its params. `device.type` says what the device is (the instrument type,
  `sampler`, `synth`, …, or the effect, `Filter`, …).
- **Feeds.** The model sync keeps, per device, a `DeviceSource`
  (`host_kinds/params.rs`: track position, `DeviceSlot`, the descriptor's
  params, the sampler voices) in `KindsShared`, replaced only when the
  descriptor or position changed, with the effect slot counts per track;
  the tick copies the macro engine's override layer when it changes. So
  every param field the reader may be asked for is computable without the
  `App`: descriptor fields are pushed once, at registration; the rest are
  Live, from the instrument or effect slot (`defaults`, `plocks`), the
  print latch (`StepPrintState::holds`) and the transport. The tick keeps
  the observed devices and params in lists rebuilt only when the observer
  epoch moves or their instances change (`ObservedList`), and per tick
  re-queries and computes only those: work in proportion to the observed
  params, not the registered ones. A param's displayed value and p-lock
  state are computed once for `value`, `locked` and `text`; `text` is
  pushed without allocating unless its label changed; `has-locks`
  (`SlotPLockData::param_has_any_plock`, an early-exit scan bounded by
  `num-steps`) only when the track's p-lock key moved. `device.playhead`
  reads the sampler voices of the device's `DeviceSource`, which since
  7b-3 the tick keeps current (`SamplerPlayhead::current`: a sample load or
  voice rebuild moves no model counter; §14.2g), observed and cold alike.
- **P-lock change tracking.** `UiInvalidationQueue` keeps a p-lock
  revision per track, moved only by invalidations that may move a
  p-lock (`UiInvalidation::plock_scope`: `Full`, `ProjectState`,
  `Pattern`, track topology, step edits, `TrackFx`/`Instrument` p-lock
  and topology, `MidiFx`, `TrackBusSend`, process lanes, the
  `Plocks`/`BusSends`/`NumSteps` track params); a base-value edit moves
  none. A track's `PlockKey` is that revision with the pattern, fx and
  UI epochs (a scene switch, an undo, a structural edit). The step p-lock
  render (legacy `seq-track-step-plocked-*`, `-plock-kind-*`,
  `-variant-{r,g,b}-*`, `step-has-plocks`) is a whole-track scan
  (`plock_variant_step_render_values` and
  `track_step_plock_mask_for_slots`), cached per track in `KindsShared`
  under its `PlockKey`: cold reads and the step diff share it (a cold
  read of every step scans once), and the step diff recomputes only for
  tracks whose steps observe one of the three fields, when the key
  moved.
- **Shared derivations.** `device_param_display` (`state_values/shared.rs`)
  serves `param.value`/`locked` and the legacy instrument / track-effect
  value fields; `DeviceSlot` (`state_values/shared.rs`: descriptor, slot
  state, units, macro key, print target, invalidations, history commands,
  `did`/`resolve`) serves the params, the device chain sync and the
  setters; `app::instrument_param_macro_key` / `effect_param_macro_key`
  serve `App::effective_*_param_value` and the kinds' override lookup;
  `SamplerPlayhead` serves `read_sampler_playhead_seconds` and
  `device.playhead`; `track_step_plock_mask_for_slots` serves
  `track_step_plock_mask`.
- **Setters.** `param.base`'s `:set` is the host command
  `set-device-param` (`:track-id`, the track's `tid`; `:device`, its
  `did`; `:param-idx`; `:value`); `(lock-param! p steps v)` and
  `(unlock-param! p steps)` (steps: step instances of the param's track,
  sent with their tracks' `tid`s) are `set-device-param-locks` /
  `clear-device-param-locks`. All resolve the device by those stable ids
  when the command lands (like send's bus id), so a reorder in between
  cannot retarget them; a device or param that is gone, a non-finite
  value or a step of another track is an error (nothing changes). Values
  are display units, clamped, rounded for enum and boolean params, and
  `true`/`false` work (a numeric `:host` field's `:set` takes a boolean,
  which the setter converts). They act only where the model differs, go
  through the knob edits' history commands (`SetInstrumentParam` /
  `SetEffectParam`; `Set…PlockMulti` / `Clear…PlockMulti`, one undo
  entry for all steps, built by the same `clear_plocks_command` and
  `step_list` as `clear-param-plocks`), and queue the invalidations that
  refresh the legacy fields. A base set goes through
  `apply_device_param_base`, which `set-instrument-param` and
  `set-effect-param` share: an enum, boolean or sampler `sens` change
  rebuilds the legacy panel (fx and UI epochs). The base setter never
  latches a print (unlike the legacy knob while recording).
  `(device-param d name)` finds a param by name (the custom-ui `(bind
  "x")` helpers become `#'(device-param d "x").value`).
- **Gestures.** A script base set while no pointer is held is an undo
  entry of its own, ended at once; while the pointer is down it stays
  open and later script base sets join it (a drag view's `set!` per
  frame is one entry, ended by the release) — the pointer state, not a
  flag, decides (`GestureState::pointer_down`). An edit landing while
  another gesture is active (a user's knob drag) is applied beside it
  (`app::edit::apply_command_beside_gesture`, over
  `UndoManager::suspend_gesture` / `resume_gesture`): its own entry, the
  drag neither split nor joined.
  Since eseq-0l17.55 `ScriptEdit::begin` takes whether the edit is
  continuous (and `end` no longer does). A non-continuous script edit
  (a flag: `(set! rs.muted true)`, voices, choke, enabled) landing during
  the script's *own* drag is applied beside it, the drag staying open, when
  `app::edit::command_can_land_beside_active_gesture` says its command is a
  device-value edit of another device than the drag's pending device-value
  entry (eseq-0l17.55: a filter param drag with a rack slot's mute toggled
  mid-drag, one entry for the drag, one for the mute) or of the dragged
  device itself (eseq-0l17.72). Device values are recorded as whole-device
  snapshots (`DeviceValuesPatch`; a rack slot's covers its strip, p-locks,
  instrument and effects), so a same-device entry beside the open drag
  would, left alone, undo to states nobody saw (undoing the drag drops the
  flag, undoing the flag restores mid-drag values). Since eseq-0l17.72
  `app::edit::apply_beside_gesture` — the path script edits beside a
  *user's* knob drag take too — rebases the two after the beside edit
  commits (`rebase_device_drag_beside`): with the drag's entry at `B → C`
  and the beside entry at `C → C'`, the beside entry becomes `B → B'` and
  the drag's `B' → C'`, where `B'` is `B` with the components the edit
  moved (`DeviceValueSnapshot::rebase_edit`, a three-way merge per
  component: strip fields one by one, params' base values one by one,
  p-locks (slot and rack-slot) cell by cell, key locks note by note,
  tensors one by one, IR, table and slice edits each whole; RackSlot and
  RackInstrument targets of one slot are one device). The sound state
  keeps `B`'s engine and preset while the edit kept them, and its dirty
  flag is `B`'s or set when the edit dirtied the preset (turned the flag
  on, or moved a sound component: the instrument, base note, key-lock
  variants or slot p-locks), so undoing the drag beside a param edit
  still shows the preset dirty. The drag closes last and undoes first, keeping the flag; the
  flag's undo then lands on `B`; redo walks back up. When the edit moved a
  component the drag moved too (a script setting the dragged gain during
  a user's gain drag), the snapshots are of different kinds, the drag's
  `after` is not the edit's `before`, or the beside edits are several
  overlapping snapshots, the drag's entry so far is committed *below* the
  beside entries instead (`UndoManager::insert_undo_entry_before`, a fresh
  revision between them) and its later frames stage a new one: the
  pre-eseq-0l17.72 split, still on states that existed. A sampler slice
  drag (merge key prefix `sampler-slice-gesture:`) is ended there instead
  of resumed: its frames reapply the marker index it started from to the
  gesture's original snapshot (which a rebase keeps), so after a split its
  next frame starts a new gesture on the list as the edit left it. The
  undo budget evicts the undo stack's front by position (revisions need
  not increase along the stack after an insert). Edits that cover
  the dragged device without being it (a track instrument's beside a rack
  slot's), and any script edit without a command to check (`apply_with`)
  or during a drag whose pending entry is not a device-value snapshot,
  keep the earlier behaviour: the edit ends the drag's entry and the
  drag's later frames start another. Not covered: an edit beside a user
  drag whose entry *covers* the dragged device (the user-drag path checks
  no scope; pre-existing).
  Step p-lock drags (a knob turned with a step held) are recorded as
  step-cell patches (`StepCellsPatch`), not device snapshots, but a device
  snapshot covers its device's lock rows, so a device-value edit beside
  one on the same track and pattern (a script `(set! rs.muted true)`
  during a rack slot lock drag, `(set! cutoff.base …)` during a filter
  lock drag) used to record the mid-drag locks: undoing it after the
  drag brought them back. Since eseq-0l17.75 `apply_beside_gesture` rebases
  such an entry too (`rebase_step_drag_beside`): the drag's cells are set
  back to its `before` for one capture of the edited device (then
  restored), and the edit's entry becomes `X → X′` where `X′` is that
  capture and `X` is `X′` with the components the edit moved taken from
  its own `before` (`DeviceValueSnapshot::rebase_edit`). The drag's
  step-cell entry is unchanged: its undo restores the locks and keeps the
  edit, the edit's undo then lands on the state before both. A conflict
  (the edit moved a dragged cell), several device entries beside it, or a
  device that no longer resolves splits the drag's entry at the edit
  instead. During a script's own step-lock drag,
  `command_can_land_beside_active_gesture` now lets any device-value
  command land beside it. The reverse holds too: a script lock
  (`lock-param!`, `lock-strip!`, `lock-rack-macro!`, a step-cell entry)
  landing beside a user's device-value drag (a base knob, a strip gain)
  on the same track and pattern used to be dropped by the drag's undo
  (its snapshot covers the lock rows). `rebase_device_drag_beside` now
  rebases the drag around such entries (`rebase_device_drag_around_cells`):
  with the drag at `B → C` and the device captured at `C′` once the locks
  landed, the drag becomes `B′ → C′`, `B′` being `B` with the components
  the locks moved. The lock entry is unchanged: the drag's undo keeps the
  lock and the lock's undo removes only it. A lock of a component the drag
  moved too, step-cell and device entries beside it at once, or a device
  that no longer resolves splits the drag at them instead.
- **Not covered** (follow-ups): MIDI fx, bus effect and rack slot devices
  (eseq-0l17.36, built: §14.2f); modulation display, process mapping, tensors, base
  note, key locks, rack and project macros, the variant chip list, the
  neural-selection display override (eseq-0l17.37, built: §14.2g) and the
  rest of the panel data (eseq-0l17.43).

### 14.2c Built in stage 7i (eseq-0l17.35)

| Kind | Key | New `:host` fields (`:set` in brackets) |
|---|---|---|
| `track` | `(index)` | settings (Model): `poly :bool` [s], `max-polyphony :int` [s], `gate :bool` [s], `supports-mono-trigger :bool`, `voice-priority :string` [s], `mono-trigger :string` [s], `mute-group :int` (0 = none) [s], `swing :number` [s], `swing-resolution :string` [s], `fts :string` [s], `tuning tuning`, `accumulator :string` [s], `accum-mode :string` [s], `accum-limit :number` [s], `output bus` (nil: sends only) [s], `mod-output :bool`; live: `mod-out-level`, `mod-in-1` … `mod-in-4 :number` (L), `bar-transposes (list-of :number)` (L; `set-bar-transpose!`), `delete-target :bool` (L) [`seq-set-track-delete-target`] |
| `tuning` | `(track index)`, one per track (index 0) | `track track`, `on :bool`, `scale :string`, `custom :bool`, `edited :bool`, `root :string` [t], `morph :number` (0–1) [t], `mode :string` [t], `period :number`, `degrees (list-of degree)` |
| `degree` | `(tuning index)` | `tuning tuning`, `index :int`, `base :number`, `offset :number` (−1200–1200 cents) [t], `enabled :bool` [t], `pitch :number`, `label :string`, `ratio :string` |
| `bus` | `(index)` | `output bus` [host command `set-bus-output`], `output-options (list-of bus)`, `mod-in-1` … `mod-in-4 :number` (L) |
| `route` | `(index)` | `index :int`, `source track`, `dest track`, `dest-bus bus`, `input :int` (1–4), `selected :bool` (L) [`seq-set-delete-target`] |
| `transport` | `()` | `roll-rate :string` (L) [`seq-set-roll-rate`], `sequence-rolling :bool` (L) |
| `engine` | `()` | `overloaded :bool` (L), `compiling :bool` (compared every tick) |
| `selection` | `()` | `steps (list-of step)` (L), `cursor-step step` (L) [host command `set-cursor-step`], `edit-step step` (L), `rack-slot :int` (−1: no rack), `auto-follow :bool` (L) |
| `project` | `()` | `routes (list-of route)`; `(routes)`; `fts-options`, `sync-options`, `accumulator-options (list-of :string)`, `output-options (list-of bus)` |

[s] = the `set-track-setting` host command; [t] = `set-tuning`. Constants in
`eseq.kinds`: `mute-group-options` (labels by `mute-group` value),
`accum-mode-options`, `tuning-root-options`, `tuning-mode-options`,
`voice-priority-options`, `mono-trigger-options`,
`swing-resolution-options`, `roll-rate-options`. Actions:
`reset-tuning!`, `justify-tuning!`, `(randomize-tuning! tn cents)`,
`(stretch-tuning! tn cents)`, `(clear-degree! dg)`, `(set-bar-transpose! t
bar v)`;
`(mod-in-level x i)` is a binding to input `i` (1–4) of a track or bus.

Built (7i):

- **The value rule** (every 7i setter; since eseq-0l17.38 the earlier
  stages' number setters too: `track.volume` / `pan`, `bus.volume`,
  `send.amount`, `transport.bpm` and the `step` params, whose natives
  and `set-track-send-base` reject what they used to clamp, the current
  value still round-tripping with no undo entry; the one exception left is
  7b's `param.base` / `lock-param!`, which clamp and round in display
  units as documented in §14.2b). A string field takes one of its labels,
  case-insensitively, and its current value always works (`(set! t.fts
  t.fts)` with an edited `Major*` scale or an imported scale's name); a
  number field takes a finite number in its range; an `:int` field an
  integer in its range; a bool field a bool; an instance field an instance
  of its kind (or nil where documented). Anything else is an error that
  changes nothing: `set!` itself rejects a value of the wrong type (the
  field's declared type), and the host rejects what is out of range or
  unknown (no silent clamping) with a `set-track-setting: …` /
  `set-tuning: …` error. A native setter's error (an unknown roll rate
  label, a `mod-in-level` input outside 1–4) reaches the status line, as
  any failing native's does (natives never raise). The roll rate's label
  is resolved by the host (`seq-set-roll-rate` takes a label or an index).
- **Settings are the track's own.** `poly` and `max-polyphony` are the
  track's flag and voice count; a drum rack's voices are its slots' (the
  legacy `tp-poly` / `tp-max-polyphony` show the selected slot's on a
  rack: rack slot devices, eseq-0l17.36). `swing` and `swing-resolution`
  are the base values (like `timebase`), never a step's p-lock.
  `mute-group` is an `:int` (bindable); its label is
  `(nth mute-group-options g)`. Picking the scale a track already plays
  by its unedited name (`"Major"` while it shows `Major*`) changes nothing,
  as in the dropdown.
- **Outputs are buses.** `track.output` is the bus instance the track
  feeds: the main mix bus for main, nil for sends only (no separate
  flag). Its setter sends the bus's `bid` (`:bus-id`; nil for sends
  only), resolved by `BusId` when it lands (`track_output_for_bus`), so
  two buses with one name are never confused. `project.output-options`
  is every bus a track may feed, in bus order (the main mix first).
- **Scales are a sub-kind.** `t.tuning` is the track's `tuning` instance
  (keyed (track instance id, 0), registered with the track); its
  `degrees` are `degree` instances keyed (tuning instance id, index),
  registered and dropped with the degree count (none while the scale is
  off). `root`, `morph`, `mode` and a degree's `offset` and `enabled` are
  settable fields (`set!`, `toggle!`, `#'`), each one undo entry; `morph`
  and `offset` are continuous (a drag's `set!`s join one entry). `morph`
  is 0–1 (the legacy `tp-tuning-morph` shows percent). Whole-scale edits
  are the actions above. Each field is its own cell, so a morph drag
  notifies only the readers of `morph` (and of the degree pitches, labels
  and ratios it moves), never a view reading `root`.
- **Feeds.** Settings are model fields: the model sync reads each track's
  raw `TrackSettings` (`host_kinds/settings.rs`: the model values, the
  accumulator's index and script name) and labels them only when pushing
  (`voice_priority_label`, `mono_trigger_label` over
  `VOICE_PRIORITY_LABELS` / `MONO_TRIGGER_LABELS`, `accumulator_name`),
  when they changed or the accumulator list did. A track's scale is read
  once per sync (one tuning lock) and compared with the last pushed (scale
  index, `TrackTuning`); only a change pushes the `tuning` and `degree`
  fields (`tuning_degrees`, which the legacy `tuning_reactive_fields`
  shares) and the track's `fts` (`fts_label`). The track's output is
  pushed per sync (an instance, no allocation). The project's option lists
  are pushed when they change (the fixed `fts-options` and `sync-options`
  once, again after a schema change). Port levels are copied from the
  meter cache (`KindsMeters::mod_ports`, which the tick keeps polled while
  one is observed with the mixer hidden: `HostKinds::wants_mod_levels`);
  `bar-transposes` is compared in place with the last push per observing
  track and rebuilt only when a bar moved; `engine.compiling`
  (`App::compile_pending`) and `selection.rack-slot`
  (`rack_slot_selection`, re-derived at the model sync and when the current
  track changes) need the `App`. `selection.steps`, `cursor-step` and
  `edit-step` are computed only when observed and the step selection, the
  current track, its length or the step cursor (the Lisp global
  `cursor-step`, `fx_step_cursor_value`) moved; `edit-step` is the first
  selected step, else the cursor (`fx_step_cursor`, shared with the
  legacy `fx-step-*` fields). Routes keep the observed ones in an
  `ObservedList`; the route sync reads the current scene's connections
  alone (`SequencerState::current_mod_connections`, no graph-override
  composition). Bus output options bound their cycle walk by the bus count
  (no allocation).
- **Identity.** Routes are keyed by their endpoints' stable ids
  (`RouteKey`: source `TrackId`, destination `TrackId` or `BusId`, input),
  each key allocated a route id while the route exists (ids are never
  reused; a key is forgotten with its route), so removing or adding another
  route or track re-keys rather than re-registers; a project load replaces
  them. A route naming a track the registry lacks (the scene's connection
  list is not remapped by a track delete today) or repeating another's
  endpoints gets no instance. Mod inputs are numbered 1–4 everywhere in
  the kinds (`mod-in-1` … `mod-in-4`, `route.input`, `mod-in-level`), as
  the mixer labels them (Ext1–4).
- **Setters.** `set-track-setting` (`:track-id`, `:setting`, `:value`, or
  `:bus-id` for `output`) and `set-tuning` (`:track-id`, `:op`, `:value`,
  `:degree`) resolve the track by `TrackId` when they land
  (`live_track_index`, which `DeviceSlot::resolve` shares), act only where
  the track differs, and go through the legacy edits' history commands
  (`slice3_command` over `slice3_numeric_payload`, `track_tuning_command`,
  `SetTrackOutput`, `accumulator_edit_payload`) with their invalidations
  (`slice3_edit_applied`, `track_output_applied`); an edit applied without
  history refreshes the same way. They live in
  `host_commands/track_settings.rs`, with `set-track-bar-transpose`
  (`set-bar-transpose!`; the bar and semitones in range, else an error) and
  `set-cursor-step`. `bus.output` goes through `set-bus-output` (by bus
  ids; absolute in `set_bus_output_recorded`). Script edits follow the 7b
  gesture rules, one helper (`host_commands::ScriptEdit`, over
  `app::edit::apply_beside_gesture`): swing, accum limit, voices, a scale
  morph or degree offset and bar transposes join one entry while the
  pointer is down; anything else is its own entry.
- **Selection setters.** `selection.cursor-step`'s setter is the host
  command `set-cursor-step` (`:track-id`, `:step`, in range): it makes the
  step's track current (`CurrentTrackSwitch`, which `seq-set-track`
  shares), sets the step cursor, refreshes the step panel's `fx-step-*`
  fields from that track's step (`sync_fx_step_cursor_binding_fields`) and
  runs the grid's cursor hook (`sequencer-cursor-step-changed`, the
  highlight a click moves), as a click on the step does; no history.
  `t.delete-target` reads true while the mixer's delete target holds the
  track (alone or among several); `(set! t.delete-target true)` makes the
  track the target unless it already holds it, `false` takes it out (a
  one-track target clears, a multi-track one shrinks):
  `seq-set-track-delete-target`, so what a setter writes the field reads.
  Route selection is the immediate `seq-set-delete-target` (clearing only
  the route's own target).
- **Deferred to eseq-0l17.36** (they need device instances for MIDI fx,
  bus effects and rack slots): `SEQ.bus-effects` → `bus.devices`,
  `SEQ.midi-effects` → `track.midi-devices`, `SEQ.rack-slot-delete-target-*`
  → `device.delete-target`, and a rack track's slot voices
  (`device.voices`). Built in 7b-2 (§14.2f).

### 14.2d Built in stage 7d (eseq-0l17.30)

| Kind | Key | New `:host` fields (`:set` in brackets) |
|---|---|---|
| `song` | `()` | `exists :bool`, `mode :string` (`stopped`, `song-playback`, `arrangement-capture`), `recording-kind :string` (empty, `take`, `dub`), `position :number` (L, beats), `cursor :number` [c], `end :number` [c], `loop :bool` [c], `manual-latch :bool` (L) [c, false only], `scene-latched :bool` (L), `edit-error :string`, `capture-failed :bool`, `capture-error :string`, `region region` (nil: none), `bound-clip clip` [c], `spans (list-of scene-span)` |
| `region` | `()` | `tracks (list-of track)`, `start`, `end :number`, `scene-lane :bool` |
| `scene-span` | `(index)` | `index :int`, `scene scene`, `start`, `end :number` |
| `clip` | `(track cid)` | `track track`, `cid :int`, `start :number` [k], `end :number` [k], `cell cell` [k] (nil for a take), `take :int` (-1 for a pattern), `offset :number` (steps), `num-steps :int`, `length :number` (beats), `events (list-of (list-of :number))`, `note-dots (list-of :any)` (the notes as the timeline's dots, .15), `dot :bool`, `dot-color :rgb` (the timeline's gray without a palette color) |
| `cell` | `(track pid)` | `track track`, `pid :int`, `active`, `assigned`, `override :bool`, `queued :bool` (L), `selected :bool` (L) [e], `banks (list-of bank)` |
| `track` | `(index)` | `clips (list-of clip)`, `cells (list-of cell)`, `governed :int` (`take-none`, `take-governed`, `take-latched`), `latched :bool` (L) [c, false only] |

[c] = the `set-song` host command (`:field`, `:value`; `:track-id` for
`latched`, `:clip-id` for `bound-clip`); [k] = `set-clip` (`:clip-id`,
`:field`, `:value`; `:track-id`, `:pattern-id` for `cell`), both in
`host_commands/arrangement.rs`; [e] = the delete-target natives, applied at
once (`seq-set-delete-target :track-pattern`, by track position and
pattern id; eseq-0l17.13 replaced the `set-cell` host command). Actions: `(launch-cell! c)` (the legacy
`set-scene-cell`, extended to take `:track-id` and no `:scene`, both
resolved when it lands: the track by id, the current scene; legacy
`:scene :track` payloads still work; with `transport.launch-quantize`),
`(select-region! t1 t2 start end :scene-lane true)` (`&key scene-lane`, a
bool defaulting to false, sent as `:scene-lane`) and `(clear-region!)`
(`set-song-region`, tracks by `tid`).
Constants: `take-none` (0), `take-governed` (1), `take-latched` (2).

Built (7d):

- **Cells are pool patterns.** The legacy `track-pattern-cell-*` family
  is keyed (track, pattern id), the mixer clip grid's patterns, not (track,
  scene): a `cell` is one pattern of a track's pool (take chunks and the
  track-sound carrier excluded, as in `track_pattern_cells`), keyed (track
  instance id, `PatternId`). Scene membership is `cell.banks` (the banks
  whose scenes use it; empty for an orphan, which the grid shows in every
  bank) and `cell.assigned` (the current scene's cell). `override` is the
  track's launched-override flag (true on every cell of the track, as the
  legacy field). A clip's `cell` is the cell it plays.
- **Identity.** Clips are keyed (track instance id, `ClipId`), the
  arrangement's stable, never-reused clip id: a move, resize or source
  change keeps the instance; a delete drops it. Clips and cells are
  registered per track (`reconcile_children`) and dropped with their track,
  so a track reorder (a delete, its undo) keeps them and a project load
  replaces them with the tracks. Scene changes carry no ids, so
  `scene-span`s are positional (`(index)`, like steps, D2): an edit
  re-pushes their values, a shorter lane drops the tail. `region` is a
  singleton; `song.region` is it while a region is selected, else nil.
- **Feeds.** Each behind its own key; none reads the history revision, so
  a knob drag re-derives none of them (`SongState::structure_syncs` counts
  the structure syncs). *Structure* (the clips and their fields, the
  spans, `song.exists`/`end`/`loop`, read through the borrowing
  `SequencerState::with_committed_song`): the committed-song revision
  (`committed_song_revision`), the track and scene instances (a reorder
  remaps the lanes in place without moving the song revision) and the cell
  set. *Source content* (a clip's `num-steps`, `length`, `events`: the
  source's preview, one pattern cycle or the whole take, from
  `collect_lane_pattern_events`, shared with `SEQ.song-lane-events`): the
  pattern epoch and pool content revision; values are cached per (track
  instance, source), only the lanes holding a missing source are
  previewed, and after a structure-only change only clips whose (track,
  source) is new are pushed. *Dots* (`App::song_clip_sounds_in` over the
  lanes in hand, shared with `SEQ.song-clip-sounds`, through
  `sound_palette_rgb`, shared with the palette rows; the timeline's
  `SOUND_DOT_GRAY` without a palette color): the song revision, the scenes
  revision (patches and their palette colors live there), the sound
  binding epoch and the structure. *Cells* (`tracks_pattern_cells`: every
  track's `track_pattern_cells` and `track_pattern_bank_indices` under one
  scenes lock, as the legacy publishers): the scenes revision, current
  scene, pattern epoch, song-row mirror epoch, track generation, take-lane
  and silenced masks and the track and bank instances. Scene spans go
  through `registry::reconcile` over their positions, so a hot reload's
  dropped instances are re-registered; a schema change or a dropped
  arrangement instance invalidates every key. `mode`, `recording-kind`,
  `cursor`, `edit-error` and the capture state are `App` state no counter
  tracks: compared every tick with what was last pushed, allocating only on
  a change, all re-pushed when the song or region singleton is a new
  instance; `region` likewise (with the structure generation: its tracks
  are positions), and when it is cleared the `region` singleton reads as
  no tracks over 0..0 outside a lane; `bound-clip` by one keyed lookup.
  `track.governed` (`song_take_lane_states`, shared with
  `SEQ.song-track-governed`) is re-derived only when song authority, the
  mirrored row, the latch mask, the running song or the structure moved.
  Live: `song.position` (`song_position` /
  `displayed_song_position_beats`, shared with `SEQ.song-position-beats`;
  the tick copies the capture's record head for the empty-song fallback),
  `manual-latch` (`song_manual_latch`), `scene-latched`, `track.latched`
  (`song_lane_latched`), `cell.queued` (`queued_track_clip`, shared with
  `SEQ.queued-track-clips`) and `cell.selected`
  (`track_pattern_cell_selected`, shared with the legacy field); observed
  cells are kept in an `ObservedList`.
- **Setters.** Clips by `cid`, tracks by `tid`, cells by (`tid`, `pid`),
  resolved when the command lands; a gone clip, track or pattern is an
  error. They act only where the model differs and go through the timeline
  commands' primitives (`App::arr_*`, one arrangement history entry; undo
  restores), landing like them (`song_edit_landed`, shared with the legacy
  song commands: a rejection, the setter's own pre-edit ones for
  `set-clip` and `song.loop`/`song.end` included, is latched in
  `song.edit-error` and reported; a success clears it and resyncs the
  piano roll). `clip.start` moves the clip (length kept), `clip.end`
  resizes it, `clip.cell` sets its source to a pattern of its own track's
  pool (another track's cell, nil, or a pattern no longer in the pool is
  an error), `song.loop`, `song.end` (before a clip's end or the last
  scene change the model refuses). `cell.selected` makes the cell the
  mixer's delete target at once (false clears it when it is; originally
  the `set-cell` host command, now the delete-target natives, .13).
- **Script drags.** While the pointer is down (and no user gesture is
  active, `ScriptEdit::drags`), every `clip.start`, `clip.end` and
  `song.end` `set!`, across any number of clips, shares ONE coalescing key
  (`kinds-arrangement`) and so forms one undo entry. Each frame is applied
  relative to the gesture's `before` arrangement: the targets the drag has
  set so far (clip id to start / end, the song end;
  `GestureState::script_arrangement_drag`, tied to the gesture's id)
  accumulate across frames, and every frame rebuilds from the before
  snapshot with all of them in a fixed order (clips by id: a start moves,
  an end resizes from the clip's start then; the song end last;
  `App::arr_script_drag`). A clip dragged across another therefore only
  occludes it where it ends up, as a timeline drag; a rejected frame
  changes nothing and keeps the earlier targets; take ends clamp. With the
  pointer up each `set!` is a plain one-shot edit and its own entry
  (`arr_clip_move`, `arr_clip_resize` — which grows a take past its
  playable end, as the timeline's resize — and `arr_set_end`). The
  primitives share `edit_arrangement_keyed` (one-shot or coalesced) and the
  in-place edits `move_clip_in`, `resize_clip_in`, `set_end_in`.
- **Selection state.** `cursor`, `bound-clip` (`select_song_clip_span`
  with the clip's span, as a title-bar click; nil deselects),
  `manual-latch` / `track.latched` (false is Back to Song, for all lanes or
  one; true is an error: only a launch latches), `cell.selected` and the
  region actions are selection or transport state, without history, as
  the legacy commands; their rejections are reported, not latched. Values:
  beats are finite numbers of at least 0 (a clip's end after its start),
  flags bools, ids non-negative integers (`SetValue::id` / `id_or_nil`,
  shared with `track.output`); `set!` rejects a wrong type, the host
  anything else (`set-clip: …`, `set-song: …`).
- **Deferred to eseq-0l17.39:** `song.pending` (the provisional capture
  surface: pending take lanes, scene and track launches) as positional
  sub-kinds. Built in 7d-2 (§14.2o).

### 14.2e Built in stage 7h (eseq-0l17.34)

| Kind | Key | New `:host` fields (`:set` in brackets) |
|---|---|---|
| `group` | `(index)` | `racks (list-of group)`, `parent group` (nesting, as `SEQ.groups` :rack-members / :parent), `armed :bool` (L) [`seq-set-rack-armed`], `pads (list-of pad)`, `clips (list-of rack-clip)`, `rack-clip rack-clip` (nil: silent or no clips), `legacy :bool`, `groove groove` (empty / nil / false on a plain group) |
| `pad` | `(group tid)` | `group group`, `track track`, `note :int` (−36–51) [p], `label :string`, `choke :int` (0–16, 0 = none) [p], `role :string` [p], `role-tag`, `role-label`, `standard-role :string` (the role key the note's standard layout infers, comparable with `role`), `standard-role-label :string`, `triggered :bool` (L) |
| `rack-clip` | `(group cid)` | `group group`, `cid :int`, `index :int`, `name :string` [r], `active :bool`, `scenes (list-of scene)`, `groove groove` (nil: follows the rack's), `own-groove :bool` [r] |
| `groove` | `(group clip)`, clip part 0 for the rack's own | `group group`, `clip rack-clip` (nil: the rack's own), `pool-groove pool-groove` (nil: none) [g], `enabled :bool` [g], `timing`, `velocity` (0–1.5), `random :number` (0–1) [g], `scale :number` [g], `grid :string`, `slots :int`, `cells (list-of :number)`, `measured (list-of :bool)`, `pads (list-of pad-groove)` |
| `pad-groove` | `(groove tid)` | `groove groove`, `pad pad`, `amount :number` (0–1) [g], `enabled :bool` [g], `cells (list-of :number)`, `measured (list-of :bool)` |
| `pool-groove` | `(index)` | `index :int`, `groove-id :int`, `name :string` [`set-pool-groove`], `grid :string`, `racks (list-of group)` |
| `library-groove` | `(index)` | `index :int`, `choice :string` (the picker key; `key` is a keyed kind's built-in field), `name :string`, `tier :string` |
| `track` | `(index)` | `pad pad` (the pad a member track backs, or nil) |
| `project` | `()` | `groove-pool (list-of pool-groove)`, `groove-library (list-of library-groove)` |

[p] = the `set-pad` host command (`:group-id`, `:track-id` of the member,
`:field`, `:value`); [r] = `set-rack-clip` (`:group-id`, `:clip-id`); [g] =
`set-groove` (`:group-id`, `:clip-id` (0: the rack's own), `:field`; a pad
share's `pad-amount` / `pad-enabled` with `:track-id`; `pool-groove` with
the groove's id as `:value`, nil for none); all in
`host_commands/rack_kinds.rs`. Clip id 0 is the one sentinel for a rack's
own groove (clip ids start at 1): the kinds send it (`groove-clip-id`), the
legacy groove commands take it (and the buffer's -1), and a clip loaded
with id 0 is given a fresh id (`repair_rack_clips`, its scene pointers
following).
Constants: `pad-role-options` (the role keys, `PadRole::ALL` order),
`groove-scale-options` (`(0.5 1 2)`, `GROOVE_SCALES`). Actions:
`(trigger-pad! p)`, `(launch-rack-clip! rc)`, `(silence-rack! g)` (both
with `transport.launch-quantize`), `(save-rack-clip-as! g name)`,
`(delete-rack-clip! rc)`, `(convert-rack-to-clips! g)`,
`(set-clip-groove! g rc field v :pad p)` (a [g] field of the groove rack
g's clip rc plays, rc nil for the rack's own; a clip that follows the
rack's gets its own, a copy, in the edit's entry),
`(use-library-groove! gr lg :clip rc)` (copy-on-apply into the pool;
`:clip` as `set-clip-groove!`),
`(apply-groove-to-all-clips! gr)`, `(extract-groove! g name bars
resolution quantize)`, `(duplicate-groove! pg)`, `(delete-groove! pg)`
(one undo entry each; deleting turns the groove off on every rack playing
it) and `(save-groove-to-library! pg)` (writes a file: not undoable), over
the legacy commands (racks by group id, clips by clip id, grooves by pool
id; `trigger-rack-pad` gained `:track-id`, the member resolved when it
lands, a gone rack or pad an error).

Built (7h):

- **Identity.** A rack's pads are keyed (group instance id, member
  `TrackId`), not by note or member position: a pad move, a swap (pad shares
  follow the drum, as `remap_pad_note`), a member joining or leaving another
  pad, and a track deleted in front (members re-index) keep the instance.
  Rack clips are keyed by the bank's stable, never-reused clip id; grooves
  (group instance id, clip id), the rack's own under 0 (clip ids start at
  1), each clip's own while it owns one (`rc.own-groove`); a pad's share
  (groove instance id, member `TrackId`). All are children of the group (or
  groove) instance, so a project load (groups replaced with the registry
  generation) drops them. Pool grooves are positional (`(index)`), the
  instance kept by `GrooveId` across reorders (`registry::reconcile`,
  replaced on a project load, like buses); library
  grooves (index) by an id allocated per picker key while the file is
  listed (like routes). `pad.track.steps` is the member's lane (legacy
  `SEQ.track-steps`); `t.pad` is the inverse.
- **The playing groove is a view derivation.** The legacy buffer shows the
  playing clip's own groove, else the rack's (`groove-state`):
  `(or (and g.rack-clip g.rack-clip.groove) g.groove)`. A `set!` acts on the
  groove instance it names: the rack's own changes what every following
  clip plays; to give the playing clip its own first (the legacy buffer's
  copy-on-first-edit), `(set! rc.own-groove true)`.
- **Feeds.** Each behind its own key; none reads the history revision
  (`RackState::syncs` counts: an edit elsewhere runs no rack sync). *Rack
  clips*: the scenes revision (every bank or pointer edit moves it), the
  current scene, the racks (id, is-rack) and the group and scene instances;
  the bank is read under one scenes lock and pushed after. *Pads, grooves,
  pool*: the groups' generation (`HostKinds::groups_generation`, moved when
  the model sync records changed groups: any pad-map or groove edit), the
  pool (by value), the track and group instances and the rack clip
  instances. A groove's lanes (`slots`,
  `cells`, `measured`, the shares' instances and lanes: `groove_lanes`,
  shared with `SEQ.rack-grooves`) are rebuilt only when the pool groove it
  plays or the pads (track ids, notes, roles) moved (`LaneKey`, compared
  in place), so an amount drag pushes the amounts alone
  (`RackState::lane_builds`). *The library*: re-listed
  (`listed_groove_library`, shared with `SEQ.groove-library`; the cached
  listing) when the UI epoch, the library generation
  (`groove::library::library_generation`; a library save, rename or delete
  moves both) or whether the project has a rack or a pool groove moved, so
  an amount drag lists nothing (`RackState::library_listings`); pushed when
  it changed. *Live*: `group.armed` from the shared armed rack
  (`KindsHandles::armed_rack`; the group sync's observed list, `mixer.rs`)
  and `pad.triggered` from the tick's pad
  lights (`read_rack_pad_trigger_flags`, which consumes the audio thread's
  trigger latch every tick; the flags are kept in
  `FrameDiffState::rack_pad_triggers` and handed over in
  `KindsMeters::pad_triggers`, so the legacy fields, gated by *fx*, and the
  kinds, gated by observers, share one read), each kept in an
  `ObservedList`.
- **Setters.** Racks by group id, pads by their member's `TrackId`, clips
  by clip id, grooves by (group id, clip id) and pool grooves by id, all
  resolved when the command lands; a gone rack, pad (a member that left),
  clip, clip groove (a clip that follows the rack again) or pool groove is
  an error. They act only where the model differs, through the panel's
  recorded edits (`set_rack_pad_note_recorded`, `…_choke_group_…`,
  `…_role_…`, `rename_rack_clip_recorded`,
  `set_rack_clip_own_groove_recorded`, `set_rack_active_groove_recorded`,
  `…_enabled_…`, `…_scale_…`, `…_pad_enabled_…`,
  `rename_pool_groove_recorded`: one bus/group structure entry each, which
  undo restores) and land like the legacy commands (`sync_rack_pad_map`;
  `groove_edit_landed`, now shared with `rack_grooves::handle`). Values
  follow §14.2c: a note an integer in −36–51 (an occupied note swaps, as the
  pad grid's drag), a choke group 0–16, a role one of `pad-role-options`
  (case-insensitive, `PadRole::from_key_ignore_case`) or "" for Standard,
  the amounts finite numbers in their ranges (no clamping; the declared
  `:range`s are checked against the host's maxima), a scale one of
  `groove-scale-options`, names non-empty (`SetValue::name`), `pool-groove`
  a pool groove or nil. `group.armed` is the absolute native
  `seq-set-rack-armed` (toggles only when the arm differs; built with
  `seq-toggle-rack-arm` from one factory, so both take the same exclusive
  arm in the same lock order; no history, as the legacy arm).
- **Gestures.** A groove amount or pad share `set!` while the pointer is
  down (and no user gesture is active, `ScriptEdit::drags`) joins the
  script's drag: `apply_rack_groove_amount_drag`, the knob drag's coalescing
  (one entry per group and clip; all of a groove's amounts and shares share
  it). Otherwise it is its own entry (`set_rack_groove_amounts_recorded`,
  the one-shot form, also used beside a user's gesture; both derive the next
  settings with `App::next_rack_groove`, and both setters and the legacy
  amount commands name the amount with `rack_grooves::Amount`); every other
  setter is its own entry.
- **Not covered.** Rack slot voices and polyphony stay with the rack slot
  devices (eseq-0l17.36, §14.2c; built: `device.voices`, §14.2f); a rack's
  macros are eseq-0l17.37 (built: `rack-macro` on the rack's instrument
  device, §14.2g).
  The groove picker's labels, headers and details (`:picker-labels`,
  `:picker-headers`, `:picker-details`) are a view's to build from
  `project.groove-pool` (`pg.racks` says where else one plays) and
  `project.groove-library`. Kit presets stay with the browser (eseq-0l17.32).

### 14.2f Built in stage 7b-2 (eseq-0l17.36)

| Kind | Key | New `:host` fields (`:set` in brackets) |
|---|---|---|
| `device` | `((track bus) did)` (was `(track did)`) | `bus bus` (nil for a track's device; `track` is nil for a bus effect), `role :string` (`instrument`, `effect`, `midi-fx`, `rack-slot`, `rack-effect`, `bus-effect`), `devices (list-of device)` (a drum rack's slots on its instrument device, a rack slot's effects on the slot), `container device` (the device whose `devices` holds it, or nil), `voices :int` (a rack slot's, 1–16; 0 otherwise; no declared range, the setter checks) [d], `delete-target :bool` (L) [d]; a rack slot's strip (eseq-0l17.42): `gain :number` (0–2) [d], `gain-display :number`, `gain-locked :bool`, `pan :number` (−1–1) [d], `pan-display`, `pan-locked`, `muted :bool` [d], `muted-display`, `muted-locked`, `soloed :bool` [d], `soloed-display`, `soloed-locked`, `choke :int` (0–16, 0 none) [d] (all L), and `enabled` takes [d]; eseq-0l17.54: a rack slot's `base-note` [d] (the 7b-3 field, §14.2g), `base-note-display :number` (−48–48), `base-note-locked :bool`, `voices-display :int` (all L; `voices` stays the model base); since .18 `builtin :bool` (an audio effect built into eseq, which the effect editor cannot open) |
| `track` | `(index)` | `midi-devices (list-of device)` |
| `bus` | `(index)` | `devices (list-of device)` |

[d] = the `set-device` host command (`:track-id` or `:bus-id`, `:device`,
`:field`, `:value`; `host_commands/devices.rs`). `param.base`, `lock-param!`
and `unlock-param!` take a bus effect's param too: every device command
names the device by `(device-target d)`, its track's `tid` or its bus's
`bid` with its `did`.

Built (7b-2):

- **One device kind, several parents.** Every device is a `device`, with
  the same `param`s, setters and panel helpers: a track's chain
  (`t.devices`: the instrument, then its effects), its MIDI effects
  (`t.midi-devices`), a drum rack's slots (the rack instrument device's
  `devices`; each slot device stands for the slot's instrument, its voices
  and its effects, its `devices`), and a bus's effects (`b.devices`). A
  device is keyed under its track, or under its bus for a bus effect (a
  parent-keyed kind may name several parent kinds, §3.1; a bus has no track
  to hang its effects off, and a separate bus device kind would need a
  separate param kind). `role` says which family it is (`kind` is a
  built-in); `slot` is its place in its own chain.
- **Identity.** The `did` is the device registry's identity of the device
  (a MIDI effect's, a rack slot's, a rack slot effect's, a bus effect's
  instance id, from the allocator the track effects use; 0 for the
  instrument). Ids are unique across families, which a track's devices of
  every family rely on (one key space under the track): every `bind_*`
  refuses an id another family holds, and a project load reallocates a
  persisted id two records share (`Project::normalize_device_instances`:
  the first holder keeps it; nothing else in a project names these ids).
  A reorder keeps the instance and its params. A device whose id is not
  bound yet uses a placeholder of its family (above 2^52, by position:
  `DeviceSlot::unbound_did`). When the registry allocates an identity for
  such a device it records the placeholder it came from (family and
  position at the bind: `DevicePlaceholder`), and `reconcile_devices`
  re-keys the placeholder instance to exactly that identity, never
  guessing by position (so a bind that also reorders, say unbound
  `(transpose arp)` moved to `(arp transpose)`, keeps every handle on its
  own effect), the track chain's effects included. An identity with no
  record (a new device, a persisted one) or two identities claiming one
  placeholder replace the instance instead; other families' devices under
  the same parent are left alone. A descriptor change in a device replaces
  its params (old handles go stale); a deleted device, track or bus, or a
  project load drops it. The track model's revision includes the
  registry's `generation`, so a bind that moves no other counter re-keys a
  chain device too.
- **Feeds.** The chain stays with the track model sync; the other families
  have a pass each (`host_kinds/devices.rs`), each behind its own key,
  compared in place and updated only when it moved: MIDI effects on the FX,
  UI and pattern epochs, the content library epoch, the registry's
  `generation` and the tracks; bus effects on the FX and UI epochs, the
  `generation` and the buses; rack slots and their effects on the
  `generation`, the tracks and the chain devices, and the rack revision
  (`RevisionedMutex`: any rack edit), where a rack whose layout
  fingerprint (slots, instruments, names, switches, voices, descriptors,
  effects and their switches) did not change is skipped, so a rack knob
  drag does no device work (`DeviceState::racks_synced`). None reads the
  history revision (`DeviceState::syncs`: a value edit or drag runs none);
  a hot reload is caught by one representative instance. MIDI effect
  descriptors come from a cache in `DeviceState`, reloaded only when the
  content library epoch moves (`midi_fx_loads`); rack devices are synced
  under the rack lock, in one pass per rack (models and placeholders built
  together, reconciled once). Shared derivations: the chains
  (`bus_device_chain`, shared with `SEQ.bus-device-chains`;
  `midi_fx_device_chain`, shared with `SEQ.midi-effects`;
  `rack_slot_effect_chain`), `effect_enabled` (over `DeviceValues`, shared
  with `track_device_chain` and the bus and rack chains),
  `rack_slot_raw_name`, `App::rack_slot_descriptor` (shared with the rack
  modulation meters), `with_rack_slot`.
- **Param values per family** (`DeviceSlot::with_values`, `read_param`):
  a MIDI effect as a chain device (`device_param_display`: the lock in
  force at the displayed step, held off-step; no project macro); a rack
  slot's instrument or effect from the rack's snapshot
  (`rack_slot_instrument_param_display`, `rack_effect_param_display`:
  the displayed step's lock, else a rack macro mapped onto it, else the
  base; shared with the rack panel's value fields); a bus effect from the
  shared bus copy (`KindsHandles::bus_state`): its base, like the legacy
  `bus-N-fx-*` field (`locked` false; `has-locks` scans the bus slot's
  locks; `printing` follows the current track's latch, as the bus knob
  does). All in display units. Observed params are computed per tick
  only (the device and param `ObservedList`s cover every family).
- **Setters.** `set-device-param` and the lock commands resolve the
  device by owner id and `did` when they land (`DeviceSlot::resolve` /
  `resolve_bus`); a placeholder `did` names the device at its family and
  position while one is there, even once an edit has bound it (two `set!`s
  of one unbound device in one eval both land, and a drag keeps landing
  until the next sync re-keys the device). A MIDI effect's descriptor comes
  from the device sync's cache and its presence from the chain's names: no
  setter reads a file (the history commands' own clamp,
  `AppCommand::SetMidiFxParam` / `SetMidiFxPlockMulti`, shared with the
  legacy knobs, still loads the descriptor per command: not yet cached).
  They act only where the model differs and go through the knob edits'
  history commands (`SetMidiFxParam`, `SetRackSlotInstrumentParam`,
  `SetRackSlotEffectParam`, their `…PlockMulti`, `ClearMidiFxPlockMulti`,
  `ClearRackSlotEffectPlockMulti`; a bus effect through
  `apply_recorded_bus_effect_value_mutation`), with the gesture rules of
  7b (a bus effect drag is one entry too). They refresh what the legacy
  commands refresh, through helpers shared with them:
  `apply_device_param_base` (now also `set-midi-fx-param`'s; its panel
  rebuild is skipped for a rack slot, whose `rack_param_applied` owns it),
  `rack_param_applied` (the four rack knob commands'; a script lock
  refreshes the shown step's rows as the knobs do,
  `RackPlockRowsSync::for_plock_write`) and `bus_effect_param_applied`
  (`set-bus-effect-param`'s). Bus effects take
  no p-locks (`lock-param!` / `unlock-param!` on one are errors). A rack
  slot instrument's locks clear through `ClearRackSlotInstrumentPlockMulti`
  (eseq-0l17.41; the `clear-param-plocks` target `rack-slot-instrument`,
  `:slot-idx` the rack slot): one entry, only the steps holding a lock,
  and a cleared modulation depth lock drops its derived active lock as
  the track instrument's clear does.
- **Voices.** `device.voices` is a rack slot's max polyphony (a model
  field of the device sync; the legacy `tp-poly` / `tp-max-polyphony` show
  the selected slot's: `(> rs.voices 1)`); any other device reads 0, so the
  field declares no `:range` (0 would violate one) and its setter checks:
  an integer in 1–16 (`SetRackSlotMaxPolyphony` through history, refreshed
  with the legacy command's `rack_slot_voices_applied`; a drag joins one
  entry); another device's is an error. Since eseq-0l17.54 it is a strip
  control (`StripControl::Voices`, below) with `voices-display` and
  p-locks.
- **Delete targets.** `device.delete-target` (live) reads true while the
  active delete target names the device (`DeviceSlot::delete_target`: a
  rack slot or rack slot effect, a bus effect, and a chain effect or MIDI
  effect of the current track: the fx panel's targets name the current
  track's chains). `(set! d.delete-target true)` makes the device the
  target; false clears it only while it names the device; an instrument,
  or another track's chain effect, is an error.
- **Strip controls** (built by eseq-0l17.42). A rack slot device carries
  its slot's strip as fields, split like a send (§14.1): `gain`, `pan`,
  `muted`, `soloed` are the slot's own values (the base: a `set!` never
  p-locks), each with `-display` (the value shown: the p-lock at the
  current track's selected step, else its playing step, else a rack macro
  mapped onto it, else the base: `rack_slot_control_value`, shared with
  the legacy `rack_slot_value_field` publisher) and `-locked` (a p-lock
  supplies it); `choke` is the choke group (0 none; no p-locks) and
  `enabled` (the model field the device sync pushes) becomes settable for
  a rack slot. Any other device reads 0 / false and its `set!` is an
  error. All are live: the device live loop queues the observed strip
  fields and reads them under one rack lock per tick
  (`DeviceState::push_strips`; `strip_locks` counts them), pushed after
  it is released; a cold read takes the lock for its one field. A strip
  edit moves the rack revision but not the layout fingerprint, so it does
  no device work. Setters: `set-device` (`:field` `gain`, `pan`,
  `muted`, `soloed`, `choke`, `enabled`) under the value rule (gain a
  number in 0–2, pan in −1–1, choke an integer in 0–16, the flags a bool;
  the value the slot holds is a no-op, even one stored out of that range:
  the base fields read the stored value unclamped, `StripControl::read`,
  so a value read back round-trips; an unknown field is an error) through
  the legacy commands' history
  edits (`SetRackSlotGain`, `SetRackSlotPan`, `SetRackSlotMute`,
  `SetRackSlotSolo`, `SetRackSlotChokeGroup`, `SetRackSlotEnabled`) and
  their refresh, now one helper the legacy `set-rack-slot-*` commands
  share (`rack_slot_strip_applied`: the control snapshot republish, the
  value field, the slot dicts for mute, solo and enabled; the choke group
  refreshes nothing, the legacy command's repaint of the unchanged
  base-note field is dropped); a gain or pan
  drag joins one entry (ScriptEdit), any other strip edit is its own.
  Locks are actions, as for params: `(lock-strip! d field steps v)` and
  `(unlock-strip! d field steps)` (`set-device-strip-locks` /
  `clear-device-strip-locks`; `field` `gain`, `pan`, `muted` or
  `soloed`, `v` under the field's rule, steps of the device's track)
  act on the steps that differ (`lock_steps`, shared with
  `lock-rack-macro!`, §14.2g) through `SetRackSlotParamPlockMulti` /
  `ClearRackSlotParamPlockMulti` (one undo entry), refreshed like
  `set-rack-slot-param-plock` (`rack_slot_plock_applied`, shared) plus
  the steps' p-lock presence. Deleting a rack slot now forgets its
  identity and shifts the later slots' down
  (`DeviceIdentityRegistry::remove_rack_slot`, in `delete-rack-slot`'s
  recorded mutation): before, the next slot inherited the deleted slot's
  `did`, so the deleted slot's handle silently retargeted it and the
  next slot's handle went stale; undo rebinds the captured identities.
- **Base note and voices** (built by eseq-0l17.54). Two more rows of the
  same tables (`StripControl`, `devices::STRIP_FIELDS`), no new path: a
  rack slot's `base-note` (its own offset, read unclamped like the other
  bases), `base-note-display` and `base-note-locked`, and
  `voices-display` (the voices base stays the model field `voices`),
  read in the strip pass under its one rack lock. `device.base-note` is
  now a strip field for every device: a track instrument's reads its
  atomic (no rack lock: `other_strip_field`; its `-display` is the same
  value and `-locked` false), any other device 0 (`voices-display` too).
  `set-device` `base-note` on a track instrument keeps its 7b-3 path
  (`SetInstrumentBaseNoteOffset`, the `BaseNote` invalidation); on a rack
  slot it is `SetRackSlotBaseNoteOffset` under −48–48, and `voices` is
  `SetRackSlotMaxPolyphony` under 1–16 (both through history, a drag
  joins one entry: `StripControl::drags`), refreshed by
  `rack_slot_strip_applied` (base note: the control snapshot republish
  and the value field, which the legacy `set-rack-slot-base-note` now
  shares; voices: `rack_slot_voices_applied`). `lock-strip!` /
  `unlock-strip!` take `base-note` (−48–48) and `voices` (an integer in
  1–16) through `SetRackSlotParamPlockMulti` /
  `ClearRackSlotParamPlockMulti` (`RackSlotParam::BaseNote`,
  `MaxPolyphony`). **Decisions.** `voices-display` is published because
  the rack panel's V picker shows the displayed value
  (`rack-slot-display-value slot :max-polyphony :max-polyphony-field`,
  `content/ui/effects/instrument-panel.lisp`), not the base;
  `voices-locked` is not: the panel's p-lock marker is the slot
  wrapper's `plock-any` over its targets (any step), never the shown
  step's lock, and the device's observed mask has the params bit last
  (30 live fields + `params` = 31 bits then, 32 after eseq-0l17.61's
  `strip-locks`; eseq-0l17.71 widened the masks from `u32` to
  `ObservedMask` = `u64`, so the device kind has room for 63 live fields
  plus `params`).
- **Not covered:** the rest of the panel extras
  (eseq-0l17.37, built: §14.2g), rack slot sampler playheads
  (`device.playhead` is a track instrument's).

### 14.2g Built in stage 7b-3 (eseq-0l17.37)

| Kind | Key | New `:host` fields (`:set` in brackets) |
|---|---|---|
| `param` | `(device index)` | `overridden :bool` (L: `value` shows a selected neuron's override); placement: `label :string`, `section :string` (`main`, `mod`, `source`, `hidden`), `mod-slot :int`, `visible :bool` (L); lanes: `mod-targets (list-of mod-target)`; display: `mod-offset`, `mod-value`, `mod-scale :number` (L), `mod-ratio :number` (L, since .14: `mod-value` / 100 for a `percent` param, else `mod-value`: visualizers drawn on the stored 0–1 scale bind it); process: `process-mapped :bool`, `process-value :number`, `process-clamped :bool` (L); `key-locks (list-of (list-of :number))` (L, `(note value)` rows) |
| `mod-target` | `(param index)` | `param param`, `index :int`, `source param` (nil: a fixed source), `slot :int`, `depth param`, `depth-min`, `depth-max :number`, `unit :string` |
| `device` | `((track bus) did)` | `base-note :number` (L, −48–48) [d], `mod-phases (list-of :number)` (L), `tensors (list-of tensor)`, `key-locked-notes (list-of :int)` (L), `variants (list-of variant)` (L, key-lock variants), `macros (list-of rack-macro)` (a drum rack's, on its instrument device) |
| `tensor` | `(device index)` | `device device`, `index :int`, `name :string`, `rows`, `cols :int`, `min`, `max :number`, `values`, `base (list-of :number)` (L), `locked :bool` (L) |
| `track` | `(index)` | `variants (list-of variant)` (L, the step variants: the chip list) |
| `variant` | `((track device) vid)` | `track track`, `device device` (nil: a step variant), `label`, `name :string`, `count :int`, `color :rgb`, `current :bool`, `notes (list-of :int)` (all L but `track`, `device`) |
| `macro` | `(index)` | `index :int`, `mid :int`, `script-key :string` (optional: empty when none), `name :string` [m], `type :string` (`mapped`, `scene`), `value :number` (0–1) [m], `mappings (list-of macro-mapping)`, `target-scene scene`, `morph-params`, `steal-patterns :bool`, `quantize :string` |
| `rack-macro` | `(device index)` | `device device`, `index :int` (0–7), `stable-key :string` (always set), `name :string` [rm], `value :number` (L), `base :number` (L) [rm], `locked`, `has-locks :bool` (L), `mappings (list-of macro-mapping)` |
| `macro-mapping` | `((macro rack-macro) index)` | `macro macro`, `rack-macro rack-macro` (one is nil), `index :int`, `target param` (nil: no device param), `label :string`, `min`, `max :number` [mm], `curve :string` [mm] (`linear`, `exp`, `log`, a project mapping's also `log-domain`), `suspended :bool` (positional: see Macros); since .18 `path`, `param-label :string` (the mapping table's columns; `label` is path · param-label) |
| `project` | `()` | `macros (list-of macro)`; `(macros)` |

[d] = `set-device` (`:field` `base-note`); [m] = `set-macro` (`:macro-id`,
`:field`); [rm] = `set-rack-macro` (`device-target`, `:macro`, `:field`
`name` or `value`); [mm] = `set-macro-mapping` (`:macro-id`, or a rack
macro's `device-target` and `:macro`; `:mapping`, `:field`); all but [d] in
`host_commands/panel.rs`. Actions: `(set-tensor-cell! tz cell v)`
(`set-device-tensor`), `(stamp-variant! t steps v)` (`stamp-variant`, nil
`v` clears the steps' variant locks), `(stamp-key-variant! d notes v)`
(`stamp-key-variant`, nil `v` clears), `(lock-rack-macro! rm steps v)` /
`(unlock-rack-macro! rm steps)` (`set-rack-macro-locks` /
`clear-rack-macro-locks`, eseq-0l17.57).

Built (7b-3):

- **Placement.** `param.section` sorts a param as the panels do
  (`PanelSection`, shared with the instrument panel builders): a `mod …`
  lane param, a voice modulator source's setting (`source`, with its
  source in `mod-slot`), host plumbing (`hidden`), else `main`; `label` is
  the name the panel shows (`mod ` stripped, a source setting by its role:
  `type`, `rate`, `attack`, …). `visible` is live: false for a hidden param
  and for a source setting the source's type does not use
  (`selected_source_param_indices` over the displayed values, computed at
  most once per device per tick), so a view builds the source sections
  from `d.params` without a dict.
- **Lanes.** `param.mod-targets` are `mod-target` instances registered
  with the params (from `instrument_modulation_targets`, each keyed (param
  instance id, lane)); a lane names its source and depth params as
  instances (`mt.depth.value` is the depth), its fixed source slot, and its
  depth range in the depth param's display units (`mod_target_depth_range`,
  shared with the rack panel: a sampler's lanes store DSP units; a lane
  whose depth param is `percent` reads its range x 100 like the depth's
  value, since .14). A descriptor change replaces them with the params.
- **Modulation display.** `mod-offset`, `mod-value`, `mod-scale`,
  `mod-ratio` and `device.mod-phases` read the tick's modulation sample
  (`ModDisplayValues`, compared and copied into `KindsShared` only while
  one of these fields is observed; a cold read otherwise sees the last
  copy): the legacy poll, now also run while one of these fields of a
  device the sample covers is observed with the fx panel hidden
  (`HostKinds::wants_mod_display`, `mod_sampled`; at the meter cadence,
  the watchlist released when neither wants it). The sample covers what the
  panel shows (every effect by its node, the current track's instrument,
  its rack's selected slot); anything else reads no modulation (offset 0,
  `mod-value` = `value`, scale 1, phases −1) and keeps no poll alive.
  Effect samples are stored units and are shown in display units, like
  every kind value (`mod_sample`, shared by the param and phase reads).
- **Process mapping.** A track instrument's params an enabled process slot
  writes (`process_bound_instrument_params`, shared with the panel),
  cached per track under its `PlockKey` (a process chain edit's
  invalidation moves it); `process-value` is the scheduler's last write
  (`SequencerState::process_effective_param`, one entry, no copy of the
  feed) while mapped, else `value`.
- **Key locks.** A track instrument's visible key locks
  (`instrument_key_locks`, shared with the instrument panel), cached per
  track under its `PlockKey` (key-lock edits move the fx and UI epochs);
  `param.key-locks` and `device.key-locked-notes` recompute only when it
  moved (the observed lists' per-entry key, like `has-locks`).
- **Base note.** `device.base-note` is a track instrument's offset (an
  atomic); `(set! d.base-note 12)` is `SetInstrumentBaseNoteOffset` through
  history (a drag joins one entry) with the legacy `BaseNote`
  invalidation; a rack slot's is its own (a strip control, built by
  eseq-0l17.54: §14.2f); any other device's is an error.
- **Tensors.** `device.tensors` are registered with the device (model;
  replaced with the params on a descriptor change); their cells are live:
  `values` the displayed step's p-lock else `base`. The tick reads only
  the observed parts, straight from the slot's cells into a reused buffer
  (`read_cells_into`, `has_tensor_plock`: no metadata copy, no base copy
  while unlocked) and compares them with the pushed list in place.
  `set-tensor-cell!` sets one base cell (value rule: a cell index in range,
  a value in `min`–`max`; a tensor with no cells is an error) through
  `DeviceSlot::tensor_cell_command` (`SetInstrumentTensorCell` /
  `SetEffectTensorCell` / `SetMidiFxTensorCell`; a drag joins one entry),
  an instrument's legacy field resynced by `sync_instrument_tensor_display`
  (shared with `set-instrument-tensor-cell`); a rack's or a bus's tensors
  read their defaults and take no `set!` yet.
- **Variants.** `t.variants` (the step panel's chips, legacy
  `SEQ.track-plock-variants` without its `def` chip) and an instrument's
  `d.variants` (key-lock variants) are `variant` instances keyed (owner
  instance id, the label's A, B, …, A', … index), so a handle is the
  variant while it exists. Each registry is read (reconciled, as the legacy
  publishers do) once per owner track `PlockKey` (`variant_snapshot`,
  cached in `KindsShared`, `plock_cached`); the lists are computed while
  observed and only when it moved; a variant's fields from the snapshot,
  `current` per tick (the first selected step plays it: its variant key is
  read once per track per key and selected step, however many variants
  observe it). Whenever an owner's key moved, the tick drops its variant
  instances whose variant is gone, observed or not
  (`HostKinds::prune_variants`), so a held handle goes stale instead of
  silently becoming a later variant that reuses its label. The chips'
  label, name, count and color are `VariantChip` (shared with the legacy
  chip lists). `stamp-variant!` and
  `stamp-key-variant!` act on the steps or keys that differ, through the
  legacy edits (`stamp_step_variant`, shared with `stamp-plock-variant`;
  `key_variant_command`, shared with `stamp-key-lock-variant`; a key
  variant's registry and assignments from one reconcile,
  `key_lock_variant_registry_with_assignments`): one undo entry; an
  unknown label, another track's step or a non-MIDI note is an error.
- **Macros.** Project macros are positional `(index)`, the instance kept
  by macro id across reorders (`reconcile`) and replaced on a project load.
  A drum rack's macros hang off its instrument device (the rack's macros
  live in the rack track's snapshot, beside its slots, not on the group):
  `rack-macro` keyed (device instance id, index). Mappings are keyed (macro
  instance id, position), as the legacy commands address them: a mapping
  has no stable id in the model, so deleting one retargets the handles of
  the mappings after it (each names the mapping now at its position). A
  mapping's
  `target` is the param instance it drives (`macro_mapping_location`,
  shared with `SEQ.macros`; the target device's params are registered for
  it), `label` the panel's (`target-label` / path · param), `min` / `max`
  in the target's display units. `key` is a keyed kind's built-in field,
  so a project macro's script key (`macro-ensure`; optional, empty when
  none) is `script-key`, and a rack macro's stable id (`macro_1`, …;
  always set) is `stable-key`. Feeds: the project macros' structure
  (everything but the values) is compared every tick with the last
  synced, and synced again when what targets resolve through moved
  (`MacroInputs`: param replacements (`params_generation`; a fresh
  registration resolves no target anew, since the macro sync registers
  the params it targets), the device registry, the FX epoch, the track,
  bus, scene and device instances; compared in place); the values every
  tick (a drag moves no counter); the racks' macros when the rack revision
  or the inputs moved, a rack whose names and mappings did not change
  skipped (a macro drag syncs nothing: `MacroState::rack_syncs`); a rack
  macro's value (`rack_macro_shown_value`, shared with
  `App::effective_rack_macro_value`: a take's override, the displayed
  step's p-lock, else the base under an engaged project macro), base and
  lock flags are live: one rack lock per tick reads every observed field
  (`MacroState::rack_live_locks`), pushed after it is released, and
  `has-locks` only when its track's `PlockKey` moved (the observed list's
  per-entry key, as params). Setters: a project macro's name, a mapping's
  range (display units, within the target's range; the bound's current
  value is a no-op checked before the range, so the value a view reads
  always sets back) and curve (`MacroCurve::from_label` /
  `RackMacroCurve::from_label`, the labels `label()` prints everywhere, so
  the current value round-trips: `linear`, `exp`, `log`, a project
  mapping's also `log-domain`) through `MacroRename` / `MacroSetRange` /
  `MacroSetCurve` (one undo entry each); its value through
  `MacroSetValue`, a performance control with no undo entry (as the macro
  panel's). A rack macro's name, base and mappings go through the rack
  panel's edit, recorded since eseq-0l17.44 (below); a rack macro's locks
  are actions (eseq-0l17.57, below).
- **Recorded rack macro edits** (built by eseq-0l17.44). A rack macro's
  name, base (`value`), and a mapping's range and curve are one recorded
  edit, `App::apply_rack_macro_edit` (a `RackMacroField`: `Name`, `Value`,
  `Range { target, min, max }`, `Curve { target, curve }`; a mapping is
  named by its `RackMacroTarget`, which a macro maps at most once, so an
  edit keeps naming its mapping when an earlier one is unmapped), a
  field-granular patch like the graph override and neural field edits
  (`stage_field_edit`, now keyed by a site: the current scene for those,
  a pattern here). The pattern is the one the live rack mirrors
  (`mirror_device_pattern_id`, as step and track param edits resolve it:
  a bound take's while a sound binding borrows the lane).
  `EditPatch::RackMacro` keeps the field before and after, keyed
  (`TrackId`, pattern, macro), so replay writes that field alone, in
  place under one scenes lock, into the rack macros of the Patch that
  pattern plays (`SequencerState::write_rack_macro_field`; the patterns'
  p-locks untouched), and into the live rack whenever the mirrored
  pattern plays the same Patch (a shared Patch entity counts: an undo
  after switching to a pattern sharing it reaches the live rack), with
  the rack panel's side effects when the live rack changed (a value: the
  runtime default and the transient target values; a mapping: a
  scheduler publish), never the macro's locks or other mappings. A
  mapping no longer mapped is a replay error that changes nothing. Every
  edit stages a coalescing gesture keyed by the field
  (`rack-macro:<track id>:<macro>:<field>`, a mapping by its target's
  address), so a drag's writes join one entry that keeps the value from
  before the drag, and a drag back to where it started records nothing;
  when the site moved under the open gesture (a scene or pattern switched
  mid-drag; graph and neural edits too), `stage_field_edit` finishes the
  old entry first, so each site keeps an entry of its own. The rack
  panel's `set-rack-macro-value`, `rename-rack-macro`,
  `set-rack-macro-range` and `set-rack-macro-curve`
  (`apply_rack_macro_edit_reactive`: a knob drag is one entry, closed by
  the pointer release or the idle timeout, typing a name one entry per
  pause) and the kind setters (`set-rack-macro` `name` / `value`,
  `set-macro-mapping` `min` / `max` / `curve` on a rack macro's mapping, as
  `ScriptEdit`s: a `value`, `min` or `max` drag joins one entry, a name or
  curve is its own) share one tail (`rack_macro_edit_reactive`, then the
  refresh: a name its text field, a value the macro's and its targets'
  value fields, a mapping the instrument panel). The value a field holds
  is a no-op (no entry). One write path: the unrecorded value reset after
  mapping a param (`App::set_rack_macro_value`) writes through it too (a
  value it already holds still reaches the targets); the unrecorded
  rename and mapping setters are gone. Mapping and unmapping a param
  (`map-rack-macro-param`, `unmap-rack-macro-param`) stay unrecorded and
  first finish an open gesture.
- **Rack macro locks** (built by eseq-0l17.57). Actions, as
  `lock-strip!` (§14.2f): `(lock-rack-macro! rm steps v)` and
  `(unlock-rack-macro! rm steps)` (`set-rack-macro-locks` /
  `clear-rack-macro-locks`: the rack's `device-target` and `:macro`, the
  steps with their `:step-tracks`; `v` under the value rule (§14.2c), a
  number in 0–1; a step of another track is an error that changes
  nothing) act on the steps whose lock differs (or that hold one, to
  clear) through `SetRackMacroPlockMulti` /
  `ClearRackMacroPlockMulti`, one undo entry (a script drag's locks of the
  same steps join one, eseq-0l17.58), nothing at all when no step
  differs (`lock_steps`, shared with `lock-strip!`: the pending steps,
  the command, the presence and the rows). The refresh is the rack
  panel's `set-rack-macro-plock`'s (`refresh_rack_macro_plock_reactive`,
  now shared and taking how the rows moved: the macro's and its targets'
  value fields, and the p-lock rows plus one UI epoch resync only when a
  lock lands anew on, or is cleared from, the shown step) plus the steps'
  p-lock presence (a `StepInvalidationBatch`). `rack-macro.value`, `locked` and `has-locks`
  follow on the next tick (the track's `PlockKey` moved).
- **Neural selection.** While a neural neuron is selected for step
  editing, its output override shows in a track instrument's or chain
  effect's `param.value`, as in the legacy value fields
  (`selected_neural_*_plock_value`; the selection is a shared handle), and
  `param.overridden` is true. `locked` keeps its one meaning: whether a
  p-lock supplies the value at the displayed step (the override hides it
  without changing it).
- **Playhead reads.** The tick compares each track instrument's sampler
  voices and sample with the `App`'s every tick
  (`SamplerPlayhead::current`, over the borrowed
  `App::sampler_path_ref_for_track`: no allocation) and refreshes its
  `DeviceSource` when they moved; an observed `device.playhead` and a cold
  read both sample those voices (no `SamplerPlayhead::of` per tick), so a
  read after a sample load or voice rebuild sees the current voices
  without a model sync.
- **Not covered** (eseq-0l17.43, built: §14.2l): the sampler panel's media
  (waveform buffer, slices, onsets, analysis, selection times), the
  sound-binding badge and display name, a modulator instrument's phase and
  level, the fixed modulators' labels, param UI metadata, effect tables and
  IR names, the built-in effect editors (eseq-0l17.56, built: §14.2p), the meter selector,
  scene macro config setters and `step.variant`. Rack macro edits are
  undoable since eseq-0l17.44 (above).
- eseqlisp strings have no `\"` escape: a quote escaped in a string (a
  `:doc` included) ends it, and the module fails to compile with no
  message.

### 14.2h Built in stage 7c (eseq-0l17.29)

| Kind | Key | New `:host` fields (`:set` in brackets) |
|---|---|---|
| `process` | `(track proc-id)` | `track track`, `proc-id :int` (the stable `ProcessInstanceId`; not `pid`, which `cell` uses for a pattern id), `index :int` (chain position, fire order), `class process-class` (nil: not in the library, and for an expr card's compiled `expr#…` body, never a library class), `class-name`, `name` (the instance name, else the class), `instance-name :string` (empty: none), `project`, `default-lane`, `roster :bool`, `enabled :bool` [x], `doc`, `source-path`, `target :string` (the class's `doc` / `source-path` / `target`, kept so a process whose `class` is nil still shows them), `lanes (list-of lane)`, `inlets (list-of inlet)`, `ports (list-of port)`, `in-ports (list-of :string)`, `cells (list-of state-cell)`, `expr :bool`, `expr-line`, `compile-error :string`, `error :string` (L) |
| `lane` | `(process index)` | `process process`, `track track`, `index :int` (position in `p.lanes`), `position :int` (position in `t.lanes`), `inlet`, `label`, `short-label`, `type :string`, `min`, `max`, `default :number`, `decimals :int`, `forked :bool`, `values (list-of :number)` (256) |
| `inlet` | `(process index)` | `process process`, `index :int` (position in `p.inlets`), `name`, `type :string`, `options (list-of :string)`, `value :number` [x], `default`, `min`, `max :number`, `decimals :int`, `doc :string` |
| `port` | `(process index)` | `process process`, `index :int` (position in `p.ports`), `name`, `label`, `hint`, `target`, `status :string` (`bound`, `hint`, `unbound`), `manual`, `disconnected`, `mappable`, `connectable`, `bindable :bool`, `target-kind :string`, `target-process process`, `target-inlet`, `target-step-param :string`, `fanout (list-of fanout)` |
| `fanout` | `(port index)` | `port port`, `index :int` (position in `pt.fanout`), `target :string`, `target-process process`, `target-inlet`, `target-step-param :string`, `lo`, `hi :number` [x] |
| `state-cell` | `(process index)` | `process process`, `index :int` (position in `p.cells`), `name :string`, `values (list-of :number)` (L) |
| `process-class` | `(index)` | `index :int`, `name`, `doc`, `source-path`, `target :string`, `lane-count :int`, `ports (list-of :string)`; since .67 `node-label :string`, `node-hidden :bool` (§13 .67) |
| `process-library` | `()` | `classes (list-of process-class)` |
| `track` | `(index)` | `processes (list-of process)`, `lanes (list-of lane)` |
| `project` | `()` | `step-param-options (list-of :string)` (the step params a port may write, by their canonical names: an option list the host owns, §14.1) |

Every part kind's `index` is its key index (its place in its process's,
or its port's, list), as `fanout.index` and `state-cell.index`.

[x] = the `edit-process` host command (`:track-id`, `:proc-id`, `:op`;
`host_commands/lanes.rs`). The `:set` of `process.enabled` and
`inlet.value` are the actions `set-process-enabled!` and `set-inlet!`;
`fanout.lo` / `hi` go through the same command (`fanout-address`).
Actions: `(set-process-enabled! p v :all true)`, `(set-inlet! i v :all
true)`, `(set-lane-steps! l steps v)` (steps: step instances of the lane's
track), `(move-process! p before)` (nil: to the end of its layer),
`(add-process! t c)` (`c` a `process-class`: a lane of the track's own, as
the patch bay's + cell), `(remove-process! p)`, `(bind-port! pt x)`,
`(add-fanout! pt x)`, `(unbind-port! pt)`, `(clear-port! pt)`,
`(remove-fanout! fo)` (each port action takes `:all true`). A port target
`x` (`port-target`) is an instance of the port's own track: a `param` of
its devices (instrument, chain effect or MIDI effect), a `send` of the
track (a bus send), a `lane` or `inlet` of another process of the track (a
wire); or a step param's name (a string: any spelling the scheduler
accepts, stored as its canonical name, one of `project.step-param-options`).
The singleton is `process-library`, not `processes`: `processes` is the
process DSL's own form (`(processes :track 0 …)`), which a referred
singleton would shadow.

Built (7c):

- **The chain is the composed one.** `t.processes` is the track's chain as
  the scheduler fires it (`composed_track_process_chain`): the project
  lanes (every track's, with this track's forks applied), then the track's
  own; `t.lanes` every lane of it in the lane selector's order (legacy
  `SEQ.track-process-lanes`, whose `lane-index` is `l.position`). The
  current track's (legacy `SEQ.process-lanes`, `SEQ.process-slots`) are
  `selection.track.lanes` / `processes`. The selector's mode numbers
  (`seqv-process-lane-mode-offset` + lane position) and the selected lane
  are view state (the editor's param mode), not host fields. A lane's
  values are per step on the track's own timebase (`t.timebase`); a step
  no write reached reads the lane's default (a write past the lane's end
  pads the steps before it with the default: the slot's inlet literal,
  else the class default, eseq-gk0h).
- **Identity.** A process is keyed (track instance id, its stable
  `ProcessInstanceId`): a reorder keeps the instance (only `index` moves),
  and a project lane is a process of every track, each its own instance
  (its lanes, inlets and bindings fork per track). Lanes, inlets, ports
  and state cells are keyed (process instance id, index) in the class's
  order; fan-out entries (port instance id, index): removing one retargets
  the handles after it, as macro mappings. A process whose class changes
  (an expr card's body) gets fresh parts. A removed process (or track)
  drops its parts; a project load drops the tracks and with them every
  process; classes are positional, kept by class id (`registry::reconcile`)
  and replaced on a project load. A chain repeating an id gets no second
  instance.
- **Feeds.** A track's processes are registered on the first read of
  `t.processes` or `t.lanes` (the reader hook, `cold_track_lanes`, or the
  tick once either is observed), like a device's params, so a project
  whose views read no lanes registers none. The tick then keeps a
  registered track current behind its lane key: the track's process
  generation (`UiInvalidationQueue::process_generation`, a per-track
  revision moved by `ProcessChain`, `ProcessLaneValues`, whole-track and
  project invalidations), the library's version and the class set. The
  global triggers, the pattern epoch (a chain edit, a scene switch, a
  roster edit, a script's `processes`) and the scenes revision (the
  project layer; an undo restores the scenes), mark no track by
  themselves: when either moves the tick fetches the project layer once
  and compares it with the last fetch, and compares each registered
  track's own chain and project lane overrides in place with those its
  composed chain was built from (`track_process_chain_is`,
  `project_lane_overrides_are`); only a track whose inputs moved is
  synced. None reads the history revision or the UI epoch: an idle tick
  or a UI epoch costs a few loads per registered track and allocates
  nothing (`LaneShared::syncs` counts the syncs, `process_syncs` the
  processes re-derived). A sync composes the chain from the project
  layer, the track's own chain and its overrides, and compares it (and the
  overrides' forked lanes: an override holding the shared values composes
  the same chain but forks the lane) with the last synced one: unchanged,
  nothing is pushed; changed only in some slots' lane values (a lane
  drag), only those processes' lanes are re-derived (each lane's fields
  compared with its cell, so a drag re-pushes that lane's `values`); else
  every process is, each push compared with its cell. The library snapshot
  is fetched once per library version (`LaneShared::published`, shared by
  the cold reads and the tick). Derivations are the legacy publishers':
  `process_slot_lane_entries` / `process_lane_entries_for_chain` (lanes),
  `process_scalar_inlet_view` / `process_scalar_inlet_names` /
  `process_inlet_def` (inlets), `process_slot_port_defs` /
  `process_port_view` (ports, fan-out), `process_port_readers`,
  `resolve_process_inlet_target` and `lane_patch_in_port` (the patchbay's
  wiring and in ports), `process_library_defs` (classes; compiled `expr#…`
  bodies excluded). Live: `process.error` (the slot's latest run error
  under its runtime id on this track, `SequencerState::process_run_error`)
  and `state-cell.values` (the cell's scope history, read in place:
  `with_process_scope_cell`), kept in `ObservedList`s and re-read only
  while observed and when, respectively, the scheduler's run error or
  scope version, or the observed set, moved. The id lists those loops read
  are rebuilt only when a track's process or state cell instances changed.
- **The patchbay is a view derivation.** Legacy `SEQ.track-lane-patch`
  maps to the processes: `p.in-ports` (lane, gate and wired inlets, in
  class order), a connectable port's `target-process` / `target-inlet`
  (its primary wire, resolved with the scheduler's same-layer rule) and
  its `fanout` entries' (the further cables); the writers into an in port
  are the ports and fan-out entries naming it. A cable's port id
  (`(track * 4096 + slot) * 16 + ordinal`) is computed by the view from
  `t.index`, `p.index` and the port's place among the connectable ones.
  `port.disconnected` and `manual` replace the legacy `clearable` /
  `disconnectable` / `primary-free` flags (`(or pt.manual
  pt.disconnected)` is clearable).
- **Setters.** `edit-process` resolves the track by `TrackId` and the
  process by its `:proc-id` (matched as the number Lisp holds, so an id
  past 2^53 still resolves; a gone track or process is an error) when it
  lands, and a port target the same way, with its track's id: a target on
  another track is an error (`a port targets its own track`; a param by
  its device's `device-target`, resolved to its slot now: another track's
  param, a drum rack slot's or a bus effect's is an error), as is a wire
  into the process itself or across layers (project to track lanes), and
  a step param name the scheduler does not take. `move-process!`'s
  `before` must be a process of the same track and layer; moving a
  project lane reorders the project layer, so it moves on every track.
  They act only where the model differs and go through the legacy edits,
  shared with `process-history-action` (`process_edit::apply_process_edit`
  over `ProcessEdit`, one recorded scene-structure entry each, undo
  restores; the bus-send pre-step included) and queue `ProcessChain`; lane
  steps go through `app::edit::apply_process_lane_drag_steps` (the drag's
  merge key: a script's `set-lane-steps!` on one lane while the pointer is
  down joins one entry, else each is its own) and queue
  `ProcessLaneValues`. Field setters edit this track (a project lane forks
  for it); the actions' `:all true` writes the shared project slot and is
  an error on a track's own process. A shared edit is a no-op, recording
  nothing, unless it changes the model (the every-track state writes
  report a change, not the slots they matched), and a shared fan-out edit
  addresses the project layer's own list (its index is the shared list's,
  whatever this track's fork holds; the edit drops every track's fork of
  the port). Values follow §14.2c: an inlet or lane value is a number of
  its type (a gate 0 or 1, or a bool; an int or track an integer; an enum
  an integer below its option count) within the class's declared range (a
  range the class does not declare, such as the ±1 hint around a float,
  is not enforced); the current value always round-trips (a lane's when
  every named step holds it). A fan-out bound is any finite number.
- **Not covered.** A graph node's process slots are `process` instances
  since 7g-2 (§14.2m), with their scopes and run errors (legacy
  `SEQ.process-scope-cells` and the node half of `SEQ.process-run-errors`;
  7c-2, §14.2n). A class's ports are listed by name
  (`process-class.ports`); a process's ports carry the rest.

### 14.2i Built in stage 7f (eseq-0l17.32)

| Kind | Key | New `:host` fields (`:set` in brackets) |
|---|---|---|
| `browser` | `()` | `track track` (the track the sidebar shows), `instrument-kind :string` (`sampler`, `instrument`, `empty`), `instrument`, `instrument-label`, `preset`, `sample :string`, `presets`, `user-presets`, `engines (list-of :string)`, `rack-slots (list-of slot-presets)`, `sound-presets`, `kit-presets (list-of preset-file)`, `library-epoch :int`, `preview-playing :bool` (L), `preview-position :number` (L) |
| `preset-file` | `(index)` | `index :int`, `type :string` (`sound`, `kit`), `icon`, `name`, `path`, `author :string`, `pads :int`, `tags (list-of :string)` |
| `slot-presets` | `(index)` | `index :int` (the slot), `device device` (the rack slot device), `instrument`, `instrument-label`, `preset :string`, `presets`, `user-presets (list-of :string)` |
| `sound-palette` | `()` | `open :bool`, `track track` (nil while closed), `target :string` (`take`, `pattern`, `cell`), `target-id :int` (-1 for cell), `instrument :string`, `sounds (list-of sound)` |
| `sound` | `(track patch-id)` | `track track`, `patch-id`, `mix-id :int` (-1: unknown), `name :string` [`sound-rename`], `referents`, `referents-short :string`, `base`, `track-sound`, `current :bool`, `preset`, `sample :string`, `diff-up`, `diff-down :int`, `colored :bool`, `color :rgb`, `glyph-key :string` |
| `editor` | `()` | `mode`, `surface`, `buffer`, `error :string`, `canceling :bool`, `run-mode :string` [`set-draft-instrument-run-mode`], `active-macro`, `active-macro-action`, `open-macro :string`, `patch-macros`, `library-macros (list-of editor-macro)`, `assets (list-of editor-asset)`, `selected-asset asset-info` (nil: none) |
| `editor-macro` | `(index)` | `name :string`, `library :bool`, `params`, `calls`, `outputs (list-of :string)`, `summary :string`, `used :bool` |
| `editor-asset` | `(index)` | `index :int`, `reference`, `tier`, `source-path :string` |
| `asset-info` | `()` | `reference`, `tensor-kind`, `layout`, `source :string`, `shape (list-of :int)`, `wave-count`, `waves-per-set`, `set-count :int`, `sets`, `wave-names (list-of :string)` |
| `learn` | `()` | `target-path`, `target-name`, `phase :string`; settings [l]: `method`, `cma-refine-mode :string`, `epochs`, `cma-generations`, `cma-population`, `cma-seed`, `cma-forward-batch`, `local-epochs`, `cma-continue`, `cma-refine-epochs`, `cma-final-epochs`, `gate-frames :int`, `cma-sigma`, `pitch-hz :number`; progress and result: `stage :string`, `current-epoch`, `total-epochs :int`, `loss`, `improvement-pct`, `abs-distance :number`, `losses`, `optimization-losses (list-of :number)`, `plan-params (list-of learn-plan-param)`, `epoch-params (list-of learn-epoch-param)`, `result-deltas (list-of learn-delta)`, `basin-check`, `seeded-wav`, `final-wav`, `error :string`, `applied :bool` |
| `learn-plan-param` / `learn-epoch-param` / `learn-delta` | `(index)` | `index :int`, `name :string`; `status`, `reason :string` / `from`, `value`, `change`, `step :number` / `from`, `to`, `change :number` |
| `retro` | `()` | `lanes (list-of retro-lane)`, `items (list-of retro-item)`, `duration :number`, `truncated :bool`, `error :string`, `playing :bool` (L), `position :number` (L, 0 to 1 in the loop), since .12 `playhead :number` (L, seconds into the capture over the audition's crop, -1 idle) |
| `retro-lane` / `retro-item` | `(index)` | `index :int`, `label :string` / `index :int`, `lane retro-lane`, `start`, `end :number` |
| `song-export` | `()` | `default-name`, `project`, `folder`, `message`, `output-name`, `reveal-label :string`, `end`, `percent :number` (-1: not rendering), `busy`, `done :bool` |
| `settings` | `()` | `audio-workers-choice :string` [`audio-set-workers`], `audio-workers-note`, `midi-error :string`, `midi-persistent :bool`, `midi-devices (list-of midi-device)` |
| `midi-device` | `(index)` | `index :int`, `device-id`, `name`, `status :string`, `enabled :bool` [`midi-set-enabled`], `connected :bool` |
| `agent` | `()` | `generation :int` |
| `factory-promote` | `()` | since .76: `target`, `destination :string`, `skipped (list-of :string)`, `blocking`, `error`, `taken :string` (Promote to factory; the presented `promote` area) |
| `project` | `()` | `name :string`, `audio-workers-options (list-of :string)`; since .17 `instances (list-of :any)` (the package instances, the Packages tree's rows) |
| `track` | `(index)` | `instrument-id :string` (the Instruments tab's `:instrument-id`) |

[l] = the `set-learn` host command (`:field`, `:value`;
`host_commands/learn.rs`). Constants: `learn-method-options`,
`learn-refine-mode-options` (the host's `LEARN_METHODS`,
`LEARN_REFINE_MODES`). Actions: `(open-sound-palette! t :target k :id n)`,
`(close-sound-palette!)`, `(apply-sound! s)`, `(apply-sound-with-mix! s)`
(a sound with no known mix is an error), `(fork-sound! t)`: the palette
commands (`sound-palette-open`, `sound-apply`, …) now also take `:track-id`,
resolved when they land (`palette_track`).

Built (7f):

- **One record, the legacy names its mirror.** Nothing here has a model
  counter: the sidebar, the listings, the palette, the editor, Patch Learn,
  the capture, the export, the settings and the agent are presentation
  state that the legacy publishers compute (`sync_sidebar_browser`,
  `sync_project_state`'s listings, `sync_sound_palette`, the edit-session
  tick) or that commands report (the editor's mode and errors, the learn
  job's events, the export job's status, the MIDI service's snapshot).
  `ui::presented` is their record and the source of truth. A command or
  event edits an area through its typed mutator (`present_editor`,
  `present_editor_sidebar`, `present_learn`, `present_retro`,
  `present_export`, `present_settings`, `present_agent`, with the helpers
  `present_editor_open` / `present_editor_closed`, `editor_error`, the
  set-error-and-refresh every editor command shares, and
  `present_learn_error`): job and session events build typed rows directly
  (plan, epoch and delta rows, editor macros and assets, the selected
  asset's `eseqlisp::editor::AssetMetadata`, MIDI devices, capture lanes
  and items). A computed snapshot is recorded whole (`present_sidebar`,
  `present_sound_presets` / `present_kit_presets`, `present_palette`).
  Each area moves its own generation only when its value changed, and the
  tick (`host_kinds::presentation`) pushes an area only when its
  generation moved (`PresentedState::pushes` counts): an idle tick
  compares counters and allocates nothing, and the kinds never list a
  directory or read a file (the listings are the legacy publisher's, which
  keeps its own triggers). The legacy reactive names (`SEQ.editor-*`,
  `SEQ.learn-*`, `EXPORT`, `AUDIO`, `MIDI.devices` / `error` /
  `persistent`, `AGENT`, `RETRO` but its live fields) are a mirror: after
  each typed edit the mutator writes the legacy fields that changed,
  derived from the record in one module, `presented/legacy.rs`, which
  holds every legacy name, lists each area's fields once (the mirror and
  the registrations share the list, so the legacy defaults are the
  record's: `LearnView::default()`, `EditorView::default()`,
  `ExportView::default()`, …), and never parses a legacy value back.
  **eseq-0l17.76 deleted `presented/legacy.rs` and the mirror calls in
  `presented` (the `mirror` argument of each mutator, the
  `*_registration` calls, the `Sink` impls); no call site changed: the
  mutators keep their now unused runtime argument.** Capture fixtures
  seed an area through `(present-fixture area fields)` (by the kind's
  field names: `song-export`, `settings`, `retro`), which edits the record
  as a command would (the mirror followed until .76). Compared every
  tick in place: the sidebar's track and slot devices against the track and device
  instances, the palette's track and the variant tint its colors go
  through (`theme::variant_display_key`, read once per tick into the
  `ModelRevision` with the track tint), `project.name` (the `App`'s) and
  `browser.library-epoch` (the content library epoch). The stale check
  (a hot reload) asks one instance per collection. Live:
  `browser.preview-*` (the preview player) and `retro.playing` /
  `position` (the audition mailbox).
- **Identity.** Sounds are keyed (track instance id, patch id),
  registered for the palette's track and dropped when the palette leaves
  the track or closes: a held sound goes stale, never another patch.
  Preset files are kept by (type, path), editor macros by (patch or
  library, name), editor assets by reference, MIDI inputs by device id,
  slot presets by their rack slot device: positional instances, each key
  allocated an id while listed (`registry::KeyedRows`, shared with the
  groove library; re-keyed on a reorder). Learn rows and capture lanes and items are positional (a new
  plan or capture re-pushes the values). The Sound and kit listings share
  `preset-file`, and the patch's and the library's macros share
  `editor-macro`, each keyed over the concatenation of the two lists; a
  `preset-file.index` is its place in its own list.
- **Setters.** `sound.name` is the palette's rename (`sound-rename` by
  the track's `tid` and the patch id; same-name is no edit, one undo
  entry, undo restores). `editor.run-mode` is the draft run-mode command:
  the label is case-insensitive, and the current mode (the session's, else
  the record's) is a no-op before anything else is checked; another mode
  needs a draft instrument edit session, else the status says so. Learn settings go
  through `set-learn` under the value rule (§14.2c): a label among the
  options (case-insensitive), an integer in the field's range (a
  population 0 or at least 4), a sigma above 0 up to 10, a positive
  pitch, a positive gate; the current value always works and changes
  nothing; anything else is an error (no clamping; the legacy
  `configure-learn`, which clamped, is deleted (.18)); no undo, as the
  legacy settings. `settings.audio-workers-choice` sends
  `audio-set-workers` with `:strict`: one of
  `project.audio-workers-options`, case-insensitively (anything else,
  `0` or a count past the options included, is an error that leaves the
  note alone); it is saved for the next launch, as the legacy dropdown. `midi-device.enabled` is `midi-set-enabled` by
  device id.
- **Names.** `export` is a module form, so the export modal is
  `song-export` (its fields drop the legacy `export-` prefix). `kind` is a
  built-in field, so the sidebar's kind is `browser.instrument-kind`, an
  asset's `asset-info.tensor-kind`, a preset file's `type`; `id` is too,
  so a MIDI input's id is `device-id`. The audio worker choices are an
  option list on `project` (§14.1). `SEQ.content-library-epoch` is
  `browser.library-epoch`: the library trees (`seq-saved-instrument-tree`,
  `seq-audio-effect-tree`) are natives taking a search filter, so the
  view reads the epoch to re-list them.
- **View-local.** Browser tabs, search, filters, selection, the preview
  path and auto-preview, the export modal's drafts, the capture's crop and
  view, the palette's rename draft, the macro sidebar's filter and
  Settings' open flag are each read by one view (Lisp `defstate`s today):
  `:state` singletons in their own modules when ported (.12, .17, .18),
  not host kinds. Sample import and resample read no host state beyond the
  preview (`browser.preview-*`; resample's print came through a `RESAMPLE`
  namespace this inventory missed, which .17 replaced with arguments to
  `eseq.resample/open`).
- **Not covered:** the legacy-only fields no content reads
  (`SEQ.sidebar-preset-tree`, `MIDI.ports`; `learn-checkpoint-wav` and
  `editor-active` were in the record, mirrored, but no kind pushed them:
  .76 dropped both from the record), and renaming a mix (the palette lists
  patches).

### 14.2j Built in stage 7e (eseq-0l17.31)

| Kind | Key | New `:host` fields (`:set` in brackets) |
|---|---|---|
| `piano-roll` | `()` | `track track` (the current track), `focus-kind :string` (`live`, `pattern`, `take`), `clip-kind :string` (`none`, `pattern`, `take`), `clip clip` (the pinned arrangement clip, nil in follow mode), `focus-label :string`, `focus-num-steps :int`, `window-marker :number` (-1: none), `window-span (list-of :number)` (`(start end)`, empty: none), `window-repeat :number` (0: none), `playhead :number` (L, -1 hidden), `notes (list-of note)` (lazy; then Model) |
| `note` | `(track nid)` | `track track`, `nid :int`, `pitch :int` (−48–48, semitones from C4) [n], `start :number` (steps on the focus axis) [n], `length :number` (1/32–32 steps) [n], `velocity :number` (0–1, its step's) [n], `selected :bool` [n], `label :string`, `hidden :bool` (a script drag's note lies over it); since .16 `item :int` (the timeline's id: step and voice) |
| `param` | `(device index)` | `step-locks (list-of (list-of :number))` (L, `(step value)` rows, display units) |
| `rack-macro` | `(device index)` | `step-locks (list-of (list-of :number))` (L, `(step value)` rows) |

[n] = the `set-note` host command (`:track-id`, `:nid`, `:field`, `:value`;
`host_commands/notes.rs`). Actions: `(add-note! start pitch length
:velocity v)` (`add-note`, on `piano-roll.track`; `&key velocity`, nil
keeps the step's; returns nil, the note shows in `piano-roll.notes` after
the next tick) and `(delete-notes! notes)` (`delete-notes`, `:nids`, with
each note's track in `:track-ids`: notes of one track, else an error that
deletes none).
Constants: `pitch-min` (−48), `pitch-max` (48), held to the host's
`PIANO_ROLL_MIN_TRANSPOSE` / `PIANO_ROLL_MAX_TRANSPOSE` by the tests; a
lane is `pitch-max` minus a pitch (the legacy `piano-roll-lanes` rows are a
view derivation from them).

Built (7e):

- **Identity.** The model gives a note no id: a step holds its notes as
  (transpose, duration, offset) entries, one per (transpose, offset). The
  host allocates each (step, transpose, offset) key (bit-exact,
  `host_kinds::NoteKey`) a note id while a note sits there (`nid`, never
  reused) and keys the instance (track instance id, nid). The note setters
  move a note's id with it (`NoteShared::rekey`), so a handle follows its
  note through a `set!` or a script drag; a note moved onto another
  replaces it (the model keeps one note per key) and the other's handle
  goes stale. An edit made any other way (the legacy piano roll, the step
  grid, recording) keeps a note's instance while it stays at its key and
  otherwise replaces it: a held handle never names another note, it goes
  stale (a gone key's id is forgotten at once, so a later note there gets
  a fresh one). Undo and redo make note handles stale: a replay
  (`App::history_replays`) that changed the source's notes (key, length or
  velocity, compared with the rows last read) forgets every id of the
  source, so every note gets a fresh handle (the ids a setter moved are not
  tracked back through history; an undo that changes no note keeps them).
  Since eseq-0l17.48 that holds only for notes without a model id (below). The notes are those of the piano roll's source
  (`NoteSource`: the current track's resolved edit focus, the effective
  pattern for a live focus): another source (a track switch, a clip pinned,
  a scene launch, a project load) replaces them all; a track reorder keeps
  them (keyed under the track instance). Model note ids, which would keep
  handles through every path: eseq-0l17.48 (below).
- **Model note ids (eseq-0l17.48).** A chord note now carries a `NoteId`
  (u32, `0` none; process-wide, `sequencer::new_note_id`) beside its lanes:
  live `ChordData` (an `ids` lane the scheduler never reads),
  `ChordSnapshot::ids` (so `TrackPatternData.chord_snapshot` and pattern
  snapshots), `StepSnapshot::chord_ids` (the step cells history and the
  step grid's moves restore; `step_snapshot_bit_exact_eq` ignores it: ids
  are identity, not content), and `PianoRollNote::id` (both writers store
  it, `0` taking a fresh one). It is allocated when a note is created
  (`add_note*`, a writer's new note, a recording) and carried by a chord
  removal (`toggle_note` shifts it), a transpose / duration edit, a step
  move or rotate, a snapshot capture / restore (scene switch, undo, redo),
  the legacy piano roll's nudge / move / resize and the setters. A copy
  beside its original gets a fresh one: a paste
  (`sanitize_pasted_step_snapshot`), a doubled pattern, `copy_step`, a
  take flattened from clips (`copy_step_content_from`; the groove's
  late-step move restores the source step's ids after it). A step cell
  replay resolves an id-less note's fresh id once, so the pool and the
  live mirror hold the same one. The chord editor's toggle that leaves one
  note keeps it a chord entry, with its id. Ids are not saved
  (a project file has no lane for them; a load gives fresh ones,
  `chord_snapshot_from_steps*`): handles never outlive a source anyway. A
  lane not beside its notes (a writer that pushed or cleared the notes
  alone) reads as no id, never another note's; take recording and
  retrospective capture push through `ChordSnapshot::push_note`, a fresh
  id beside each note. The `note` kind's `nid` is the model id: a handle follows
  its note through every path above, and an undo brings a replaced note
  back under its id (as a new instance: the old handle was dropped with
  the note). What keeps a host id: a step's single note held by its step
  parameters (a step turned on in the grid, which the model gives no chord
  entry): the host gives it an id by its `NoteKey` from the same allocator
  and registers it with the piano roll lanes (`set_implicit_note_ids`, the
  source's id-less notes by track, focus and key; cleared with the
  source). Every `PianoRollLanes` reader but the host's own
  (`step_rows_batch`) reports such a note with that id, so any writer that
  rewrites its step (a setter, `add-note!` beside it, the legacy piano
  roll's add, paste, delete, nudge, move or resize) stores it in the model:
  from then on it is a chord note with that id. A step move before that,
  and a replay that changes the notes, still make its handle stale. A model id repeated in one source is id-less from its
  second occurrence. So the key table stays, as where each id sits (the
  setters resolve through it) and the id-less notes' identity. `note.item`
  stays: the factory piano roll addresses the legacy timeline actions by
  item id (step and voice), not by note id. Behaviour changes: undo and
  redo keep note handles (`undo_and_redo_keep_note_handles`, replacing
  `undo_and_redo_make_note_handles_stale`); a legacy nudge / move, a step
  move and a recording keep them
  (`note_handles_follow_legacy_moves_step_moves_and_recording`); a note
  joining an id-less note's step keeps its handle
  (`an_implicit_note_keeps_its_handle_when_a_note_joins_its_step`); the
  chord toggle's last note keeps its id
  (`piano_note_toggle_is_one_lossless_history_entry`, which now expects one
  chord entry, not a collapse to the step parameters);
  `pattern_step_cell_replay_gives_pool_and_live_the_same_ids`; model
  tests `note_ids_follow_their_notes_and_copies_get_fresh_ones`,
  `chord_snapshot_ids_never_name_another_note`.
- **Feeds.** The focus fields are model fields behind one key compared
  every tick without allocating (the source, the pinned clip and its source
  kind, the committed song and scenes revisions, the pattern epoch, the live
  length, the song structure generation); they derive through the `App`'s
  focus accessors (`focus_label`, `focus_num_steps`,
  `focus_window_overlay`, `focus_clip_source_kind`), shared with the legacy
  `sync_piano_roll_state`. `clip` is the 7d `clip` instance, so the clip
  panel reads `pr.clip.start` / `end` / `offset` and sets them through
  `clip.start` / `end`. The notes register lazily, on the first read of
  `piano-roll.notes` (the reader hook, or the tick once observed), like a
  device's params; then the tick re-reads them
  (`PianoRollLanes::note_rows_batch`, the legacy items' batch read with
  each step's velocity under the same lock; the instances through
  `reconcile_children`, wanted: the listed notes and the hidden ones) only
  when the source's track was published again (a live focus: the scheduler
  snapshot version moved and the track's published snapshot is another
  one; a roll-record write shows once it publishes), the scenes revision,
  pool content revision, pattern epoch or source moved, an undo or redo
  replayed, or a setter edited them (`NoteShared::syncs` counts): none
  reads the history revision, and an idle tick loads a few counters. The
  host's key table holds both directions (key to id, id to key). `note.selected` is the legacy piano roll's
  selection (item ids), compared in place every tick while notes are
  registered, copied only when it changed. `piano-roll.playhead` (live,
  `App::focus_playhead_step`, the legacy `piano-roll-playhead`) is computed
  per tick only while observed; a cold read of a live focus reads the track
  playhead, of a pinned one -1 while stopped, else the last computed.
- **Setters.** `set-note` resolves the track by `TrackId` and the note by
  its id through the host kinds' key table when it lands; a note that is
  gone (or of another source than the track's piano roll edits now) is an
  error that changes nothing ("the note is gone"), as is a hidden one ("the
  note is hidden under a dragged note until the drag ends"). A `set!` on a
  stale handle sends nothing (a dropped instance takes no write, §4 "stale
  self"); `delete-notes!` of one is the "the note is gone" error. The value rule (§14.2c): `pitch` an integer in
  range, `start` a finite number from 0 to below `focus-num-steps` (its
  offset into the step is the fraction), `length` 1/32–32, `velocity` 0–1,
  `selected` a bool; a full step takes no more notes (an error). Setters act
  only where the note differs, through the legacy piano roll's focus-aware
  history (`app::edit::apply_recorded_focus_step_mutation`, the piano
  roll's pool-first writes; one undo entry each, undo restores) and land
  like the legacy actions (`piano_roll_edit_landed`, now shared with the
  three legacy piano-roll history commands). A note's velocity is its
  step's (a chord's notes share it); a note moving to a step it has to
  itself keeps its own. The selection (no history) follows the selected
  notes where an edit puts them (item ids are (step, voice), and an edit
  shifts the voices beside the notes it moves).
- **Script drags.** While the pointer is down (and no user gesture is
  active, `ScriptEdit::drags`), every `pitch`, `start`, `length` and
  `velocity` `set!`, of any number of notes of one source, joins ONE undo
  entry. A `set!` only records its target (where the note was when the
  drag started and where it goes) in `GestureState::script_note_drag`,
  tied to the drag's gesture id (minted with the history's ids,
  `app::edit::next_gesture_id`; `App::active_note_drag`); the frame is
  built once per command batch (`notes::flush_note_drag`, run by the event
  loop after the batch and before the host kinds sync, and before any
  other command lands): `app::edit::note_drag_frame` (an App-held
  `FocusStepGesture`, `App::pending_drag` like the process-lane and rack
  groove drags) captures the steps the drag newly touches, puts every
  captured step back as the drag found it
  (`FocusStepGesture::restore_before`), places every target there in id
  order and publishes the scheduler once. So a note dragged across another
  replaces it only where it ends up; the note it lies over stays
  registered, unlisted with `hidden` true, until it reappears or the drag
  ends (a `set!` or delete of it is an error meanwhile). A frame that fails
  (a full step) puts back that frame's `set!`s and keeps the earlier
  targets. A drag on another source (a scene launched or a clip pinned
  mid-drag) first ends the open drag, recording every write it made. The
  drag commits as one entry when the gesture finishes
  (`finish_active_gesture`'s hook); Esc rolls it back
  (`notes::cancel_note_drag`: the steps, the notes' ids and the selection
  as the drag found them; "Note drag canceled"). With the pointer up each
  `set!` is its own entry.
- **The tracker** (`alez.tracker`, port .65) needs no kind of its own:
  `tracker-rows` is the steps' fields (`active`, `transpose`, `velocity`,
  the step params) and, per device param column, `param.step-locks` /
  `rack-macro.step-locks` (the pattern's locks, `(step value)` rows,
  computed while observed when the track's p-lock key moved, as
  `has-locks`; the legacy cells' values, `build_tracker_rows_value`);
  `track-automation` (the columns with a lock) is `param.has-locks` /
  `rack-macro.has-locks` and the step params a step holds off their
  default; `track-lock-targets` is the track's devices' params
  (`t.devices`, `t.midi-devices`, a rack's `device.macros`) and lanes; the
  grid playheads derive from `track.playhead` and `transport.position`
  (which copy of a repeating step lights: the transport's sixteenth modulo
  the grid height, confirmed against the track's own step), `-current`
  with `selection.track`. The port (.65) made that derivation the live
  field `track.playhead-row` (and the current track's,
  `selection.playhead-row`), which a row's shader compares, so playback
  re-renders nothing, and the step params off their default the live
  field `track.step-params-in-use`, so the view reads no step.
- **Not covered:** the automation lane under the piano roll
  (`SEQ.piano-roll-automation`, `-automation-params`): the focus axis's step
  params (a pinned source's steps, which `step` does not reach) and the
  lane's points: eseq-0l17.47 (built, §14.2r). View-local (built by the
  port, .16: `piano-roll-view`): the piano roll's arrangement mode
  (`SEQV.piano-roll-arrangement-mode`), scroll, zoom, cursor and marquee.

### 14.2k Built in stage 7g (eseq-0l17.33)

| Kind | Key | New `:host` fields (`:set` in brackets) |
|---|---|---|
| `graph` | `(index)` | `index :int`, `gid :int` (the sequencer id: a created instance's id), `name :string`, `owner group` (nil: the project), `variable :bool`, `min-nodes`, `max-nodes :int`, `node-count :int` [g], `reset-bars :number` [g], `max-poly :int` [g], `max-poly-selection :string` [g], `group-trace-decay` (0–1), `group-coupling-scale` (0–2), `group-excite-floor :number` (0–1) [g], `group-gain`, `group-coupling (list-of :number)` (4×4 row-major: cell `(+ (* row 4) col)`), `nodes (list-of graph-node)`; live: `active :bool`, `beat :number`, `energy`, `triggers (list-of :number)`, `dampening (list-of (list-of :number))` (all L) |
| `graph-node` | `(graph index)` | `graph graph`, `index :int`, `resolution :string` [g], `resolution-cycle (list-of :string)` [g], `quantize :string` [g], `quantize-cycle (list-of :string)` [g], `delay :int` [g], `route track` [g] (nil: off or gating a generator), `generator :int` (-1: none), `restart :bool`, `seed-route :bool` [g], `seeds (list-of track)` [g], `seed-on-reset :number` [g], `group :int` (0–3) [g], `params (list-of graph-param)`, `edges (list-of graph-edge)` (both lazy; then Model), `sounding (list-of (list-of :number))` (L, `(note velocity)` rows) |
| `graph-edge` | `(graph-node index)` | `from`, `to graph-node`, `params (list-of graph-param)` (lazy) |
| `graph-param` | `((graph-node graph-edge) pname)` (by name) | `node graph-node`, `edge graph-edge` (one is nil), `index :int`, `name`, `type :string` (`float`, `int`), `min`, `max`, `default :number`, `value :number` [g] |
| `track` | `(index)` | `active-notes (list-of (list-of :number))` (L, `(note velocity trigger-id)` rows) |
| `project` | `()` | `graphs (list-of graph)`; `(graphs)` |

[g] = the `set-graph` host command (`:graph-id`, `:field`, `:value`; `:node`
for a node's field, with `:param` for its param or `:to` and `:param` for
an edge's; `:row` / `:col` for a group cell; `:track-id` for a route;
`:param` and `:count` for `node-params`, one param of the first nodes at
once (eseq-0l17.67); `host_commands/graphs.rs`). Actions: `(set-group-gain!
g row col v)`, `(set-group-coupling! g row col v)`, `(gate-generator! n id
:restart r)` (`&key restart`), `(set-graph-params! g count name v)` (since
.67: nodes 0 to count - 1, up to `g.max-nodes`, one entry). Helpers: `(graph-of x)` (a created instance or a
sequencer id), `(graph-param-named x name)` (a node's or an edge's),
`(graph-edge-to n m)`. Constants: `graph-timebase-options`
(`Timebase::LABELS`), `graph-quantize-options` (`(cons "off" graph-timebase-options)`),
`graph-max-poly-selection-options` (`NeuralMaxPolySelection::ALL`).

Built (7g):

- **Graphs and created instances.** A created kind such as `neural` stays
  a created kind (D5: no `:host`); the host publishes every graph-mode
  sequencer it runs (an instance's or a script's `def-sequencer`) as a
  `graph`, and an instance's graph carries the instance's id as `gid`
  (the published sequencer id IS the instance id), so a view reaches its
  graph with `(graph-of self)`. View state (expanded node, selected
  neuron, map arming, piano depth) stays the instance's `:state`;
  document state (the overrides) is the graph kinds'. The legacy
  `SEQ.graph-sequencers` (the mixer's attach menu) is `project.graphs`
  with `g.owner`.
- **Identity.** Graphs are positional, kept by sequencer id
  (`registry::reconcile`), replaced on a project load (instance ids
  restart). Creating or deleting an instance publishes or unpublishes its
  sequencer, which registers or drops its graph (a held handle goes
  stale; undoing the delete brings a fresh one). Nodes are keyed (graph
  instance id, node index), as the model numbers them: a node-count change
  drops or adds the last nodes (a dropped node's edges, the edges into it
  and their params go with it). Edges are keyed (source node instance id,
  target index), params (node or edge instance id, name: a stable key per
  name), so a re-evaluated prototype that reorders its `:params` keeps
  each held handle on its param (its `index` moves) and one that renames
  or drops a param drops that instance (the handle goes stale, never
  retargets another param); both register on the first read of
  `n.params`, `n.edges` or `e.params` (the reader hook), like a device's
  params, and are then kept current with their graph.
- **Feeds.** The model fields sync behind one key compared without
  allocating (the published sequencer version, the scenes revision, which
  every override edit moves, the current scene, the groups' generation
  and the track and group instances); the manifests are re-read only when
  the published version moved (and compared only then), the current
  scene's overrides once per sync (each graph's moved out, not copied),
  and only a graph whose manifest, overrides or rack members changed is
  re-derived (`GraphShared::derives`; a step edit or a UI epoch re-derives
  none, nothing reads the history revision, and a sync that re-derived
  nothing and kept every graph instance pushes no node list or
  `project.graphs`). Values derive
  through the legacy reads' helpers, now shared: `graph_node_intrinsic_value`
  (`graph-node-value`), `graph_node_param_value` (`graph-param-value`),
  `graph_edge_param_value` (`graph-edge-value`), `graph_config_field_value`
  (`graph-config-value`), `graph_group_cells`, `graph_seed_follows_route`.
  A rack-owned graph's routes and seeds are member indices: the kinds show
  the member's track (`group.members`). Live, observed only and compared
  in place with the last push (an idle tick allocates nothing): playback
  from the scheduler's visualization snapshot, read in place
  (`SequencerState::with_graph_visualization`) with the legacy
  `SEQ.graph-visualizations` transforms (energy and triggers clamped,
  dampening by from row and to column; zeros before the scheduler ran the
  graph), a node's `sounding` (the legacy `graph-node-notes` read, at most
  eight, none while stopped; nodes in an `ObservedList`, read under one
  snapshot lock per graph) and `track.active-notes`
  (`active_note_activity_into` a scratch buffer, the legacy
  `SEQ.track-active-notes`); the live caches drop gone graphs and tracks. A piano-keyboard's `:notes-by-track` takes
  the rows (`(note velocity trigger-id)`, as the activity maps).
- **Setters.** `set-graph` resolves the graph by its sequencer id (matched
  as the number Lisp holds, so a legacy hashed id past 2^53 still
  resolves), a node among the active ones, an edge by its endpoints and a
  track by `TrackId` when it lands; a gone graph, a node past the active
  count, a missing edge, an unknown param or a track outside a rack-owned
  graph's rack is an error. Values follow §14.2c: labels among their
  options (case-insensitive; a cycle a non-empty list of them, `off` only
  alone), param values finite and in the param's range (an int param an
  integer), `delay` / `max-poly` integers of at least 0, `node-count` an
  integer in `min-nodes`–`max-nodes` (a fixed graph's is an error),
  `seed-on-reset` / `reset-bars` numbers of at least 0, the group cells and
  config numbers in their ranges, a generator a generator instance of the
  graph's owner (a tick-mode instance of its rack, or of the project),
  never the graph's own instance: no clamping (unlike the legacy
  natives). `seed-route` false stops seeding
  from the route; `seeds` sets explicit tracks (and stops following the
  route). Setters act only where the current scene's resolved value
  differs and write the override the legacy `graph-*` natives write,
  through `App::apply_graph_override_edit`: one undo entry
  (`EditPatch::GraphOverride`) per field, a `GraphOverrideSlot`
  (`runtime/graph/override_slot.rs`): one node intrinsic (`NodeField`; a
  node's process chain is not one), one node or edge param, one
  sequencer-level config field (`ConfigField`) or one group matrix cell
  (`GroupCell`), before and after, that undo and redo write back into the
  scene it was made in (`restore_graph_override`), leaving every other
  field alone, a legacy write to another field of the same node, the
  config or the same matrix included. Every edit stages a
  coalescing gesture keyed by its field, as a bar transpose does: a
  numeric field's `set!`s while the pointer is down join one entry
  (`ScriptEdit`), a drag over two fields records two, anything else is its
  own entry. The legacy `graph-param`, `graph-edge` and `graph-config`
  natives write through the same slots. Since eseq-0l17.53 the legacy
  `graph-node`, `graph-param`, `graph-edge` and `graph-config` writes are
  recorded too (the kinds still pick them up at the next sync): each
  native captures every field it sets, before and after, inside its
  `edit_current_graph_overrides` and queues
  `GRAPH_OVERRIDE_HISTORY_COMMAND` (`graph-override-history`: scene,
  sequencer id, the slot pairs as JSON; `GraphOverrideSlot` and its parts
  serialize for it), and the writes of one pass to one graph (until the
  host drains its commands, `NativeContext::with_last_queued_command`)
  extend that one command, so a batch (graph-kit's `set-param-on-nodes!`,
  `scale-delays!`, `shift-timebases!`, a script's init) is one entry. The
  host records it as a script edit through
  `App::record_graph_override_edits`: one field stages with
  `App::record_graph_override_edit` (the staging half of
  `apply_graph_override_edit`, merge key `graph:{id}:{address}`, so a
  native write joins a kind drag of the same field), several as one
  composite keyed by their addresses (a batch slider's drag joins one
  entry). `GraphNodeField` gained `Duration` and `Swing` (`graph-node
  :duration` / `:swing`; no kind setter). An instance's `:on-create`
  writes stay part of its creation: their history commands are dropped
  (`run_on_create`). A `graph-node` call naming no field no longer
  creates an empty node override entry.
  A kind setter does not echo into legacy `bind-graph` handles (the views
  move to `#'p.value`); tracked `graph-*-value` reads refresh through the
  tick's sweep as for any non-Lisp edit.
- **The legacy GRAPH namespace maps to fields.** `bind-graph` / `bind-graph-config`
  / `reactive-set "GRAPH"` handles become `#'` bindings of the fields
  (`#'(graph-param-named n "threshold").value`, `#'n.delay`,
  `#'g.reset-bars`), the enum fields labels (`n.resolution`, `n.quantize`,
  `g.max-poly-selection`) and the route a track instance (a dropdown's
  index is a view derivation); the route color strips
  (`gvr-route-color-field`, `ggm-route-color-field`) are a view derivation
  from `n.route.color`. `bind-graph-node-notes` is `n.sounding`.
- **Not covered:** the native neural engine's networks
  (`SEQ.neural-networks`, `neural-*-matrix`, the neuron selection;
  eseq-0l17.50, built: §14.2q); event streams (the graph's event history, node events,
  deltas and group traces; `SEQ.track-events`, `track-event-current-beat`;
  eseq-0l17.51, built: §14.2s); generator marks (alez.jaki, eseq-0l17.52,
  built: §14.2t); a node's
  `duration` / `swing` overrides (no content edits them).

### 14.2l Built in stage 7b-4 (eseq-0l17.43)

| Kind | Key | New `:host` fields (`:set` in brackets) |
|---|---|---|
| `device` | `((track bus) did)` | header: `display-name :string`, `sound-binding :string`, `meter :any`, `modulators (list-of modulator)`, `modulator-phase`, `modulator-level :number` (L); tables: `table-name`, `table-mode`, `table-engine`, `table-data-key`, `ir-name :string`, `table-options (list-of :string)` (all L); sampler media (model, computed while observed): `sample-buffer :any`, `sample-duration`, `start-time`, `end-time :number`, `slices`, `slice-active`, `onsets (list-of :number)`, `analysis-status`, `analysis-message :string`, `analysis-bpm`, `analysis-confidence`, `downbeat-time :number` |
| `modulator` | `(device index)` | `device device`, `index :int`, `slot :int`, `label :string` |
| `param` | `(device index)` | UI metadata: `group`, `env`, `role`, `display-name :string`, `asset-options :any` |
| `macro` | `(index)` | `target-scene` [m] (now settable), `morph-params` [m], `steal-patterns` [m], `quantize` [m], `tracks (list-of track)` [m], `diff-count :int` |
| `step` | `(track index)` | `variant variant` (L) |

[m] = `set-macro` (`:macro-id`, `:field`, `:value`; `target-scene` by the
scene's `index`, `tracks` as a list of `tid`s).

Built (7b-4):

- **Header.** `display-name` is what the panel header shows
  (`instrument_panel_display_name`, now shared with the instrument and rack
  panels: a drum rack's track name, Sampler for a sampler (whose panel
  carries none), else the instrument's name without its folder or pin; a
  rack slot's `instrument_display_name` of its raw name;
  any other device's `name`). `sound-binding` is a track instrument's bound
  sound (`App::sound_binding_label`, now the panels' too: the current
  palette entry's name, read from that one Patch rather than the palette's
  diff over every Patch, else the binding's label); empty when unbound and
  on any other device. It takes the scenes lock, so a track instrument's is
  computed only while observed (`HostKinds::sync_sound_bindings`, an
  `ObservedList`): recomputed at each model sync (the legacy panels' at
  each rebuild) or when it starts being observed; unobserved it keeps its
  last value ("" from its registration). `meter` is the `device-meter` selector the panel
  dicts carry (`device_meter_value`, shared with `device_meter_source`):
  `track`, `track-effect`, `rack-slot`, `rack-effect` or `bus-effect` by
  position (a bus effect by its bus id); nil for a MIDI effect. EQ8's
  spectrum source is the same selector. `display-name`, `meter` and any
  other device's empty `sound-binding` are model fields pushed with the
  device's other fields (`push_device`): the chain on the track model sync
  (its revision carries `sound_binding_epoch`), the other families on their
  passes.
- **Fixed modulators.** `d.modulators` are `modulator` instances keyed
  (device instance id, index), registered with the descriptor like the
  tensors (`media::sync_device_modulators`); the descriptor sameness now
  compares the modulators, the params' UI metadata and the name (another
  effect is another device: its table and IR fields follow the name), so a
  relabelled modulator or a metadata change replaces the instances.
- **Modulator envelope.** `modulator-phase` / `-level` read the meter
  cache (`cached_modulator_phases` / `levels`, now in `KindsMeters` and
  copied into `KindsShared` like the peaks) at the device's track: a track
  instrument's (0 unless a modulator), 0 for any other device. Live; an
  observed modulator track's instrument keeps the cache polled with the fx
  panel hidden (`HostKinds::wants_modulator_meters`, the meter cadence,
  and again whenever the cache's length is not the graph's), as the mod
  display does.
- **Param UI metadata.** `group`, `env`, `role`, `display-name` (empty
  when the descriptor has none) and `asset-options` (an unresolved options
  reference as the legacy maps' `:options` map, `param_asset_options_value`
  shared with `insert_param_ui_metadata`; nil for an enum param, whose
  labels resolved into `options`) are pushed at registration.
- **Tables.** A Filter Table's `table-name` (No table), `table-mode`,
  `table-engine`, `table-data-key` (empty unless a table is prepared) and a
  Convolution Reverb's `ir-name` (No IR) read the effect node's registries
  (`EffectTableFields::of`, shared with the legacy track, bus and rack
  effect panels; by `effect_node`): live, per tick while observed, no
  `App`, the registries read once per device. `table-options` (the table
  asset stems; the listing stats the asset folders) is listed and built
  into its list at most once per (UI epoch, FX epoch, content library
  epoch), the legacy panel's rebuild gate (`KindsShared::table_options`),
  and pushed to an observer only when that key moved or it starts
  observing (no list per tick). Any other device reads them empty.
- **Sampler media.** A track sampler's or a sampler rack slot's. They need
  the `App` (the sample's path, the analysis cache), so they are model
  fields computed only while observed (`HostKinds::sync_sampler_media`,
  an `ObservedList` over the devices, like `project.instances`), by group,
  each with its key compared in place every tick (no allocation): the
  sample (path, buffer: `sample-buffer`, `sample-duration`), its analysis
  (the analysis cache's entry and onset table for the buffer, looked up
  only when the cache's generation moved, and the graph's sample rate,
  which the onsets' and downbeat's seconds are in: `onsets`, `analysis-*`,
  `downbeat-time`) and the slices (those, the slice mode and sensitivity
  at the displayed step, the slot's slice edits as stored; the ones for
  another sample are filtered out when computing, `edits_for_sample_path`:
  `slices`, `slice-active`). Only a group whose key moved, or a field that
  starts being observed, is recomputed, through the legacy panels' helpers
  (`sampler_waveform_sample`, `sampler_slices`, `SamplerAnalysis`,
  `sampler_selection`, `rack_slot_sample_path`, now shared), and a list is
  pushed only when it differs from the cell; `start-time` and `end-time`
  (start and end at the displayed step × the duration) are compared every
  tick, so a start drag recomputes nothing else. A rack slot's are read
  under the rack lock; the sample loads after it is released. `slice-active` keeps the waveform's numbers (1 / 0,
  as `sampler_slices` returns them). Every device is pushed the no-sampler
  defaults when it registers (nil, 1, 0, empty lists, `none`, "", -1), so
  a by-value read of one never observed reads them; an unobserved media
  field keeps its last pushed value. The legacy panel's analysis publish
  (`publish_sampler_analysis_runtime`, a side effect) stays with the
  panel.
- **Scene macro config.** `set-macro` takes a scene macro's
  `target-scene` (an integer scene position), `morph-params`,
  `steal-patterns` (bools), `quantize` (`off`, `sixteenth`, `bar`) and
  `tracks` (a list of track ids; the tracks it acts on), each one
  `MacroSceneConfig` through history (one undo entry); the value the field
  reads is a no-op (`tracks` compares the tracks it acts on, so reading
  every track of an unmasked macro and setting them back changes nothing);
  a mapped macro's is an error. `tracks` reads every track while the mask
  names none (`SceneMacroConfig::covers_track`, shared with
  `scene_macro_mappings`). `quantize` parses through
  `StealQuantize::LABELS` (shared with the legacy macros' reader and
  parser). `diff-count` (`App::scene_macro_diff_count`, the legacy's) walks
  every track's scene target, so it is computed only while observed (an
  unobserved one costs a query and reads its last value, nil before) and
  recomputed only when the UI epoch, the history or the scenes moved
  (`MacroState::diff_key`) or it starts being observed.
  `bus_pattern_snapshot_or_default` no longer marks the scenes written when
  it fills nothing: the diff count's read moved the scenes revision (and so
  every model gate reading it) each time it ran.
- **`step.variant`.** The step variant (a `t.variants` instance) the step
  plays, nil for none: the p-lock render now carries the variant's `vid`
  (`PlockVariantStepRender::vid`, from the registry's assignment), so the
  step diff compares it with the render it already makes when the track's
  `PlockKey` moved; before pushing it reconciles the track's variant
  instances (`owner_variants`, skipped when they already match that key),
  so a variant gone since never leaves a stale instance behind.
- **Not covered:** the built-in effect editors' host state, the Filter
  Table response editor session (`filter_table_editor_value`, the fx
  dict's `:editor`; eseq-0l17.56, built: §14.2p). EQ8's editor reads no
  host state but its params and `device.meter`.

### 14.2p Built in stage 7b-5 (eseq-0l17.56)

| Kind | Key | New `:host` fields (`:set` in brackets) |
|---|---|---|
| `table-editor` | `()` | all (L): `device device` (nil while closed), `open :bool`, `frames :int`, `selected-frame :int` [`filter-table-editor-frame`], `selected-frame-normalized :number`, `can-undo`, `can-redo`, `dirty :bool`, `op-count :int`, `band-kind :string` (empty: no band), `band-freq`, `band-gain`, `band-q :number` (0: no band) |

Actions (the legacy `filter-table-editor-*` host commands, their payload
shapes): `(table-editor-open! d)`, `(table-editor-close!)`,
`(table-editor-band! kind freq gain q &key (phase "change"))`,
`(table-editor-op! kind &rest options)` (`:frame-start`, `:frame-end`,
`:value`, `:radius`, `:to`, `:start`, `:end`, `:frame`, `:points-json`),
`(table-editor-add-node! kind)`, `(table-editor-frame! frame)`,
`(table-editor-undo!)`, `(table-editor-redo!)`,
`(table-editor-save! &key (name nil))`.

Built (7b-5):

- **A singleton, not device fields.** The session is one global
  (`filter_table_editor`), outside the `App`, bound to one effect node, so
  it is the `:key ()` `table-editor` kind with its own observed mask (the
  device kind's live mask is full) rather than `d.editor-*` fields nil on
  every other device. `device` is the device instance whose effect node
  the session edits (`editor_device`: a registered Filter Table whose
  `effect_node` is the session's node); nil while closed, or while no
  registered device names the node. `open` is whether a session exists
  (the legacy map is present only on its device's panel: `te.device`
  says which). While closed every field reads nil, false, 0 or empty.
- **The band is flat.** The legacy `:band` map becomes `band-kind`,
  `band-freq`, `band-gain`, `band-q` (the newest edit when it is a
  parametric node; `band-kind` empty and the numbers 0 otherwise), on the
  response-curve-editor's axes through `ParametricNode::curve_band` (freq
  the harmonic-bin position `2^center_oct * 24`, 1–1024; q `2 / width`,
  0.25–16), which the legacy `filter_table_editor_value` now shares, as it
  shares `SessionUiState::selected_frame_normalized`.
- **Feeds.** Every field is live. The session now has a revision
  (`filter_table_editor::session_revision`), moved while the lock is held
  by every access that can change it (a `with_session` of a live session,
  `set_session`, a `take_session_for_node` that took one); read-only
  accesses (`session_ui_state`, `read_session`: a band drag's preview, the
  save's snapshot) move nothing, so a drag's previews read nothing (they
  change nothing the kind shows). While a field is observed the tick
  (`HostKinds::sync_table_editor`) compares (revision, the device sources'
  generation `KindsShared::devices_generation`, the observed mask) with
  its last read, and while a session is open checks the device binding:
  the resolved device still names the session's node (one node read; a
  bus effect's node comes from the bus mirror, which lags an engine swap
  the session was reattached across), or, while none does, whether one
  does now (a walk over the Filter Table devices). Only when one of those
  moved does it read the session once, resolve the device and push the
  observed fields (each compared with its cell). Unobserved, it reads
  nothing; a cold read goes through the reader hook (`device`'s the only
  one that walks the device sources).
- **Actions and the setter.** Each action is the legacy host command with
  its payload, so it applies, previews, coalesces (a band drag's commits
  replace the band they started from) and refreshes the panels exactly as
  the legacy view's buttons do; the editor's history is its own, and only
  a save is a project undo entry (the recorded table load). Errors (no
  session open, an unknown node kind or op) reach the status line.
  `table-editor-open!` sends the device's `device-target`:
  `filter-table-editor-open` now also takes `:track-id` / `:bus-id` and
  `:device`, resolved when it lands (`devices::addressed`); only a
  track's or a bus's Filter Table has an editor (another device is an
  error). `(set! te.selected-frame n)` (and `table-editor-frame!`, the
  same setter) is `filter-table-editor-frame` under the value rule: a
  whole number below `frames` (a fraction or a negative is an error,
  never truncated; out of range too; the current frame is a no-op).
  `App::filter_table_save_dir` redirects saves (tests: scratch space; the
  user library otherwise).
- **Not ported:** the view (`content/ui/effects/builtin/filter-table.lisp`
  still reads the fx dict's `:editor`); eseq-0l17.14 ports it.

### 14.2m Built in stage 7g-2 (eseq-0l17.49)

| Kind | Key | New `:host` fields (`:set` in brackets) |
|---|---|---|
| `process` | `((track graph-node) proc-id)` (was `(track proc-id)`) | `node graph-node` (nil for a track's; `track` is nil for a node's), `known :bool` (its class is loaded), `expr-source :string` (an expr card's body; empty otherwise), `promoted-expr :bool`, `as-expr-reason :string` (empty for an expr card or a promoted one) |
| `graph-node` | `(graph index)` | `processes (list-of process)` (lazy; then Model) |

The part kinds (`inlet`, `port`, `fanout`, `state-cell`; `lane`, none on a
node) are unchanged: a node's process carries them as a track's does. The
setters and actions of §14.2h take a node's processes too:
`process.enabled`, `inlet.value`, `fanout.lo` / `hi`, `move-process!`,
`(add-process! n c)` (`n` a graph node), `remove-process!`, `bind-port!`,
`add-fanout!`, `unbind-port!`, `clear-port!`, `remove-fanout!`; their
`edit-process` address is the process's owner (`process-owner`: `:track-id`,
or `:graph-id` and `:node`) and its `:proc-id`, a port target's and a
move's `before` the target process's address (`process-target`; `before`
was `:before` / `:before-track-id`).

Built (7g-2):

- **A node's patch is processes.** `n.processes` is the node's chain as its
  graph's runtime config resolves it (`GraphNode::process_chain`: the
  current scene's override), in run order. A node process derives through
  the track processes' code (`host_kinds/lanes.rs`, over a
  `ProcessOwner`): `name` is its instance name, else the class's node label
  (`graph_node_process_label`, the legacy slot's `:label`); a node has no
  layers and no lanes, so `p.lanes` is empty and `p.inlets` lists every
  numeric inlet, the class's lane inlets included (`process_inlet_names`,
  the legacy `:inlet-defs`); `in-ports`, ports, fan-out and their wiring are
  the track derivations over the node's chain (the legacy
  `graph-node-lane-patch`'s in ports and readers); `promoted-expr` and
  `as-expr-reason` are the legacy slot's (`process_slot_as_expr`, shared
  with `graph-node-process-chain`), computed for a track's processes too.
  The node bay's cable ids are view derivations (as the track patchbay's):
  `graph-node-patch-namespace` stays a native.
- **Identity.** A node's process is keyed (node instance id, slot id): a
  reorder keeps it, a removed slot's goes stale, and the node's go with
  their node (a node-count change, the graph's instance deleted, a project
  load). The slot ids are the node band's (`1 << 45` up), never a track's.
- **Feeds.** Registered on the first read of `n.processes` (the reader
  hook, `cold_graph_parts`), then synced with their graph: every override
  edit (a chain edit included) re-derives its graph, and the node's
  processes are re-derived only when its chain or the library (version,
  class instances) differs from the last sync (`sync_node_lanes`, the
  record in `LaneShared::nodes`); the lane tick re-syncs registered nodes,
  from the chain they last synced, only when the library (its version and
  the class instances' generation) moved since it last checked (an idle
  tick allocates nothing). A chain about to sync whose expr bodies are not
  compiled yet compiles them first, as the legacy read does
  (`ensure_graph_node_expr_classes`; the library is read after). A node the
  graph sync drops (a node-count change, the graph gone) is forgotten in
  that same sync (`LaneShared::retain_nodes`). The live loop's id lists
  (`process.error`, `state-cell.values`) include a node's processes, whose
  runtime id is the slot's own id (the node runner's), so `error` and the
  scopes read through the same machinery (§14.2n). The live loop re-reads (its lists rebuilt, its
  last pushes forgotten) when a track's or a node's instances changed or
  any process's or state cell's runtime record did, with the same
  instances: a project lane's runtime id follows its track's position, a
  named slot's its name.
- **Setters.** `edit-process` resolves a node address as `set-graph` does
  (the graph by sequencer id, an active node by index, `host_commands::graphs::Graph`)
  and the process by its `:proc-id` in the node's current chain; a gone
  graph, node or process is an error. A node's process edits its own slot
  (`process_edit::apply_to_node_chain`, the legacy natives' semantics: no
  project fork, removing a slot drops the wires and fan-out into it,
  `TrackProcessChain::remove_slot_and_wires`), in the current scene's
  overrides through `lisp_host::edit_graph_node_process_chain_now` (shared
  with the natives; a minted node slot id is claimed there), recorded as
  the natives' history records it (`App::record_applied_graph_node_process_edit`,
  `EditPatch::GraphNodeProcessChain`: the node's chain before and after,
  undo and redo restore it into the scene it was made in). Each edit is one
  entry; an inlet's `set!`s while the pointer is down join one, under the
  natives' picker key (`graph_node_process_inlet_merge_key`), so a kinds
  drag and a legacy picker drag never split each other. Values follow
  §14.2h's rule (an inlet a number of its type in its declared range, a
  gate 0 or 1 or a bool; the current value is no edit). A node's port takes
  another process of its node (wires stay on their node: a target or a
  `before` on another node or track is an error) or a fire payload field
  (`transpose`, `velocity`, `duration`, `delay`:
  `GRAPH_NODE_PAYLOAD_FIELDS`, shared with `graph-node-process-map`: by
  name, case-insensitively, or by any step param spelling of one; `delay`
  is no step param, stored as the payload field itself); a param or send
  target, `:all` and lane steps are errors. An add's slot id is minted when it lands (the node band's next,
  inside the override edit; a track's next roster id), and it takes the
  classes `graph-node-process-add` takes (the library's and the default
  lane classes). Legacy `graph-node-process-*` edits reach the kinds at the
  next sync; a kind setter reaches the legacy tracked reads through the
  tick's graph read sweep.
- **Not covered:** the expr card actions (commit a body, promote to My
  processes, edit as expr, rebind a class: `graph-node-process-expr-set`,
  `-promote`, `-edit-as-expr`, `-rebind-class` and the promote check) stay
  natives addressed by (graph, node, `proc-id`): they return a result the
  view shows at once, which a host command cannot. A track's expr cards
  have no kind action either. The node picker's class list (node labels,
  hidden classes: `graph-node-process-classes`) is a view derivation from
  `process-library.classes` and `GRAPH_NODE_HIDDEN_PROCESS_CLASSES` (not
  published; since eseq-0l17.67 published as `process-class.node-label` /
  `node-hidden`).

### 14.2n Built in stage 7c-2 (eseq-0l17.45)

No new fields: a graph node's process slots are `process` instances
(§14.2m), and their run errors and scopes are the track processes' live
fields, `process.error` and `state-cell.values` (§14.2h).

- **Runtime ids.** A node slot runs under its own id (`proc-id`, the node
  runner's runtime id; never a track's position-mixed id), so
  `process.error` is `SequencerState::process_run_error` of it and a
  state cell's `values` its scope history under that id
  (`with_process_scope_cell`), the legacy `SEQ.process-run-errors` /
  `SEQ.process-scope-cells` entries whose `:runtime-id` is the slot's
  `:instance-id`.
- **Feeds.** As a track's: observed only (`ObservedList`s over every
  registered track's and node's processes and cells), re-read when the
  scheduler's run error or scope version, the observer epoch or the
  observed set moved; a cold read asks the host. A removed slot or a
  dropped node takes its process and cells with it (stale handles), and
  the loop reads nothing more for them.
- **The node bay's readers map to fields** (the views port in .11 / .20):
  `lane-patch-run-error` (a runtime id's error) is `p.error`;
  `lane-patch-expr-error` is `p.compile-error`, else the edit buffer's
  failed commit (`eseq.expr-buffer/commit-error`: view state, not host
  state), else `p.error`; `process-scope-cells-for` (a slot's cells by
  name, each a history) is `p.cells` (`(part p.cells "snap").values`,
  the newest last; an empty history reads as before the first fire, the
  legacy nil entry: "waiting for a fire").

### 14.2r Built in stage 7e-2 (eseq-0l17.47)

| Kind | Key | New `:host` fields (`:set` in brackets) |
|---|---|---|
| `piano-roll` | `()` | `steps (list-of focus-step)` (lazy; then Model) |
| `project` | `()` | `focus-step-params (list-of :any)` (one dict per step param: `:name` (its field), `:label`, `:min`, `:max`, `:default`, `:increment`) |
| `focus-step` | `(track index)` | `track track` (the piano roll's), `index :int` (on the source's axis), `active :bool` (holds a note), `start :number` (its earliest note's onset; `index` without notes), `end :number` (where its last note ends; `start` without notes), `duration`, `velocity`, `delay`, `aux-a`, `transpose`, `pan`, `sync`, `retrig`, `retrig-rate :number` [f] |

[f] = the `set-focus-step` host command (`:track-id`, `:index`, `:field`,
`:value`; `host_commands/focus_steps.rs`); `(set-focus-step! fs name v)` is
the same by name. `(focus-step-params)` is `project.focus-step-params`, the
step params in the legacy lane picker's order (`StepParam::VISIBLE`),
which the host builds from `FOCUS_STEP_PARAMS` (each visible param's
`focus-step` field in `PUBLISHED`, by its `step_param_named` name) and
pushes once, with the other fixed option lists. `(focus-step-value fs
name)` reads a param by name from a table of readers (one of the `:name`s;
any other is an error).

Built (7e-2):

- **The lane is a view.** Legacy `SEQ.piano-roll-automation-params` is
  `project.focus-step-params` (the step params) plus the track's device
  params and rack macros with `has-locks` (`t.devices`' and
  `t.midi-devices`' `params`, a rack's `device.macros`), labelled by the
  view (`p.name`, `rm.name`; the group a device's `name`). Legacy
  `SEQ.piano-roll-automation` is derived per selected parameter: a step
  param's points are the active focus steps (`start`, `end`, the param's
  value, locked); a device param's (live focus only, as the legacy lane:
  device locks are the live pattern's) are each focus step holding a lock
  in `p.step-locks` (its lock, locked) and each other active one (`p.base`,
  unlocked: the legacy `held_plock_value` walk stops at an active step at
  once, so an active step without a lock always showed the base). The
  scale is the param's (`min`, `max`, `default`; `focus-step-params`' for a
  step param). The lane's selected parameter is view state (built by the
  port, .16: `lane-view`, replacing the pinned
  `eseq.vanilla/piano-roll-automation-param`; the
  `piano-roll-automation-refresh` command stayed for the tracker until its
  port, .65, removed it).
  `piano_roll_step_span` (a step's span over its notes) was shared by the
  legacy lane and `focus-step.start` / `end`; the tests held the derivation
  to the legacy lane's points for step and effect params (since .16 they
  assert the values and the view's lane).
- **Identity.** Positional (D2): keyed (track instance id, index) under the
  piano roll's track, registered for that track alone (another track's
  are dropped when the piano roll moves to it) and dropped past the
  source's length. Another source of the same track (a clip pinned, a
  scene launched) keeps the instances; only the values move, as `step`
  under a scene switch. (A singleton cannot be a keyed parent, so the
  steps hang off the track, beside its `step`s.)
- **Feeds.** Model fields, registered on the first read of
  `piano-roll.steps` (the reader hook, or the tick once observed), like the
  notes, then re-read with the notes' triggers: one `ContentKey` (the
  source, the scenes and pool content revisions, the pattern epoch, the
  live length, a live focus's track republished, `App::history_replays`,
  `App::focus_step_edits`), computed once per tick while either is
  registered. `App::focus_step_edits` moves with every
  `FocusStepGesture` commit or rollback and every script note or focus
  step drag frame: a pinned source's pool writes move no other counter, so
  without it a note setter's edit, a focus step setter's or an Esc would
  leave the other kind (or both) stale; it replaces the notes' and focus
  steps' dirty flags. When either is due the tick reads the source once
  (`source_rows`: `PianoRollLanes::step_rows_batch`, the notes' batch
  read, now with every step param) and both sync from that read, each
  field compared with its cell; an idle tick loads a few counters
  (`FocusStepShared::syncs` counts).
- **Setters.** `set-focus-step` resolves the track by `TrackId` and the step
  by its index on the track's edit focus when it lands (positional, as
  `step`'s setters: a source change in flight lands on the new source).
  The value rule (§14.2c): a finite number in the param's range
  (`StepParam::min` to `max`, no clamping); the current value always
  works and changes nothing; an index past the source's length or a field
  a focus step does not set is an error. It goes through the legacy lane's
  path (`app::edit::apply_recorded_focus_step_mutation`, one undo entry,
  undo restores; a transpose or duration moves the step's chord with it,
  `PianoRollLanes::set_step_param`) and lands like the piano roll's edits
  (`piano_roll_edit_landed`). A pinned pattern's or take's step writes the
  pool, never the live pattern. A transpose (or a lone note's delay) moves
  the step's notes, whose keys (step, transpose, offset) change: their
  note ids move with them (`NoteShared::move_notes`, by voice), so note
  handles follow, as through the note setters.
- **Script drags.** While the pointer is down (no user gesture active,
  `ScriptEdit::drags`) every focus step `set!` of one focus (any field, any
  step) joins ONE undo entry: `app::edit::focus_step_param_drag` opens a
  `FocusStepGesture` under `PendingDrag::FocusSteps` (the note drag's
  `FocusStepDrag`, now shared: `PendingDrag::Note` holds one too), captures
  each step before its first write and writes in place (a live focus
  publishes its track per write), as the legacy
  `update-automation-step-param` gesture; the gesture commits on release
  (`finish_active_gesture`'s hook). A source moved under the drag (a scene
  launched in follow mode) commits the open drag and opens another; Esc
  rolls it back (`cancel_active_gesture`, "Parameter edit canceled"; the
  notes it moved then get fresh handles, as after an undo). The drag's
  entry is labelled as a one-shot edit's ("Edit piano-roll automation");
  a drag that cannot be recorded says "Step drag could not be recorded"
  (a note drag "Note drag …").
- **Not covered:** writing a device param's or rack macro's lane points.
  A device param's are `lock-param!` / `unlock-param!` on the live steps
  (one undo entry per call; a drag's locks of one step join one since
  eseq-0l17.58, §13.1 .16 notes), a rack macro's `lock-rack-macro!` /
  `unlock-rack-macro!` (built by eseq-0l17.57, §14.2g; the factory lane
  uses them since its port, .16).

### 14.2o Built in stage 7d-2 (eseq-0l17.39)

| Kind | Key | New `:host` fields |
|---|---|---|
| `song` | `()` | `pending :bool` (a capture take exists), `pending-origin :number` (the capture's start beat), `pending-head :number` (the record head, floored to a 16th), `pending-lanes (list-of pending-lane)`, `pending-scenes (list-of pending-scene)`, `pending-launches (list-of pending-launch)` |
| `pending-lane` | `(index)` | `index :int`, `track track`, `start :number` (punch-in), `end :number` (the growing edge), `num-steps :int`, `length :number` (beats), `events (list-of (list-of :number))` (as `clip.events`) |
| `pending-scene` | `(index)` | `index :int`, `scene scene`, `start :number` |
| `pending-launch` | `(index)` | `index :int`, `track track`, `start :number`, `cell cell` (the pattern it plays), `num-steps :int`, `length :number` (one cycle), `events (list-of (list-of :number))` |

All read-only (provisional content is inert until the stop-commit,
docs/realtime-arrangement-feedback-spec.md 7).

Built (7d-2):

- **Shared with the legacy surface.** The content is the legacy
  publisher's: `pending_capture_content` (over `build_pending_content`),
  the head `quantized_pending_head` (floored to `PENDING_HEAD_QUANTUM`),
  a lane's end `PendingLaneSpan::end_beat` (the growing edge, floored to
  the lane's step, never short of the music it holds; the span is all a
  lane keeps between rebuilds, not its events) and the events
  `pattern_events_value` (as `clip.events`), each now one function both
  `sync_song_pending` and the host kinds call. Legacy `origin-beat`,
  `head-beat`, `lanes` (`track`, `start-beat`, `end-beat`, `num-steps`,
  `length-beats`, `events`), `scene-events` (`start-beat`, `scene`) and
  `track-events` (`track`, `start-beat`, `pattern-id`, `num-steps`,
  `length-beats`, `events`) map to the fields above; positions become
  instances (`track`, `scene`, and a launch's pattern id its `cell`, the
  7d cell of that track and pattern; nil if unregistered).
- **Identity.** Positional (`(index)`, like `scene-span`): a new note or
  launch re-pushes values in place (a held `pending-lane` handle is "the
  i-th lane", not a lane), a shorter list drops the tail, and the capture
  ending (stop, cancel or a failed commit, which all drop the capture take)
  drops every instance explicitly and pushes `song.pending` false,
  `pending-origin` / `pending-head` 0 and the lists empty. Positional
  instances (`pending-*`, `scene-span`, the presentation rows) share
  `registry::positional` (`reconcile` over `0..count`).
- **Feeds** (`host_kinds/pending.rs`). No capture take: one bool per tick
  (`quantized_pending_head` is `None`); the clear runs once, when the take
  goes. While one exists the content is rebuilt (`PendingState::syncs`)
  only when `pending_content_key` or the arrangement structure generation
  (the track, scene and cell instances the content names) moved.
  `pending_content_key` is `App::pending_revision` (a recorded note, a
  captured launch, and a capture take beginning, being discarded or
  finishing, so a capture started in the tick another ended never shows
  the old one's content) with the pool-content and project-scenes
  revisions (a launched pattern's steps, length or timebase; a scene's
  cell assignment, which the whole-song start and every scene launch
  expand through); the legacy `SEQ.song-pending` keys on the same
  function. `pending-head` and each lane's `end` are re-pushed only when
  the quantized head moves (`PendingState::head_pushes`; a sub-quantum
  advance pushes nothing, and neither rebuilds the content). Every push
  is compared with its cell; the stale check covers the first instance of
  each kind.
- **Ported (.15):** the view (`ui/arrangement.lisp`) reads these fields;
  the legacy `SEQ.song-pending` publisher is removed (the shared content
  functions stay, the host kinds' alone).
### 14.2q Built in stage 7g-3 (eseq-0l17.50)

| Kind | Key | New `:host` fields (`:set` in brackets) |
|---|---|---|
| `network` | `(index)` | `index :int`, `nid :int` (its id in the scene: the `neural-*` natives' network id), `name :string` [n], `enabled :bool` [n], `neuron-count :int` (1–16), `reset-bars :number` [n] (0.25 or more), `energy-decay :number` (0–1) [n], `max-poly :int` [n] (1 or more), `max-poly-selection :string` [n], `weights (list-of (list-of :number))` [n] (rows from-neuron, columns to-neuron, `neuron-count` square), `neurons (list-of neuron)`; live: `active :bool` (L) |
| `neuron` | `(network index)` | `network network`, `index :int`, `route track` [n] (nil: none), `resolution :string` [n], `delay :int` [n], `threshold :number` [n] (0 or more), `transpose :number` [n], `quantize :string` [n] (`off` or a timebase label), `dampening-amount`, `dampening-recovery :number` (0–1) [n]; live: `selected :bool` (L) [`neural-set-neuron-selected`], `energy`, `trigger :number`, `dampening (list-of :number)` (L) |
| `project` | `()` | `networks (list-of network)`; `(networks)` |

[n] = the `set-neural` host command (`:network-id`, `:field`, `:value`;
`:neuron` for a neuron's field, `:track-id` for its route, `:from` / `:to`
for one weight cell; `host_commands/neural.rs`). Actions:
`(set-neural-weight! nw from to v)` (neuron indices) and
`(set-neural-thresholds! nw v)` (every neuron's threshold, one edit:
field `"thresholds"`, added by .20). Labels share
`graph-timebase-options`, `graph-quantize-options` and
`graph-max-poly-selection-options`. `neuron` is a sub-kind, so it is not
exported (`network` is); the process DSL's `(neuron k :note)` native keeps
its global name.

Built (7g-3):

- **The native engine, not graph sequencers.** The networks the
  `neural-create` / `neural-set` / `neural-neuron` / `neural-weight(s)`
  natives author (`ProjectNeuralNetwork`, per scene) are `network`s of the
  current scene, their neurons `neuron`s; graph-mode sequencers (an
  `alez.neural` instance included) stay `graph` / `graph-node` (§14.2k).
- **Identity.** Networks are positional, kept by network id
  (`registry::reconcile`; a repeated id keeps its first network), replaced
  on a project load. `neural-create` mints ids that are never reused
  (`SequencerState::mint_neural_network_id`: above every id any scene holds
  and every id minted before in the session; the counter never goes back,
  not with an undo, a scene rebuild or a load), so deleting a network and
  creating another never hands its id, its held handles or its history
  entries to the new one. The ids are not persisted: on load they start
  above the loaded project's, and no handle or history entry survives a
  load. A scene switch shows the new scene's networks: an id both scenes
  hold (a cloned scene's copy) keeps its instance, now showing the new
  scene's network; any other goes stale. Neurons are keyed (network instance id, neuron index), registered
  and dropped with the neuron count, and go with their network (a
  `neural-delete`, a project load).
- **Feeds.** The model fields sync behind one key compared without
  allocating (the scenes revision, which every network edit moves, the
  current scene and the track instances, which a route names); when it
  moves the sync reads the current scene's networks once and pushes only a
  network that differs from its last push (a step edit pushes none). Values
  are the legacy `SEQ.neural-networks` map's (`neural_network_value`):
  clocks as labels (the legacy keywords; no quantize is `off`), the route a
  track instance (the legacy track index), the neuron map's `dampening` is
  `dampening-amount` (`dampening` is the live row), the weights the matrix
  sized `neuron-count` square (`ProjectNeuralNetwork::shaped_weights`, which
  the natives' shape normalization shares). Live, observed only and
  compared in place with the last push (an idle tick allocates nothing):
  `network.active` and a neuron's `energy`, `trigger` and `dampening` (its
  edges' row, by target neuron) from the engine's visualization snapshot,
  read once per tick with the legacy `SEQ.neural-*-matrix` transforms
  (`neural_energy_display_value`, `neural_trigger_display_value`,
  `neural_dampening_display_value`, sized by `neural_snapshot_size`, shared
  with the legacy builders); a network the engine does not run (it runs
  the current scene's first enabled one) reads zeros and `active` false. A
  neuron's `selected` is the step-editing selection
  (`SharedSelectedNeuralNeurons`, keyed (scene, network id, neuron), as the
  legacy `neural-neuron-selected-{pattern}-{network}-{neuron}` fields),
  locked once per tick only while one is observed. A cold read asks the
  host (the network's id and count from its cells).
- **Setters.** `set-neural` resolves the network by id among the current
  scene's networks, a neuron (or a cell's `from` / `to`) below the neuron
  count and a route's track by `TrackId` when it lands; a network the scene
  no longer holds is an error. Values follow §14.2c, and where the natives
  clamp (a threshold below 0, an energy decay or a dampening past 1, a
  max-poly below 1, reset bars below 0.25) the setters reject, so every
  value they write is one the natives keep; `weights` takes `neuron-count`
  lists of `neuron-count` finite numbers (the `neural-weights` parse,
  `parse_neural_weight_matrix`). Setters act only where the value
  differs and write the field the natives write, through
  `App::apply_neural_network_edit`: one undo entry
  (`EditPatch::NeuralNetwork`) per field, a `NeuralSlot`
  (`neural/slot.rs`: a network setting, one weight cell or the matrix, one
  neuron field), before and after, that undo and redo write back into the
  network in the scene it was made in (`edit_scene_neural_networks`),
  leaving every other field alone (an unrecorded legacy write included). A
  replay onto a network that is gone, or whose neuron count no longer
  covers the slot (a neuron or cell past it, a matrix of another size), is
  an error that changes nothing (`NeuralSlot::write`). The graph override
  edit and this one share the gesture staging (`stage_field_edit`: the
  scene, the staged entry's `before`, the write and the entry): a numeric field's `set!`s while the pointer is down
  join one entry (`ScriptEdit`), anything else is its own entry. Legacy
  `neural-*` edits stay unrecorded; the kinds pick them up at the next
  sync. An undo of a neural edit takes the full refresh (no targeted
  replay), so the legacy `SEQ.neural-networks` publisher follows it.
- **Selection.** `(set! nr.selected true)` selects the neuron alone (as
  `neural-select-neuron`), `false` deselects it, through the
  `neural-set-neuron-selected` native (UI-thread state: no history, applied
  at once; a gone network or a neuron past the count is a native error on
  the status line). `neural-select-neuron`, `neural-neuron-selected?` and
  this setter resolve the neuron the same way (`resolve_selected_neuron`;
  the predicate reads false for a gone network). The legacy natives and the kind field share the
  handle, so either sees the other's selection.
- **The router's legacy reads map to fields** (the view ports in .20):
  `SEQ.neural-networks` is `project.networks` (a network by name:
  `(first (filter (lambda (n) (= n.name …)) (networks)))`);
  `SEQ.neural-energy-matrix` / `-trigger-matrix` (column matrices) are
  `(map (lambda (nr) (list nr.energy)) nw.neurons)` and its triggers; the
  dampening matrix is `(map (lambda (nr) nr.dampening) nw.neurons)`; the
  row selection field (`neural-neuron-selected-…`, `bind-seq`) is
  `#'nr.selected`; the row controls' `neural-neuron` / `neural-set` /
  `neural-weights` calls are `set!`s of the fields.
- **Not covered:** creating, deleting and enabling-by-name stay the
  natives (`neural-create`, `neural-delete`: they return the network the
  view uses at once); a neuron's output p-locks (`neural-plock-*`; their
  display is `param.value` / `param.overridden`, §14.2g); a network's
  `seed-on-reset` (no content reads it).

### 14.2s Built in stage 7g-4 (eseq-0l17.51)

| Kind | Key | New `:host` fields (`:set` in brackets) |
|---|---|---|
| `graph` | `(index)` | live: `deltas (list-of (list-of :number))` (each edge's weight delta, by from row and to column), `node-deltas (list-of :number)` (each node's summed delay and param delta magnitudes), `group-activity`, `group-suppression (list-of :number)` (4 groups), `events (list-of (list-of :number))` (its fired events, oldest first, at most 1024), `node-events (list-of (list-of :number))` (each node's latest event while it shows, an empty row when none) (all L) |
| `transport` | `()` | live: `track-events (list-of (list-of :number))` (the tracks' output notes, oldest first, at most 1024), `track-events-beat :number` (the scheduler's rendered beat) (L) |

An event is a positional row `(node track beat transpose velocity)`
(`ROW_FIELDS` in `widget_render/event_view.rs`), -1 for no node (every
track output event) or no track (a graph event whose node routes none).

Built (7g-4):

- **Rows, not an event sub-kind.** The event-view widget takes a list of
  events and reads each by field name (`:x :transpose`, `:y :node`, `:z
  :beat-phase`, `:color-by :track`); it never needs an event's identity, a
  setter or a per-event binding. A sub-kind would register, key and drop an
  instance per emission (up to 1024 per history, each with its field cells)
  and reconcile the whole history on every fire, for handles nothing holds.
  Rows cost one list of numbers per event, built only when the history
  moved, and nothing while idle (below). The widget now reads a row as well
  as the legacy map (`EventItem`; a row's negative node or track reads as
  missing, as a map's nil does; a row has no `sample`, which no view plots),
  and no longer copies each event while filtering. The shape matches
  `track.active-notes` and `graph-node.sounding` (number rows the widget
  reads by position).
- **Feeds.** Live, observed only. Each stream carries a revision so an
  idle tick compares one number and copies nothing: a graph snapshot's
  `history_stamp` (`event_history`: an emission, a reset) and
  `node_events_stamp` (`node_events`: an emission, a node event expiring, a
  reset), each taken anew from a process-wide counter (unique across
  runtimes, so a rebuilt runtime never repeats an older one's stamp), and the
  track output history's revision
  (`SequencerState::track_output_events_revision`, moved under its lock by an
  append that added events or a clear that removed some). A stream is copied
  only while observed and when its own stamp moved (`node-events` also with
  the node count), so an expiring node event re-pushes `node-events` alone.
  A history push builds cells for its new rows only (`RowHistory`: the rows
  and cells of the last push are kept; a history drops from the front and
  grows at the end, so the longest old tail the new rows start with keeps its
  cells, and a reset keeps none): an emission costs its new rows, one vector
  of cell pointers and the store's compare-before-push (numbers, nothing
  allocated), not 1024 rebuilt rows. The graph's other new fields are read
  with its playback under the same snapshot lock and compared in place
  (`deltas` through `graph_delta_values`, shared with the legacy
  `delta-matrix` / `node-delta-column`); `track-events-beat` is a number
  compared as any live field. Before the scheduler runs a graph: no events,
  an empty row per node, zero deltas and group traces. A cold read asks the
  host (it builds its own cells).
- **Values.** As the legacy `SEQ.graph-visualizations` entry: `events` is
  its `event-history` (raw transpose and velocity), `node-events` its
  `node-events` with their display transforms (transpose to 0.01, velocity
  clamped 0–1: `graph_weight_display_value`, `neural_trigger_display_value`),
  `deltas` its `delta-matrix`, `node-deltas`, `group-activity` and
  `group-suppression` its column matrices flattened (like `energy`);
  `transport.track-events` is `SEQ.track-events`, `track-events-beat`
  `SEQ.track-event-current-beat`.
- **The legacy reads map to fields** (the views port in .20): `(get viz
  :event-history)` → `g.events`, `(get viz :current-beat)` → `g.beat` (or
  `#'g.beat`), `:node-events` → `g.node-events`, `:events` (the nodes'
  latest, flattened) → `(filter (lambda (r) (> (len r) 0)) g.node-events)`,
  `:delta-matrix` → `g.deltas`, `:node-delta-column` → `(map (lambda (d)
  (list d)) g.node-deltas)`, the `:group-*-matrix` columns likewise;
  `SEQ.track-events` → `transport.track-events`,
  `SEQ.track-event-current-beat` → `transport.track-events-beat`. A list
  field is value-only (no `#'`): the event-view's `:events` reads the field
  in its `subtree` (as the views do now), its `:current-beat` may bind
  `#'g.beat` / `#'transport.track-events-beat`.
- **Not covered:** an event's audio sample time (the legacy maps' `sample`;
  no view plots it); the legacy `weight-matrix`, `delay-matrix`, `edges`
  and `delta-leak-per-beat` (the weights are `graph-param.value`, §14.2k;
  nothing reads the rest).

### 14.2t Built in stage 7g-5 (eseq-0l17.52)

| Kind | Key | New `:host` fields (`:set` in brackets) |
|---|---|---|
| `generator` | `(index)` | `index :int`, `gid :int` (the sequencer id: a created instance's id), `name :string`, `owner group` (nil: the project), `marks (list-of generator-mark)` (sorted by name) |
| `generator-mark` | `(generator mname)` (by name) | `generator generator`, `name :string` (its `gen-mark` key; `""` for an unkeyed `(gen-mark v)`); live: `value :number` (L) |
| `project` | `()` | `generators (list-of generator)`; `(generators)` |

Helpers: `(generator-of x)` (a created instance or a sequencer id, like
`graph-of`), `(generator-mark-named g key)` (nil before the key's first
stamp), `(generator-mark-of x key)` (the two at once, nil without a
generator; added by .20). Nothing is settable: marks are what a tick stamps. A mark's key
field is `name` (`key` is every keyed instance's builtin field; declaring
it is a compile error since eseq-0l17.60, §3.1).
`generator-mark` is a sub-kind, so it is not exported (`generator` is).

Built (7g-5):

- **Generators and created instances.** A created kind such as `jaki`
  stays a created kind (D5: no `:host`); the host publishes every
  tick-mode sequencer it runs (an instance's `:generator`, or a script's
  `def-sequencer` with a `:tick`; a graph-mode one is a `graph`, §14.2k)
  as a `generator`, whose `gid` is the published sequencer id (an
  instance's own id), so a view reaches its marks with `(generator-of
  self)`. View state stays the instance's `:state`.
- **Identity.** Generators are positional, kept by sequencer id
  (`registry::reconcile`), replaced on a project load. Creating or deleting
  an instance publishes or unpublishes its sequencer, which registers or
  drops its generator and with it its marks (held handles go stale). Marks
  are keyed (generator instance id, a stable sub-key per mark key string,
  never reused for another string until a project load replaces every
  generator and resets them), registered when their key gets its first
  stamp (the legacy field likewise appeared then) and dropped with the
  generator's marks: unpublishing a sequencer (by id or by name) drops its
  marks (`SequencerState::drop_generator_marks`, new), so a script
  generator removed and defined again starts with none, and a clear
  (`clear_generator_marks`: a project load, an instance replacement) drops
  them all.
- **Feeds.** The model fields sync behind one key compared without
  allocating: the published sequencer version, the mark keys' revision
  (`SequencerState::generator_mark_keys_revision`, new: moved under the
  marks lock by the first stamp under a new (generator, key) and by a clear
  that removed some) and the group instances (an owner). The published
  generators are re-read only when the published version moved, the mark
  keys only when their revision did, so a stamp under a known key (every
  hit) costs no model sync. A mark's `value` is live, observed only: the
  observed marks (an `ObservedList`) are read under one lock per tick
  (`SequencerState::with_shown_generator_marks`, new, which the legacy
  `sync_generator_mark_fields` now reads through too, once per pass for
  every consumed field: the latest stamp at
  or before `audio_rendered_sample` while the transport plays, else 0) and
  pushed where they moved since the last push; with nothing observed a tick
  reads nothing. A mark's slot (sequencer id, key) is kept host-side
  (`KindsShared::generator_marks`), exact where `gid` is an f64 (a legacy
  hashed id past 2^53); a cold read asks the host through it.
- **The jaki reads map to fields** (`packages/alez.jaki/src/kind.lisp`;
  the view ports in .20). Let `g` be `(generator-of self)` and `(mark g k)`
  `(let ((m (generator-mark-named g k))) (if m … 0))`, read in the subtree
  that draws it (it re-runs when `g.marks` gains the key):
  `(reactive-value (bind-seq (str "generator-mark-" self.id)))` (the hit
  strip's playhead, `jk-playhead`) → the `""` mark's `m.value`;
  `… "-chord"` (`jk-chord-now`) → the `"chord"` mark's `m.value`; a
  row's `:lit (bind-seq (str "generator-mark-" self.id "-" route-slot))`
  → `#'m.value` of the mark named `route-slot` (0 without one); its
  `:lit-values` `(bind-seq (str … route-slot "." k))` → `#'m.value` of
  the marks named `(str route-slot "." k)`.
- **Not covered:** the mark history (only the shown value is published;
  no view reads past stamps); a generator's tick source, resolution and
  `:requires` (no content reads them); its tick errors (the status line
  reports them, `GeneratorTickErrorNotice`).

### 14.3 Follow-up beads

Each port bead depends on the beads whose rows it uses (`bd dep`).

| Tag | Bead | Kinds | Ports blocked |
|---|---|---|---|
| 7b | eseq-0l17.28 (built) | `param` under `device` (values, p-lock display, print latch), `device.playhead`, step p-lock render (`plocked`, `lock-kind`, `variant-color`), send p-lock flags | .11 .13 .14 .18 .19 .21 |
| 7b-2 | eseq-0l17.36 (built) | devices (and params) for MIDI fx, bus effects, rack slots; `bus.devices`, `track.midi-devices`, `device.delete-target` (from 7i), `device.voices` | .13 .14 .18 .19 .21 |
| 7b-2a | eseq-0l17.41 (built) | the clear command for a rack slot instrument's p-locks (`unlock-param!` on a rack slot param) | — |
| 7b-2b | eseq-0l17.42 (built) | rack slot strip controls (gain, pan, mute, solo, choke, enabled) on the rack slot device, with their p-lock display; `lock-strip!` / `unlock-strip!` | .14 .19 |
| 7b-2c | eseq-0l17.54 (built) | a rack slot's base note (base, display, lock) and max-polyphony p-locks on the rack slot device | .14 .19 |
| 7b-3 | eseq-0l17.37 (built) | panel extras: param placement and lanes, modulation display, process mapping, tensors, base note, key locks, rack and project macros, variant chip list, neural-selection display | .14 .18 |
| 7b-4 | eseq-0l17.43 (built) | the rest of the panel data: sampler media (rack slot selection included), sound binding, display name, meter selector, fixed modulators and the modulator envelope, tables and IR names, param UI metadata, scene macro config setters and `diff-count`, `step.variant` | .14 .18 |
| 7b-5 | eseq-0l17.56 (built) | the built-in effect editors' host state: the `table-editor` singleton (the Filter Table response editor session) and its actions | .14 |
| 7b-3a | eseq-0l17.44 (built) | recorded (undoable) drum rack macro edits: name, base, mapping range and curve (`App::apply_rack_macro_edit`, `EditPatch::RackMacro`), shared by the rack panel's commands and the kind setters | .18 |
| 7b-3b | eseq-0l17.57 (built) | rack macro p-lock actions: `lock-rack-macro!` / `unlock-rack-macro!` (`SetRackMacroPlockMulti` / `ClearRackMacroPlockMulti`, one undo entry) | .16 |
| 7c | eseq-0l17.29 (built) | `process` (a track's chain), `lane`, `inlet`, `port`, `fanout`, `state-cell`, `process-class`, `process-library`; `track.processes` / `lanes` | .11 .14 .20 |
| 7c-2 | eseq-0l17.45 (built) | graph-node process slot probes and run errors (the node bay's scopes): `process.error`, `state-cell.values` of `n.processes` | .11 .20 |
| 7d | eseq-0l17.30 (built) | `song` and `region` singletons, `scene-span`, `clip`, pattern `cell`, `track.governed` / `latched` | .11 .12 .13 .15 .17 .20 |
| 7d-2 | eseq-0l17.39 (built) | `song.pending` (the provisional capture surface) as positional sub-kinds: `pending-lane`, `pending-scene`, `pending-launch` | .15 |
| 7e | eseq-0l17.31 (built) | `note`, `piano-roll` singleton, tracker rows (`param.step-locks`, `rack-macro.step-locks`) and grid playheads (view derivation; a live field since .65, `track.playhead-row`) | .16 .20 .65 |
| 7e-2 | eseq-0l17.47 (built) | the piano roll's automation lane: `focus-step` (focus-axis step params, `piano-roll.steps`), the lane a view | .16 |
| 7e-3 | eseq-0l17.48 (built) | model note ids (handles kept through undo and legacy edits; a grid step's id-less single note keeps a host id until its first setter edit) | — |
| 7f | eseq-0l17.32 (built) | `browser`, `sound-palette` / `sound`, `editor`, `learn`, `retro`, `song-export`, `settings` and `agent` singletons and their rows, `project.name`, `track.instrument-id` | .12 .17 .18 |
| 7g | eseq-0l17.33 (built) | `graph`, `graph-node`, `graph-edge`, `graph-param` (the GRAPH namespace, graph playback), `project.graphs`, `track.active-notes` | .13 .14 .20 |
| 7g-2 | eseq-0l17.49 (built) | a graph node's process patch as `process` instances (`graph-node.processes`), its setters through `edit-process` | .20 (and .45) |
| 7g-3 | eseq-0l17.50 (built) | the native neural engine's networks and neuron selection: `network`, `neuron`, `project.networks` | .20 |
| 7g-4 | eseq-0l17.51 (built) | event streams as positional rows: `graph.events`, `node-events`, `deltas`, `node-deltas`, `group-activity`, `group-suppression`; `transport.track-events`, `track-events-beat` | .20 |
| 7g-5 | eseq-0l17.52 (built) | generators (tick-mode sequencers) and their marks: `generator`, `generator-mark`, `project.generators`, `generator-of`, `generator-mark-named` | .20 |
| 7h | eseq-0l17.34 (built) | rack pads, rack clips, grooves (rack, clip, pad shares, pool, library), armed rack | .11 .13 .19 |
| 7i | eseq-0l17.35 (built) | track settings (`tp-*`), scales (`tuning`, `degree`), routing (outputs, mod routes and levels), the project's option lists and option constants, selection extras (delete targets, step cursor, auto-follow), transport/engine extras | .11 .12 .13 .14 .18 |

### 14.4 Families

Uses = accesses in content; Publisher = where the host writes it
(`sv/` = `ui/state_values/`); Feed = how the legacy publisher updates it
(live: per frame, meters, playheads, shared atomics; model: epoch- or
edit-gated); Status: built (.10), a follow-up bead, view-local (a `:state`
singleton in the view), remove, or keep. `<f>` names a content helper that
builds the field name.

| Family | Uses | Files | Publisher | Feed | → kind.field | Status | Port |
|---|---|---|---|---|---|---|---|
| `SEQ.<slot-page-active-field>` | 1 | sequencer | sv/expanded_step.rs | model | track.playhead | built (.10); ported (.66), removed | .11 .66 |
| `SEQ.<slot-param-field>` | 2 | sequencer | sv/expanded_step.rs | model | step.‹param› | built (.10); ported (.66), removed | .11 .66 |
| `SEQ.<track-bus-send-field>` | 2 | effects/track-panels | sv/track_and_mixer.rs | model | send.display (of selection.track) | built (.10); ported (.61: the track panel's send controls were dead code, deleted), legacy removed (`tp-bus-N-send` and `tp-bus-sends`: publishers, registration; the send controls read send.display) | .14 .61 |
| `SEQ.<track-pan-field>` | 1 | mixer | sv/track_and_mixer.rs | model | track.pan | built (.10); ported (.13), removed | .13 |
| `SEQ.<track-volume-field>` | 2 | mixer, sequencer | sv/track_and_mixer.rs | model | track.volume | built (.10); ported (.13), kept: sequencer; ported (.11 grid), kept: unread, .22; removed (.66) | .11 .13 .66 |
| `SEQ.auxas` | 1 | seqv-track-params | reactive_sync.rs | model | step.aux-a | built (.10); ported (.66), kept: unread, .22 | .11 .66 |
| `SEQ.bpm` | 2 | effects/builtin/phaser-flanger, transport | bounce/job.rs | live | transport.bpm | built (.10); ported (.12), kept: phaser-flanger (.14); ported (.61: phaser-flanger binds #'transport.bpm) | .12 .14 .61 |
| `SEQ.bus-mutes` | 5 | mixer, sequencer, legacy/mixer | sv/track_and_mixer.rs | model | bus.muted | built (.10); ported (.13), kept: sequencer; ported (.11 grid), kept: unread, .22; removed (.66) | .11 .13 .66 |
| `SEQ.bus-names` | 25 | mixer, legacy/mixer, seq-core-state +2 | sv/track_and_mixer.rs | model | bus.name | built (.10); ported (.13), kept: seq-core-state; ported (.14 A: panel-widgets); kept (.19): no content reader left, the bus invalidations still publish it, .22 | .11 .13 .14 .19 |
| `SEQ.bus-peak-*` | 3 | mixer, sequencer | sv/meters_and_modulation.rs | live | bus.peak | built (.10); ported (.13), kept: sequencer; ported (.11 grid), kept: unread, .22; kept (.66): unread, .22 | .11 .13 .66 |
| `SEQ.bus-solos` | 4 | mixer, sequencer, legacy/mixer | sv/track_and_mixer.rs | model | bus.soloed | built (.10); ported (.13), kept: sequencer; ported (.11 grid), kept: unread, .22; removed (.66) | .11 .13 .66 |
| `SEQ.bus-volumes` | 3 | mixer, sequencer, legacy/mixer | sv/track_and_mixer.rs | model | bus.volume | built (.10); ported (.13), kept: sequencer; ported (.11 grid), kept: unread, .22; removed (.66) | .11 .13 .66 |
| `SEQ.cpu-load-pct` | 1 | transport | reactive_tick.rs | live | engine.cpu-load | built (.10); ported, legacy removed (.12) | .12 |
| `SEQ.current-pattern` | 18 | transport, arrangement, mixer +10 | sv/topology_and_visualization.rs | model | transport.scene (s.index) | built (.10); ported (.12, .13, .15, .20: jaki-builder-demo, .64: the graph demo scripts), no content reader left (its publisher remains); ported (.12, .13, .15), kept: macros, scripts | .12 .13 .15 .20 .64 |
| `SEQ.current-track` | 108 | piano-roll, effects/process-panel, browser +19 | piano_roll.rs | live | selection.track | built (.10); ported (.13, .15, .16, .17, .65: alez.tracker), kept: many; ported (.14 A: param-controls, panel-frame read selection.track.index); ported (.18: application menus read selection.track); ported (.61: process panel, buffers, convolution-reverb read selection.track / dv/current-track-index); ported (.19: drum-rack-v2, rack-groove-buffer), kept: step-grid (unloaded), .22 | .11 .13 .14 .15 .16 .17 .18 .19 .20 .61 .65 |
| `SEQ.delays` | 1 | seqv-track-params | event_loop.rs | model | step.delay | built (.10); ported (.66), kept: unread, .22 | .11 .66 |
| `SEQ.durations` | 2 | seq-core-state, seqv-track-params | event_loop.rs | model | step.duration | built (.10); ported (.66), kept: seq-core-state COMPAT (.14) | .11 .66 |
| `SEQ.groups` | 53 | mixer, drum-rack-v2, seq-core-state +12 | project.rs | model | group.* via (groups), track.group | built (.10); ported (.13, .17, .20: alez.jaki, .64: the graph demos' route menus), kept: drum-rack-v2, seq-core-state, alez.neural +; ported (.13, .17), kept: drum-rack-v2, seq-core-state +; ported (.67: alez.neural), kept: drum-rack-v2, effects/buffers +; ported (.19: drum-rack-v2, effects/buffers, browser, step-grid-interactions, track-panels), removed (with `SEQ.group-collapsed`: `sync_groups_bindings`, `build_groups_value`, the registrations) | .11 .13 .17 .19 .20 .64 .67 |
| `SEQ.master-peak-l` | 2 | mixer, transport | event_loop.rs | live | master.peak-l | built (.10); ported (.12, .13), removed | .12 .13 |
| `SEQ.master-peak-r` | 2 | mixer, transport | event_loop.rs | live | master.peak-r | built (.10); ported (.12, .13), removed | .12 .13 |
| `SEQ.master-recording` | 2 | transport | reactive_tick.rs | live | master.recording | built (.10); ported, legacy removed (.12) | .12 |
| `SEQ.metronome` | 1 | transport | host_commands/misc.rs | live | transport.metronome | built (.10); ported, legacy removed (.12) | .12 |
| `SEQ.output-latency-ms` | 1 | transport | reactive_tick.rs | live | engine.latency-ms | built (.10); ported, legacy removed (.12) | .12 |
| `SEQ.pans` | 2 | seq-core-state, seqv-track-params | event_loop.rs | model | step.pan | built (.10); ported (.66), kept: seq-core-state COMPAT (.14) | .11 .66 |
| `SEQ.playhead-active-*` | 1 | step-grid | sv/meters_and_modulation.rs | live | step.playing | built (.10); kept (.66): step-grid (unloaded), .22 | .11 .66 |
| `SEQ.playhead-page` | 1 | seq-core-state | sv/meters_and_modulation.rs | live | track.playhead (page = playhead / 16 in the view) | built (.10); ported (.66), kept: unread, .22 | .11 .66 |
| `SEQ.playing` | 11 | retrospective, transport, effects/track-panels +4 | sequencer/state/sequencer_state/scene_launch.rs | live | transport.playing | built (.10); ported (.12), kept: track-panels, param-controls, seq-core-state, sequencer; ported (.14 A: param.printing carries the gate); ported (.61: the step panel's print gate reads transport.playing); ported (.12, .65: alez.tracker), kept: track-panels, param-controls, seq-core-state, sequencer | .11 .12 .14 .20 .61 .65 |
| `SEQ.queued-scene` | 2 | transport | event_loop.rs | model | transport.queued | built (.10); ported, legacy removed (.12) | .12 |
| `SEQ.record-armed` | 4 | mixer, sequencer, legacy/mixer | event_loop.rs | live | track.armed | built (.10); ported (.13), kept: sequencer; ported (.11 grid), kept: tracker | .11 .13 |
| `SEQ.record-quantize` | 1 | transport | host_commands/misc.rs | live | transport.record-quantize | built (.10); ported, legacy removed (.12) | .12 |
| `SEQ.recording` | 4 | effects/track-panels, transport, effects/param-controls | reactive_sync.rs | live | transport.recording | built (.10); ported (.12), kept: track-panels, param-controls (.14); ported (.14 A: param.printing carries the gate), kept: track-panels; ported (.61: the step panel's print gate reads transport.recording) | .12 .14 .61 |
| `SEQ.retrig-rates` | 2 | seq-core-state, seqv-track-params | reactive_sync.rs | model | step.retrig-rate | built (.10); ported (.66), kept: seq-core-state COMPAT (.14) | .11 .66 |
| `SEQ.retrigs` | 2 | seq-core-state, seqv-track-params | reactive_sync.rs | model | step.retrig | built (.10); ported (.66), kept: seq-core-state COMPAT (.14) | .11 .66 |
| `SEQ.roll-mode` | 2 | transport | reactive_tick.rs | live | transport.roll-mode | built (.10); ported, legacy removed (.12) | .12 |
| `SEQ.scene-banks` | 2 | scene-banks | sv/song_state.rs | model | (banks) → bank.label/scenes | built (.10); ported, legacy removed (.12) | .12 |
| `SEQ.scene-launch-quantize` | 6 | transport, drum-rack-v2, mixer | rack_clip_switch_probe.rs | model | transport.launch-quantize | built (.10); ported (.12, .13), kept: drum-rack-v2; the host kinds read transport.launch-quantize from it; ported (.19), kept: no content reader left, but the host kinds' launch-quantize source, .22 | .12 .13 .19 |
| `SEQ.scene-names` | 8 | browser, arrangement | sv/song_state.rs | model | scene.name | built (.10); ported (.15, .17), legacy removed (.15) | .15 .17 |
| `SEQ.selected-steps` | 4 | step-grid, effects/param-controls | reactive_tick.rs | live | step.selected | built (.10); ported (.14 A: (len selection.steps)), kept: step-grid | .11 .14 |
| `SEQ.selected-tracks` | 5 | mixer, step-grid-interactions | sv/steps_and_pattern.rs | live | selection.tracks | built (.10); ported (.13), kept: step-grid-interactions | .11 .13 |
| `SEQ.seq-track-step-active-*` | 2 | sequencer | sv/steps_and_pattern.rs | live | step.active | built (.10); ported (.11 grid), kept: unread, .22; removed (.66) | .11 .66 |
| `SEQ.seq-track-step-duration-*` | 1 | sequencer | sv/steps_and_pattern.rs | model | step.held | built (.10); ported (.11 grid), kept: unread, .22; removed (.66) | .11 .66 |
| `SEQ.seq-track-step-param-haptic-*` | 1 | sequencer | sv/steps_and_pattern.rs | model | step.‹param› (detent in the view) | built (.10); ported (.66), removed | .11 .66 |
| `SEQ.seq-track-step-param-slider-*` | 1 | sequencer | sv/steps_and_pattern.rs | model | step.‹param› (normalize in the view) | built (.10); ported (.66), removed | .11 .66 |
| `SEQ.seq-track-step-selected-*` | 2 | sequencer | sv/steps_and_pattern.rs | live | step.selected | built (.10); ported (.11 grid), kept: unread, .22; removed (.66) | .11 .66 |
| `SEQ.seqv-cursor-param-value-*` | 1 | sequencer | sv/expanded_step.rs | model | step.‹param› of the cursor step (cursor is view-local) | built (.10); ported (.66), removed | .11 .66 |
| `SEQ.seqv-cursor-sync-index-*` | 1 | sequencer | sv/expanded_step.rs | model | step.sync of the cursor step | built (.10); ported (.66), removed | .11 .66 |
| `SEQ.seqv-slot-length-active-*` | 1 | sequencer | sv/expanded_step.rs | model | track.num-steps | built (.10); ported (.66), removed | .11 .66 |
| `SEQ.step-color-b-effective` | 2 | sequencer | sv/track_and_mixer.rs | model | track.color × track.audible | built (.10); ported (.11), removed | .11 |
| `SEQ.step-color-g-effective` | 2 | sequencer | sv/track_and_mixer.rs | model | track.color × track.audible | built (.10); ported (.11), removed | .11 |
| `SEQ.step-color-r-effective` | 2 | sequencer | sv/track_and_mixer.rs | model | track.color × track.audible (+ track.governed, 7d) | built (.10); ported (.11), removed | .11 |
| `SEQ.steps` | 3 | step-grid | lisp_host/eseq/graph_authoring.rs | model | selection.track.steps → step.active | built (.10); kept (.66): step-grid (unloaded), .22 | .11 .66 |
| `SEQ.syncs` | 2 | seqv-track-params, seq-grid-mode | ui_benchmark/ipc.rs | model | step.sync | built (.10); ported (.66), kept: unread, .22 | .11 .66 |
| `SEQ.tp-is-rack` | 5 | effects/track-panels, mixer | sv/project_state.rs | model | selection.track.rack | built (.10); ported (.13), kept: track-panels; ported (.61: track panels read selection.track.rack / selection.rack-slot), legacy removed (publisher, registration) | .13 .14 .61 |
| `SEQ.tp-num-steps` | 8 | seq-grid-mode, seq-core-state, piano-roll +2 | reactive_sync.rs | model | selection.track.num-steps | built (.10); ported (.16), kept: seq-grid-mode, seq-core-state +; ported (.61: track panel reads t.num-steps), kept: seq-grid-mode, seq-core-state, step-grid-interactions; ported (.66: seq-grid-mode, seq-core-state), kept: track-panels | .11 .14 .16 .61 .66 |
| `SEQ.tp-timebase` | 2 | step-grid, effects/track-panels | sv/project_state.rs | model | selection.track.timebase | built (.10); ported (.61: track panel reads t.timebase and its lock in track.setting-locks), kept: step-grid | .11 .14 .61 |
| `SEQ.track-auxas` | 1 | seqv-track-params | reactive_sync.rs | model | step.aux-a | built (.10); ported (.66), kept: unread, .22 | .11 .66 |
| `SEQ.track-bus-sends` | 2 | mixer, midi-midimix | reactive_sync.rs | model | track.sends → send.bus, send.display | built (.10); ported (.13), removed | .13 |
| `SEQ.track-collapsed` | 2 | track-collapse | reactive_sync.rs | live | track.collapsed | built (.10); ported (.11 grid), kept: unread, .22; removed (.66) | .11 .66 |
| `SEQ.track-color-b-effective` | 1 | sequencer | sv/track_and_mixer.rs | model | track.color × track.audible | built (.10); ported (.11), removed | .11 |
| `SEQ.track-color-g-effective` | 1 | sequencer | sv/track_and_mixer.rs | model | track.color × track.audible | built (.10); ported (.11), removed | .11 |
| `SEQ.track-color-r-effective` | 1 | sequencer | sv/track_and_mixer.rs | model | track.color × track.audible (dim in the shader) | built (.10); ported (.11), removed | .11 |
| `SEQ.track-colors` | 21 | mixer, rack-groove-buffer, arrangement +12 | sv/track_and_mixer.rs | model | track.color | built (.10); ported (.13, .15, .16, .20: alez.jaki, the event-view demos, .64: the graph demos, .65: alez.tracker), kept: sequencer, alez.neural +; ported (.14 A: panel-bodies); ported (.13, .15), kept: sequencer +; ported (.67: alez.neural), kept: rack-groove-buffer, step-grid, tracker; ported (.19: rack-groove-buffer reads t.color), kept: step-grid (unloaded), .22 | .11 .13 .14 .15 .16 .19 .20 .64 .65 .67 |
| `SEQ.track-delays` | 1 | seqv-track-params | reactive_sync.rs | model | step.delay | built (.10); ported (.66), kept: unread, .22 | .11 .66 |
| `SEQ.track-durations` | 1 | seqv-track-params | reactive_sync.rs | model | step.duration | built (.10); ported (.66), kept: unread, .22 | .11 .66 |
| `SEQ.track-instrument-types` | 14 | track-collapse, mixer, application-menus | sv/track_and_mixer.rs | model | track.instrument-type | built (.10); ported (.13, .16, .11 grid), kept: track-collapse +; ported (.18: application menus read selection.track.instrument-type); ported (.13, .16), kept: track-collapse +; ported (.11 grid), kept: application-menus | .11 .13 .18 |
| `SEQ.track-length-row-*` | 1 | sequencer | sv/expanded_step.rs | model | track.num-steps | built (.10); ported (.11), removed | .11 |
| `SEQ.track-muted-effective` | 11 | mixer, sequencer, legacy/mixer +1 | sv/track_and_mixer.rs | live | not track.audible | built (.10); ported (.13, .11 grid), kept: sequencer +; ported (.61: the step panel's track chip binds t.audible) | .11 .13 .14 .61 |
| `SEQ.track-mutes` | 3 | mixer, sequencer, legacy/mixer | reactive_sync.rs | live | track.muted | built (.10); ported (.13, .11 grid), kept: sequencer; removed (.66) | .11 .13 .66 |
| `SEQ.track-names` | 28 | sequencer, mixer, packages/alez.jaki/src/kind +14 | reactive_sync.rs | model | track.name | built (.10); ported (.13, .16, .20: alez.jaki, .64: the graph demos, .65: alez.tracker), kept: many; ported (.18: the scene macro track mask lists (tracks)); ported (.13, .16, .11 grid), kept: many; ported (.61: buffers' rack pad label reads the track); ported (.67: alez.neural), kept: rack-groove-buffer, effects/buffers, tracker; ported (.19: rack-groove-buffer and buffers read t.name), kept: no content reader left (the param words' track names ride it), .22 | .11 .13 .14 .16 .18 .19 .20 .61 .64 .65 .67 |
| `SEQ.track-num-steps` | 5 | sequencer, packages/alez.tracker/src/ui | event_loop.rs | model | track.num-steps | built (.10); ported (.11 grid, .65: alez.tracker), kept: unread, .22 | .11 .20 .65 |
| `SEQ.track-pans` | 1 | seqv-track-params | reactive_sync.rs | model | step.pan (per-step lists, see steps) | built (.10); ported (.66), kept: unread, .22 | .11 .66 |
| `SEQ.track-peak-*` | 3 | mixer, sequencer, legacy/mixer | sv/meters_and_modulation.rs | live | track.peak | built (.10); ported (.13), kept: sequencer; ported (.11 grid), kept: unread, .22; kept (.66): unread, .22 | .11 .13 .66 |
| `SEQ.track-playhead-page-*` | 1 | sequencer | sv/expanded_step.rs | live | track.playhead | built (.10); ported (.11), removed | .11 |
| `SEQ.track-playhead-row-*` | 1 | sequencer | sv/expanded_step.rs | live | track.playhead | built (.10); ported (.11), removed | .11 |
| `SEQ.track-playhead-row-active-*` | 1 | sequencer | sv/expanded_step.rs | live | track.playhead | built (.10); ported (.11), removed | .11 |
| `SEQ.track-retrig-rates` | 1 | seqv-track-params | reactive_sync.rs | model | step.retrig-rate | built (.10); ported (.66), kept: unread, .22 | .11 .66 |
| `SEQ.track-retrigs` | 1 | seqv-track-params | sv/topology_and_visualization.rs | model | step.retrig | built (.10); ported (.66), kept: unread, .22 | .11 .66 |
| `SEQ.track-selected-*` | 3 | mixer, seq-core-state | sv/steps_and_pattern.rs | model | track.selected / (member t selection.tracks) | built (.10); ported (.13), kept: sequencer, seq-core-state; ported (.11), removed | .11 .13 |
| `SEQ.track-solos` | 3 | mixer, sequencer, legacy/mixer | reactive_sync.rs | live | track.soloed | built (.10); ported (.13), kept: sequencer; ported (.11 grid), kept: unread, .22; removed (.66) | .11 .13 .66 |
| `SEQ.track-syncs` | 1 | seqv-track-params | reactive_sync.rs | model | step.sync | built (.10); ported (.66), kept: unread, .22 | .11 .66 |
| `SEQ.track-timebases` | 2 | sequencer | sv/param_fields_and_sync.rs | model | track.timebase | built (.10); ported (.11 grid), kept: unread, .22; removed (.66) | .11 .66 |
| `SEQ.track-transposes` | 1 | seqv-track-params | host_commands/step_history.rs | model | step.transpose | built (.10); ported (.66), kept: unread, .22 | .11 .66 |
| `SEQ.track-velocities` | 1 | seqv-track-params | reactive_sync.rs | model | step.velocity | built (.10); ported (.66), kept: unread, .22 | .11 .66 |
| `SEQ.track-volumes` | 3 | sequencer, legacy/mixer | reactive_sync.rs | live | track.volume | built (.10); ported (.13), kept: sequencer; ported (.11 grid), kept: unread, .22; removed (.66) | .11 .13 .66 |
| `SEQ.transport-playhead` | 1 | transport | ui_replay_probe.rs | live | transport.position | built (.10); ported, legacy removed (.12) | .12 |
| `SEQ.transposes` | 2 | seq-core-state, seqv-track-params | reactive_sync.rs | model | step.transpose | built (.10); ported (.66), kept: seq-core-state COMPAT (.14) | .11 .66 |
| `SEQ.velocities` | 2 | seq-core-state, seqv-track-params | app/retrospective.rs | model | step.velocity | built (.10); ported (.66), kept: seq-core-state COMPAT (.14) | .11 .66 |
| `SEQV.<sel-track-vis-field>` | 1 | seq-core-state | Lisp (reactive-set) | Lisp-owned | track.selected | built (.10); ported (.11), removed | .11 |
| `<ns-var name>` | 2 | effects/drum-surface | custom_ui.rs | - | param.value via (device-param d "x") | built (.28) | .14 |
| `SEQ.<get>` | 23 | effects/param-controls, effects/instrument-panel, effects/sampler-panel +9 | sv/param_fields_and_sync.rs, instrument_panel.rs, effects_panel.rs | model | param.value / param.name (panel :value-field, :label-field, :name-field, :short-field); MIDI fx / bus / rack slot params (built .36), rack macro names → rack-macro.name (built .37), a rack slot's strip value fields (`rack_slot_value_field`: `track-N-rack-slot-K-gain`, `-pan`, `-mute`, `-solo`; the slot dict's `:gain-field`, …) → device.gain-display / pan-display / muted-display / soloed-display, their lock state → `-locked` (built .42); the base note and voices value fields (`-base-note`, `-max-polyphony`; the slot dict's `:base-note-field`, `:max-polyphony-field`) → device.base-note-display / -locked, device.voices-display (built .54); the sampler selection times (`track-N-sampler-selection-start-time`, `track-N-rack-slot-K-sampler-selection-*-time`; the dicts' `:start-time-field` / `:end-time-field`) → device.start-time / end-time, `modulator-phase-N` / `modulator-level-N` (the dicts' `:phase-field` / `:level-field`) → device.modulator-phase / modulator-level (built .43) | built (.28, .36, .37, .42, .43, .54); factory device UIs (.21) read no field name except spatial-harmonic-delay's COMPAT `:value-field` tap count, the panel's fields stay with the custom-UI runtime (.14); ported (.14 A: param-controls and the custom-UI runtime bind param.value through eseq.effects.devices/param-of; % effect params read display units, the effect dicts too), kept: instrument-panel macros, track-panels lock rows, sampler-panel; ported (.18: the mapping table reads macro-mapping / rack-macro), kept: macro-state's COMPAT macro-name (the rack panel's live `:name-field`, until .61); ported (.61: rack macros → rack-macro.value / base / locked / has-locks / name, rack slot strip → device.*-display / delete-target, sampler selection → device.start-time / end-time, modulator → device.modulator-phase / -level, the source editors' slot phases → param.mod-phase (new)); legacy removed: the `modulator-phase-N` / `-level-N` fields and the slot phase fields' publishers (`instrument-mod-slot-phase-*`, `fx-mod-slot-phase-*`, rack slot ones); kept (eseq-0l17.22): the dicts' `*-field` strings (the host-less test seeds key on them), the param value publishers (the p-lock table rows' `:value-field`), the rack macro name fields (main's alez.tracker until eseq-0l17.65 merges); ported (.74: the p-lock table binds `param.value` / `rack-macro.value`; the print latch shows on `param.value` while it prints), legacy removed: the print latch display writer (`sync_print_latch_display`); kept (eseq-0l17.22, after eseq-0l17.19): the param value publishers (`sync_fx_param_binding_fields*`, the rack value field syncs; unread by content now, read by the host-less test seeds and Rust tests); ported (.82: every panel control binds its param instance, `:prm` in the panel dict `eseq.effects.panel-data` builds), legacy removed: the dicts' `*-field` strings and every param value publisher (`sync_*_value_field*`, `sync_fx_param_binding_fields*`, the rack macro name / value and rack slot strip syncs, the field-name helpers in `sv/shared.rs` and `sv/meters_and_modulation.rs`, the learn preview field writes); the host-less test seeds' handle maps keep test-local names | .14 .16 .18 .19 .20 .21 .61 .74 .82 |
| `SEQ.<slot-field>` | 12 | sequencer | sv/expanded_step.rs | model | step.active/selected/playing/plocked/lock-kind/variant-color through the view's own slot→step map (expanded-step projection removed) | built (.28); ported (.66), removed | .11 .66 |
| `SEQ.<var field>` | 8 | effects/param-controls, effects/custom-ui-runtime, mixer +1 | sv/param_fields_and_sync.rs | model | param.value / send.display (field strings from panel data); mod / process fields → param.mod-offset / mod-value / mod-scale / process-value / process-clamped, device.mod-phases (built .37) | built (.28, .37); mixer sends ported (.13): `track-N-bus-M-send` and its `-plock-*` / `-proc-*` removed (kept: `tp-bus-M-send`, track-panels); ported (.14 A: param-controls, custom-ui-runtime; mod/process display → param.mod-offset / mod-value / mod-scale / process-value / process-clamped, unconditionally bound: an unmodulated param reads 0 and draws no dot), kept: track-panels (tp-bus-M-send) | .13 .14 |
| `SEQ.effects` | 3 | application-menus, effects/index, effects/buffers | lisp_host/dgen/instrument_storage.rs | model | track.devices → device.params; mod targets, sources, tensors → param.mod-targets / section / mod-slot / visible, device.tensors (built .37); `:table-name` / `:table-options` / `:table-mode` / `:table-engine` / `:table-data-key` / `:ir-name` → device.table-* / ir-name, `:meter` → device.meter, `:modulators` → device.modulators (modulator), param `:group` / `:env` / `:role` / `:display-name` / `:options` (an unresolved reference) → param.group / env / role / display-name / asset-options (built .43); `:editor` (the Filter Table response editor) → table-editor (its `:band` → band-kind / band-freq / band-gain / band-q; which device: table-editor.device), the `filter-table-editor-*` commands → the `table-editor-…!` actions and `(set! te.selected-frame n)` (built .56) | built (.28, .37, .43, .56); kept as structure (.14 A reads every value from the device; index, buffers, effect-panels read the dicts: B/C); ported (.18: application menus find the armed effect among selection.track.devices and each bus's devices, device.builtin new); ported (.61: Filter Table → device.table-* and the table-editor singleton, Convolution Reverb → device.ir-name, EQ8's spectrum → device.meter), kept as structure (eseq-0l17.22): buffers, eseq.effects/device-panel, tests and capture fixtures; kept (.74): `buffers` is ported by eseq-0l17.19 (the drum rack lane); the panels' layout from kinds follows it (eseq-0l17.22); ported (.82: `eseq.effects.panel-data` builds each effect panel from track.devices / the rack slot's devices; buffers, device-panel and the capture fixtures read it; `fx-node-id` reads device.node-id), legacy removed (`build_effects_value`, `filter_table_editor_value`, the registration and the reactive tick's fx panel sync) | .14 .18 .61 .74 .82 |
| `SEQ.instrument-panel` | 10 | effects/param-controls, browser, effects/index +3 | reactive_tick.rs | model | device panel data (device.params; rack slots: the rack device's devices (built .36); key locks → param.key-locks / device.key-locked-notes / device.variants, macros → device.macros, modulation → param.mod-* / mod-targets, base note → device.base-note, tensors → device.tensors, process → param.process-* (built .37); a rack slot's strip (the slot dict's `:gain`, `:pan`, `:mute`, `:solo`, `:enabled`, choke group) → device.gain / pan / muted / soloed / enabled / choke, its `set-rack-slot-*` / `set-rack-slot-param-plock` commands → their `set!`s and `lock-strip!` / `unlock-strip!` (built .42); the slot dict's `:base-note` / `:max-polyphony` → device.base-note / voices, `set-rack-slot-base-note` / `-max-polyphony` and their p-locks → `set!` and `lock-strip!` (built .54); sampler media (`:buffer`, `:duration`, `:start-time`, `:end-time`, `:slices`, `:slice-active`, `:onsets`, `:analysis-*`, `:downbeat-time`) → device.sample-buffer / sample-duration / start-time / end-time / slices / slice-active / onsets / analysis-* / downbeat-time, `:sound-binding` / `:display-name` → device.sound-binding / display-name, `:meter` → device.meter, `:modulators` → device.modulators, `:phase-field` / `:level-field` → device.modulator-phase / modulator-level (built .43)) | built (.28, .36, .37, .42, .43, .54); ported (.17: the browser's rack check, dead before the port (it also required `SEQ.sidebar-kind` "rack", which the host never set), reads browser.track.rack alone and is live now); ported (.14 A: param-controls, panel-bodies read key locks, variants and rack macros from the device), kept as structure: index, buffers, instrument-panel, sampler-panel, effect-panels (B/C); ported (.18: the mapping table reads the armed rack-macro of selection.track's instrument device); ported (.61: sampler media → device.sample-*, slices, start/end-time, rack macros → rack-macro, the rack slot macro dot reads the dict's own :macros), kept as structure (eseq-0l17.22): buffers, eseq.effects/device-panel, tests and capture fixtures; kept (.74): `buffers` is ported by eseq-0l17.19 (the drum rack lane); the panels' layout from kinds follows it (eseq-0l17.22); ported (.82: `eseq.effects.panel-data` builds the synth, sampler and rack panels from the instrument device, its params (param.section / visible / text / host-modulatable) and the rack's slot devices (device.strip-macros, device.instrument-name)), legacy removed (`build_instrument_panel_value*`, `build_sampler_panel_value`, `build_rack_panel_value` and the rack slot builders, the rack slot selection fields, `sync_shared_panel_state`, the registration); each device's dict is built in its own subtree, a dropdown binds its option (`:value-index`) | .14 .17 .18 .61 .74 .82 |
| `SEQ.macros` | 5 | macros, effects/param-controls | project.rs | model | project macros → project.macros / macro (mappings → macro-mapping); rack macros → device.macros of the rack's instrument (rack-macro) (built .37); a scene macro's `:target-scene` / `:morph-params` / `:steal-patterns` / `:quantize` / `:track-mask` → macro.target-scene / morph-params / steal-patterns / quantize / tracks, settable (`macro-scene-config` → their `set!`s), `:diff-count` → macro.diff-count (built .43) | built (.37, .43); ported (.14 A: param-controls reads (macros) / mm.target), kept: macros; ported (.18), legacy removed (publisher, registration, the parity test) | .14 .18 |
| `SEQ.sampler-playhead` | 1 | effects/sampler-panel | reactive_tick.rs | live | device.playhead (live) | built (.28); ported (.61: the waveform binds #'d.playhead; the tick keeps the sampler voices watched while it is observed, HostKinds::wants_sampler_playhead), legacy removed (publisher, registration) | .14 .61 |
| `SEQ.seq-track-step-plock-kind-*` | 1 | sequencer | sv/steps_and_pattern.rs | model | step.lock-kind | built (.28); ported (.11 grid), kept: unread, .22 | .11 |
| `SEQ.seq-track-step-plocked-*` | 1 | sequencer | sv/steps_and_pattern.rs | model | step.plocked | built (.28); ported (.11 grid), kept: unread, .22 | .11 |
| `SEQ.seq-track-step-variant-b-*` | 1 | sequencer | - | model | step.variant-color | built (.28); ported (.11 grid), kept: unread, .22 | .11 |
| `SEQ.seq-track-step-variant-g-*` | 1 | sequencer | - | model | step.variant-color | built (.28); ported (.11 grid), kept: unread, .22 | .11 |
| `SEQ.seq-track-step-variant-r-*` | 1 | sequencer | - | model | step.variant-color | built (.28); ported (.11 grid), kept: unread, .22 | .11 |
| `SEQ.step-has-plocks` | 2 | step-grid | reactive_tick.rs | model | step.plocked | built (.28); kept (.82): still written on a track fx param / p-lock invalidation; no content reader (eseq-0l17.78 deletes it) | .11 .82 |
| `SEQ.track-plock-any` | 1 | effects/param-controls | event_loop.rs | model | param.has-locks, send.has-locks | built (.28); ported (.14 A: param.has-locks), kept: track-panels (track-level rows, rack macro and slot control targets: tp/target-plock-any?); ported (.61: rack macros → rack-macro.has-locks, rack slot strip → device.strip-locks (new)), legacy removed (publisher, registration, the *plock-any-sync* projection) | .14 .61 |
| `SEQ.track-plock-printing` | 1 | effects/param-controls | step_print.rs | model | param.printing | built (.28); ported (.14 A: param.printing); legacy removed (publisher, registration, row test) | .14 |
| `SEQ.track-plock-variants` | 3 | effects/track-panels, effects/param-controls | reactive_sync.rs | model | step.variant-color (built .28) + variant chip list → track.variants / variant (built .37); the variant a step plays → step.variant (built .43) | built (.28, .37, .43); ported (.14 A: the p-lock accent is the current track variant's color, the plock-color singleton), kept: track-panels; kept (.61): the p-lock table's chips (no kind holds the def chip or a preview); ported (.74: the chips are `selection.track.variants` and a def chip, lit by `selection.plock-variant` (new: the selected step's variant, else the previewed one, else def)), legacy removed (publishers, registration, `build_track_plock_variants_value`) | .14 .61 .74 |
| `SEQ.track-plocks` | 9 | effects/track-panels, effects/param-controls | reactive_sync.rs | model | param.locked / param.base (the -on / -def projections; step panel rows from device.params) | built (.28); ported (.14 A: param.locked / param.base), kept: track-panels; ported (.61: track-level rows → track.setting-locks (new); rack macro row names → rack-macro.name; the *plock-sync* projection deleted), kept: the p-lock table (track-panels), filter-core; ported (.74: the p-lock table reads `selection.plock-rows` (new kind `plock-row`: target, domain, source, name, value, text, default, min, max, options, step, param, rack-macro, address); a device param's or rack macro's row binds that instance's value and edits through `lock-param!` / `lock-rack-macro!`), legacy removed (publishers, registration, the rows' `:value-field` / `:name-field`, `sync_track_plocks_for_neural_selection`, `sync_track_plock_variant_preview`); kept (eseq-0l17.22): the host-less test seeds publish the legacy rows, which `seed_panel_kinds` turns into `plock-row`s; ported (.82: the host-less test seeds build the panels' kinds from the App (`seed_app_panels`) or a `PanelSeed`) | .14 .61 .74 .82 |
| `SEQ.process-lanes` | 3 | seqv-track-params, seq-grid-mode, sequencer | input.rs | model | selection.track.lanes → lane | built (.29); ported (.66), kept: tracker | .11 .66 |
| `SEQ.process-library` | 3 | sequencer, packages/alez.neural/src/variable-reset | input.rs | model | process-library.classes → process-class | built (.29); ported (.66, .67: alez.neural, the node picker reads process-class.node-label / node-hidden), kept: unread, .22 | .11 .20 .67 |
| `SEQ.process-run-errors` | 1 | sequencer | reactive_tick.rs | model | process.error (live): a track slot's, and a graph node slot's (n.processes, under the slot's id); `lane-patch-run-error` → p.error, `lane-patch-expr-error` → p.compile-error, else the expr buffer's commit error (view state), else p.error | built (.29, .45); ported (.66), removed | .11 .66 |
| `SEQ.process-scope-cells` | 1 | sequencer | ui_replay_probe.rs | live | graph node slot scopes: state-cell.values of n.processes (live; `process-scope-cells-for` id → p.cells by name, each a history) | built (.45); ported (.66), removed | .11 .20 .66 |
| `SEQ.process-slots` | 2 | effects/process-panel | input.rs | model | selection.track.processes → process (inlets, ports) | built (.29); ported (.61: process panel reads selection.track.processes), legacy removed (publisher, registration; `track-process-slots` kept: sequencer) | .14 .61 |
| `SEQ.track-lane-patch` | 2 | sequencer | input.rs | model | t.processes: p.in-ports, port.target-process / target-inlet, fanout.target-process (cable ids derived in the view) | built (.29); ported (.66), removed | .11 .66 |
| `SEQ.track-process-lane-values` | 2 | seqv-track-params, packages/alez.tracker/src/ui | sv/param_fields_and_sync.rs | model | lane.values | built (.29); ported (.66), kept: tracker; ported (.65: alez.tracker), kept: seqv-track-params | .11 .20 .65 .66 |
| `SEQ.track-process-lanes` | 2 | seqv-track-params, packages/alez.tracker/src/ui | sv/topology_and_visualization.rs | model | track.lanes → lane | built (.29); ported (.66), kept: tracker; ported (.65: alez.tracker), kept: seqv-track-params | .11 .20 .65 .66 |
| `SEQ.track-process-scopes` | 3 | sequencer | ui_replay_probe.rs | live | process.cells → state-cell.values (live) | built (.29); ported (.66), removed | .11 .66 |
| `SEQ.track-process-slots` | 4 | sequencer, seqv-track-params, scripts/sequencers/band-coupling-matrix-demo | input.rs | model | track.processes → process | built (.29); ported (.20: band-coupling-matrix-demo), kept: sequencer, seqv-track-params; ported (.66), kept: band-coupling-matrix-demo | .11 .20 .66 |
| `SEQ.queued-track-clips` | 1 | mixer | event_loop.rs | model | cell.queued (live) | built (.30); ported (.13), removed | .13 |
| `SEQ.scene-spans` | 9 | arrangement | sv/song_state.rs | model | song.spans → scene-span | built (.30); ported, legacy removed (.15) | .15 |
| `SEQ.song-bound-clip` | 2 | arrangement, sound-palette | sv/song_state.rs | model | song.bound-clip | built (.30); ported (.15, .17), legacy removed (.15) | .15 .17 |
| `SEQ.song-clip-sounds` | 2 | arrangement | sv/sound_palette.rs | model | clip.dot / dot-color | built (.30); ported, legacy removed (.15) | .15 |
| `SEQ.song-cursor-beats` | 1 | transport | sv/song_state.rs | model | song.cursor | built (.30); ported, legacy removed (.12) | .12 |
| `SEQ.song-edit-error` | 2 | arrangement | sv/song_state.rs | model | song.edit-error | built (.30); ported, legacy removed (.15) | .15 |
| `SEQ.song-end-beat` | 2 | arrangement | sv/song_state.rs | model | song.end | built (.30); ported, legacy removed (.15) | .15 |
| `SEQ.song-lane-events` | 4 | arrangement | sv/song_state.rs | model | clip.events / num-steps / length | built (.30); ported, legacy removed (.15) | .15 |
| `SEQ.song-lanes` | 7 | arrangement, sound-palette | sv/song_state.rs | model | `t.clips` → clip | built (.30); ported (.15, .17: a bound clip's c.take / c.cell), legacy removed (.15) | .15 .17 |
| `SEQ.song-manual-latch` | 2 | transport | sv/song_state.rs | model | song.manual-latch | built (.30); ported, legacy removed (.12) | .12 |
| `SEQ.song-mode` | 2 | transport, arrangement | sv/song_state.rs | model | song.mode | built (.30); ported (.12, .15), legacy removed (.15) | .12 .15 |
| `SEQ.song-pending` | 8 | arrangement | sv/song_state.rs | model | song.pending, pending-origin, pending-head, pending-lanes → pending-lane, pending-scenes → pending-scene, pending-launches → pending-launch | built (.39); ported, legacy removed (.15) | .15 |
| `SEQ.song-position-beats` | 5 | arrangement, transport | sv/song_state.rs | live | song.position (live) | built (.30); ported (.12, .15), legacy removed (.15) | .12 .15 |
| `SEQ.song-region` | 31 | arrangement | sv/song_state.rs | model | song.region | built (.30); ported, legacy removed (.15) | .15 |
| `SEQ.song-scene-latched` | 1 | arrangement | sv/song_state.rs | model | song.scene-latched | built (.30); ported, legacy removed (.15) | .15 |
| `SEQ.song-track-governed` | 3 | sequencer | sv/song_state.rs | model | track.governed | built (.30); ported (.11), removed | .11 |
| `SEQ.song-track-latched` | 1 | arrangement | sv/song_state.rs | model | track.latched | built (.30); ported, legacy removed (.15) | .15 |
| `SEQ.track-pattern-cell-active-*` | 2 | mixer, arrangement | sv/steps_and_pattern.rs | model | cell.active | built (.30); ported (.13, .15), legacy removed (.15) | .13 .15 |
| `SEQ.track-pattern-cell-assigned-*` | 1 | mixer | sv/steps_and_pattern.rs | model | cell.assigned | built (.30); ported (.13), removed | .13 |
| `SEQ.track-pattern-cell-override-*` | 1 | mixer | sv/steps_and_pattern.rs | model | cell.override | built (.30); ported (.13), removed | .13 |
| `SEQ.track-pattern-cell-selected-*` | 1 | mixer | sv/steps_and_pattern.rs | model | cell.selected | built (.30); ported (.13), removed | .13 |
| `SEQ.track-pattern-cells` | 4 | mixer, arrangement | sv/track_and_mixer.rs | model | cell kind (track pid): `t.cells` | built (.30); ported (.13, .15), legacy removed (.15) | .13 .15 |
| `SEQ.focus-clip-end` | 3 | piano-roll | piano_roll.rs | model | piano-roll.clip.end (`clip`, 7d) | built (.31); ported (.16), removed | .16 |
| `SEQ.focus-clip-kind` | 5 | piano-roll | piano_roll.rs | model | piano-roll.clip-kind | built (.31); ported (.16), removed | .16 |
| `SEQ.focus-clip-offset` | 4 | piano-roll | piano_roll.rs | model | piano-roll.clip.offset | built (.31); ported (.16), removed | .16 |
| `SEQ.focus-clip-start` | 5 | piano-roll | piano_roll.rs | model | piano-roll.clip.start | built (.31); ported (.16), removed | .16 |
| `SEQ.focus-kind` | 6 | piano-roll | piano_roll.rs | model | piano-roll.focus-kind | built (.31); ported (.16), removed | .16 |
| `SEQ.focus-label` | 1 | piano-roll | piano_roll.rs | model | piano-roll.focus-label | built (.31); ported (.16), removed | .16 |
| `SEQ.focus-num-steps` | 1 | piano-roll | piano_roll.rs | model | piano-roll.focus-num-steps | built (.31); ported (.16), removed | .16 |
| `SEQ.focus-window-marker` | 1 | piano-roll | piano_roll.rs | model | piano-roll.window-marker | built (.31); ported (.16), removed | .16 |
| `SEQ.focus-window-repeat` | 1 | piano-roll | piano_roll.rs | model | piano-roll.window-repeat | built (.31); ported (.16), removed | .16 |
| `SEQ.focus-window-span` | 1 | piano-roll | piano_roll.rs | model | piano-roll.window-span | built (.31); ported (.16), removed | .16 |
| `SEQ.piano-roll-automation` | 1 | piano-roll | piano_roll.rs | model | view derivation: piano-roll.steps → focus-step (start, end, active, the step params) + param.step-locks / base, rack-macro.step-locks / base; selected param → piano-roll view singleton | built (.47); ported (.16), removed | .16 |
| `SEQ.piano-roll-automation-params` | 1 | piano-roll | piano_roll.rs | model | project.focus-step-params + param.has-locks / rack-macro.has-locks | built (.47); ported (.16), removed | .16 |
| `SEQ.piano-roll-items` | 8 | piano-roll | sv/topology_and_visualization.rs | model | piano-roll.notes → note | built (.31); ported (.16), removed | .16 |
| `SEQ.piano-roll-lanes` | 2 | piano-roll | natives.rs | model | view derivation from pitch-min / pitch-max (lane = pitch-max − note.pitch) | built (.31); ported (.16), removed | .16 |
| `SEQ.piano-roll-playhead` | 1 | piano-roll | piano_roll.rs | live | piano-roll.playhead (live) | built (.31); ported (.16), removed | .16 |
| `SEQ.piano-roll-selection` | 1 | piano-roll | piano_roll.rs | model | note.selected | built (.31); ported (.16), removed | .16 |
| `SEQ.track-automation` | 1 | packages/alez.tracker/src/ui | piano_roll.rs | model | param.has-locks / rack-macro.has-locks, track.step-params-in-use (the step params off their default) | built (.31); ported (.65), removed | .65 |
| `SEQ.track-grid-playhead-*` | 1 | packages/alez.tracker/src/ui | piano_roll.rs | live | track.playhead-row (live, compared in the row's shader) | built (.65); ported (.65), removed | .65 |
| `SEQ.track-grid-playhead-current` | 1 | packages/alez.tracker/src/ui | piano_roll.rs | live | selection.playhead-row (the current track's; the gutter's lamp binds it) | built (.65); ported (.65), removed | .65 |
| `SEQ.track-grid-playhead-row-current` | 1 | packages/alez.tracker/src/ui | piano_roll.rs | live | `#'selection.playhead-row` (the scroll's follow) | built (.65); ported (.65), removed | .65 |
| `SEQ.track-lock-targets` | 1 | packages/alez.tracker/src/ui | piano_roll.rs | model | t.devices / t.midi-devices → d.params, device.macros (rack-macro) | built (.31); ported (.65), removed | .65 |
| `SEQ.tracker-rows` | 1 | packages/alez.tracker/src/ui | piano_roll.rs | model | step.active / transpose / velocity / ‹param› + param.step-locks / rack-macro.step-locks | built (.31); ported (.65), removed | .65 |
| `AGENT.generation` | 2 | agent | browser.rs | model | agent.generation | built (.32); ported, legacy removed (.76) | .76 |
| `AUDIO.workers-choice` | 1 | settings | host_commands/audio_settings.rs | model | settings.audio-workers-choice | built (.32); ported, legacy removed (.18: all of `AUDIO`) | .18 |
| `AUDIO.workers-note` | 1 | settings | host_commands/audio_settings.rs | model | settings.audio-workers-note | built (.32); ported, legacy removed (.18) | .18 |
| `AUDIO.workers-options` | 1 | settings | host_commands/audio_settings.rs | model | project.audio-workers-options (an option list, §14.1) | built (.32); ported, legacy removed (.18) | .18 |
| `EXPORT.export-busy` | 4 | export-song | host_commands/export.rs | model | song-export.busy | built (.32); ported, legacy removed (.76) | .76 |
| `EXPORT.export-default-name` | 1 | export-song | host_commands/export.rs | model | song-export.default-name | built (.32); ported, legacy removed (.76) | .76 |
| `EXPORT.export-done` | 3 | export-song | host_commands/export.rs | model | song-export.done | built (.32); ported, legacy removed (.76) | .76 |
| `EXPORT.export-end` | 1 | export-song | host_commands/export.rs | model | song-export.end | built (.32); ported, legacy removed (.76) | .76 |
| `EXPORT.export-folder` | 1 | export-song | host_commands/export.rs | model | song-export.folder | built (.32); ported, legacy removed (.76) | .76 |
| `EXPORT.export-message` | 2 | export-song | host_commands/export.rs | model | song-export.message | built (.32); ported, legacy removed (.76) | .76 |
| `EXPORT.export-output-name` | 1 | export-song | host_commands/export.rs | model | song-export.output-name | built (.32); ported, legacy removed (.76) | .76 |
| `EXPORT.export-percent` | 1 | export-song | host_commands/export.rs | model | song-export.percent | built (.32); ported, legacy removed (.76) | .76 |
| `EXPORT.export-project` | 1 | export-song | host_commands/export.rs | model | song-export.project | built (.32); ported, legacy removed (.76) | .76 |
| `EXPORT.export-reveal-label` | 1 | export-song | host_commands/export.rs | model | song-export.reveal-label | built (.32); ported, legacy removed (.76) | .76 |
| `FACTORY_PROMOTE.*` | 11 | factory-promote | host_commands/factory_promote.rs | model | factory-promote.target (was `kind`) / destination / skipped / blocking / error / taken | missed by the stage 7 inventory; built, ported, legacy removed (.76) | .76 |
| `MIDI.devices` | 1 | settings | midi_dispatch.rs | model | settings.midi-devices → midi-device (by device id) | built (.32); ported, legacy removed (.18; `MIDI.ports` stays: the dispatch's port identities) | .18 |
| `MIDI.error` | 1 | settings | lisp_host/eseq/expr_process.rs | model | settings.midi-error | built (.32); ported, legacy removed (.18) | .18 |
| `MIDI.persistent` | 1 | settings | midi_dispatch.rs | model | settings.midi-persistent | built (.32); ported, legacy removed (.18) | .18 |
| `RETRO.duration` | 9 | retrospective | lisp_host/value_helpers.rs | model | retro.duration | built (.32); ported, legacy removed (.12) | .12 |
| `RETRO.error` | 3 | retrospective | lisp_host/eseq/expr_process.rs | model | retro.error | built (.32); ported, legacy removed (.12) | .12 |
| `RETRO.items` | 5 | retrospective | agent/network.rs | model | retro.items → retro-item | built (.32); ported, legacy removed (.12) | .12 |
| `RETRO.lanes` | 2 | retrospective | retrospective.rs | model | retro.lanes → retro-lane | built (.32); ported, legacy removed (.12) | .12 |
| `RETRO.playing` | 5 | retrospective | sequencer/state/sequencer_state/scene_launch.rs | live | retro.playing (live) | built (.32); ported, legacy removed (.12) | .12 |
| `RETRO.position` | 1 | retrospective | retrospective.rs | live | retro.playhead (live, .12: the host maps the loop position onto the crop) | built (.32); ported, legacy removed (.12) | .12 |
| `RETRO.truncated` | 1 | retrospective | retrospective.rs | model | retro.truncated | built (.32); ported, legacy removed (.12) | .12 |
| `SEQ.browser-preview-playhead` | 3 | sample-import, browser, resample | reactive_tick.rs | live | browser.preview-position (live) | built (.32); ported, legacy removed (.17) | .17 |
| `SEQ.browser-preview-playing` | 4 | browser, sample-import, resample | reactive_tick.rs | model | browser.preview-playing (live) | built (.32); ported, legacy removed (.17) | .17 |
| `SEQ.content-library-epoch` | 2 | browser | lisp_hot_reload.rs | model | browser.library-epoch (the library trees are natives taking a search filter, so a read of the epoch re-lists them) | built (.32); ported, legacy removed (.17) | .17 |
| `SEQ.current-pNroject-name` | 1 | browser | - | model | project.name (typo in browser.lisp) | built (.32); ported (.17): the typo read an unregistered name | .17 |
| `SEQ.current-project-name` | 4 | browser, application-menus | sv/project_state.rs | model | project.name | built (.32); ported (.17), kept: application-menus; ported, legacy removed (.18) | .17 .18 |
| `SEQ.editor-active-macro-action` | 3 | browser | reactive_tick.rs | model | editor.active-macro-action | built (.32); ported, legacy removed (.17) | .17 |
| `SEQ.editor-active-macro-name` | 1 | browser | reactive_tick.rs | model | editor.active-macro | built (.32); ported, legacy removed (.17) | .17 |
| `SEQ.editor-assets` | 2 | patch-macros | reactive_tick.rs | model | editor.assets → editor-asset | built (.32); ported, legacy removed (.18) | .18 |
| `SEQ.editor-buffer-name` | 1 | browser | event_loop.rs | model | editor.buffer | built (.32); ported, legacy removed (.17) | .17 |
| `SEQ.editor-canceling` | 4 | browser | event_loop.rs | model | editor.canceling | built (.32); ported, legacy removed (.17) | .17 |
| `SEQ.editor-error` | 5 | browser | event_loop.rs | model | editor.error | built (.32); ported, legacy removed (.17) | .17 |
| `SEQ.editor-instrument-run-mode` | 4 | browser | host_commands/instrument_authoring.rs | model | editor.run-mode | built (.32); ported, legacy removed (.17) | .17 |
| `SEQ.editor-library-macros` | 2 | patch-macros | reactive_tick.rs | model | editor.library-macros → editor-macro | built (.32); ported, legacy removed (.18) | .18 |
| `SEQ.editor-mode` | 18 | browser, seq-panels | event_loop.rs | model | editor.mode | built (.32); ported (.17), kept: seq-panels, scale-editor; legacy removed (.76: the last reader, the Shift+Tab shortcut in `input.rs`, reads the presented editor) | .11 .17 .76 |
| `SEQ.editor-open-macro` | 2 | patch-macros | reactive_tick.rs | model | editor.open-macro | built (.32); ported, legacy removed (.18) | .18 |
| `SEQ.editor-patch-macros` | 4 | patch-macros | reactive_tick.rs | model | editor.patch-macros → editor-macro | built (.32); ported, legacy removed (.18) | .18 |
| `SEQ.editor-selected-asset` | 1 | patch-macros | reactive_tick.rs | model | editor.selected-asset → asset-info (nil: none) | built (.32); ported, legacy removed (.18) | .18 |
| `SEQ.editor-surface` | 3 | browser | host_commands/instrument_authoring.rs | model | editor.surface | built (.32); ported, legacy removed (.17) | .17 |
| `SEQ.kit-presets` | 2 | browser | host_commands/drum_rack_v2.rs | model | browser.kit-presets → preset-file | built (.32); ported, legacy removed (.17) | .17 |
| `SEQ.learn-abs-distance` | 1 | patch-learn | patch_learn.rs | model | learn.abs-distance | built (.32); ported, legacy removed (.18) | .18 |
| `SEQ.learn-applied` | 1 | patch-learn | patch_learn.rs | model | learn.applied | built (.32); ported, legacy removed (.18) | .18 |
| `SEQ.learn-basin-check` | 1 | patch-learn | patch_learn.rs | model | learn.basin-check | built (.32); ported, legacy removed (.18) | .18 |
| `SEQ.learn-cma-continue` | 2 | patch-learn | host_commands/learn.rs | model | learn.cma-continue | built (.32); ported, legacy removed (.18) | .18 |
| `SEQ.learn-cma-final-epochs` | 2 | patch-learn | host_commands/learn.rs | model | learn.cma-final-epochs | built (.32); ported, legacy removed (.18) | .18 |
| `SEQ.learn-cma-forward-batch` | 2 | patch-learn | host_commands/learn.rs | model | learn.cma-forward-batch | built (.32); ported, legacy removed (.18) | .18 |
| `SEQ.learn-cma-generations` | 3 | patch-learn | host_commands/learn.rs | model | learn.cma-generations | built (.32); ported, legacy removed (.18) | .18 |
| `SEQ.learn-cma-population` | 6 | patch-learn | host_commands/learn.rs | model | learn.cma-population | built (.32); ported, legacy removed (.18) | .18 |
| `SEQ.learn-cma-refine-epochs` | 2 | patch-learn | host_commands/learn.rs | model | learn.cma-refine-epochs | built (.32); ported, legacy removed (.18) | .18 |
| `SEQ.learn-cma-refine-mode` | 2 | patch-learn | host_commands/learn.rs | model | learn.cma-refine-mode | built (.32); ported, legacy removed (.18) | .18 |
| `SEQ.learn-cma-seed` | 2 | patch-learn | host_commands/learn.rs | model | learn.cma-seed | built (.32); ported, legacy removed (.18) | .18 |
| `SEQ.learn-cma-sigma` | 2 | patch-learn | host_commands/learn.rs | model | learn.cma-sigma | built (.32); ported, legacy removed (.18) | .18 |
| `SEQ.learn-current-epoch` | 1 | patch-learn | patch_learn.rs | model | learn.current-epoch | built (.32); ported, legacy removed (.18) | .18 |
| `SEQ.learn-epoch-params` | 1 | patch-learn | patch_learn.rs | model | learn.epoch-params → learn-epoch-param | built (.32); ported, legacy removed (.18) | .18 |
| `SEQ.learn-epochs` | 2 | patch-learn | host_commands/learn.rs | model | learn.epochs | built (.32); ported, legacy removed (.18) | .18 |
| `SEQ.learn-error` | 1 | patch-learn | patch_learn.rs | model | learn.error | built (.32); ported, legacy removed (.18) | .18 |
| `SEQ.learn-final-wav` | 1 | patch-learn | patch_learn.rs | model | learn.final-wav | built (.32); ported, legacy removed (.18) | .18 |
| `SEQ.learn-gate-frames` | 4 | patch-learn | patch_learn.rs | model | learn.gate-frames | built (.32); ported, legacy removed (.18) | .18 |
| `SEQ.learn-improvement-pct` | 1 | patch-learn | patch_learn.rs | model | learn.improvement-pct | built (.32); ported, legacy removed (.18) | .18 |
| `SEQ.learn-local-epochs` | 2 | patch-learn | host_commands/learn.rs | model | learn.local-epochs | built (.32); ported, legacy removed (.18) | .18 |
| `SEQ.learn-loss` | 1 | patch-learn | patch_learn.rs | model | learn.loss | built (.32); ported, legacy removed (.18) | .18 |
| `SEQ.learn-losses` | 1 | patch-learn | patch_learn.rs | model | learn.losses | built (.32); ported, legacy removed (.18) | .18 |
| `SEQ.learn-method` | 7 | patch-learn | host_commands/learn.rs | model | learn.method | built (.32); ported, legacy removed (.18) | .18 |
| `SEQ.learn-optimization-losses` | 1 | patch-learn | patch_learn.rs | model | learn.optimization-losses | built (.32); ported, legacy removed (.18) | .18 |
| `SEQ.learn-phase` | 5 | patch-learn | patch_learn.rs | model | learn.phase | built (.32); ported, legacy removed (.18) | .18 |
| `SEQ.learn-pitch-hz` | 4 | patch-learn | patch_learn.rs | model | learn.pitch-hz | built (.32); ported, legacy removed (.18) | .18 |
| `SEQ.learn-plan-params` | 2 | patch-learn | patch_learn.rs | model | learn.plan-params → learn-plan-param | built (.32); ported, legacy removed (.18) | .18 |
| `SEQ.learn-result-deltas` | 1 | patch-learn | patch_learn.rs | model | learn.result-deltas → learn-delta | built (.32); ported, legacy removed (.18) | .18 |
| `SEQ.learn-seeded-wav` | 1 | patch-learn | patch_learn.rs | model | learn.seeded-wav | built (.32); ported, legacy removed (.18) | .18 |
| `SEQ.learn-stage` | 1 | patch-learn | patch_learn.rs | model | learn.stage | built (.32); ported, legacy removed (.18) | .18 |
| `SEQ.learn-target-name` | 3 | patch-learn | host_commands/learn.rs | model | learn.target-name | built (.32); ported, legacy removed (.18) | .18 |
| `SEQ.learn-target-path` | 3 | patch-learn | host_commands/learn.rs | model | learn.target-path | built (.32); ported, legacy removed (.18) | .18 |
| `SEQ.learn-total-epochs` | 1 | patch-learn | patch_learn.rs | model | learn.total-epochs | built (.32); ported, legacy removed (.18) | .18 |
| `SEQ.project-instrument-engines` | 1 | browser | sv/project_state.rs | model | browser.engines | built (.32); ported, legacy removed (.17) | .17 |
| `SEQ.sidebar-instrument-display-name` | 2 | browser | sv/project_state.rs | model | browser.instrument-label | built (.32); ported, legacy removed (.17) | .17 |
| `SEQ.sidebar-instrument-name` | 5 | browser, application-menus, effects/panel-frame | sv/project_state.rs | model | browser.instrument | built (.32); ported (.17), kept: application-menus, panel-frame; ported (.14 A: panel-frame reads browser.instrument), kept: application-menus; ported, legacy removed (.18: application menus read browser.instrument) | .14 .17 .18 |
| `SEQ.sidebar-kind` | 7 | browser | sv/project_state.rs | model | browser.instrument-kind (`kind` is a built-in field; "sampler", "instrument" or "empty", never "rack": the rack checks read browser.track.rack) | built (.32); ported, legacy removed (.17) | .17 |
| `SEQ.sidebar-loaded-preset` | 3 | browser | sv/project_state.rs | model | browser.preset | built (.32); ported, legacy removed (.17) | .17 |
| `SEQ.sidebar-presets` | 1 | browser | sv/project_state.rs | model | browser.presets | built (.32); ported, legacy removed (.17) | .17 |
| `SEQ.sidebar-rack-slot-presets` | 1 | browser | sv/project_state.rs | model | browser.rack-slots → slot-presets (each names its rack slot device) | built (.32); ported, legacy removed (.17) | .17 |
| `SEQ.sidebar-selected-sample` | 7 | browser | sv/project_state.rs | model | browser.sample | built (.32); ported, legacy removed (.17) | .17 |
| `SEQ.sidebar-track-index` | 4 | browser | sv/project_state.rs | model | browser.track | built (.32); ported, legacy removed (.17) | .17 |
| `SEQ.sidebar-user-presets` | 1 | browser | sv/project_state.rs | model | browser.user-presets | built (.32); ported, legacy removed (.17) | .17 |
| `SEQ.sound-palette` | 7 | sound-palette | sv/sound_palette.rs | model | sound-palette singleton; sound-palette.sounds → sound (track, patch-id) | built (.32); ported, legacy removed (.17) | .17 |
| `SEQ.sound-presets` | 2 | browser | sv/project_state.rs | model | browser.sound-presets → preset-file | built (.32); ported, legacy removed (.17) | .17 |
| `SEQ.track-instrument-ids` | 1 | browser | sv/track_and_mixer.rs | model | track.instrument-id | built (.32); ported, legacy removed (.17) | .17 |
| `GRAPH.<ggm-route-color-field>` | 4 | scripts/sequencers/graph-neural-group-matrix-demo | lisp_host/eseq/graph_authoring.rs (+ Lisp writes) | model | n.route.color (view derivation from graph-node.route) | built (.33); ported (.64), removed with the demo's writes | .20 .64 |
| `GRAPH.<gvr-route-color-field>` | 4 | scripts/sequencers/graph-neural-variable-reset-demo | lisp_host/eseq/graph_authoring.rs (+ Lisp writes) | model | n.route.color (view derivation from graph-node.route) | built (.33); ported (.64), removed with the demo's writes | .20 .64 |
| `SEQ.<neural->` | 8 | scripts/sequencers/neural-8x8-track-router | sv/topology_and_visualization.rs | live | neuron.selected (the native neural engine; `nr.selected`, setter `neural-set-neuron-selected`) | built (.50); ported (.20), removed | .20 |
| `SEQ.generator-mark-*` | 4 | packages/alez.jaki/src/kind | sv/meters_and_modulation.rs | live | `generator-mark-<id>[-<key>]` → `(generator-mark-named (generator-of self) key).value` (live; key `""` unkeyed) | built (.52); ported (.20), removed | .20 |
| `SEQ.graph-sequencers` | 1 | mixer | reactive_tick.rs | model | project.graphs → graph (gid, name, owner) | built (.33); ported (.13), removed | .13 |
| `SEQ.graph-visualizations` | 14 | scripts/sequencers/graph-neural-variable-reset-demo, packages/alez.neural/src/variable-reset, scripts/sequencers/graph-neural-16-demo +5 | sv/topology_and_visualization.rs | model | graph.active / beat / energy / triggers / dampening (live), weights → graph-param.value; event-history → graph.events, node-events → graph.node-events (events: its non-empty rows), delta-matrix → graph.deltas, node-delta-column → graph.node-deltas, group-activity / group-suppression-matrix → graph.group-activity / group-suppression (live, built .51) | built (.33, .51); ported (.64: the seven graph demos), kept: alez.neural; ported (.67: alez.neural), removed | .20 .64 .67 |
| `SEQ.neural-dampening-matrix` | 1 | scripts/sequencers/neural-8x8-track-router | sv/topology_and_visualization.rs | live | neuron.dampening (live; the matrix is `(map (lambda (nr) nr.dampening) nw.neurons)`) | built (.50); ported (.20), removed | .20 |
| `SEQ.neural-energy-matrix` | 1 | scripts/sequencers/neural-8x8-track-router | sv/topology_and_visualization.rs | live | neuron.energy (live; one per neuron) | built (.50); ported (.20), removed | .20 |
| `SEQ.neural-networks` | 1 | scripts/sequencers/neural-8x8-track-router | sv/topology_and_visualization.rs | model | project.networks → network / neuron | built (.50); ported (.20), removed | .20 |
| `SEQ.neural-trigger-matrix` | 1 | scripts/sequencers/neural-8x8-track-router | sv/topology_and_visualization.rs | live | neuron.trigger (live; one per neuron) | built (.50); ported (.20), removed | .20 |
| `SEQ.track-active-notes` | 5 | effects/panel-bodies, scripts/sequencers/graph-neural-8x8-demo, scripts/sequencers/graph-neural-variable-reset-demo +2 | reactive_tick.rs | live | track.active-notes (live; `(note velocity trigger-id)` rows) | built (.33); ported (.14 A: panel-bodies reads t.active-notes; .64: the graph demos), kept: alez.neural, panel_kinds_seed; ported (.67: alez.neural; panel_kinds_seed seeds track.active-notes), removed | .14 .20 .64 .67 |
| `SEQ.track-event-current-beat` | 3 | scripts/processes/process-ui-control-demo, scripts/sequencers/band-coupling-matrix-demo, scripts/sequencers/graph-neural-8x8-demo | ui_replay_probe.rs | live | transport.track-events-beat (live) | built (.51); ported (.20: process-ui-control-demo, band-coupling-matrix-demo; .64: graph-neural-8x8-demo), removed (.64) | .20 .64 |
| `SEQ.track-events` | 3 | scripts/processes/process-ui-control-demo, scripts/sequencers/band-coupling-matrix-demo, scripts/sequencers/graph-neural-8x8-demo | ui_replay_probe.rs | model | transport.track-events (live, positional rows) | built (.51); ported (.20: process-ui-control-demo, band-coupling-matrix-demo; .64: graph-neural-8x8-demo), removed (.64) | .20 .64 |
| `SEQ.<rack/groove-amount-field>` | 1 | rack-groove-buffer | sv/rack_groove_fields.rs | model | groove.timing / velocity / random; a pad's share pad-groove.amount (of the playing clip's groove: `(or g.rack-clip.groove g.groove)`) | built (.34); ported (.19), removed | .19 |
| `SEQ.armed-rack-id` | 2 | mixer, drum-rack-v2 | reactive_tick.rs | model | group.armed (live) | built (.34); ported (.13), kept: drum-rack-v2; ported (.19), removed (publisher, `prev_armed_rack`, registration) | .13 .19 |
| `SEQ.groove-pool` | 2 | rack-groove-buffer | sv/rack_groove_fields.rs | model | project.groove-pool (pool-groove) | built (.34); ported (.19), removed (with `SEQ.groove-library`) | .19 |
| `SEQ.rack-clip-active-*` | 3 | sequencer, mixer | sv/topology_and_visualization.rs | model | rack-clip.active | built (.34); ported (.13), kept: sequencer; ported (.11), removed | .11 .13 |
| `SEQ.rack-clip-banks` | 1 | drum-rack-v2 | sv/topology_and_visualization.rs | model | group.clips (rack-clip.cid, name) | built (.34); ported (.19), removed | .19 |
| `SEQ.rack-clip-index-*` | 1 | sequencer | sv/topology_and_visualization.rs | model | group.rack-clip (an instance, nil while silent; its position rc.index) | built (.34); ported (.11), removed | .11 |
| `SEQ.rack-clips` | 2 | mixer, drum-rack-v2 | sv/topology_and_visualization.rs | model | rack-clip kind: group.clips, rack-clip.scenes (:scene-clips), group.legacy (no entry) | built (.34); ported (.13), kept: drum-rack-v2; ported (.19), removed (`sync_rack_clip_state`, registrations) | .13 .19 |
| `SEQ.rack-grooves` | 1 | drum-rack-v2 | sv/rack_groove_fields.rs | model | groove kind (group.groove, rack-clip.groove; lanes, pad-groove shares); the picker: project.groove-pool, project.groove-library | built (.34); ported (.19), removed | .19 |
| `SEQ.rack-pad-trigger-*` | 3 | sequencer | sv/drum_rack.rs | live | pad.triggered (live; a member track's pad: t.pad) | built (.34); ported (.66), removed | .11 .66 |
| `SEQ.track-steps` | 2 | rack-groove-buffer | sv/param_fields_and_sync.rs | model | pad.track.steps → step.active | built (.34); ported (.19), kept: no content reader left; removed with the per-track step lists (.22) | .19 |
| `SEQ.<slot-bar-transpose-field>` | 1 | sequencer | sv/expanded_step.rs | model | track.bar-transposes | built (.35); ported (.66), removed | .11 .66 |
| `SEQ.<slot-bar-transpose-set-field>` | 1 | sequencer | sv/expanded_step.rs | model | track.bar-transposes (≠ 0; `set-bar-transpose!`) | built (.35); ported (.66), removed | .11 .66 |
| `SEQ.accum-mode-options` | 1 | effects/track-panels | sv/project_state.rs | model | constant | built (.35); ported (.61), legacy removed (publisher, registration) | .14 .61 |
| `SEQ.accumulator-options` | 1 | effects/track-panels | sv/project_state.rs | model | project.accumulator-options | built (.35); ported (.61), legacy removed (publisher, registration) | .14 .61 |
| `SEQ.auto-follow` | 2 | seq-core-state, sequencer | reactive_tick.rs | model | selection.auto-follow | built (.35) | .11 |
| `SEQ.bus-effects` | 3 | application-menus, effects/buffers, effects/panel-widgets | event_loop.rs | model | bus.devices | built (.36); ported (.14 A: panel-widgets counts (buses)), kept: buffers, application-menus; ported (.18: application menus read (buses) b.devices), kept: buffers; kept as structure (.61; eseq-0l17.22): buffers; kept (.74): `buffers` is ported by eseq-0l17.19 (the drum rack lane); the panels' layout from kinds follows it (eseq-0l17.22); ported (.82: buffers builds each bus effect panel from (buses) b.devices inside its subtree), legacy removed (`build_bus_effects_value*`, publisher, registration) | .14 .18 .61 .74 .82 |
| `SEQ.bus-mod-in-level-*` | 1 | mixer | sv/meters_and_modulation.rs | live | bus.mod-in-1 … -4 (live; `(mod-in-level b i)`) | built (.35); ported (.13), removed | .13 |
| `SEQ.bus-output-routes` | 1 | mixer | sv/track_and_mixer.rs | model | bus.output, bus.output-options | built (.35); ported (.13), removed | .13 |
| `SEQ.compiling` | 1 | effects/buffers | sv/host_commands.rs | model | engine.compiling | built (.35); ported (.61: buffers read engine.compiling); legacy removed (.74: publishers, registration, test seeds) | .14 .61 .74 |
| `SEQ.cpu-overloaded` | 2 | transport | reactive_tick.rs | live | engine.overloaded | built (.35); ported, legacy removed (.12) | .12 |
| `SEQ.fts-options` | 2 | effects/track-panels, effects/scale-editor | sv/project_state.rs | model | project.fts-options | built (.35); ported (.61), legacy removed (publisher, registration) | .14 .61 |
| `SEQ.fx-step-cursor-number` | 1 | effects/track-panels | sv/param_fields_and_sync.rs | model | selection.cursor-step (index + 1) | built (.35); ported (.61), kept: seq-core-state | .14 .61 |
| `SEQ.fx-step-parameter-step` | 1 | seq-core-state | sv/topology_and_visualization.rs | model | selection.edit-step | built (.35); ported (.66), kept: track-panels (.14) | .11 .66 |
| `SEQ.fx-step-selection-count` | 2 | effects/track-panels, seq-core-state | sv/param_fields_and_sync.rs | model | (len selection.steps) | built (.35); ported (.61), kept: seq-core-state | .11 .14 .61 |
| `SEQ.fx-step-value-*` | 1 | effects/track-panels | step_print.rs | model | step.‹param› of selection.edit-step | built (.35); ported (.61: the pickers bind the edit step's fields, which show the print latch; `step_print`'s latch writers removed), kept: seq-core-state | .14 .61 |
| `SEQ.midi-effects` | 1 | effects/buffers | event_loop.rs | model | track.midi-devices | built (.36); kept as structure (.61; eseq-0l17.22): buffers; kept (.74): `buffers` is ported by eseq-0l17.19 (the drum rack lane); the panels' layout from kinds follows it (eseq-0l17.22); ported (.82: buffers builds the MIDI effect panels from track.midi-devices), legacy removed (`build_midi_effects_value`, publisher, registration) | .14 .61 .74 .82 |
| `SEQ.mixer-track-delete-target-*` | 1 | mixer | sv/steps_and_pattern.rs | model | track.delete-target | built (.35); ported (.13), removed | .13 |
| `SEQ.mod-in-level-*` | 1 | mixer | sv/meters_and_modulation.rs | live | track.mod-in-1 … -4 (live; `(mod-in-level t i)`) | built (.35); ported (.13), removed | .13 |
| `SEQ.mod-out-level-*` | 1 | mixer | sv/meters_and_modulation.rs | live | track.mod-out-level (live) | built (.35); ported (.13), removed | .13 |
| `SEQ.mod-routes` | 6 | mixer | reactive_sync.rs | model | route kind, `(routes)` | built (.35); ported (.13), removed | .13 |
| `SEQ.mute-group-options` | 1 | effects/track-panels | sv/project_state.rs | model | constant | built (.35); ported (.61), legacy removed (publisher, registration) | .14 .61 |
| `SEQ.rack-slot-delete-target-*` | 1 | effects/instrument-panel | sv/steps_and_pattern.rs | model | device.delete-target | built (.36); ported (.61: the rack slot row binds #'sd.delete-target), legacy removed (publisher `sync_mixer_delete_target_binding_fields`) | .14 .61 |
| `SEQ.roll-rate` | 1 | transport | reactive_tick.rs | live | transport.roll-rate | built (.35); ported, legacy removed (.12) | .12 |
| `SEQ.selected-mod-routes` | 2 | mixer | sv/steps_and_pattern.rs | model | route.selected | built (.35); ported (.13), removed | .13 |
| `SEQ.sequence-rolling` | 1 | transport | reactive_tick.rs | live | transport.sequence-rolling | built (.35); ported, legacy removed (.12) | .12 |
| `SEQ.sync-labels` | 8 | sequencer, step-grid, seqv-track-params +1 | natives.rs | model | project.sync-options | built (.35); ported (.66), kept: step-grid (unloaded), .22 | .11 .66 |
| `SEQ.tp-accum-limit` | 1 | effects/track-panels | sv/project_state.rs | model | track.accum-limit | built (.35); ported (.61), legacy removed (publisher, registration) | .14 .61 |
| `SEQ.tp-accum-mode` | 1 | effects/track-panels | sv/project_state.rs | model | track.accum-mode | built (.35); ported (.61), legacy removed (publisher, registration) | .14 .61 |
| `SEQ.tp-accumulator` | 1 | effects/track-panels | sv/project_state.rs | model | track.accumulator | built (.35); ported (.61), legacy removed (publisher, registration) | .14 .61 |
| `SEQ.tp-fts` | 2 | effects/track-panels, effects/scale-editor | sv/project_state.rs | model | track.fts | built (.35); ported (.61), legacy removed (publisher, registration) | .14 .61 |
| `SEQ.tp-gate` | 4 | effects/sampler-panel | sv/project_state.rs | model | track.gate | built (.35); ported (.61), legacy removed (publisher, registration) | .14 .61 |
| `SEQ.tp-max-polyphony` | 2 | mixer, effects/track-panels | host_commands/rack.rs | model | track.max-polyphony (the track's own; a rack slot's: device.voices) | built (.35, .36); ported (.13), kept: track-panels; ported (.61), legacy removed (publisher, registration) | .13 .14 .61 |
| `SEQ.tp-mono-trigger` | 1 | effects/track-panels | sv/project_state.rs | model | track.mono-trigger | built (.35); ported (.61), legacy removed (publisher, registration) | .14 .61 |
| `SEQ.tp-mute-group` | 1 | effects/track-panels | sv/project_state.rs | model | track.mute-group (`:int`; label `(nth mute-group-options g)`) | built (.35); ported (.61), legacy removed (publisher, registration) | .14 .61 |
| `SEQ.tp-poly` | 12 | effects/track-panels, mixer, effects/instrument-panel | sv/project_state.rs | model | track.poly (the track's own; a rack slot's: (> device.voices 1)) | built (.35, .36); ported (.13), kept: track-panels; ported (.61), legacy removed (publisher, registration) | .13 .14 .61 |
| `SEQ.tp-rack-slot-idx` | 5 | effects/track-panels, mixer | sv/project_state.rs | model | selection.rack-slot (-1: no rack) | built (.35); ported (.13), kept: track-panels; ported (.61), legacy removed (publisher, registration) | .13 .14 .61 |
| `SEQ.tp-supports-mono-trigger` | 2 | effects/track-panels | sv/project_state.rs | model | track.supports-mono-trigger | built (.35); ported (.61), legacy removed (publisher, registration) | .14 .61 |
| `SEQ.tp-swing` | 2 | effects/track-panels | sv/project_state.rs | model | track.swing | built (.35); ported (.61), legacy removed (publisher, registration) | .14 .61 |
| `SEQ.tp-swing-resolution` | 1 | effects/track-panels | sv/project_state.rs | model | track.swing-resolution | built (.35); ported (.61), legacy removed (publisher, registration) | .14 .61 |
| `SEQ.tp-tuning-base` | 1 | effects/scale-editor | sv/param_fields_and_sync.rs | model | degree.base of track.tuning.degrees | built (.35); ported (.61), legacy removed (publisher, registration) | .14 .61 |
| `SEQ.tp-tuning-enabled` | 1 | effects/scale-editor | sv/param_fields_and_sync.rs | model | degree.enabled [s] of track.tuning.degrees | built (.35); ported (.61), legacy removed (publisher, registration) | .14 .61 |
| `SEQ.tp-tuning-labels` | 1 | effects/scale-editor | sv/param_fields_and_sync.rs | model | degree.label of track.tuning.degrees | built (.35); ported (.61), legacy removed (publisher, registration) | .14 .61 |
| `SEQ.tp-tuning-mode` | 4 | effects/scale-editor | sv/param_fields_and_sync.rs | model | tuning.mode (`set!`) | built (.35); ported (.61), legacy removed (publisher, registration) | .14 .61 |
| `SEQ.tp-tuning-morph` | 1 | effects/scale-editor | sv/param_fields_and_sync.rs | model | tuning.morph (`set!`; 0-1; the legacy field is percent) | built (.35); ported (.61), legacy removed (publisher, registration) | .14 .61 |
| `SEQ.tp-tuning-offsets` | 1 | effects/scale-editor | sv/param_fields_and_sync.rs | model | degree.offset (`set!`) | built (.35); ported (.61), legacy removed (publisher, registration) | .14 .61 |
| `SEQ.tp-tuning-on` | 1 | effects/scale-editor | sv/param_fields_and_sync.rs | model | tuning.on | built (.35); ported (.61), legacy removed (publisher, registration) | .14 .61 |
| `SEQ.tp-tuning-period` | 1 | effects/scale-editor | sv/param_fields_and_sync.rs | model | tuning.period | built (.35); ported (.61), legacy removed (publisher, registration) | .14 .61 |
| `SEQ.tp-tuning-pitches` | 1 | effects/scale-editor | sv/param_fields_and_sync.rs | model | degree.pitch of track.tuning.degrees | built (.35); ported (.61), legacy removed (publisher, registration) | .14 .61 |
| `SEQ.tp-tuning-root` | 1 | effects/scale-editor | sv/param_fields_and_sync.rs | model | tuning.root (`set!`) | built (.35); ported (.61), legacy removed (publisher, registration) | .14 .61 |
| `SEQ.tp-voice-priority` | 1 | effects/track-panels | sv/project_state.rs | model | track.voice-priority | built (.35); ported (.61), legacy removed (publisher, registration) | .14 .61 |
| `SEQ.track-mod-output-available` | 2 | mixer | sv/track_and_mixer.rs | model | track.mod-output | built (.35); ported (.13), removed | .13 |
| `SEQ.track-output-options` | 1 | mixer | host_commands/routing.rs | model | project.output-options (bus instances; nil is sends only) | built (.35); ported (.13), removed | .13 |
| `SEQ.track-outputs` | 1 | mixer | sv/track_and_mixer.rs | model | track.output (a bus; nil is sends only) | built (.35); ported (.13), removed | .13 |
| `SEQ.tuning-root-options` | 1 | effects/scale-editor | sv/project_state.rs | model | constant | built (.35); ported (.61), legacy removed (publisher, registration) | .14 .61 |
| `SEQV.<adsr-stage-active-field>` | 1 | effects/custom-ui-sections | Lisp (reactive-set) | Lisp-owned | custom-ui view state | view-local; ported (.14 A: the view-local adsr-gesture kind, keyed by scope and section, in eseq.effects.custom-ui-sections; .73) | .14 |
| `SEQV.<channel>` | 20 | arrangement | Lisp (reactive-set) | Lisp-owned | arrangement view singleton (arr-*) | view-local; ported (.15: arr-select, arr-lanes, arr-drag, with the timeline's lane ownership), removed | .15 |
| `SEQV.<cursor-highlight-field>` | 1 | sequencer | Lisp (reactive-set) | Lisp-owned | sequencer view singleton (cursor) | view-local; ported (.11), removed | .11 |
| `SEQV.<expanded-track-field>` | 1 | sequencer | Lisp (reactive-set) | Lisp-owned | sequencer view singleton (expanded tracks) | view-local; ported (.11), removed | .11 |
| `SEQV.<sel-bus-vis-field>` | 1 | seq-core-state | Lisp (reactive-set) | Lisp-owned | bus-highlight.selected (view-local, keyed by bus; `*sel-sync*` writes it) | view-local; ported (.77: `bus-selected-ref`), removed | .11 .77 |
| `SEQV.<sel-group-vis-field>` | 1 | seq-core-state | Lisp (reactive-set) | Lisp-owned | bus-highlight.selected of the group's bus | view-local; ported (.77: `group-selected-ref`), removed | .11 .77 |
| `SEQV.alez.tracker/*` | 4 | packages/alez.tracker/src/ui | Lisp (eseq.bindings channels) | Lisp-owned | tracker view singleton (`tracker-cursor`, compared in the cells' shaders) | view-local; ported (.65), removed (eseq.bindings has no reader left; .22) | .65 |
| `SEQV.arr-content-length` | 3 | arrangement | Lisp (reactive-set) | Lisp-owned | arrangement view singleton | view-local; ported (.15: arr-view), removed | .15 |
| `SEQV.arr-view-duration` | 3 | arrangement | Lisp (reactive-set) | Lisp-owned | arrangement view singleton | view-local; ported (.15: arr-view), removed | .15 |
| `SEQV.arr-view-start` | 3 | arrangement | Lisp (reactive-set) | Lisp-owned | arrangement view singleton | view-local; ported (.15: arr-view), removed | .15 |
| `SEQV.cursor-field-*` | 1 | sequencer | Lisp (reactive-set) | Lisp-owned | sequencer view singleton (cursor) | view-local; ported (.11), removed | .11 |
| `SEQV.cursor-step-*` | 1 | sequencer | Lisp (reactive-set) | Lisp-owned | sequencer view singleton (cursor) | view-local; ported (.11), removed | .11 |
| `SEQV.piano-roll-arrangement-mode` | 1 | piano-roll | Lisp (reactive-set) | Lisp-owned | piano-roll view singleton | view-local; ported (.16): piano-roll-view.arrangement, removed | .16 |
| `SEQV.plk-t-*` | 2 | effects/track-panels | Lisp (reactive-set) | Lisp-owned | p-lock menu view singleton | view-local; removed (.61: track.setting-locks; the *plock-sync* projection deleted) | .14 .61 |
| `SEQV.plk-var-b` | 1 | effects/param-controls | Lisp (reactive-set) | Lisp-owned | p-lock menu view singleton | view-local; ported (.14 A: plock-color singleton) | .14 |
| `SEQV.plk-var-g` | 1 | effects/param-controls | Lisp (reactive-set) | Lisp-owned | p-lock menu view singleton | view-local; ported (.14 A: plock-color singleton) | .14 |
| `SEQV.plk-var-r` | 1 | effects/param-controls | Lisp (reactive-set) | Lisp-owned | p-lock menu view singleton (:rgb) | view-local; ported (.14 A: plock-color singleton, three :number fields: a float prop reads one component) | .14 |
| `SEQV.rack-clip-center-*` | 1 | mixer | Lisp (reactive-set) | Lisp-owned | mixer view singleton | view-local; ported (.13), removed | .13 |
| `:bindable` | 97 | effects/physical-model-surface, sequencer, effects/drum-surface +24 | - | - | delete (ignored since stage 5) | remove (gone from .12's files); gone from .13's files; gone from the factory device UIs (.21); removed in ui/effects and ui/materials (.61) | .11 .12 .13 .14 .20 .21 .61 |
| `<ns-var namespace>` | 3 | bindings | - | - | bindings.lisp generic scopes → kinds | remove; kept (.18): eseq.bindings' only reader is alez.tracker (.20); removed (.77: ui/bindings.lisp, its imports in ui/main.lisp and ui/noui.lisp, and its state_values test) | .18 .77 |
| `reactive-value` | 75 | instruments/Synths/Heat/ui, effects/param-controls, scripts/sequencers/graph-neural-variable-reset-demo +27 | - | - | t.x / #'t.x read as a value (§8) | remove; gone from .13's files; gone from the factory device UIs (.21: custom-ui value helpers or the binding read as a value); gone from the panel plumbing (.14 A: custom-ui-param-value, fx-param-numeric-value-for); gone from .20's ported files (alez.jaki: generator marks); ported in ui/effects (.61: value twins custom-ui-param-value, comparisons read refs) | .11 .13 .14 .20 .21 .61 |
| `SEQ.bus-ids` | 10 | mixer, drum-rack-v2, seq-core-state +1 | sv/track_and_mixer.rs | model | instance identity | remove; ported (.13), kept: drum-rack-v2, seq-core-state; ported (.19), removed | .11 .13 .19 |
| `SEQ.delete-target-version` | 4 | mixer, browser, application-menus +1 | reactive_tick.rs | model | implicit (fields re-render) | remove; ported (.13, .17: the browser reads slot device.delete-target), kept: application-menus +; ported (.14 A: panel-bodies reads the effect device's delete-target); ported, legacy removed (.18: the last reader; its publishers in the tick, the invalidation apply and the registration) | .13 .14 .17 .18 |
| `SEQ.num-patterns` | 6 | transport, macros, scene-banks | sv/topology_and_visualization.rs | model | (len (scenes)) | remove; ported (.12), kept: macros (.18); ported, legacy removed (.18: scene macros list (scenes)) | .12 .18 |
| `SEQ.num-tracks` | 38 | mixer, track-collapse, sequencer +10 | reactive_sync.rs | model | (len (tracks)) | remove; ported (.13, .17), kept: many; ported (.18: application menus read (tracks)); ported (.61: buffers and step-buffer read (tracks)); ported (.19: drum-rack-v2, rack-groove-buffer), kept: step-grid (unloaded), .22 | .11 .13 .14 .17 .18 .19 .61 |
| `SEQ.rack-panel-view-generation` | 1 | effects/state | sv/project_state.rs | model | implicit | remove; removed (.14 A: rack panel views live in the rack-panel-view singleton by track id; the host calls eseq.effects.state/reset-rack-panel-views! on a project replacement) | .14 |
| `SEQ.scene-bank-view-generation` | 1 | scene-banks | sv/project_state.rs | model | implicit (collections re-render) | remove; ported, legacy removed (.12) | .12 |
| `SEQ.track-ids` | 30 | sequencer, arrangement, mixer +1 | reactive_sync.rs | model | instance identity (subtree :key t) | remove; ported (.13, .15), kept: sequencer +; ported (.11 grid, .65: alez.tracker), kept: unread, .22 | .11 .13 .15 .20 .65 |
| `SEQ.instances` | 3 | mixer, browser, packages/alez.neural/src/variable-reset | lisp_host/eseq/process_dsl_parse.rs | model | package instances (live_instances); project.instances (.17) | keep; ported (.13, .17: the Packages tree reads project.instances), kept: alez.neural; ported (.67: alez.neural reads (generators) and the instance records), kept: unread, .22 (the Packages host commands parse it) | .13 .17 .20 .67 |
| `THEME.buffer_bg` | 1 | sequencer | - | model | THEME stays (theme namespace, not host state) | keep | .11 |
| `THEME.plock_base` | 2 | effects/panel-bodies, effects/track-panels | - | model | THEME stays (theme namespace, not host state) | keep | .14 |
| `THEME.scene_clip_bg` | 1 | arrangement | - | model | THEME stays (theme namespace, not host state) | keep; kept (.15) | .15 |
| `GRAPH` via `bind-graph` / `bind-graph-config` (103 calls) | 103 | scripts/sequencers/graph-*, packages/alez.neural | lisp_host/eseq/graph_authoring.rs | model | graph-node.‹field›, graph-param.value, graph.‹field› (`#'` bindings) | built (.33); ported (.64: the seven graph demos), kept: alez.neural; ported (.67: alez.neural), removed with `graph-key`, `graph-edge-key`, `graph-config-key`, `bind-graph-node-notes` and the GRAPH namespace | .20 .64 .67 |
| `reactive-set "GRAPH"` (52 writes) | 52 | scripts/sequencers/graph-* | Lisp | Lisp-owned | graph-node / graph-param / graph `:set` (`set-graph`) | built (.33); ported (.64), gone from content | .20 .64 |
| `reactive-set "SEQ" "fx-step-*"` (8 writes) | 8 | seq-core-state | Lisp | Lisp-owned | selection.cursor-step (`:set`) / step.‹param› | built (.35) | .11 |

## Appendix A. Target example (abridged)

The real, complete view is `docs/examples/mini-daw.lisp`; this is its shape.

```lisp
(import eseq.kinds :refer (tracks transport banks scenes selection
                           launch! clone-scene! delete-scene! step-preset!))
(import eseq.effects :as fx)
(import eseq.step-grid-interactions :as sgi)

(def-kind view
  :key ()
  :state ((bank -1)                         ; -1 = follow the playing scene
          (open-device device :default nil)))

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
    (sdf/fill (clip-x bar track.volume) (* (if track.audible 1 0.35) (vgrad track.color)))
    (outline bar)
    (sdf/fill (clip-x meter track.peak) (vgrad (rgb 0.45 0.9 0.4)))))

(defmacro on-track (t &rest body)           ; a handler that selects t first
  `(lambda (e) (do (set! selection.track ,t) ,@body)))

(defmacro big-pill (text lit &rest props)
  `(box :background "pill" :active ,lit :width (sc 10) :height (sc 4)
        :h-align :center :v-align :center ,@props
     (label ,text :bg :transparent :color :white :active ,lit :active-color :black)))

(def step-view (s t)
  (box :width (sc 8) :height (sc 4)
    :on-mouse-down   (lambda (e) (sgi/down s e))
    :on-drag         (lambda (e) (sgi/drag s e))
    :on-mouse-up     (lambda (e) (sgi/up s e))
    :on-double-click (lambda (e) (sgi/double-click s e))
    (step-cell :step s :track t :seed (~slider 68.457 :min 0 :max 100))))

(sgi/bind-step-keys)                        ; Esc, Cmd-A, BS on the steps

(def track-view (t)
  (subtree :key t
    (box :on-mouse-down (on-track t)
      (h-stack :v-align :top
        (label (substring t.name 0 3)
          :font-size (sc 32) :color :dim
          :active #'t.selected :active-color :white)
        (grid :cols 8 :col-width (sc 8) :col-gap 1 :row-height (sc 4)
          (each t.steps |s| (step-view s t)))
        (v-stack :gap (sc 0.5)
          (h-stack :gap (sc 1)
            (box :background "mute-button" :track t :on-click (on-track t (toggle! t.muted)))
            (box :background "arm-button" :armed #'t.armed
              :on-click (on-track t (toggle! t.armed)))
            (box :background "fader" :track t
              :on-drag (on-track t (set! t.volume e.u))))
          (subtree :key (list :preset t) (preset-row t))
          (subtree :key (list :devices t) (slot-row t)))))))

(def scenes-view ()
  (let ((shown (if (< view.bank 0) transport.scene.bank (nth (banks) view.bank))))
    (v-stack :gap (sc 1)
      (h-stack :gap (sc 1)
        (each (banks) |b|
          (big-pill b.label (= b shown)
            :queued (and b.playing (not (= b shown)))
            :on-click (lambda (e) (set! view.bank b.index)))))
      (h-stack :gap (sc 1)
        (each shown.scenes |s|
          (big-pill (str s.number) #'s.active
            :queued #'s.queued
            :on-click (lambda (e) (launch! s))
            :on-right-click (lambda (e)
              (do (set! scene-menu.scene s)
                  (set! scene-menu.at e.at)
                  (set! scene-menu.open true))))))
      (context-menu :is-open scene-menu.open :anchor scene-menu.at
        :on-close (lambda () (set! scene-menu.open false))
        (menu-item "Clone scene"  :on-select (lambda (e) (clone-scene! scene-menu.scene)))
        (menu-item "Delete scene" :disabled (<= (len (scenes)) 1)
                                  :on-select (lambda (e) (delete-scene! scene-menu.scene)))))))

(effect-buffer "*sequencer*"
  (v-stack :padding (sc 1) :gap (sc 1)
    (each (tracks) |t| (track-view t))))

(effect-buffer "*fx*"                       ; fx/panel-buffer
  (let ((d view.open-device))
    (if (fx/device-panel d)
      (subtree :key (list :device d) (framed (fx/device-panel-body d)))
      (subtree :key :scenes (scenes-view)))))
```

Note the scene pills (`#'s.active`, repaint on launch) and bank pills
(`(= b shown)`, re-render) share `big-pill`; only the argument form differs.
