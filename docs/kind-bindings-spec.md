# Kind bindings

Status: spec rev 3, 2026-10-04. Stages 1–6 built, stage 7 in part (§14; 7, 7b, 7b-2, 7b-3, 7c, 7d, 7e, 7f, 7g, 7h and 7i built), stage 8 in part (§13, §13.1: .12 and .13 ported) (§3.1, §3.2, §3.3, §3.4, §4, §7.1, §7.3, §8, §9 notes). Bead: epic `eseq-0l17` (`bd list --label kind-bindings`).
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
- `((p1 p2 …) index)`: keyed under any one of several parent kinds (built
  in 7b-2 for `device`, under a track or a bus). The first key part is a
  live instance of one of them; each parent drops exactly its own children.
  The parent list is a set: naming a parent twice is an error (at compile
  time and in the VM's own `:key` parser), and a re-definition that lists
  the same parents in another order keeps the key's shape.
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
  project`, and since stage 7 `send bus group master engine`, since 7b `param`, since 7i `route`, since 7d `song region scene-span clip cell`, since 7h `pad rack-clip groove pad-groove pool-groove library-groove`, since 7b-3 `mod-target tensor variant macro rack-macro macro-mapping`, since 7c `process-class process-library process lane inlet port fanout state-cell`, since 7f `browser preset-file slot-presets sound sound-palette editor editor-macro editor-asset asset-info learn learn-plan-param learn-epoch-param learn-delta retro retro-lane retro-item song-export settings midi-device agent`, since 7e `note piano-roll`) before
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
- **Errors.** The plan's, the budget's and a name collision are evaluation
  errors (`VMError::Instance`). A shader codegen error (`<widget>: shader
  error: …`) stays non-fatal: a `[defwidget] warning:` on stderr and the
  form's string value, as before. `rec-arm-dot` in `ui/legacy/mixer.lisp`
  calls `eseq.materials/color`; since stage 8 (.13, eseq-0l17.25) that file
  imports `eseq.materials`, so the widget registers when it loads on its
  own (`legacy_mixer_definitions_are_top_level_and_source_loads`). Making
  the error fatal still breaks a boot: `ui/sequencer.lisp`,
  `ui/step-grid.lisp`, `ui/effects/param-grid.lisp` and
  `ui/effects/track-panels.lisp` call `eseq.materials/` in shaders without
  importing it, and `metal_seq_main_import_block_boots_in_reverse_order`
  loads the sequencer before the materials (`seqv-rec-arm-dot`).
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
  shader reads. An instance state takes an instance and a uniform name is
  never a prop, so `(step-cell :cell #'x)` gets the widget diagnostic
  `step-cell: :cell does not accept reactive bindings`.
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
  `host_fields_observed(id, &[fields]) -> u32` (bit `i` = `fields[i]`)
  formats the namespace once and locks the slot store once; the tick asks
  it once per instance. `instance_observer_epoch` moves whenever a field
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
     the process fixtures `one_slot_chain` (`host_kinds::tests`); `solo_binding_tests::mixer_mute_and_solo_only_repaint`
     asserts the mixer and patch mixer repaint without re-running for
     mute, solo, audibility, arm, selection, delete target, faders and
     meters; the MIDImix tests publish their topology as kinds.
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
     rejects a ref): pass a value there;
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
   The authoring rules: no `\"` escapes inside Lisp strings, no `cond`
   (`match` / `if` / `when` / `unless`).
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
  `seq-set-track-step-param` (any track, clamped, one undo entry, no
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
| `param` | `(device index)` | `device device`, `index :int`, `name :string`, `min`, `max`, `default :number`, `type :string` (`continuous`, `enum`, `boolean`), `options (list-of :string)`, `unit :string`, `value :number` (L, shown), `base :number` (L) [host command `set-device-param`], `locked :bool` (L), `has-locks :bool` (L), `text :string` (L), `printing :bool` (L) |
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
`(stretch-tuning! tn cents)`, `(set-bar-transpose! t bar v)`;
`(mod-in-level x i)` is a binding to input `i` (1–4) of a track or bus.

Built (7i):

- **The value rule** (every 7i setter; the earlier stages' clamping
  setters are unchanged). A string field takes one of its labels,
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
| `clip` | `(track cid)` | `track track`, `cid :int`, `start :number` [k], `end :number` [k], `cell cell` [k] (nil for a take), `take :int` (-1 for a pattern), `offset :number` (steps), `num-steps :int`, `length :number` (beats), `events (list-of (list-of :number))`, `dot :bool`, `dot-color :rgb` (the timeline's gray without a palette color) |
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
  sub-kinds.

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
`(use-library-groove! gr lg)` (copy-on-apply into the pool),
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
| `device` | `((track bus) did)` (was `(track did)`) | `bus bus` (nil for a track's device; `track` is nil for a bus effect), `role :string` (`instrument`, `effect`, `midi-fx`, `rack-slot`, `rack-effect`, `bus-effect`), `devices (list-of device)` (a drum rack's slots on its instrument device, a rack slot's effects on the slot), `container device` (the device whose `devices` holds it, or nil), `voices :int` (a rack slot's, 1–16; 0 otherwise; no declared range, the setter checks) [d], `delete-target :bool` (L) [d] |
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
  no p-locks (`lock-param!` / `unlock-param!` on one are errors), and a
  rack slot instrument's locks have no clear command (eseq-0l17.41).
- **Voices.** `device.voices` is a rack slot's max polyphony (a model
  field of the device sync; the legacy `tp-poly` / `tp-max-polyphony` show
  the selected slot's: `(> rs.voices 1)`); any other device reads 0, so the
  field declares no `:range` (0 would violate one) and its setter checks:
  an integer in 1–16 (`SetRackSlotMaxPolyphony` through history, refreshed
  with the legacy command's `rack_slot_voices_applied`; a drag joins one
  entry); another device's is an error.
- **Delete targets.** `device.delete-target` (live) reads true while the
  active delete target names the device (`DeviceSlot::delete_target`: a
  rack slot or rack slot effect, a bus effect, and a chain effect or MIDI
  effect of the current track: the fx panel's targets name the current
  track's chains). `(set! d.delete-target true)` makes the device the
  target; false clears it only while it names the device; an instrument,
  or another track's chain effect, is an error.
- **Not covered:** rack slot strip controls (gain, pan, mute, solo, choke,
  enabled as settable fields, with their p-lock display) (eseq-0l17.42),
  base note and the rest of the panel extras (eseq-0l17.37, built:
  §14.2g), rack slot sampler playheads (`device.playhead` is a track
  instrument's).

### 14.2g Built in stage 7b-3 (eseq-0l17.37)

| Kind | Key | New `:host` fields (`:set` in brackets) |
|---|---|---|
| `param` | `(device index)` | `overridden :bool` (L: `value` shows a selected neuron's override); placement: `label :string`, `section :string` (`main`, `mod`, `source`, `hidden`), `mod-slot :int`, `visible :bool` (L); lanes: `mod-targets (list-of mod-target)`; display: `mod-offset`, `mod-value`, `mod-scale :number` (L); process: `process-mapped :bool`, `process-value :number`, `process-clamped :bool` (L); `key-locks (list-of (list-of :number))` (L, `(note value)` rows) |
| `mod-target` | `(param index)` | `param param`, `index :int`, `source param` (nil: a fixed source), `slot :int`, `depth param`, `depth-min`, `depth-max :number`, `unit :string` |
| `device` | `((track bus) did)` | `base-note :number` (L, −48–48) [d], `mod-phases (list-of :number)` (L), `tensors (list-of tensor)`, `key-locked-notes (list-of :int)` (L), `variants (list-of variant)` (L, key-lock variants), `macros (list-of rack-macro)` (a drum rack's, on its instrument device) |
| `tensor` | `(device index)` | `device device`, `index :int`, `name :string`, `rows`, `cols :int`, `min`, `max :number`, `values`, `base (list-of :number)` (L), `locked :bool` (L) |
| `track` | `(index)` | `variants (list-of variant)` (L, the step variants: the chip list) |
| `variant` | `((track device) vid)` | `track track`, `device device` (nil: a step variant), `label`, `name :string`, `count :int`, `color :rgb`, `current :bool`, `notes (list-of :int)` (all L but `track`, `device`) |
| `macro` | `(index)` | `index :int`, `mid :int`, `script-key :string` (optional: empty when none), `name :string` [m], `type :string` (`mapped`, `scene`), `value :number` (0–1) [m], `mappings (list-of macro-mapping)`, `target-scene scene`, `morph-params`, `steal-patterns :bool`, `quantize :string` |
| `rack-macro` | `(device index)` | `device device`, `index :int` (0–7), `stable-key :string` (always set), `name :string` [rm], `value :number` (L), `base :number` (L) [rm], `locked`, `has-locks :bool` (L), `mappings (list-of macro-mapping)` |
| `macro-mapping` | `((macro rack-macro) index)` | `macro macro`, `rack-macro rack-macro` (one is nil), `index :int`, `target param` (nil: no device param), `label :string`, `min`, `max :number` [mm], `curve :string` [mm] (`linear`, `exp`, `log`, a project mapping's also `log-domain`), `suspended :bool` (positional: see Macros) |
| `project` | `()` | `macros (list-of macro)`; `(macros)` |

[d] = `set-device` (`:field` `base-note`); [m] = `set-macro` (`:macro-id`,
`:field`); [rm] = `set-rack-macro` (`device-target`, `:macro`, `:field`
`name` or `value`); [mm] = `set-macro-mapping` (`:macro-id`, or a rack
macro's `device-target` and `:macro`; `:mapping`, `:field`); all but [d] in
`host_commands/panel.rs`. Actions: `(set-tensor-cell! tz cell v)`
(`set-device-tensor`), `(stamp-variant! t steps v)` (`stamp-variant`, nil
`v` clears the steps' variant locks), `(stamp-key-variant! d notes v)`
(`stamp-key-variant`, nil `v` clears).

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
  shared with the rack panel: a sampler's lanes store DSP units). A
  descriptor change replaces them with the params.
- **Modulation display.** `mod-offset`, `mod-value`, `mod-scale` and
  `device.mod-phases` read the tick's modulation sample
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
  invalidation; another device's is an error (a rack slot's base note is a
  strip control: eseq-0l17.42).
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
  panel's edits, which are not recorded (eseq-0l17.44), with their legacy
  refreshes (`rename_rack_macro_reactive`, shared with
  `rename-rack-macro`); a rack macro's locks are the rack panel's p-lock
  commands.
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
- **Not covered** (eseq-0l17.43): the sampler panel's media (waveform
  buffer, slices, onsets, analysis, selection times), the sound-binding
  badge and display name, a modulator instrument's phase and level, the
  fixed modulators' labels, param UI metadata, effect tables and IR names,
  the built-in effect editors, the meter selector, scene macro config
  setters and `step.variant`. Rack macro edits are not undoable
  (eseq-0l17.44).
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
| `process-class` | `(index)` | `index :int`, `name`, `doc`, `source-path`, `target :string`, `lane-count :int`, `ports (list-of :string)` |
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
- **Not covered.** A graph node's process slots (the node bay's scopes and
  run errors, legacy `SEQ.process-scope-cells` and the node half of
  `SEQ.process-run-errors`): eseq-0l17.45, with the graph-node kinds
  (eseq-0l17.33). A class's ports are listed by name
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
| `project` | `()` | `name :string`, `audio-workers-options (list-of :string)` |
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
  **eseq-0l17.22 deletes `presented/legacy.rs` and the mirror calls in
  `presented` (the `mirror` argument of each mutator, the
  `*_registration` calls); no call site changes then.** Capture fixtures
  seed an area through `(present-fixture area fields)` (by the kind's
  field names: `song-export`, `settings`, `retro`), which edits the record
  as a command would, the mirror following. Compared every tick in place:
  the sidebar's track and slot devices against the track and device
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
  nothing; anything else is an error (no clamping, unlike the legacy
  `configure-learn`, which clamps into the same ranges and then stores
  only what the same rule, `validate_learn_setting`, accepts); no undo, as
  the legacy settings. `settings.audio-workers-choice` sends
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
  preview (`browser.preview-*`).
- **Not covered:** the legacy-only fields no content reads
  (`SEQ.sidebar-preset-tree`, `MIDI.ports`; `learn-checkpoint-wav` and
  `editor-active` are in the record, mirrored, but no kind pushes them),
  and renaming a mix (the palette lists patches).

### 14.2j Built in stage 7e (eseq-0l17.31)

| Kind | Key | New `:host` fields (`:set` in brackets) |
|---|---|---|
| `piano-roll` | `()` | `track track` (the current track), `focus-kind :string` (`live`, `pattern`, `take`), `clip-kind :string` (`none`, `pattern`, `take`), `clip clip` (the pinned arrangement clip, nil in follow mode), `focus-label :string`, `focus-num-steps :int`, `window-marker :number` (-1: none), `window-span (list-of :number)` (`(start end)`, empty: none), `window-repeat :number` (0: none), `playhead :number` (L, -1 hidden), `notes (list-of note)` (lazy; then Model) |
| `note` | `(track nid)` | `track track`, `nid :int`, `pitch :int` (−48–48, semitones from C4) [n], `start :number` (steps on the focus axis) [n], `length :number` (1/32–32 steps) [n], `velocity :number` (0–1, its step's) [n], `selected :bool` [n], `label :string`, `hidden :bool` (a script drag's note lies over it) |
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
  tracked back through history; an undo that changes no note keeps them). The notes are those of the piano roll's source
  (`NoteSource`: the current track's resolved edit focus, the effective
  pattern for a live focus): another source (a track switch, a clip pinned,
  a scene launch, a project load) replaces them all; a track reorder keeps
  them (keyed under the track instance). Model note ids, which would keep
  handles through every path: eseq-0l17.48.
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
- **The tracker** (`alez.tracker`, port .20) needs no kind of its own:
  `tracker-rows` is the steps' fields (`active`, `transpose`, `velocity`,
  the step params) and, per device param column, `param.step-locks` /
  `rack-macro.step-locks` (the pattern's locks, `(step value)` rows,
  computed while observed when the track's p-lock key moved, as
  `has-locks`; the legacy cells' values, `build_tracker_rows_value`);
  `track-automation` (the columns with a lock) is `param.has-locks` /
  `rack-macro.has-locks` and the step params a step holds off their
  default; `track-lock-targets` is the track's devices' params
  (`t.devices`, `t.midi-devices`, a rack's `device.macros`) and lanes; the
  grid playheads are a view derivation from `track.playhead` and
  `transport.position` (which copy of a repeating step lights: the
  transport's sixteenth modulo the grid height, confirmed against the
  track's own step), `-current` with `selection.track`.
- **Not covered:** the automation lane under the piano roll
  (`SEQ.piano-roll-automation`, `-automation-params`): the focus axis's step
  params (a pinned source's steps, which `step` does not reach) and the
  lane's points: eseq-0l17.47. View-local (port .16): the piano roll's
  arrangement mode (`SEQV.piano-roll-arrangement-mode`), scroll, zoom, tool,
  cursor and marquee.

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
`host_commands/graphs.rs`). Actions: `(set-group-gain! g row col v)`,
`(set-group-coupling! g row col v)`, `(gate-generator! n id :restart r)`
(`&key restart`). Helpers: `(graph-of x)` (a created instance or a
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
  field alone, an unrecorded legacy write to another field of the same
  node, the config or the same matrix included. Every edit stages a
  coalescing gesture keyed by its field, as a bar transpose does: a
  numeric field's `set!`s while the pointer is down join one entry
  (`ScriptEdit`), a drag over two fields records two, anything else is its
  own entry. The legacy `graph-param`, `graph-edge` and `graph-config`
  natives write through the same slots. Legacy
  `graph-*` edits stay unrecorded; the kinds pick them up at the next sync.
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
- **Not covered:** a node's process patch as `process` instances
  (eseq-0l17.49, then the node bay's scopes and run errors,
  eseq-0l17.45); the native neural engine's networks
  (`SEQ.neural-networks`, `neural-*-matrix`, the neuron selection;
  eseq-0l17.50); event streams (the graph's event history, node events,
  deltas and group traces; `SEQ.track-events`, `track-event-current-beat`;
  eseq-0l17.51); generator marks (alez.jaki, eseq-0l17.52); a node's
  `duration` / `swing` overrides (no content edits them).

### 14.3 Follow-up beads

Each port bead depends on the beads whose rows it uses (`bd dep`).

| Tag | Bead | Kinds | Ports blocked |
|---|---|---|---|
| 7b | eseq-0l17.28 (built) | `param` under `device` (values, p-lock display, print latch), `device.playhead`, step p-lock render (`plocked`, `lock-kind`, `variant-color`), send p-lock flags | .11 .13 .14 .18 .19 .21 |
| 7b-2 | eseq-0l17.36 (built) | devices (and params) for MIDI fx, bus effects, rack slots; `bus.devices`, `track.midi-devices`, `device.delete-target` (from 7i), `device.voices` | .13 .14 .18 .19 .21 |
| 7b-2a | eseq-0l17.41 | the clear command for a rack slot instrument's p-locks (`unlock-param!` on a rack slot param) | — |
| 7b-2b | eseq-0l17.42 | rack slot strip controls (gain, pan, mute, solo, choke, enabled) on the rack slot device, with their p-lock display | .14 .19 |
| 7b-3 | eseq-0l17.37 (built) | panel extras: param placement and lanes, modulation display, process mapping, tensors, base note, key locks, rack and project macros, variant chip list, neural-selection display | .14 .18 |
| 7b-4 | eseq-0l17.43 | the rest of the panel data: sampler media, sound binding, modulator display, tables and IR names, effect editors, param UI metadata, scene macro config | .14 .18 |
| 7b-3a | eseq-0l17.44 | recorded (undoable) drum rack macro edits | .18 |
| 7c | eseq-0l17.29 (built) | `process` (a track's chain), `lane`, `inlet`, `port`, `fanout`, `state-cell`, `process-class`, `process-library`; `track.processes` / `lanes` | .11 .14 .20 |
| 7c-2 | eseq-0l17.45 | graph-node process slot probes and run errors (the node bay's scopes); needs 7g-2 | .11 .20 |
| 7d | eseq-0l17.30 (built) | `song` and `region` singletons, `scene-span`, `clip`, pattern `cell`, `track.governed` / `latched` | .11 .12 .13 .15 .17 .20 |
| 7d-2 | eseq-0l17.39 | `song.pending` (the provisional capture surface) as positional sub-kinds | .15 |
| 7e | eseq-0l17.31 (built) | `note`, `piano-roll` singleton, tracker rows (`param.step-locks`, `rack-macro.step-locks`) and grid playheads (view derivation) | .16 .20 |
| 7e-2 | eseq-0l17.47 | the piano roll's automation lane: focus-axis step params, lane points | .16 |
| 7e-3 | eseq-0l17.48 | model note ids (handles kept through undo and legacy edits) | — |
| 7f | eseq-0l17.32 (built) | `browser`, `sound-palette` / `sound`, `editor`, `learn`, `retro`, `song-export`, `settings` and `agent` singletons and their rows, `project.name`, `track.instrument-id` | .12 .17 .18 |
| 7g | eseq-0l17.33 (built) | `graph`, `graph-node`, `graph-edge`, `graph-param` (the GRAPH namespace, graph playback), `project.graphs`, `track.active-notes` | .13 .14 .20 |
| 7g-2 | eseq-0l17.49 | a graph node's process patch as `process` instances | .20 (and .45) |
| 7g-3 | eseq-0l17.50 | the native neural engine's networks and neuron selection | .20 |
| 7g-4 | eseq-0l17.51 | event streams: graph event history, deltas, group traces; track events | .20 |
| 7g-5 | eseq-0l17.52 | generator marks (alez.jaki) | .20 |
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
| `SEQ.<slot-page-active-field>` | 1 | sequencer | sv/expanded_step.rs | model | track.playhead | built (.10) | .11 |
| `SEQ.<slot-param-field>` | 2 | sequencer | sv/expanded_step.rs | model | step.‹param› | built (.10) | .11 |
| `SEQ.<track-bus-send-field>` | 2 | effects/track-panels | sv/track_and_mixer.rs | model | send.display (of selection.track) | built (.10) | .14 |
| `SEQ.<track-pan-field>` | 1 | mixer | sv/track_and_mixer.rs | model | track.pan | built (.10); ported (.13), removed | .13 |
| `SEQ.<track-volume-field>` | 2 | mixer, sequencer | sv/track_and_mixer.rs | model | track.volume | built (.10); ported (.13), kept: sequencer | .11 .13 |
| `SEQ.auxas` | 1 | seqv-track-params | reactive_sync.rs | model | step.aux-a | built (.10) | .11 |
| `SEQ.bpm` | 2 | effects/builtin/phaser-flanger, transport | bounce/job.rs | live | transport.bpm | built (.10); ported (.12), kept: phaser-flanger (.14) | .12 .14 |
| `SEQ.bus-mutes` | 5 | mixer, sequencer, legacy/mixer | sv/track_and_mixer.rs | model | bus.muted | built (.10); ported (.13), kept: sequencer | .11 .13 |
| `SEQ.bus-names` | 25 | mixer, legacy/mixer, seq-core-state +2 | sv/track_and_mixer.rs | model | bus.name | built (.10); ported (.13), kept: seq-core-state | .11 .13 .14 |
| `SEQ.bus-peak-*` | 3 | mixer, sequencer | sv/meters_and_modulation.rs | live | bus.peak | built (.10); ported (.13), kept: sequencer | .11 .13 |
| `SEQ.bus-solos` | 4 | mixer, sequencer, legacy/mixer | sv/track_and_mixer.rs | model | bus.soloed | built (.10); ported (.13), kept: sequencer | .11 .13 |
| `SEQ.bus-volumes` | 3 | mixer, sequencer, legacy/mixer | sv/track_and_mixer.rs | model | bus.volume | built (.10); ported (.13), kept: sequencer | .11 .13 |
| `SEQ.cpu-load-pct` | 1 | transport | reactive_tick.rs | live | engine.cpu-load | built (.10); ported, legacy removed (.12) | .12 |
| `SEQ.current-pattern` | 18 | transport, arrangement, mixer +10 | sv/topology_and_visualization.rs | model | transport.scene (s.index) | built (.10); ported (.12, .13), kept: arrangement, macros, scripts | .12 .13 .15 .20 |
| `SEQ.current-track` | 108 | piano-roll, effects/process-panel, browser +19 | piano_roll.rs | live | selection.track | built (.10); ported (.13), kept: many | .11 .13 .14 .15 .16 .17 .18 .19 .20 |
| `SEQ.delays` | 1 | seqv-track-params | event_loop.rs | model | step.delay | built (.10) | .11 |
| `SEQ.durations` | 2 | seq-core-state, seqv-track-params | event_loop.rs | model | step.duration | built (.10) | .11 |
| `SEQ.groups` | 53 | mixer, drum-rack-v2, seq-core-state +12 | project.rs | model | group.* via (groups), track.group | built (.10); ported (.13), kept: drum-rack-v2, seq-core-state + | .11 .13 .17 .19 .20 |
| `SEQ.master-peak-l` | 2 | mixer, transport | event_loop.rs | live | master.peak-l | built (.10); ported (.12, .13), removed | .12 .13 |
| `SEQ.master-peak-r` | 2 | mixer, transport | event_loop.rs | live | master.peak-r | built (.10); ported (.12, .13), removed | .12 .13 |
| `SEQ.master-recording` | 2 | transport | reactive_tick.rs | live | master.recording | built (.10); ported, legacy removed (.12) | .12 |
| `SEQ.metronome` | 1 | transport | host_commands/misc.rs | live | transport.metronome | built (.10); ported, legacy removed (.12) | .12 |
| `SEQ.output-latency-ms` | 1 | transport | reactive_tick.rs | live | engine.latency-ms | built (.10); ported, legacy removed (.12) | .12 |
| `SEQ.pans` | 2 | seq-core-state, seqv-track-params | event_loop.rs | model | step.pan | built (.10) | .11 |
| `SEQ.playhead-active-*` | 1 | step-grid | sv/meters_and_modulation.rs | live | step.playing | built (.10) | .11 |
| `SEQ.playhead-page` | 1 | seq-core-state | sv/meters_and_modulation.rs | live | track.playhead (page = playhead / 16 in the view) | built (.10) | .11 |
| `SEQ.playing` | 11 | retrospective, transport, effects/track-panels +4 | sequencer/state/sequencer_state/scene_launch.rs | live | transport.playing | built (.10); ported (.12), kept: track-panels, param-controls, seq-core-state, sequencer | .11 .12 .14 .20 |
| `SEQ.queued-scene` | 2 | transport | event_loop.rs | model | transport.queued | built (.10); ported, legacy removed (.12) | .12 |
| `SEQ.record-armed` | 4 | mixer, sequencer, legacy/mixer | event_loop.rs | live | track.armed | built (.10); ported (.13), kept: sequencer | .11 .13 |
| `SEQ.record-quantize` | 1 | transport | host_commands/misc.rs | live | transport.record-quantize | built (.10); ported, legacy removed (.12) | .12 |
| `SEQ.recording` | 4 | effects/track-panels, transport, effects/param-controls | reactive_sync.rs | live | transport.recording | built (.10); ported (.12), kept: track-panels, param-controls (.14) | .12 .14 |
| `SEQ.retrig-rates` | 2 | seq-core-state, seqv-track-params | reactive_sync.rs | model | step.retrig-rate | built (.10) | .11 |
| `SEQ.retrigs` | 2 | seq-core-state, seqv-track-params | reactive_sync.rs | model | step.retrig | built (.10) | .11 |
| `SEQ.roll-mode` | 2 | transport | reactive_tick.rs | live | transport.roll-mode | built (.10); ported, legacy removed (.12) | .12 |
| `SEQ.scene-banks` | 2 | scene-banks | sv/song_state.rs | model | (banks) → bank.label/scenes | built (.10); ported, legacy removed (.12) | .12 |
| `SEQ.scene-launch-quantize` | 6 | transport, drum-rack-v2, mixer | rack_clip_switch_probe.rs | model | transport.launch-quantize | built (.10); ported (.12, .13), kept: drum-rack-v2; the host kinds read transport.launch-quantize from it | .12 .13 .19 |
| `SEQ.scene-names` | 8 | browser, arrangement | sv/song_state.rs | model | scene.name | built (.10) | .15 .17 |
| `SEQ.selected-steps` | 4 | step-grid, effects/param-controls | reactive_tick.rs | live | step.selected | built (.10) | .11 .14 |
| `SEQ.selected-tracks` | 5 | mixer, step-grid-interactions | sv/steps_and_pattern.rs | live | selection.tracks | built (.10); ported (.13), kept: step-grid-interactions | .11 .13 |
| `SEQ.seq-track-step-active-*` | 2 | sequencer | sv/steps_and_pattern.rs | live | step.active | built (.10) | .11 |
| `SEQ.seq-track-step-duration-*` | 1 | sequencer | sv/steps_and_pattern.rs | model | step.held | built (.10) | .11 |
| `SEQ.seq-track-step-param-haptic-*` | 1 | sequencer | sv/steps_and_pattern.rs | model | step.‹param› (detent in the view) | built (.10) | .11 |
| `SEQ.seq-track-step-param-slider-*` | 1 | sequencer | sv/steps_and_pattern.rs | model | step.‹param› (normalize in the view) | built (.10) | .11 |
| `SEQ.seq-track-step-selected-*` | 2 | sequencer | sv/steps_and_pattern.rs | live | step.selected | built (.10) | .11 |
| `SEQ.seqv-cursor-param-value-*` | 1 | sequencer | sv/expanded_step.rs | model | step.‹param› of the cursor step (cursor is view-local) | built (.10) | .11 |
| `SEQ.seqv-cursor-sync-index-*` | 1 | sequencer | sv/expanded_step.rs | model | step.sync of the cursor step | built (.10) | .11 |
| `SEQ.seqv-slot-length-active-*` | 1 | sequencer | sv/expanded_step.rs | model | track.num-steps | built (.10) | .11 |
| `SEQ.step-color-b-effective` | 2 | sequencer | sv/track_and_mixer.rs | model | track.color × track.audible | built (.10) | .11 |
| `SEQ.step-color-g-effective` | 2 | sequencer | sv/track_and_mixer.rs | model | track.color × track.audible | built (.10) | .11 |
| `SEQ.step-color-r-effective` | 2 | sequencer | sv/track_and_mixer.rs | model | track.color × track.audible (+ track.governed, 7d) | built (.10) | .11 |
| `SEQ.steps` | 3 | step-grid | lisp_host/eseq/graph_authoring.rs | model | selection.track.steps → step.active | built (.10) | .11 |
| `SEQ.syncs` | 2 | seqv-track-params, seq-grid-mode | ui_benchmark/ipc.rs | model | step.sync | built (.10) | .11 |
| `SEQ.tp-is-rack` | 5 | effects/track-panels, mixer | sv/project_state.rs | model | selection.track.rack | built (.10); ported (.13), kept: track-panels | .13 .14 |
| `SEQ.tp-num-steps` | 8 | seq-grid-mode, seq-core-state, piano-roll +2 | reactive_sync.rs | model | selection.track.num-steps | built (.10) | .11 .14 .16 |
| `SEQ.tp-timebase` | 2 | step-grid, effects/track-panels | sv/project_state.rs | model | selection.track.timebase | built (.10) | .11 .14 |
| `SEQ.track-auxas` | 1 | seqv-track-params | reactive_sync.rs | model | step.aux-a | built (.10) | .11 |
| `SEQ.track-bus-sends` | 2 | mixer, midi-midimix | reactive_sync.rs | model | track.sends → send.bus, send.display | built (.10); ported (.13), removed | .13 |
| `SEQ.track-collapsed` | 2 | track-collapse | reactive_sync.rs | live | track.collapsed | built (.10) | .11 |
| `SEQ.track-color-b-effective` | 1 | sequencer | sv/track_and_mixer.rs | model | track.color × track.audible | built (.10) | .11 |
| `SEQ.track-color-g-effective` | 1 | sequencer | sv/track_and_mixer.rs | model | track.color × track.audible | built (.10) | .11 |
| `SEQ.track-color-r-effective` | 1 | sequencer | sv/track_and_mixer.rs | model | track.color × track.audible (dim in the shader) | built (.10) | .11 |
| `SEQ.track-colors` | 21 | mixer, rack-groove-buffer, arrangement +12 | sv/track_and_mixer.rs | model | track.color | built (.10); ported (.13), kept: sequencer + | .11 .13 .14 .15 .16 .19 .20 |
| `SEQ.track-delays` | 1 | seqv-track-params | reactive_sync.rs | model | step.delay | built (.10) | .11 |
| `SEQ.track-durations` | 1 | seqv-track-params | reactive_sync.rs | model | step.duration | built (.10) | .11 |
| `SEQ.track-instrument-types` | 14 | track-collapse, mixer, application-menus | sv/track_and_mixer.rs | model | track.instrument-type | built (.10); ported (.13), kept: track-collapse + | .11 .13 .18 |
| `SEQ.track-length-row-*` | 1 | sequencer | sv/expanded_step.rs | model | track.num-steps | built (.10) | .11 |
| `SEQ.track-muted-effective` | 11 | mixer, sequencer, legacy/mixer +1 | sv/track_and_mixer.rs | live | not track.audible | built (.10); ported (.13), kept: sequencer + | .11 .13 .14 |
| `SEQ.track-mutes` | 3 | mixer, sequencer, legacy/mixer | reactive_sync.rs | live | track.muted | built (.10); ported (.13), kept: sequencer | .11 .13 |
| `SEQ.track-names` | 28 | sequencer, mixer, packages/alez.jaki/src/kind +14 | reactive_sync.rs | model | track.name | built (.10); ported (.13), kept: many | .11 .13 .14 .16 .18 .19 .20 |
| `SEQ.track-num-steps` | 5 | sequencer, packages/alez.tracker/src/ui | event_loop.rs | model | track.num-steps | built (.10) | .11 .20 |
| `SEQ.track-pans` | 1 | seqv-track-params | reactive_sync.rs | model | step.pan (per-step lists, see steps) | built (.10) | .11 |
| `SEQ.track-peak-*` | 3 | mixer, sequencer, legacy/mixer | sv/meters_and_modulation.rs | live | track.peak | built (.10); ported (.13), kept: sequencer | .11 .13 |
| `SEQ.track-playhead-page-*` | 1 | sequencer | sv/expanded_step.rs | live | track.playhead | built (.10) | .11 |
| `SEQ.track-playhead-row-*` | 1 | sequencer | sv/expanded_step.rs | live | track.playhead | built (.10) | .11 |
| `SEQ.track-playhead-row-active-*` | 1 | sequencer | sv/expanded_step.rs | live | track.playhead | built (.10) | .11 |
| `SEQ.track-retrig-rates` | 1 | seqv-track-params | reactive_sync.rs | model | step.retrig-rate | built (.10) | .11 |
| `SEQ.track-retrigs` | 1 | seqv-track-params | sv/topology_and_visualization.rs | model | step.retrig | built (.10) | .11 |
| `SEQ.track-selected-*` | 3 | mixer, seq-core-state | sv/steps_and_pattern.rs | model | track.selected / (member t selection.tracks) | built (.10); ported (.13), kept: sequencer, seq-core-state | .11 .13 |
| `SEQ.track-solos` | 3 | mixer, sequencer, legacy/mixer | reactive_sync.rs | live | track.soloed | built (.10); ported (.13), kept: sequencer | .11 .13 |
| `SEQ.track-syncs` | 1 | seqv-track-params | reactive_sync.rs | model | step.sync | built (.10) | .11 |
| `SEQ.track-timebases` | 2 | sequencer | sv/param_fields_and_sync.rs | model | track.timebase | built (.10) | .11 |
| `SEQ.track-transposes` | 1 | seqv-track-params | host_commands/step_history.rs | model | step.transpose | built (.10) | .11 |
| `SEQ.track-velocities` | 1 | seqv-track-params | reactive_sync.rs | model | step.velocity | built (.10) | .11 |
| `SEQ.track-volumes` | 3 | sequencer, legacy/mixer | reactive_sync.rs | live | track.volume | built (.10); ported (.13), kept: sequencer | .11 .13 |
| `SEQ.transport-playhead` | 1 | transport | ui_replay_probe.rs | live | transport.position | built (.10); ported, legacy removed (.12) | .12 |
| `SEQ.transposes` | 2 | seq-core-state, seqv-track-params | reactive_sync.rs | model | step.transpose | built (.10) | .11 |
| `SEQ.velocities` | 2 | seq-core-state, seqv-track-params | app/retrospective.rs | model | step.velocity | built (.10) | .11 |
| `SEQV.<sel-track-vis-field>` | 1 | seq-core-state | Lisp (reactive-set) | Lisp-owned | track.selected | built (.10) | .11 |
| `<ns-var name>` | 2 | effects/drum-surface | custom_ui.rs | - | param.value via (device-param d "x") | built (.28) | .14 |
| `SEQ.<get>` | 23 | effects/param-controls, effects/instrument-panel, effects/sampler-panel +9 | sv/param_fields_and_sync.rs, instrument_panel.rs, effects_panel.rs | model | param.value / param.name (panel :value-field, :label-field, :name-field, :short-field); MIDI fx / bus / rack slot params (built .36), rack macro names → rack-macro.name (built .37) | built (.28, .36, .37) | .14 .16 .18 .19 .20 .21 |
| `SEQ.<slot-field>` | 12 | sequencer | sv/expanded_step.rs | model | step.active/selected/playing/plocked/lock-kind/variant-color through the view's own slot→step map (expanded-step projection removed) | built (.28) | .11 |
| `SEQ.<var field>` | 8 | effects/param-controls, effects/custom-ui-runtime, mixer +1 | sv/param_fields_and_sync.rs | model | param.value / send.display (field strings from panel data); mod / process fields → param.mod-offset / mod-value / mod-scale / process-value / process-clamped, device.mod-phases (built .37) | built (.28, .37); mixer sends ported (.13): `track-N-bus-M-send` and its `-plock-*` / `-proc-*` removed (kept: `tp-bus-M-send`, track-panels) | .13 .14 |
| `SEQ.effects` | 3 | application-menus, effects/index, effects/buffers | lisp_host/dgen/instrument_storage.rs | model | track.devices → device.params; mod targets, sources, tensors → param.mod-targets / section / mod-slot / visible, device.tensors (built .37); tables, IR names, editors .43 | built (.28, .37) | .14 .18 |
| `SEQ.instrument-panel` | 10 | effects/param-controls, browser, effects/index +3 | reactive_tick.rs | model | device panel data (device.params; rack slots: the rack device's devices (built .36); key locks → param.key-locks / device.key-locked-notes / device.variants, macros → device.macros, modulation → param.mod-* / mod-targets, base note → device.base-note, tensors → device.tensors, process → param.process-* (built .37); sampler media, sound binding, modulator display .43) | built (.28, .36, .37) | .14 .17 .18 |
| `SEQ.macros` | 5 | macros, effects/param-controls | project.rs | model | project macros → project.macros / macro (mappings → macro-mapping); rack macros → device.macros of the rack's instrument (rack-macro) | built (.37) | .14 .18 |
| `SEQ.sampler-playhead` | 1 | effects/sampler-panel | reactive_tick.rs | live | device.playhead (live) | built (.28) | .14 |
| `SEQ.seq-track-step-plock-kind-*` | 1 | sequencer | sv/steps_and_pattern.rs | model | step.lock-kind | built (.28) | .11 |
| `SEQ.seq-track-step-plocked-*` | 1 | sequencer | sv/steps_and_pattern.rs | model | step.plocked | built (.28) | .11 |
| `SEQ.seq-track-step-variant-b-*` | 1 | sequencer | - | model | step.variant-color | built (.28) | .11 |
| `SEQ.seq-track-step-variant-g-*` | 1 | sequencer | - | model | step.variant-color | built (.28) | .11 |
| `SEQ.seq-track-step-variant-r-*` | 1 | sequencer | - | model | step.variant-color | built (.28) | .11 |
| `SEQ.step-has-plocks` | 2 | step-grid | reactive_tick.rs | model | step.plocked | built (.28) | .11 |
| `SEQ.track-plock-any` | 1 | effects/param-controls | event_loop.rs | model | param.has-locks, send.has-locks | built (.28) | .14 |
| `SEQ.track-plock-printing` | 1 | effects/param-controls | step_print.rs | model | param.printing | built (.28) | .14 |
| `SEQ.track-plock-variants` | 3 | effects/track-panels, effects/param-controls | reactive_sync.rs | model | step.variant-color (built .28) + variant chip list → track.variants / variant (built .37) | built (.28, .37) | .14 |
| `SEQ.track-plocks` | 9 | effects/track-panels, effects/param-controls | reactive_sync.rs | model | param.locked / param.base (the -on / -def projections; step panel rows from device.params) | built (.28) | .14 |
| `SEQ.process-lanes` | 3 | seqv-track-params, seq-grid-mode, sequencer | input.rs | model | selection.track.lanes → lane | built (.29) | .11 |
| `SEQ.process-library` | 3 | sequencer, packages/alez.neural/src/variable-reset | input.rs | model | process-library.classes → process-class | built (.29) | .11 .20 |
| `SEQ.process-run-errors` | 1 | sequencer | reactive_tick.rs | model | process.error (a track slot's, live); a graph node slot's: .45 (after .49) | built (.29), .45 | .11 |
| `SEQ.process-scope-cells` | 1 | sequencer | ui_replay_probe.rs | live | graph node slot scopes (the node bay; slots as instances: .49) | .45 | .11 .20 |
| `SEQ.process-slots` | 2 | effects/process-panel | input.rs | model | selection.track.processes → process (inlets, ports) | built (.29) | .14 |
| `SEQ.track-lane-patch` | 2 | sequencer | input.rs | model | t.processes: p.in-ports, port.target-process / target-inlet, fanout.target-process (cable ids derived in the view) | built (.29) | .11 |
| `SEQ.track-process-lane-values` | 2 | seqv-track-params, packages/alez.tracker/src/ui | sv/param_fields_and_sync.rs | model | lane.values | built (.29) | .11 .20 |
| `SEQ.track-process-lanes` | 2 | seqv-track-params, packages/alez.tracker/src/ui | sv/topology_and_visualization.rs | model | track.lanes → lane | built (.29) | .11 .20 |
| `SEQ.track-process-scopes` | 3 | sequencer | ui_replay_probe.rs | live | process.cells → state-cell.values (live) | built (.29) | .11 |
| `SEQ.track-process-slots` | 4 | sequencer, seqv-track-params, scripts/sequencers/band-coupling-matrix-demo | input.rs | model | track.processes → process | built (.29) | .11 .20 |
| `SEQ.queued-track-clips` | 1 | mixer | event_loop.rs | model | cell.queued (live) | built (.30); ported (.13), removed | .13 |
| `SEQ.scene-spans` | 9 | arrangement | sv/song_state.rs | model | song.spans → scene-span | built (.30) | .15 |
| `SEQ.song-bound-clip` | 2 | arrangement, sound-palette | sv/song_state.rs | model | song.bound-clip | built (.30) | .15 .17 |
| `SEQ.song-clip-sounds` | 2 | arrangement | sv/sound_palette.rs | model | clip.dot / dot-color | built (.30) | .15 |
| `SEQ.song-cursor-beats` | 1 | transport | sv/song_state.rs | model | song.cursor | built (.30); ported, legacy removed (.12) | .12 |
| `SEQ.song-edit-error` | 2 | arrangement | sv/song_state.rs | model | song.edit-error | built (.30) | .15 |
| `SEQ.song-end-beat` | 2 | arrangement | sv/song_state.rs | model | song.end | built (.30) | .15 |
| `SEQ.song-lane-events` | 4 | arrangement | sv/song_state.rs | model | clip.events / num-steps / length | built (.30) | .15 |
| `SEQ.song-lanes` | 7 | arrangement, sound-palette | sv/song_state.rs | model | `t.clips` → clip | built (.30) | .15 .17 |
| `SEQ.song-manual-latch` | 2 | transport | sv/song_state.rs | model | song.manual-latch | built (.30); ported, legacy removed (.12) | .12 |
| `SEQ.song-mode` | 2 | transport, arrangement | sv/song_state.rs | model | song.mode | built (.30); ported (.12), kept: arrangement (.15) | .12 .15 |
| `SEQ.song-pending` | 8 | arrangement | sv/song_state.rs | model | song.pending | .39 | .15 |
| `SEQ.song-position-beats` | 5 | arrangement, transport | sv/song_state.rs | live | song.position (live) | built (.30); ported (.12), kept: arrangement (.15) | .12 .15 |
| `SEQ.song-region` | 31 | arrangement | sv/song_state.rs | model | song.region | built (.30) | .15 |
| `SEQ.song-scene-latched` | 1 | arrangement | sv/song_state.rs | model | song.scene-latched | built (.30) | .15 |
| `SEQ.song-track-governed` | 3 | sequencer | sv/song_state.rs | model | track.governed | built (.30) | .11 |
| `SEQ.song-track-latched` | 1 | arrangement | sv/song_state.rs | model | track.latched | built (.30) | .15 |
| `SEQ.track-pattern-cell-active-*` | 2 | mixer, arrangement | sv/steps_and_pattern.rs | model | cell.active | built (.30); ported (.13), kept: arrangement | .13 .15 |
| `SEQ.track-pattern-cell-assigned-*` | 1 | mixer | sv/steps_and_pattern.rs | model | cell.assigned | built (.30); ported (.13), removed | .13 |
| `SEQ.track-pattern-cell-override-*` | 1 | mixer | sv/steps_and_pattern.rs | model | cell.override | built (.30); ported (.13), removed | .13 |
| `SEQ.track-pattern-cell-selected-*` | 1 | mixer | sv/steps_and_pattern.rs | model | cell.selected | built (.30); ported (.13), removed | .13 |
| `SEQ.track-pattern-cells` | 4 | mixer, arrangement | sv/track_and_mixer.rs | model | cell kind (track pid): `t.cells` | built (.30); ported (.13), kept: arrangement | .13 .15 |
| `SEQ.focus-clip-end` | 3 | piano-roll | piano_roll.rs | model | piano-roll.clip.end (`clip`, 7d) | built (.31) | .16 |
| `SEQ.focus-clip-kind` | 5 | piano-roll | piano_roll.rs | model | piano-roll.clip-kind | built (.31) | .16 |
| `SEQ.focus-clip-offset` | 4 | piano-roll | piano_roll.rs | model | piano-roll.clip.offset | built (.31) | .16 |
| `SEQ.focus-clip-start` | 5 | piano-roll | piano_roll.rs | model | piano-roll.clip.start | built (.31) | .16 |
| `SEQ.focus-kind` | 6 | piano-roll | piano_roll.rs | model | piano-roll.focus-kind | built (.31) | .16 |
| `SEQ.focus-label` | 1 | piano-roll | piano_roll.rs | model | piano-roll.focus-label | built (.31) | .16 |
| `SEQ.focus-num-steps` | 1 | piano-roll | piano_roll.rs | model | piano-roll.focus-num-steps | built (.31) | .16 |
| `SEQ.focus-window-marker` | 1 | piano-roll | piano_roll.rs | model | piano-roll.window-marker | built (.31) | .16 |
| `SEQ.focus-window-repeat` | 1 | piano-roll | piano_roll.rs | model | piano-roll.window-repeat | built (.31) | .16 |
| `SEQ.focus-window-span` | 1 | piano-roll | piano_roll.rs | model | piano-roll.window-span | built (.31) | .16 |
| `SEQ.piano-roll-automation` | 1 | piano-roll | piano_roll.rs | model | piano-roll automation lane (focus steps, lane points) | .47 | .16 |
| `SEQ.piano-roll-automation-params` | 1 | piano-roll | piano_roll.rs | model | step params (constant) + param.has-locks / rack-macro.has-locks | .47 | .16 |
| `SEQ.piano-roll-items` | 8 | piano-roll | sv/topology_and_visualization.rs | model | piano-roll.notes → note | built (.31) | .16 |
| `SEQ.piano-roll-lanes` | 2 | piano-roll | natives.rs | model | view derivation from pitch-min / pitch-max (lane = pitch-max − note.pitch) | built (.31) | .16 |
| `SEQ.piano-roll-playhead` | 1 | piano-roll | piano_roll.rs | live | piano-roll.playhead (live) | built (.31) | .16 |
| `SEQ.piano-roll-selection` | 1 | piano-roll | piano_roll.rs | model | note.selected | built (.31) | .16 |
| `SEQ.track-automation` | 1 | packages/alez.tracker/src/ui | piano_roll.rs | model | param.has-locks / rack-macro.has-locks (+ step params off their default, in the view) | built (.31) | .20 |
| `SEQ.track-grid-playhead-*` | 1 | packages/alez.tracker/src/ui | piano_roll.rs | live | view derivation from track.playhead and transport.position (live) | built (.31) | .20 |
| `SEQ.track-grid-playhead-current` | 1 | packages/alez.tracker/src/ui | piano_roll.rs | live | the same, of selection.track | built (.31) | .20 |
| `SEQ.track-grid-playhead-row-current` | 1 | packages/alez.tracker/src/ui | piano_roll.rs | live | the same, of selection.track | built (.31) | .20 |
| `SEQ.track-lock-targets` | 1 | packages/alez.tracker/src/ui | piano_roll.rs | model | t.devices / t.midi-devices → d.params, device.macros (rack-macro) | built (.31) | .20 |
| `SEQ.tracker-rows` | 1 | packages/alez.tracker/src/ui | piano_roll.rs | model | step.active / transpose / velocity / ‹param› + param.step-locks / rack-macro.step-locks | built (.31) | .20 |
| `AGENT.generation` | 2 | agent | browser.rs | model | agent.generation | built (.32) | — |
| `AUDIO.workers-choice` | 1 | settings | host_commands/audio_settings.rs | model | settings.audio-workers-choice | built (.32) | .18 |
| `AUDIO.workers-note` | 1 | settings | host_commands/audio_settings.rs | model | settings.audio-workers-note | built (.32) | .18 |
| `AUDIO.workers-options` | 1 | settings | host_commands/audio_settings.rs | model | project.audio-workers-options (an option list, §14.1) | built (.32) | .18 |
| `EXPORT.export-busy` | 4 | export-song | host_commands/export.rs | model | song-export.busy | built (.32) | — |
| `EXPORT.export-default-name` | 1 | export-song | host_commands/export.rs | model | song-export.default-name | built (.32) | — |
| `EXPORT.export-done` | 3 | export-song | host_commands/export.rs | model | song-export.done | built (.32) | — |
| `EXPORT.export-end` | 1 | export-song | host_commands/export.rs | model | song-export.end | built (.32) | — |
| `EXPORT.export-folder` | 1 | export-song | host_commands/export.rs | model | song-export.folder | built (.32) | — |
| `EXPORT.export-message` | 2 | export-song | host_commands/export.rs | model | song-export.message | built (.32) | — |
| `EXPORT.export-output-name` | 1 | export-song | host_commands/export.rs | model | song-export.output-name | built (.32) | — |
| `EXPORT.export-percent` | 1 | export-song | host_commands/export.rs | model | song-export.percent | built (.32) | — |
| `EXPORT.export-project` | 1 | export-song | host_commands/export.rs | model | song-export.project | built (.32) | — |
| `EXPORT.export-reveal-label` | 1 | export-song | host_commands/export.rs | model | song-export.reveal-label | built (.32) | — |
| `MIDI.devices` | 1 | settings | midi_dispatch.rs | model | settings.midi-devices → midi-device (by device id) | built (.32) | .18 |
| `MIDI.error` | 1 | settings | lisp_host/eseq/expr_process.rs | model | settings.midi-error | built (.32) | .18 |
| `MIDI.persistent` | 1 | settings | midi_dispatch.rs | model | settings.midi-persistent | built (.32) | .18 |
| `RETRO.duration` | 9 | retrospective | lisp_host/value_helpers.rs | model | retro.duration | built (.32); ported, legacy removed (.12) | .12 |
| `RETRO.error` | 3 | retrospective | lisp_host/eseq/expr_process.rs | model | retro.error | built (.32); ported, legacy removed (.12) | .12 |
| `RETRO.items` | 5 | retrospective | agent/network.rs | model | retro.items → retro-item | built (.32); ported, legacy removed (.12) | .12 |
| `RETRO.lanes` | 2 | retrospective | retrospective.rs | model | retro.lanes → retro-lane | built (.32); ported, legacy removed (.12) | .12 |
| `RETRO.playing` | 5 | retrospective | sequencer/state/sequencer_state/scene_launch.rs | live | retro.playing (live) | built (.32); ported, legacy removed (.12) | .12 |
| `RETRO.position` | 1 | retrospective | retrospective.rs | live | retro.playhead (live, .12: the host maps the loop position onto the crop) | built (.32); ported, legacy removed (.12) | .12 |
| `RETRO.truncated` | 1 | retrospective | retrospective.rs | model | retro.truncated | built (.32); ported, legacy removed (.12) | .12 |
| `SEQ.browser-preview-playhead` | 3 | sample-import, browser, resample | reactive_tick.rs | live | browser.preview-position (live) | built (.32) | .17 |
| `SEQ.browser-preview-playing` | 4 | browser, sample-import, resample | reactive_tick.rs | model | browser.preview-playing (live) | built (.32) | .17 |
| `SEQ.content-library-epoch` | 2 | browser | lisp_hot_reload.rs | model | browser.library-epoch (the library trees are natives taking a search filter, so a read of the epoch re-lists them) | built (.32) | .17 |
| `SEQ.current-pNroject-name` | 1 | browser | - | model | project.name (typo in browser.lisp) | built (.32) | .17 |
| `SEQ.current-project-name` | 4 | browser, application-menus | sv/project_state.rs | model | project.name | built (.32) | .17 .18 |
| `SEQ.editor-active-macro-action` | 3 | browser | reactive_tick.rs | model | editor.active-macro-action | built (.32) | .17 |
| `SEQ.editor-active-macro-name` | 1 | browser | reactive_tick.rs | model | editor.active-macro | built (.32) | .17 |
| `SEQ.editor-assets` | 2 | patch-macros | reactive_tick.rs | model | editor.assets → editor-asset | built (.32) | .18 |
| `SEQ.editor-buffer-name` | 1 | browser | event_loop.rs | model | editor.buffer | built (.32) | .17 |
| `SEQ.editor-canceling` | 4 | browser | event_loop.rs | model | editor.canceling | built (.32) | .17 |
| `SEQ.editor-error` | 5 | browser | event_loop.rs | model | editor.error | built (.32) | .17 |
| `SEQ.editor-instrument-run-mode` | 4 | browser | host_commands/instrument_authoring.rs | model | editor.run-mode | built (.32) | .17 |
| `SEQ.editor-library-macros` | 2 | patch-macros | reactive_tick.rs | model | editor.library-macros → editor-macro | built (.32) | .18 |
| `SEQ.editor-mode` | 18 | browser, seq-panels | event_loop.rs | model | editor.mode | built (.32) | .11 .17 |
| `SEQ.editor-open-macro` | 2 | patch-macros | reactive_tick.rs | model | editor.open-macro | built (.32) | .18 |
| `SEQ.editor-patch-macros` | 4 | patch-macros | reactive_tick.rs | model | editor.patch-macros → editor-macro | built (.32) | .18 |
| `SEQ.editor-selected-asset` | 1 | patch-macros | reactive_tick.rs | model | editor.selected-asset → asset-info (nil: none) | built (.32) | .18 |
| `SEQ.editor-surface` | 3 | browser | host_commands/instrument_authoring.rs | model | editor.surface | built (.32) | .17 |
| `SEQ.kit-presets` | 2 | browser | host_commands/drum_rack_v2.rs | model | browser.kit-presets → preset-file | built (.32) | .17 |
| `SEQ.learn-abs-distance` | 1 | patch-learn | patch_learn.rs | model | learn.abs-distance | built (.32) | .18 |
| `SEQ.learn-applied` | 1 | patch-learn | patch_learn.rs | model | learn.applied | built (.32) | .18 |
| `SEQ.learn-basin-check` | 1 | patch-learn | patch_learn.rs | model | learn.basin-check | built (.32) | .18 |
| `SEQ.learn-cma-continue` | 2 | patch-learn | host_commands/learn.rs | model | learn.cma-continue | built (.32) | .18 |
| `SEQ.learn-cma-final-epochs` | 2 | patch-learn | host_commands/learn.rs | model | learn.cma-final-epochs | built (.32) | .18 |
| `SEQ.learn-cma-forward-batch` | 2 | patch-learn | host_commands/learn.rs | model | learn.cma-forward-batch | built (.32) | .18 |
| `SEQ.learn-cma-generations` | 3 | patch-learn | host_commands/learn.rs | model | learn.cma-generations | built (.32) | .18 |
| `SEQ.learn-cma-population` | 6 | patch-learn | host_commands/learn.rs | model | learn.cma-population | built (.32) | .18 |
| `SEQ.learn-cma-refine-epochs` | 2 | patch-learn | host_commands/learn.rs | model | learn.cma-refine-epochs | built (.32) | .18 |
| `SEQ.learn-cma-refine-mode` | 2 | patch-learn | host_commands/learn.rs | model | learn.cma-refine-mode | built (.32) | .18 |
| `SEQ.learn-cma-seed` | 2 | patch-learn | host_commands/learn.rs | model | learn.cma-seed | built (.32) | .18 |
| `SEQ.learn-cma-sigma` | 2 | patch-learn | host_commands/learn.rs | model | learn.cma-sigma | built (.32) | .18 |
| `SEQ.learn-current-epoch` | 1 | patch-learn | patch_learn.rs | model | learn.current-epoch | built (.32) | .18 |
| `SEQ.learn-epoch-params` | 1 | patch-learn | patch_learn.rs | model | learn.epoch-params → learn-epoch-param | built (.32) | .18 |
| `SEQ.learn-epochs` | 2 | patch-learn | host_commands/learn.rs | model | learn.epochs | built (.32) | .18 |
| `SEQ.learn-error` | 1 | patch-learn | patch_learn.rs | model | learn.error | built (.32) | .18 |
| `SEQ.learn-final-wav` | 1 | patch-learn | patch_learn.rs | model | learn.final-wav | built (.32) | .18 |
| `SEQ.learn-gate-frames` | 4 | patch-learn | patch_learn.rs | model | learn.gate-frames | built (.32) | .18 |
| `SEQ.learn-improvement-pct` | 1 | patch-learn | patch_learn.rs | model | learn.improvement-pct | built (.32) | .18 |
| `SEQ.learn-local-epochs` | 2 | patch-learn | host_commands/learn.rs | model | learn.local-epochs | built (.32) | .18 |
| `SEQ.learn-loss` | 1 | patch-learn | patch_learn.rs | model | learn.loss | built (.32) | .18 |
| `SEQ.learn-losses` | 1 | patch-learn | patch_learn.rs | model | learn.losses | built (.32) | .18 |
| `SEQ.learn-method` | 7 | patch-learn | host_commands/learn.rs | model | learn.method | built (.32) | .18 |
| `SEQ.learn-optimization-losses` | 1 | patch-learn | patch_learn.rs | model | learn.optimization-losses | built (.32) | .18 |
| `SEQ.learn-phase` | 5 | patch-learn | patch_learn.rs | model | learn.phase | built (.32) | .18 |
| `SEQ.learn-pitch-hz` | 4 | patch-learn | patch_learn.rs | model | learn.pitch-hz | built (.32) | .18 |
| `SEQ.learn-plan-params` | 2 | patch-learn | patch_learn.rs | model | learn.plan-params → learn-plan-param | built (.32) | .18 |
| `SEQ.learn-result-deltas` | 1 | patch-learn | patch_learn.rs | model | learn.result-deltas → learn-delta | built (.32) | .18 |
| `SEQ.learn-seeded-wav` | 1 | patch-learn | patch_learn.rs | model | learn.seeded-wav | built (.32) | .18 |
| `SEQ.learn-stage` | 1 | patch-learn | patch_learn.rs | model | learn.stage | built (.32) | .18 |
| `SEQ.learn-target-name` | 3 | patch-learn | host_commands/learn.rs | model | learn.target-name | built (.32) | .18 |
| `SEQ.learn-target-path` | 3 | patch-learn | host_commands/learn.rs | model | learn.target-path | built (.32) | .18 |
| `SEQ.learn-total-epochs` | 1 | patch-learn | patch_learn.rs | model | learn.total-epochs | built (.32) | .18 |
| `SEQ.project-instrument-engines` | 1 | browser | sv/project_state.rs | model | browser.engines | built (.32) | .17 |
| `SEQ.sidebar-instrument-display-name` | 2 | browser | sv/project_state.rs | model | browser.instrument-label | built (.32) | .17 |
| `SEQ.sidebar-instrument-name` | 5 | browser, application-menus, effects/panel-frame | sv/project_state.rs | model | browser.instrument | built (.32) | .14 .17 .18 |
| `SEQ.sidebar-kind` | 7 | browser | sv/project_state.rs | model | browser.instrument-kind (`kind` is a built-in field) | built (.32) | .17 |
| `SEQ.sidebar-loaded-preset` | 3 | browser | sv/project_state.rs | model | browser.preset | built (.32) | .17 |
| `SEQ.sidebar-presets` | 1 | browser | sv/project_state.rs | model | browser.presets | built (.32) | .17 |
| `SEQ.sidebar-rack-slot-presets` | 1 | browser | sv/project_state.rs | model | browser.rack-slots → slot-presets (each names its rack slot device) | built (.32) | .17 |
| `SEQ.sidebar-selected-sample` | 7 | browser | sv/project_state.rs | model | browser.sample | built (.32) | .17 |
| `SEQ.sidebar-track-index` | 4 | browser | sv/project_state.rs | model | browser.track | built (.32) | .17 |
| `SEQ.sidebar-user-presets` | 1 | browser | sv/project_state.rs | model | browser.user-presets | built (.32) | .17 |
| `SEQ.sound-palette` | 7 | sound-palette | sv/sound_palette.rs | model | sound-palette singleton; sound-palette.sounds → sound (track, patch-id) | built (.32) | .17 |
| `SEQ.sound-presets` | 2 | browser | sv/project_state.rs | model | browser.sound-presets → preset-file | built (.32) | .17 |
| `SEQ.track-instrument-ids` | 1 | browser | sv/track_and_mixer.rs | model | track.instrument-id | built (.32) | .17 |
| `GRAPH.<ggm-route-color-field>` | 4 | scripts/sequencers/graph-neural-group-matrix-demo | lisp_host/eseq/graph_authoring.rs (+ Lisp writes) | model | n.route.color (view derivation from graph-node.route) | built (.33) | .20 |
| `GRAPH.<gvr-route-color-field>` | 4 | scripts/sequencers/graph-neural-variable-reset-demo | lisp_host/eseq/graph_authoring.rs (+ Lisp writes) | model | n.route.color (view derivation from graph-node.route) | built (.33) | .20 |
| `SEQ.<neural->` | 8 | scripts/sequencers/neural-8x8-track-router | sv/topology_and_visualization.rs | model | neuron.selected (the native neural engine) | .50 | .20 |
| `SEQ.generator-mark-*` | 4 | packages/alez.jaki/src/kind | sv/meters_and_modulation.rs | model | jaki generator marks | .52 | .20 |
| `SEQ.graph-sequencers` | 1 | mixer | reactive_tick.rs | model | project.graphs → graph (gid, name, owner) | built (.33); ported (.13), removed | .13 |
| `SEQ.graph-visualizations` | 14 | scripts/sequencers/graph-neural-variable-reset-demo, packages/alez.neural/src/variable-reset, scripts/sequencers/graph-neural-16-demo +5 | sv/topology_and_visualization.rs | model | graph.active / beat / energy / triggers / dampening (live), weights → graph-param.value; event history, deltas, group traces: .51 | built (.33), .51 | .20 |
| `SEQ.neural-dampening-matrix` | 1 | scripts/sequencers/neural-8x8-track-router | sv/topology_and_visualization.rs | model | network.dampening-matrix | .50 | .20 |
| `SEQ.neural-energy-matrix` | 1 | scripts/sequencers/neural-8x8-track-router | sv/topology_and_visualization.rs | live | network.energy-matrix (live) | .50 | .20 |
| `SEQ.neural-networks` | 1 | scripts/sequencers/neural-8x8-track-router | sv/topology_and_visualization.rs | model | neural network kind | .50 | .20 |
| `SEQ.neural-trigger-matrix` | 1 | scripts/sequencers/neural-8x8-track-router | sv/topology_and_visualization.rs | live | network.trigger-matrix (live) | .50 | .20 |
| `SEQ.track-active-notes` | 5 | effects/panel-bodies, scripts/sequencers/graph-neural-8x8-demo, scripts/sequencers/graph-neural-variable-reset-demo +2 | reactive_tick.rs | live | track.active-notes (live; `(note velocity trigger-id)` rows) | built (.33) | .14 .20 |
| `SEQ.track-event-current-beat` | 3 | scripts/processes/process-ui-control-demo, scripts/sequencers/band-coupling-matrix-demo, scripts/sequencers/graph-neural-8x8-demo | ui_replay_probe.rs | live | track events | .51 | .20 |
| `SEQ.track-events` | 3 | scripts/processes/process-ui-control-demo, scripts/sequencers/band-coupling-matrix-demo, scripts/sequencers/graph-neural-8x8-demo | ui_replay_probe.rs | model | track events (demo scripts) | .51 | .20 |
| `SEQ.<rack/groove-amount-field>` | 1 | rack-groove-buffer | sv/rack_groove_fields.rs | model | groove.timing / velocity / random; a pad's share pad-groove.amount (of the playing clip's groove: `(or g.rack-clip.groove g.groove)`) | built (.34) | .19 |
| `SEQ.armed-rack-id` | 2 | mixer, drum-rack-v2 | reactive_tick.rs | model | group.armed (live) | built (.34); ported (.13), kept: drum-rack-v2 | .13 .19 |
| `SEQ.groove-pool` | 2 | rack-groove-buffer | sv/rack_groove_fields.rs | model | project.groove-pool (pool-groove) | built (.34) | .19 |
| `SEQ.rack-clip-active-*` | 3 | sequencer, mixer | sv/topology_and_visualization.rs | model | rack-clip.active | built (.34); ported (.13), kept: sequencer | .11 .13 |
| `SEQ.rack-clip-banks` | 1 | drum-rack-v2 | sv/topology_and_visualization.rs | model | group.clips (rack-clip.cid, name) | built (.34) | .19 |
| `SEQ.rack-clip-index-*` | 1 | sequencer | sv/topology_and_visualization.rs | model | group.rack-clip (an instance, nil while silent; its position rc.index) | built (.34) | .11 |
| `SEQ.rack-clips` | 2 | mixer, drum-rack-v2 | sv/topology_and_visualization.rs | model | rack-clip kind: group.clips, rack-clip.scenes (:scene-clips), group.legacy (no entry) | built (.34); ported (.13), kept: drum-rack-v2 | .13 .19 |
| `SEQ.rack-grooves` | 1 | drum-rack-v2 | sv/rack_groove_fields.rs | model | groove kind (group.groove, rack-clip.groove; lanes, pad-groove shares); the picker: project.groove-pool, project.groove-library | built (.34) | .19 |
| `SEQ.rack-pad-trigger-*` | 3 | sequencer | sv/drum_rack.rs | live | pad.triggered (live; a member track's pad: t.pad) | built (.34) | .11 |
| `SEQ.track-steps` | 2 | rack-groove-buffer | sv/param_fields_and_sync.rs | model | pad.track.steps → step.active | built (.34) | .19 |
| `SEQ.<slot-bar-transpose-field>` | 1 | sequencer | sv/expanded_step.rs | model | track.bar-transposes | built (.35) | .11 |
| `SEQ.<slot-bar-transpose-set-field>` | 1 | sequencer | sv/expanded_step.rs | model | track.bar-transposes (≠ 0; `set-bar-transpose!`) | built (.35) | .11 |
| `SEQ.accum-mode-options` | 1 | effects/track-panels | sv/project_state.rs | model | constant | built (.35) | .14 |
| `SEQ.accumulator-options` | 1 | effects/track-panels | sv/project_state.rs | model | project.accumulator-options | built (.35) | .14 |
| `SEQ.auto-follow` | 2 | seq-core-state, sequencer | reactive_tick.rs | model | selection.auto-follow | built (.35) | .11 |
| `SEQ.bus-effects` | 3 | application-menus, effects/buffers, effects/panel-widgets | event_loop.rs | model | bus.devices | built (.36) | .14 .18 |
| `SEQ.bus-mod-in-level-*` | 1 | mixer | sv/meters_and_modulation.rs | live | bus.mod-in-1 … -4 (live; `(mod-in-level b i)`) | built (.35); ported (.13), removed | .13 |
| `SEQ.bus-output-routes` | 1 | mixer | sv/track_and_mixer.rs | model | bus.output, bus.output-options | built (.35); ported (.13), removed | .13 |
| `SEQ.compiling` | 1 | effects/buffers | sv/host_commands.rs | model | engine.compiling | built (.35) | .14 |
| `SEQ.cpu-overloaded` | 2 | transport | reactive_tick.rs | live | engine.overloaded | built (.35); ported, legacy removed (.12) | .12 |
| `SEQ.fts-options` | 2 | effects/track-panels, effects/scale-editor | sv/project_state.rs | model | project.fts-options | built (.35) | .14 |
| `SEQ.fx-step-cursor-number` | 1 | effects/track-panels | sv/param_fields_and_sync.rs | model | selection.cursor-step (index + 1) | built (.35) | .14 |
| `SEQ.fx-step-parameter-step` | 1 | seq-core-state | sv/topology_and_visualization.rs | model | selection.edit-step | built (.35) | .11 |
| `SEQ.fx-step-selection-count` | 2 | effects/track-panels, seq-core-state | sv/param_fields_and_sync.rs | model | (len selection.steps) | built (.35) | .11 .14 |
| `SEQ.fx-step-value-*` | 1 | effects/track-panels | step_print.rs | model | step.‹param› of selection.edit-step | built (.35) | .14 |
| `SEQ.midi-effects` | 1 | effects/buffers | event_loop.rs | model | track.midi-devices | built (.36) | .14 |
| `SEQ.mixer-track-delete-target-*` | 1 | mixer | sv/steps_and_pattern.rs | model | track.delete-target | built (.35); ported (.13), removed | .13 |
| `SEQ.mod-in-level-*` | 1 | mixer | sv/meters_and_modulation.rs | live | track.mod-in-1 … -4 (live; `(mod-in-level t i)`) | built (.35); ported (.13), removed | .13 |
| `SEQ.mod-out-level-*` | 1 | mixer | sv/meters_and_modulation.rs | live | track.mod-out-level (live) | built (.35); ported (.13), removed | .13 |
| `SEQ.mod-routes` | 6 | mixer | reactive_sync.rs | model | route kind, `(routes)` | built (.35); ported (.13), removed | .13 |
| `SEQ.mute-group-options` | 1 | effects/track-panels | sv/project_state.rs | model | constant | built (.35) | .14 |
| `SEQ.rack-slot-delete-target-*` | 1 | effects/instrument-panel | sv/steps_and_pattern.rs | model | device.delete-target | built (.36) | .14 |
| `SEQ.roll-rate` | 1 | transport | reactive_tick.rs | live | transport.roll-rate | built (.35); ported, legacy removed (.12) | .12 |
| `SEQ.selected-mod-routes` | 2 | mixer | sv/steps_and_pattern.rs | model | route.selected | built (.35); ported (.13), removed | .13 |
| `SEQ.sequence-rolling` | 1 | transport | reactive_tick.rs | live | transport.sequence-rolling | built (.35); ported, legacy removed (.12) | .12 |
| `SEQ.sync-labels` | 8 | sequencer, step-grid, seqv-track-params +1 | natives.rs | model | project.sync-options | built (.35) | .11 |
| `SEQ.tp-accum-limit` | 1 | effects/track-panels | sv/project_state.rs | model | track.accum-limit | built (.35) | .14 |
| `SEQ.tp-accum-mode` | 1 | effects/track-panels | sv/project_state.rs | model | track.accum-mode | built (.35) | .14 |
| `SEQ.tp-accumulator` | 1 | effects/track-panels | sv/project_state.rs | model | track.accumulator | built (.35) | .14 |
| `SEQ.tp-fts` | 2 | effects/track-panels, effects/scale-editor | sv/project_state.rs | model | track.fts | built (.35) | .14 |
| `SEQ.tp-gate` | 4 | effects/sampler-panel | sv/project_state.rs | model | track.gate | built (.35) | .14 |
| `SEQ.tp-max-polyphony` | 2 | mixer, effects/track-panels | host_commands/rack.rs | model | track.max-polyphony (the track's own; a rack slot's: device.voices) | built (.35, .36); ported (.13), kept: track-panels | .13 .14 |
| `SEQ.tp-mono-trigger` | 1 | effects/track-panels | sv/project_state.rs | model | track.mono-trigger | built (.35) | .14 |
| `SEQ.tp-mute-group` | 1 | effects/track-panels | sv/project_state.rs | model | track.mute-group (`:int`; label `(nth mute-group-options g)`) | built (.35) | .14 |
| `SEQ.tp-poly` | 12 | effects/track-panels, mixer, effects/instrument-panel | sv/project_state.rs | model | track.poly (the track's own; a rack slot's: (> device.voices 1)) | built (.35, .36); ported (.13), kept: track-panels | .13 .14 |
| `SEQ.tp-rack-slot-idx` | 5 | effects/track-panels, mixer | sv/project_state.rs | model | selection.rack-slot (-1: no rack) | built (.35); ported (.13), kept: track-panels | .13 .14 |
| `SEQ.tp-supports-mono-trigger` | 2 | effects/track-panels | sv/project_state.rs | model | track.supports-mono-trigger | built (.35) | .14 |
| `SEQ.tp-swing` | 2 | effects/track-panels | sv/project_state.rs | model | track.swing | built (.35) | .14 |
| `SEQ.tp-swing-resolution` | 1 | effects/track-panels | sv/project_state.rs | model | track.swing-resolution | built (.35) | .14 |
| `SEQ.tp-tuning-base` | 1 | effects/scale-editor | sv/param_fields_and_sync.rs | model | degree.base of track.tuning.degrees | built (.35) | .14 |
| `SEQ.tp-tuning-enabled` | 1 | effects/scale-editor | sv/param_fields_and_sync.rs | model | degree.enabled [s] of track.tuning.degrees | built (.35) | .14 |
| `SEQ.tp-tuning-labels` | 1 | effects/scale-editor | sv/param_fields_and_sync.rs | model | degree.label of track.tuning.degrees | built (.35) | .14 |
| `SEQ.tp-tuning-mode` | 4 | effects/scale-editor | sv/param_fields_and_sync.rs | model | tuning.mode (`set!`) | built (.35) | .14 |
| `SEQ.tp-tuning-morph` | 1 | effects/scale-editor | sv/param_fields_and_sync.rs | model | tuning.morph (`set!`; 0-1; the legacy field is percent) | built (.35) | .14 |
| `SEQ.tp-tuning-offsets` | 1 | effects/scale-editor | sv/param_fields_and_sync.rs | model | degree.offset (`set!`) | built (.35) | .14 |
| `SEQ.tp-tuning-on` | 1 | effects/scale-editor | sv/param_fields_and_sync.rs | model | tuning.on | built (.35) | .14 |
| `SEQ.tp-tuning-period` | 1 | effects/scale-editor | sv/param_fields_and_sync.rs | model | tuning.period | built (.35) | .14 |
| `SEQ.tp-tuning-pitches` | 1 | effects/scale-editor | sv/param_fields_and_sync.rs | model | degree.pitch of track.tuning.degrees | built (.35) | .14 |
| `SEQ.tp-tuning-root` | 1 | effects/scale-editor | sv/param_fields_and_sync.rs | model | tuning.root (`set!`) | built (.35) | .14 |
| `SEQ.tp-voice-priority` | 1 | effects/track-panels | sv/project_state.rs | model | track.voice-priority | built (.35) | .14 |
| `SEQ.track-mod-output-available` | 2 | mixer | sv/track_and_mixer.rs | model | track.mod-output | built (.35); ported (.13), removed | .13 |
| `SEQ.track-output-options` | 1 | mixer | host_commands/routing.rs | model | project.output-options (bus instances; nil is sends only) | built (.35); ported (.13), removed | .13 |
| `SEQ.track-outputs` | 1 | mixer | sv/track_and_mixer.rs | model | track.output (a bus; nil is sends only) | built (.35); ported (.13), removed | .13 |
| `SEQ.tuning-root-options` | 1 | effects/scale-editor | sv/project_state.rs | model | constant | built (.35) | .14 |
| `SEQV.<adsr-stage-active-field>` | 1 | effects/custom-ui-sections | Lisp (reactive-set) | Lisp-owned | custom-ui view state | view-local | .14 |
| `SEQV.<channel>` | 20 | arrangement | Lisp (reactive-set) | Lisp-owned | arrangement view singleton (arr-*) | view-local | .15 |
| `SEQV.<cursor-highlight-field>` | 1 | sequencer | Lisp (reactive-set) | Lisp-owned | sequencer view singleton (cursor) | view-local | .11 |
| `SEQV.<expanded-track-field>` | 1 | sequencer | Lisp (reactive-set) | Lisp-owned | sequencer view singleton (expanded tracks) | view-local | .11 |
| `SEQV.<sel-bus-vis-field>` | 1 | seq-core-state | Lisp (reactive-set) | Lisp-owned | bus selection (view singleton) | view-local | .11 |
| `SEQV.<sel-group-vis-field>` | 1 | seq-core-state | Lisp (reactive-set) | Lisp-owned | group selection (view singleton) | view-local | .11 |
| `SEQV.arr-content-length` | 3 | arrangement | Lisp (reactive-set) | Lisp-owned | arrangement view singleton | view-local | .15 |
| `SEQV.arr-view-duration` | 3 | arrangement | Lisp (reactive-set) | Lisp-owned | arrangement view singleton | view-local | .15 |
| `SEQV.arr-view-start` | 3 | arrangement | Lisp (reactive-set) | Lisp-owned | arrangement view singleton | view-local | .15 |
| `SEQV.cursor-field-*` | 1 | sequencer | Lisp (reactive-set) | Lisp-owned | sequencer view singleton (cursor) | view-local | .11 |
| `SEQV.cursor-step-*` | 1 | sequencer | Lisp (reactive-set) | Lisp-owned | sequencer view singleton (cursor) | view-local | .11 |
| `SEQV.piano-roll-arrangement-mode` | 1 | piano-roll | Lisp (reactive-set) | Lisp-owned | piano-roll view singleton | view-local | .16 |
| `SEQV.plk-t-*` | 2 | effects/track-panels | Lisp (reactive-set) | Lisp-owned | p-lock menu view singleton | view-local | .14 |
| `SEQV.plk-var-b` | 1 | effects/param-controls | Lisp (reactive-set) | Lisp-owned | p-lock menu view singleton | view-local | .14 |
| `SEQV.plk-var-g` | 1 | effects/param-controls | Lisp (reactive-set) | Lisp-owned | p-lock menu view singleton | view-local | .14 |
| `SEQV.plk-var-r` | 1 | effects/param-controls | Lisp (reactive-set) | Lisp-owned | p-lock menu view singleton (:rgb) | view-local | .14 |
| `SEQV.rack-clip-center-*` | 1 | mixer | Lisp (reactive-set) | Lisp-owned | mixer view singleton | view-local; ported (.13), removed | .13 |
| `:bindable` | 97 | effects/physical-model-surface, sequencer, effects/drum-surface +24 | - | - | delete (ignored since stage 5) | remove (gone from .12's files); gone from .13's files | .11 .12 .13 .14 .20 .21 |
| `<ns-var namespace>` | 3 | bindings | - | - | bindings.lisp generic scopes → kinds | remove | .18 |
| `reactive-value` | 75 | instruments/Synths/Heat/ui, effects/param-controls, scripts/sequencers/graph-neural-variable-reset-demo +27 | - | - | t.x / #'t.x read as a value (§8) | remove; gone from .13's files | .11 .13 .14 .20 .21 |
| `SEQ.bus-ids` | 10 | mixer, drum-rack-v2, seq-core-state +1 | sv/track_and_mixer.rs | model | instance identity | remove; ported (.13), kept: drum-rack-v2, seq-core-state | .11 .13 .19 |
| `SEQ.delete-target-version` | 4 | mixer, browser, application-menus +1 | reactive_tick.rs | model | implicit (fields re-render) | remove; ported (.13), kept: browser, application-menus + | .13 .14 .17 .18 |
| `SEQ.num-patterns` | 6 | transport, macros, scene-banks | sv/topology_and_visualization.rs | model | (len (scenes)) | remove; ported (.12), kept: macros (.18) | .12 .18 |
| `SEQ.num-tracks` | 38 | mixer, track-collapse, sequencer +10 | reactive_sync.rs | model | (len (tracks)) | remove; ported (.13), kept: many | .11 .13 .14 .17 .18 .19 |
| `SEQ.rack-panel-view-generation` | 1 | effects/state | sv/project_state.rs | model | implicit | remove | .14 |
| `SEQ.scene-bank-view-generation` | 1 | scene-banks | sv/project_state.rs | model | implicit (collections re-render) | remove; ported, legacy removed (.12) | .12 |
| `SEQ.track-ids` | 30 | sequencer, arrangement, mixer +1 | reactive_sync.rs | model | instance identity (subtree :key t) | remove; ported (.13), kept: sequencer, arrangement + | .11 .13 .15 .20 |
| `SEQ.instances` | 3 | mixer, browser, packages/alez.neural/src/variable-reset | lisp_host/eseq/process_dsl_parse.rs | model | package instances (live_instances) | keep; ported (.13), kept: browser, alez.neural | .13 .17 .20 |
| `THEME.buffer_bg` | 1 | sequencer | - | model | THEME stays (theme namespace, not host state) | keep | .11 |
| `THEME.plock_base` | 2 | effects/panel-bodies, effects/track-panels | - | model | THEME stays (theme namespace, not host state) | keep | .14 |
| `THEME.scene_clip_bg` | 1 | arrangement | - | model | THEME stays (theme namespace, not host state) | keep | .15 |
| `GRAPH` via `bind-graph` / `bind-graph-config` (103 calls) | 103 | scripts/sequencers/graph-*, packages/alez.neural | lisp_host/eseq/graph_authoring.rs | model | graph-node.‹field›, graph-param.value, graph.‹field› (`#'` bindings) | built (.33) | .20 |
| `reactive-set "GRAPH"` (52 writes) | 52 | scripts/sequencers/graph-* | Lisp | Lisp-owned | graph-node / graph-param / graph `:set` (`set-graph`) | built (.33) | .20 |
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
        (grid :cols 8 :col-width (sc 8) :row-height (sc 4)
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
