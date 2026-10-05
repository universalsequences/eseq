# Kind bindings

Status: spec rev 3, 2026-10-04. Stages 1–6 built, stage 7 in part (§14; 7, 7b and 7i built) (§3.1, §3.2, §3.3, §3.4, §4, §7.1, §7.3, §8, §9 notes). Bead: epic `eseq-0l17` (`bd list --label kind-bindings`).
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
  project`, and since stage 7 `send bus group master engine`, since 7b `param`, since 7i `route`) before
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
  | `device` | `(track slot)`, slot part = slot + 1 (since 7b `(track did)`, §14.2b) | `track track`, `slot :int` (-1 = instrument), `name :string`, `enabled :bool` |
  | `scene` | `(index)` | `index :int`, `number :int` (1-based in its bank), `name :string`, `active :bool`, `queued :bool`, `bank bank` |
  | `bank` | `(index)` | `index :int`, `label :string`, `scenes (list-of scene)`, `playing :bool` |
  | `transport` | `()` | `playing :bool` [`seq-set-playing`], `recording :bool` [`seq-set-recording`], `scene scene`, `queued scene` (nil when none), `launch-quantize :string` |
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
  form's string value, as before. Making it fatal broke a content load:
  `rec-arm-dot` in `ui/legacy/mixer.lisp` calls `eseq.materials/color`,
  which is unknown when that file loads without `ui/materials.lisp`
  (`legacy_mixer_definitions_are_top_level_and_source_loads`); it was
  silently unregistered before. No other content `defwidget` hits one.
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
   Built (stage 7i, eseq-0l17.35): track settings, routing (`route`, bus
   outputs, mod port levels), option constants, selection, transport and
   engine extras (§14.2c).
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
  sync resolutions, the accumulators, the track outputs) are `project`
  fields the host publishes (`project.fts-options`, `sync-options`,
  `accumulator-options`, `output-options`); short fixed enums
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
  is re-resolved from the `App` per tick read (a sample load or voice
  rebuild moves no model counter); a cold read uses the sampler voices of
  the last model sync.
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
  (eseq-0l17.36); modulation display, process mapping, tensors, base
  note, key locks, rack and project macros, the variant chip list, the
  neural-selection display override and the rest of the panel data
  (eseq-0l17.37).

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
  → `device.delete-target`, and a rack track's slot voices.

### 14.3 Follow-up beads

Each port bead depends on the beads whose rows it uses (`bd dep`).

| Tag | Bead | Kinds | Ports blocked |
|---|---|---|---|
| 7b | eseq-0l17.28 (built) | `param` under `device` (values, p-lock display, print latch), `device.playhead`, step p-lock render (`plocked`, `lock-kind`, `variant-color`), send p-lock flags | .11 .13 .14 .18 .19 .21 |
| 7b-2 | eseq-0l17.36 | devices (and params) for MIDI fx, bus effects, rack slots; `bus.devices`, `track.midi-devices`, `device.delete-target` (from 7i) | .13 .14 .18 .19 .21 |
| 7b-3 | eseq-0l17.37 | panel extras: modulation display, process mapping, tensors, base note, key locks, rack and project macros, variant chip list, neural-selection display | .14 .18 |
| 7c | eseq-0l17.29 | `lane`, process slots and scopes, process library singleton | .11 .14 .20 |
| 7d | eseq-0l17.30 | `song` singleton, `clip`, pattern `cell`, `track.governed` / `latched` | .11 .12 .13 .15 .17 .20 |
| 7e | eseq-0l17.31 | `note`, `piano-roll` singleton, tracker rows and grid playheads | .16 .20 |
| 7f | eseq-0l17.32 | `browser`, `sound`, `editor`, `learn`, `retro`, `export`, settings and agent singletons, `track.instrument-id` | .12 .17 .18 |
| 7g | eseq-0l17.33 | `graph-node`, neural networks, visualizations, generator marks, track events | .20 |
| 7h | eseq-0l17.34 | rack pads, rack clips, grooves, armed rack | .11 .13 .19 |
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
| `SEQ.<track-pan-field>` | 1 | mixer | sv/track_and_mixer.rs | model | track.pan | built (.10) | .13 |
| `SEQ.<track-volume-field>` | 2 | mixer, sequencer | sv/track_and_mixer.rs | model | track.volume | built (.10) | .11 .13 |
| `SEQ.auxas` | 1 | seqv-track-params | reactive_sync.rs | model | step.aux-a | built (.10) | .11 |
| `SEQ.bpm` | 2 | effects/builtin/phaser-flanger, transport | bounce/job.rs | live | transport.bpm | built (.10) | .12 .14 |
| `SEQ.bus-mutes` | 5 | mixer, sequencer, legacy/mixer | sv/track_and_mixer.rs | model | bus.muted | built (.10) | .11 .13 |
| `SEQ.bus-names` | 25 | mixer, legacy/mixer, seq-core-state +2 | sv/track_and_mixer.rs | model | bus.name | built (.10) | .11 .13 .14 |
| `SEQ.bus-peak-*` | 3 | mixer, sequencer | sv/meters_and_modulation.rs | live | bus.peak | built (.10) | .11 .13 |
| `SEQ.bus-solos` | 4 | mixer, sequencer, legacy/mixer | sv/track_and_mixer.rs | model | bus.soloed | built (.10) | .11 .13 |
| `SEQ.bus-volumes` | 3 | mixer, sequencer, legacy/mixer | sv/track_and_mixer.rs | model | bus.volume | built (.10) | .11 .13 |
| `SEQ.cpu-load-pct` | 1 | transport | reactive_tick.rs | live | engine.cpu-load | built (.10) | .12 |
| `SEQ.current-pattern` | 18 | transport, arrangement, mixer +10 | sv/topology_and_visualization.rs | model | transport.scene (s.index) | built (.10) | .12 .13 .15 .20 |
| `SEQ.current-track` | 108 | piano-roll, effects/process-panel, browser +19 | piano_roll.rs | live | selection.track | built (.10) | .11 .13 .14 .15 .16 .17 .18 .19 .20 |
| `SEQ.delays` | 1 | seqv-track-params | event_loop.rs | model | step.delay | built (.10) | .11 |
| `SEQ.durations` | 2 | seq-core-state, seqv-track-params | event_loop.rs | model | step.duration | built (.10) | .11 |
| `SEQ.groups` | 53 | mixer, drum-rack-v2, seq-core-state +12 | project.rs | model | group.* via (groups), track.group | built (.10) | .11 .13 .17 .19 .20 |
| `SEQ.master-peak-l` | 2 | mixer, transport | event_loop.rs | live | master.peak-l | built (.10) | .12 .13 |
| `SEQ.master-peak-r` | 2 | mixer, transport | event_loop.rs | live | master.peak-r | built (.10) | .12 .13 |
| `SEQ.master-recording` | 2 | transport | reactive_tick.rs | live | master.recording | built (.10) | .12 |
| `SEQ.metronome` | 1 | transport | host_commands/misc.rs | live | transport.metronome | built (.10) | .12 |
| `SEQ.output-latency-ms` | 1 | transport | reactive_tick.rs | live | engine.latency-ms | built (.10) | .12 |
| `SEQ.pans` | 2 | seq-core-state, seqv-track-params | event_loop.rs | model | step.pan | built (.10) | .11 |
| `SEQ.playhead-active-*` | 1 | step-grid | sv/meters_and_modulation.rs | live | step.playing | built (.10) | .11 |
| `SEQ.playhead-page` | 1 | seq-core-state | sv/meters_and_modulation.rs | live | track.playhead (page = playhead / 16 in the view) | built (.10) | .11 |
| `SEQ.playing` | 11 | retrospective, transport, effects/track-panels +4 | sequencer/state/sequencer_state/scene_launch.rs | live | transport.playing | built (.10) | .11 .12 .14 .20 |
| `SEQ.queued-scene` | 2 | transport | event_loop.rs | model | transport.queued | built (.10) | .12 |
| `SEQ.record-armed` | 4 | mixer, sequencer, legacy/mixer | event_loop.rs | live | track.armed | built (.10) | .11 .13 |
| `SEQ.record-quantize` | 1 | transport | host_commands/misc.rs | live | transport.record-quantize | built (.10) | .12 |
| `SEQ.recording` | 4 | effects/track-panels, transport, effects/param-controls | reactive_sync.rs | live | transport.recording | built (.10) | .12 .14 |
| `SEQ.retrig-rates` | 2 | seq-core-state, seqv-track-params | reactive_sync.rs | model | step.retrig-rate | built (.10) | .11 |
| `SEQ.retrigs` | 2 | seq-core-state, seqv-track-params | reactive_sync.rs | model | step.retrig | built (.10) | .11 |
| `SEQ.roll-mode` | 2 | transport | reactive_tick.rs | live | transport.roll-mode | built (.10) | .12 |
| `SEQ.scene-banks` | 2 | scene-banks | sv/song_state.rs | model | (banks) → bank.label/scenes | built (.10) | .12 |
| `SEQ.scene-launch-quantize` | 6 | transport, drum-rack-v2, mixer | rack_clip_switch_probe.rs | model | transport.launch-quantize | built (.10) | .12 .13 .19 |
| `SEQ.scene-names` | 8 | browser, arrangement | sv/song_state.rs | model | scene.name | built (.10) | .15 .17 |
| `SEQ.selected-steps` | 4 | step-grid, effects/param-controls | reactive_tick.rs | live | step.selected | built (.10) | .11 .14 |
| `SEQ.selected-tracks` | 5 | mixer, step-grid-interactions | sv/steps_and_pattern.rs | live | selection.tracks | built (.10) | .11 .13 |
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
| `SEQ.tp-is-rack` | 5 | effects/track-panels, mixer | sv/project_state.rs | model | selection.track.rack | built (.10) | .13 .14 |
| `SEQ.tp-num-steps` | 8 | seq-grid-mode, seq-core-state, piano-roll +2 | reactive_sync.rs | model | selection.track.num-steps | built (.10) | .11 .14 .16 |
| `SEQ.tp-timebase` | 2 | step-grid, effects/track-panels | sv/project_state.rs | model | selection.track.timebase | built (.10) | .11 .14 |
| `SEQ.track-auxas` | 1 | seqv-track-params | reactive_sync.rs | model | step.aux-a | built (.10) | .11 |
| `SEQ.track-bus-sends` | 2 | mixer, midi-midimix | reactive_sync.rs | model | track.sends → send.bus, send.display | built (.10) | .13 |
| `SEQ.track-collapsed` | 2 | track-collapse | reactive_sync.rs | live | track.collapsed | built (.10) | .11 |
| `SEQ.track-color-b-effective` | 1 | sequencer | sv/track_and_mixer.rs | model | track.color × track.audible | built (.10) | .11 |
| `SEQ.track-color-g-effective` | 1 | sequencer | sv/track_and_mixer.rs | model | track.color × track.audible | built (.10) | .11 |
| `SEQ.track-color-r-effective` | 1 | sequencer | sv/track_and_mixer.rs | model | track.color × track.audible (dim in the shader) | built (.10) | .11 |
| `SEQ.track-colors` | 21 | mixer, rack-groove-buffer, arrangement +12 | sv/track_and_mixer.rs | model | track.color | built (.10) | .11 .13 .14 .15 .16 .19 .20 |
| `SEQ.track-delays` | 1 | seqv-track-params | reactive_sync.rs | model | step.delay | built (.10) | .11 |
| `SEQ.track-durations` | 1 | seqv-track-params | reactive_sync.rs | model | step.duration | built (.10) | .11 |
| `SEQ.track-instrument-types` | 14 | track-collapse, mixer, application-menus | sv/track_and_mixer.rs | model | track.instrument-type | built (.10) | .11 .13 .18 |
| `SEQ.track-length-row-*` | 1 | sequencer | sv/expanded_step.rs | model | track.num-steps | built (.10) | .11 |
| `SEQ.track-muted-effective` | 11 | mixer, sequencer, legacy/mixer +1 | sv/track_and_mixer.rs | live | not track.audible | built (.10) | .11 .13 .14 |
| `SEQ.track-mutes` | 3 | mixer, sequencer, legacy/mixer | reactive_sync.rs | live | track.muted | built (.10) | .11 .13 |
| `SEQ.track-names` | 28 | sequencer, mixer, packages/alez.jaki/src/kind +14 | reactive_sync.rs | model | track.name | built (.10) | .11 .13 .14 .16 .18 .19 .20 |
| `SEQ.track-num-steps` | 5 | sequencer, packages/alez.tracker/src/ui | event_loop.rs | model | track.num-steps | built (.10) | .11 .20 |
| `SEQ.track-pans` | 1 | seqv-track-params | reactive_sync.rs | model | step.pan (per-step lists, see steps) | built (.10) | .11 |
| `SEQ.track-peak-*` | 3 | mixer, sequencer, legacy/mixer | sv/meters_and_modulation.rs | live | track.peak | built (.10) | .11 .13 |
| `SEQ.track-playhead-page-*` | 1 | sequencer | sv/expanded_step.rs | live | track.playhead | built (.10) | .11 |
| `SEQ.track-playhead-row-*` | 1 | sequencer | sv/expanded_step.rs | live | track.playhead | built (.10) | .11 |
| `SEQ.track-playhead-row-active-*` | 1 | sequencer | sv/expanded_step.rs | live | track.playhead | built (.10) | .11 |
| `SEQ.track-retrig-rates` | 1 | seqv-track-params | reactive_sync.rs | model | step.retrig-rate | built (.10) | .11 |
| `SEQ.track-retrigs` | 1 | seqv-track-params | sv/topology_and_visualization.rs | model | step.retrig | built (.10) | .11 |
| `SEQ.track-selected-*` | 3 | mixer, seq-core-state | sv/steps_and_pattern.rs | model | track.selected / (member t selection.tracks) | built (.10) | .11 .13 |
| `SEQ.track-solos` | 3 | mixer, sequencer, legacy/mixer | reactive_sync.rs | live | track.soloed | built (.10) | .11 .13 |
| `SEQ.track-syncs` | 1 | seqv-track-params | reactive_sync.rs | model | step.sync | built (.10) | .11 |
| `SEQ.track-timebases` | 2 | sequencer | sv/param_fields_and_sync.rs | model | track.timebase | built (.10) | .11 |
| `SEQ.track-transposes` | 1 | seqv-track-params | host_commands/step_history.rs | model | step.transpose | built (.10) | .11 |
| `SEQ.track-velocities` | 1 | seqv-track-params | reactive_sync.rs | model | step.velocity | built (.10) | .11 |
| `SEQ.track-volumes` | 3 | sequencer, legacy/mixer | reactive_sync.rs | live | track.volume | built (.10) | .11 .13 |
| `SEQ.transport-playhead` | 1 | transport | ui_replay_probe.rs | live | transport.position | built (.10) | .12 |
| `SEQ.transposes` | 2 | seq-core-state, seqv-track-params | reactive_sync.rs | model | step.transpose | built (.10) | .11 |
| `SEQ.velocities` | 2 | seq-core-state, seqv-track-params | app/retrospective.rs | model | step.velocity | built (.10) | .11 |
| `SEQV.<sel-track-vis-field>` | 1 | seq-core-state | Lisp (reactive-set) | Lisp-owned | track.selected | built (.10) | .11 |
| `<ns-var name>` | 2 | effects/drum-surface | custom_ui.rs | - | param.value via (device-param d "x") | built (.28) | .14 |
| `SEQ.<get>` | 23 | effects/param-controls, effects/instrument-panel, effects/sampler-panel +9 | sv/param_fields_and_sync.rs, instrument_panel.rs, effects_panel.rs | model | param.value / param.name (panel :value-field, :label-field, :name-field, :short-field); MIDI fx / bus / rack slot params .36, rack macro names .37 | built (.28) | .14 .16 .18 .19 .20 .21 |
| `SEQ.<slot-field>` | 12 | sequencer | sv/expanded_step.rs | model | step.active/selected/playing/plocked/lock-kind/variant-color through the view's own slot→step map (expanded-step projection removed) | built (.28) | .11 |
| `SEQ.<var field>` | 8 | effects/param-controls, effects/custom-ui-runtime, mixer +1 | sv/param_fields_and_sync.rs | model | param.value / send.display (field strings from panel data); mod / process fields .37 | built (.28) | .13 .14 |
| `SEQ.effects` | 3 | application-menus, effects/index, effects/buffers | lisp_host/dgen/instrument_storage.rs | model | track.devices → device.params (other panel data .37) | built (.28) | .14 .18 |
| `SEQ.instrument-panel` | 10 | effects/param-controls, browser, effects/index +3 | reactive_tick.rs | model | device panel data (device.params; rack slots .36, key locks / macros / modulation .37) | built (.28) | .14 .17 .18 |
| `SEQ.macros` | 5 | macros, effects/param-controls | project.rs | model | rack macros (group.macros) and project macros | .37 | .14 .18 |
| `SEQ.sampler-playhead` | 1 | effects/sampler-panel | reactive_tick.rs | live | device.playhead (live) | built (.28) | .14 |
| `SEQ.seq-track-step-plock-kind-*` | 1 | sequencer | sv/steps_and_pattern.rs | model | step.lock-kind | built (.28) | .11 |
| `SEQ.seq-track-step-plocked-*` | 1 | sequencer | sv/steps_and_pattern.rs | model | step.plocked | built (.28) | .11 |
| `SEQ.seq-track-step-variant-b-*` | 1 | sequencer | - | model | step.variant-color | built (.28) | .11 |
| `SEQ.seq-track-step-variant-g-*` | 1 | sequencer | - | model | step.variant-color | built (.28) | .11 |
| `SEQ.seq-track-step-variant-r-*` | 1 | sequencer | - | model | step.variant-color | built (.28) | .11 |
| `SEQ.step-has-plocks` | 2 | step-grid | reactive_tick.rs | model | step.plocked | built (.28) | .11 |
| `SEQ.track-plock-any` | 1 | effects/param-controls | event_loop.rs | model | param.has-locks, send.has-locks | built (.28) | .14 |
| `SEQ.track-plock-printing` | 1 | effects/param-controls | step_print.rs | model | param.printing | built (.28) | .14 |
| `SEQ.track-plock-variants` | 3 | effects/track-panels, effects/param-controls | reactive_sync.rs | model | step.variant-color (built .28) + variant chip list | .37 | .14 |
| `SEQ.track-plocks` | 9 | effects/track-panels, effects/param-controls | reactive_sync.rs | model | param.locked / param.base (the -on / -def projections; step panel rows from device.params) | built (.28) | .14 |
| `SEQ.process-lanes` | 3 | seqv-track-params, seq-grid-mode, sequencer | input.rs | model | lane kind | .29 | .11 |
| `SEQ.process-library` | 3 | sequencer, packages/alez.neural/src/variable-reset | input.rs | model | processes singleton | .29 | .11 .20 |
| `SEQ.process-run-errors` | 1 | sequencer | reactive_tick.rs | model | processes.errors | .29 | .11 |
| `SEQ.process-scope-cells` | 1 | sequencer | ui_replay_probe.rs | live | process scope (live) | .29 | .11 |
| `SEQ.process-slots` | 2 | effects/process-panel | input.rs | model | selection.track.processes | .29 | .14 |
| `SEQ.track-lane-patch` | 2 | sequencer | input.rs | model | lane.patch | .29 | .11 |
| `SEQ.track-process-lane-values` | 2 | seqv-track-params, packages/alez.tracker/src/ui | sv/param_fields_and_sync.rs | model | lane.values | .29 | .11 .20 |
| `SEQ.track-process-lanes` | 2 | seqv-track-params, packages/alez.tracker/src/ui | sv/topology_and_visualization.rs | model | track.lanes | .29 | .11 .20 |
| `SEQ.track-process-scopes` | 3 | sequencer | ui_replay_probe.rs | live | process scope (live) | .29 | .11 |
| `SEQ.track-process-slots` | 4 | sequencer, seqv-track-params, scripts/sequencers/band-coupling-matrix-demo | input.rs | model | track.processes | .29 | .11 .20 |
| `SEQ.queued-track-clips` | 1 | mixer | event_loop.rs | model | cell.queued | .30 | .13 |
| `SEQ.scene-spans` | 9 | arrangement | sv/song_state.rs | model | clip spans | .30 | .15 |
| `SEQ.song-bound-clip` | 2 | arrangement, sound-palette | sv/song_state.rs | model | song.bound-clip | .30 | .15 .17 |
| `SEQ.song-clip-sounds` | 2 | arrangement | sv/sound_palette.rs | model | clip.sound | .30 | .15 |
| `SEQ.song-cursor-beats` | 1 | transport | sv/song_state.rs | model | song.cursor | .30 | .12 |
| `SEQ.song-edit-error` | 2 | arrangement | sv/song_state.rs | model | song.edit-error | .30 | .15 |
| `SEQ.song-end-beat` | 2 | arrangement | sv/song_state.rs | model | song.end | .30 | .15 |
| `SEQ.song-lane-events` | 4 | arrangement | sv/song_state.rs | model | clip kind | .30 | .15 |
| `SEQ.song-lanes` | 7 | arrangement, sound-palette | sv/song_state.rs | model | clip lanes | .30 | .15 .17 |
| `SEQ.song-manual-latch` | 2 | transport | sv/song_state.rs | model | song.manual-latch | .30 | .12 |
| `SEQ.song-mode` | 2 | transport, arrangement | sv/song_state.rs | model | song.mode | .30 | .12 .15 |
| `SEQ.song-pending` | 8 | arrangement | sv/song_state.rs | model | song.pending | .30 | .15 |
| `SEQ.song-position-beats` | 5 | arrangement, transport | sv/song_state.rs | live | song.position (live) | .30 | .12 .15 |
| `SEQ.song-region` | 31 | arrangement | sv/song_state.rs | model | song.region | .30 | .15 |
| `SEQ.song-scene-latched` | 1 | arrangement | sv/song_state.rs | model | song.scene-latched | .30 | .15 |
| `SEQ.song-track-governed` | 3 | sequencer | sv/song_state.rs | model | track.governed | .30 | .11 |
| `SEQ.song-track-latched` | 1 | arrangement | sv/song_state.rs | model | track.latched | .30 | .15 |
| `SEQ.track-pattern-cell-active-*` | 2 | mixer, arrangement | sv/steps_and_pattern.rs | model | cell.active | .30 | .13 .15 |
| `SEQ.track-pattern-cell-assigned-*` | 1 | mixer | sv/steps_and_pattern.rs | model | cell.assigned | .30 | .13 |
| `SEQ.track-pattern-cell-override-*` | 1 | mixer | sv/steps_and_pattern.rs | model | cell.override | .30 | .13 |
| `SEQ.track-pattern-cell-selected-*` | 1 | mixer | sv/steps_and_pattern.rs | model | cell.selected | .30 | .13 |
| `SEQ.track-pattern-cells` | 4 | mixer, arrangement | sv/track_and_mixer.rs | model | cell kind (track scene) | .30 | .13 .15 |
| `SEQ.focus-clip-end` | 3 | piano-roll | piano_roll.rs | model | piano-roll.clip-end | .31 | .16 |
| `SEQ.focus-clip-kind` | 5 | piano-roll | piano_roll.rs | model | piano-roll.clip-kind | .31 | .16 |
| `SEQ.focus-clip-offset` | 4 | piano-roll | piano_roll.rs | model | piano-roll.clip-offset | .31 | .16 |
| `SEQ.focus-clip-start` | 5 | piano-roll | piano_roll.rs | model | piano-roll.clip-start | .31 | .16 |
| `SEQ.focus-kind` | 6 | piano-roll | piano_roll.rs | model | piano-roll.focus-kind | .31 | .16 |
| `SEQ.focus-label` | 1 | piano-roll | piano_roll.rs | model | piano-roll.focus-label | .31 | .16 |
| `SEQ.focus-num-steps` | 1 | piano-roll | piano_roll.rs | model | piano-roll.focus-num-steps | .31 | .16 |
| `SEQ.focus-window-marker` | 1 | piano-roll | piano_roll.rs | model | piano-roll.window-marker | .31 | .16 |
| `SEQ.focus-window-repeat` | 1 | piano-roll | piano_roll.rs | model | piano-roll.window-repeat | .31 | .16 |
| `SEQ.focus-window-span` | 1 | piano-roll | piano_roll.rs | model | piano-roll.window-span | .31 | .16 |
| `SEQ.piano-roll-automation` | 1 | piano-roll | piano_roll.rs | model | piano-roll.automation | .31 | .16 |
| `SEQ.piano-roll-automation-params` | 1 | piano-roll | piano_roll.rs | model | piano-roll.automation-params | .31 | .16 |
| `SEQ.piano-roll-items` | 8 | piano-roll | sv/topology_and_visualization.rs | model | note kind | .31 | .16 |
| `SEQ.piano-roll-lanes` | 2 | piano-roll | natives.rs | model | piano-roll.lanes | .31 | .16 |
| `SEQ.piano-roll-playhead` | 1 | piano-roll | piano_roll.rs | live | piano-roll.playhead (live) | .31 | .16 |
| `SEQ.piano-roll-selection` | 1 | piano-roll | piano_roll.rs | model | note.selected | .31 | .16 |
| `SEQ.track-automation` | 1 | packages/alez.tracker/src/ui | piano_roll.rs | model | tracker automation | .31 | .20 |
| `SEQ.track-grid-playhead-*` | 1 | packages/alez.tracker/src/ui | piano_roll.rs | live | track grid playhead (live) | .31 | .20 |
| `SEQ.track-grid-playhead-current` | 1 | packages/alez.tracker/src/ui | piano_roll.rs | live | track grid playhead | .31 | .20 |
| `SEQ.track-grid-playhead-row-current` | 1 | packages/alez.tracker/src/ui | piano_roll.rs | live | track grid playhead | .31 | .20 |
| `SEQ.track-lock-targets` | 1 | packages/alez.tracker/src/ui | piano_roll.rs | model | tracker lock targets | .31 | .20 |
| `SEQ.tracker-rows` | 1 | packages/alez.tracker/src/ui | piano_roll.rs | model | tracker rows | .31 | .20 |
| `AGENT.generation` | 2 | agent | browser.rs | model | agent.generation | .32 | — |
| `AUDIO.workers-choice` | 1 | settings | host_commands/audio_settings.rs | model | settings.audio-workers-choice | .32 | .18 |
| `AUDIO.workers-note` | 1 | settings | host_commands/audio_settings.rs | model | settings.audio-workers-note | .32 | .18 |
| `AUDIO.workers-options` | 1 | settings | host_commands/audio_settings.rs | model | settings.audio-workers-options | .32 | .18 |
| `EXPORT.export-busy` | 4 | export-song | host_commands/export.rs | model | export.export-busy | .32 | — |
| `EXPORT.export-default-name` | 1 | export-song | host_commands/export.rs | model | export.export-default-name | .32 | — |
| `EXPORT.export-done` | 3 | export-song | host_commands/export.rs | model | export.export-done | .32 | — |
| `EXPORT.export-end` | 1 | export-song | host_commands/export.rs | model | export.export-end | .32 | — |
| `EXPORT.export-folder` | 1 | export-song | host_commands/export.rs | model | export.export-folder | .32 | — |
| `EXPORT.export-message` | 2 | export-song | host_commands/export.rs | model | export.export-message | .32 | — |
| `EXPORT.export-output-name` | 1 | export-song | host_commands/export.rs | model | export.export-output-name | .32 | — |
| `EXPORT.export-percent` | 1 | export-song | host_commands/export.rs | model | export.export-percent | .32 | — |
| `EXPORT.export-project` | 1 | export-song | host_commands/export.rs | model | export.export-project | .32 | — |
| `EXPORT.export-reveal-label` | 1 | export-song | host_commands/export.rs | model | export.export-reveal-label | .32 | — |
| `MIDI.devices` | 1 | settings | midi_dispatch.rs | model | settings.midi-devices | .32 | .18 |
| `MIDI.error` | 1 | settings | lisp_host/eseq/expr_process.rs | model | settings.midi-error | .32 | .18 |
| `MIDI.persistent` | 1 | settings | midi_dispatch.rs | model | settings.midi-persistent | .32 | .18 |
| `RETRO.duration` | 9 | retrospective | lisp_host/value_helpers.rs | model | retro.duration | .32 | .12 |
| `RETRO.error` | 3 | retrospective | lisp_host/eseq/expr_process.rs | model | retro.error | .32 | .12 |
| `RETRO.items` | 5 | retrospective | agent/network.rs | model | retro.items | .32 | .12 |
| `RETRO.lanes` | 2 | retrospective | retrospective.rs | model | retro.lanes | .32 | .12 |
| `RETRO.playing` | 5 | retrospective | sequencer/state/sequencer_state/scene_launch.rs | live | retro.playing | .32 | .12 |
| `RETRO.position` | 1 | retrospective | retrospective.rs | live | retro.position | .32 | .12 |
| `RETRO.truncated` | 1 | retrospective | retrospective.rs | model | retro.truncated | .32 | .12 |
| `SEQ.browser-preview-playhead` | 3 | sample-import, browser, resample | reactive_tick.rs | live | browser.preview-position (live) | .32 | .17 |
| `SEQ.browser-preview-playing` | 4 | browser, sample-import, resample | reactive_tick.rs | model | browser.preview-playing | .32 | .17 |
| `SEQ.content-library-epoch` | 2 | browser | lisp_hot_reload.rs | model | implicit (browser collections) | .32 | .17 |
| `SEQ.current-pNroject-name` | 1 | browser | - | model | project.name (typo in browser.lisp) | .32 | .17 |
| `SEQ.current-project-name` | 4 | browser, application-menus | sv/project_state.rs | model | project.name | .32 | .17 .18 |
| `SEQ.editor-active-macro-action` | 3 | browser | reactive_tick.rs | model | editor.active-macro-action | .32 | .17 |
| `SEQ.editor-active-macro-name` | 1 | browser | reactive_tick.rs | model | editor.active-macro | .32 | .17 |
| `SEQ.editor-assets` | 2 | patch-macros | reactive_tick.rs | model | editor.assets | .32 | .18 |
| `SEQ.editor-buffer-name` | 1 | browser | event_loop.rs | model | editor.buffer | .32 | .17 |
| `SEQ.editor-canceling` | 4 | browser | event_loop.rs | model | editor.canceling | .32 | .17 |
| `SEQ.editor-error` | 5 | browser | event_loop.rs | model | editor.error | .32 | .17 |
| `SEQ.editor-instrument-run-mode` | 4 | browser | host_commands/instrument_authoring.rs | model | editor.run-mode | .32 | .17 |
| `SEQ.editor-library-macros` | 2 | patch-macros | reactive_tick.rs | model | editor.library-macros | .32 | .18 |
| `SEQ.editor-mode` | 18 | browser, seq-panels | event_loop.rs | model | editor.mode | .32 | .11 .17 |
| `SEQ.editor-open-macro` | 2 | patch-macros | reactive_tick.rs | model | editor.open-macro | .32 | .18 |
| `SEQ.editor-patch-macros` | 4 | patch-macros | reactive_tick.rs | model | editor.patch-macros | .32 | .18 |
| `SEQ.editor-selected-asset` | 1 | patch-macros | reactive_tick.rs | model | editor.selected-asset | .32 | .18 |
| `SEQ.editor-surface` | 3 | browser | host_commands/instrument_authoring.rs | model | editor.surface | .32 | .17 |
| `SEQ.kit-presets` | 2 | browser | host_commands/drum_rack_v2.rs | model | browser.kit-presets | .32 | .17 |
| `SEQ.learn-abs-distance` | 1 | patch-learn | patch_learn.rs | model | learn.abs-distance | .32 | .18 |
| `SEQ.learn-applied` | 1 | patch-learn | patch_learn.rs | model | learn.applied | .32 | .18 |
| `SEQ.learn-basin-check` | 1 | patch-learn | patch_learn.rs | model | learn.basin-check | .32 | .18 |
| `SEQ.learn-cma-continue` | 2 | patch-learn | host_commands/learn.rs | model | learn.cma-continue | .32 | .18 |
| `SEQ.learn-cma-final-epochs` | 2 | patch-learn | host_commands/learn.rs | model | learn.cma-final-epochs | .32 | .18 |
| `SEQ.learn-cma-forward-batch` | 2 | patch-learn | host_commands/learn.rs | model | learn.cma-forward-batch | .32 | .18 |
| `SEQ.learn-cma-generations` | 3 | patch-learn | host_commands/learn.rs | model | learn.cma-generations | .32 | .18 |
| `SEQ.learn-cma-population` | 6 | patch-learn | host_commands/learn.rs | model | learn.cma-population | .32 | .18 |
| `SEQ.learn-cma-refine-epochs` | 2 | patch-learn | host_commands/learn.rs | model | learn.cma-refine-epochs | .32 | .18 |
| `SEQ.learn-cma-refine-mode` | 2 | patch-learn | host_commands/learn.rs | model | learn.cma-refine-mode | .32 | .18 |
| `SEQ.learn-cma-seed` | 2 | patch-learn | host_commands/learn.rs | model | learn.cma-seed | .32 | .18 |
| `SEQ.learn-cma-sigma` | 2 | patch-learn | host_commands/learn.rs | model | learn.cma-sigma | .32 | .18 |
| `SEQ.learn-current-epoch` | 1 | patch-learn | patch_learn.rs | model | learn.current-epoch | .32 | .18 |
| `SEQ.learn-epoch-params` | 1 | patch-learn | patch_learn.rs | model | learn.epoch-params | .32 | .18 |
| `SEQ.learn-epochs` | 2 | patch-learn | host_commands/learn.rs | model | learn.epochs | .32 | .18 |
| `SEQ.learn-error` | 1 | patch-learn | patch_learn.rs | model | learn.error | .32 | .18 |
| `SEQ.learn-final-wav` | 1 | patch-learn | patch_learn.rs | model | learn.final-wav | .32 | .18 |
| `SEQ.learn-gate-frames` | 4 | patch-learn | patch_learn.rs | model | learn.gate-frames | .32 | .18 |
| `SEQ.learn-improvement-pct` | 1 | patch-learn | patch_learn.rs | model | learn.improvement-pct | .32 | .18 |
| `SEQ.learn-local-epochs` | 2 | patch-learn | host_commands/learn.rs | model | learn.local-epochs | .32 | .18 |
| `SEQ.learn-loss` | 1 | patch-learn | patch_learn.rs | model | learn.loss | .32 | .18 |
| `SEQ.learn-losses` | 1 | patch-learn | patch_learn.rs | model | learn.losses | .32 | .18 |
| `SEQ.learn-method` | 7 | patch-learn | host_commands/learn.rs | model | learn.method | .32 | .18 |
| `SEQ.learn-optimization-losses` | 1 | patch-learn | patch_learn.rs | model | learn.optimization-losses | .32 | .18 |
| `SEQ.learn-phase` | 5 | patch-learn | patch_learn.rs | model | learn.phase | .32 | .18 |
| `SEQ.learn-pitch-hz` | 4 | patch-learn | patch_learn.rs | model | learn.pitch-hz | .32 | .18 |
| `SEQ.learn-plan-params` | 2 | patch-learn | patch_learn.rs | model | learn.plan-params | .32 | .18 |
| `SEQ.learn-result-deltas` | 1 | patch-learn | patch_learn.rs | model | learn.result-deltas | .32 | .18 |
| `SEQ.learn-seeded-wav` | 1 | patch-learn | patch_learn.rs | model | learn.seeded-wav | .32 | .18 |
| `SEQ.learn-stage` | 1 | patch-learn | patch_learn.rs | model | learn.stage | .32 | .18 |
| `SEQ.learn-target-name` | 3 | patch-learn | host_commands/learn.rs | model | learn.target-name | .32 | .18 |
| `SEQ.learn-target-path` | 3 | patch-learn | host_commands/learn.rs | model | learn.target-path | .32 | .18 |
| `SEQ.learn-total-epochs` | 1 | patch-learn | patch_learn.rs | model | learn.total-epochs | .32 | .18 |
| `SEQ.project-instrument-engines` | 1 | browser | sv/project_state.rs | model | browser.engines | .32 | .17 |
| `SEQ.sidebar-instrument-display-name` | 2 | browser | sv/project_state.rs | model | browser.instrument-label | .32 | .17 |
| `SEQ.sidebar-instrument-name` | 5 | browser, application-menus, effects/panel-frame | sv/project_state.rs | model | browser.instrument | .32 | .14 .17 .18 |
| `SEQ.sidebar-kind` | 7 | browser | sv/project_state.rs | model | browser.kind | .32 | .17 |
| `SEQ.sidebar-loaded-preset` | 3 | browser | sv/project_state.rs | model | browser.preset | .32 | .17 |
| `SEQ.sidebar-presets` | 1 | browser | sv/project_state.rs | model | browser.presets | .32 | .17 |
| `SEQ.sidebar-rack-slot-presets` | 1 | browser | sv/project_state.rs | model | browser.rack-slot-presets | .32 | .17 |
| `SEQ.sidebar-selected-sample` | 7 | browser | sv/project_state.rs | model | browser.sample | .32 | .17 |
| `SEQ.sidebar-track-index` | 4 | browser | sv/project_state.rs | model | browser.track | .32 | .17 |
| `SEQ.sidebar-user-presets` | 1 | browser | sv/project_state.rs | model | browser.user-presets | .32 | .17 |
| `SEQ.sound-palette` | 7 | sound-palette | sv/sound_palette.rs | model | sound kind | .32 | .17 |
| `SEQ.sound-presets` | 2 | browser | sv/project_state.rs | model | browser.sound-presets | .32 | .17 |
| `SEQ.track-instrument-ids` | 1 | browser | sv/track_and_mixer.rs | model | track.instrument-id | .32 | .17 |
| `GRAPH.<ggm-route-color-field>` | 4 | scripts/sequencers/graph-neural-group-matrix-demo | lisp_host/eseq/graph_authoring.rs (+ Lisp writes) | model | graph-node.‹ggm-route-color-field› | .33 | .20 |
| `GRAPH.<gvr-route-color-field>` | 4 | scripts/sequencers/graph-neural-variable-reset-demo | lisp_host/eseq/graph_authoring.rs (+ Lisp writes) | model | graph-node.‹gvr-route-color-field› | .33 | .20 |
| `SEQ.<neural->` | 8 | scripts/sequencers/neural-8x8-track-router | sv/topology_and_visualization.rs | model | neuron.selected | .33 | .20 |
| `SEQ.generator-mark-*` | 4 | packages/alez.jaki/src/kind | sv/meters_and_modulation.rs | model | jaki generator marks | .33 | .20 |
| `SEQ.graph-sequencers` | 1 | mixer | reactive_tick.rs | model | track.graph-sequencer | .33 | .13 |
| `SEQ.graph-visualizations` | 14 | scripts/sequencers/graph-neural-variable-reset-demo, packages/alez.neural/src/variable-reset, scripts/sequencers/graph-neural-16-demo +5 | sv/topology_and_visualization.rs | model | graph visualization kind | .33 | .20 |
| `SEQ.neural-dampening-matrix` | 1 | scripts/sequencers/neural-8x8-track-router | sv/topology_and_visualization.rs | model | network.dampening-matrix | .33 | .20 |
| `SEQ.neural-energy-matrix` | 1 | scripts/sequencers/neural-8x8-track-router | sv/topology_and_visualization.rs | live | network.energy-matrix (live) | .33 | .20 |
| `SEQ.neural-networks` | 1 | scripts/sequencers/neural-8x8-track-router | sv/topology_and_visualization.rs | model | neural network kind | .33 | .20 |
| `SEQ.neural-trigger-matrix` | 1 | scripts/sequencers/neural-8x8-track-router | sv/topology_and_visualization.rs | live | network.trigger-matrix (live) | .33 | .20 |
| `SEQ.track-active-notes` | 5 | effects/panel-bodies, scripts/sequencers/graph-neural-8x8-demo, scripts/sequencers/graph-neural-variable-reset-demo +2 | reactive_tick.rs | live | track.active-notes (live) | .33 | .14 .20 |
| `SEQ.track-event-current-beat` | 3 | scripts/processes/process-ui-control-demo, scripts/sequencers/band-coupling-matrix-demo, scripts/sequencers/graph-neural-8x8-demo | ui_replay_probe.rs | live | track events | .33 | .20 |
| `SEQ.track-events` | 3 | scripts/processes/process-ui-control-demo, scripts/sequencers/band-coupling-matrix-demo, scripts/sequencers/graph-neural-8x8-demo | ui_replay_probe.rs | model | track events (demo scripts) | .33 | .20 |
| `SEQ.<rack/groove-amount-field>` | 1 | rack-groove-buffer | sv/rack_groove_fields.rs | model | groove.amount | .34 | .19 |
| `SEQ.armed-rack-id` | 2 | mixer, drum-rack-v2 | reactive_tick.rs | model | group.armed | .34 | .13 .19 |
| `SEQ.groove-pool` | 2 | rack-groove-buffer | sv/rack_groove_fields.rs | model | groove pool | .34 | .19 |
| `SEQ.rack-clip-active-*` | 3 | sequencer, mixer | sv/topology_and_visualization.rs | model | rack-clip.active | .34 | .11 .13 |
| `SEQ.rack-clip-banks` | 1 | drum-rack-v2 | sv/topology_and_visualization.rs | model | rack-clip banks | .34 | .19 |
| `SEQ.rack-clip-index-*` | 1 | sequencer | sv/topology_and_visualization.rs | model | group.rack-clip | .34 | .11 |
| `SEQ.rack-clips` | 2 | mixer, drum-rack-v2 | sv/topology_and_visualization.rs | model | rack-clip kind | .34 | .13 .19 |
| `SEQ.rack-grooves` | 1 | drum-rack-v2 | sv/rack_groove_fields.rs | model | groove kind | .34 | .19 |
| `SEQ.rack-pad-trigger-*` | 3 | sequencer | sv/drum_rack.rs | live | pad.triggered (live) | .34 | .11 |
| `SEQ.track-steps` | 2 | rack-groove-buffer | sv/param_fields_and_sync.rs | model | rack member steps | .34 | .19 |
| `SEQ.<slot-bar-transpose-field>` | 1 | sequencer | sv/expanded_step.rs | model | track.bar-transposes | built (.35) | .11 |
| `SEQ.<slot-bar-transpose-set-field>` | 1 | sequencer | sv/expanded_step.rs | model | track.bar-transposes (≠ 0; `set-bar-transpose!`) | built (.35) | .11 |
| `SEQ.accum-mode-options` | 1 | effects/track-panels | sv/project_state.rs | model | constant | built (.35) | .14 |
| `SEQ.accumulator-options` | 1 | effects/track-panels | sv/project_state.rs | model | project.accumulator-options | built (.35) | .14 |
| `SEQ.auto-follow` | 2 | seq-core-state, sequencer | reactive_tick.rs | model | selection.auto-follow | built (.35) | .11 |
| `SEQ.bus-effects` | 3 | application-menus, effects/buffers, effects/panel-widgets | event_loop.rs | model | bus.devices | .36 | .14 .18 |
| `SEQ.bus-mod-in-level-*` | 1 | mixer | sv/meters_and_modulation.rs | live | bus.mod-in-1 … -4 (live; `(mod-in-level b i)`) | built (.35) | .13 |
| `SEQ.bus-output-routes` | 1 | mixer | sv/track_and_mixer.rs | model | bus.output, bus.output-options | built (.35) | .13 |
| `SEQ.compiling` | 1 | effects/buffers | sv/host_commands.rs | model | engine.compiling | built (.35) | .14 |
| `SEQ.cpu-overloaded` | 2 | transport | reactive_tick.rs | live | engine.overloaded | built (.35) | .12 |
| `SEQ.fts-options` | 2 | effects/track-panels, effects/scale-editor | sv/project_state.rs | model | project.fts-options | built (.35) | .14 |
| `SEQ.fx-step-cursor-number` | 1 | effects/track-panels | sv/param_fields_and_sync.rs | model | selection.cursor-step (index + 1) | built (.35) | .14 |
| `SEQ.fx-step-parameter-step` | 1 | seq-core-state | sv/topology_and_visualization.rs | model | selection.edit-step | built (.35) | .11 |
| `SEQ.fx-step-selection-count` | 2 | effects/track-panels, seq-core-state | sv/param_fields_and_sync.rs | model | (len selection.steps) | built (.35) | .11 .14 |
| `SEQ.fx-step-value-*` | 1 | effects/track-panels | step_print.rs | model | step.‹param› of selection.edit-step | built (.35) | .14 |
| `SEQ.midi-effects` | 1 | effects/buffers | event_loop.rs | model | track.midi-devices | .36 | .14 |
| `SEQ.mixer-track-delete-target-*` | 1 | mixer | sv/steps_and_pattern.rs | model | track.delete-target | built (.35) | .13 |
| `SEQ.mod-in-level-*` | 1 | mixer | sv/meters_and_modulation.rs | live | track.mod-in-1 … -4 (live; `(mod-in-level t i)`) | built (.35) | .13 |
| `SEQ.mod-out-level-*` | 1 | mixer | sv/meters_and_modulation.rs | live | track.mod-out-level (live) | built (.35) | .13 |
| `SEQ.mod-routes` | 6 | mixer | reactive_sync.rs | model | route kind, `(routes)` | built (.35) | .13 |
| `SEQ.mute-group-options` | 1 | effects/track-panels | sv/project_state.rs | model | constant | built (.35) | .14 |
| `SEQ.rack-slot-delete-target-*` | 1 | effects/instrument-panel | sv/steps_and_pattern.rs | model | device.delete-target | .36 | .14 |
| `SEQ.roll-rate` | 1 | transport | reactive_tick.rs | live | transport.roll-rate | built (.35) | .12 |
| `SEQ.selected-mod-routes` | 2 | mixer | sv/steps_and_pattern.rs | model | route.selected | built (.35) | .13 |
| `SEQ.sequence-rolling` | 1 | transport | reactive_tick.rs | live | transport.sequence-rolling | built (.35) | .12 |
| `SEQ.sync-labels` | 8 | sequencer, step-grid, seqv-track-params +1 | natives.rs | model | project.sync-options | built (.35) | .11 |
| `SEQ.tp-accum-limit` | 1 | effects/track-panels | sv/project_state.rs | model | track.accum-limit | built (.35) | .14 |
| `SEQ.tp-accum-mode` | 1 | effects/track-panels | sv/project_state.rs | model | track.accum-mode | built (.35) | .14 |
| `SEQ.tp-accumulator` | 1 | effects/track-panels | sv/project_state.rs | model | track.accumulator | built (.35) | .14 |
| `SEQ.tp-fts` | 2 | effects/track-panels, effects/scale-editor | sv/project_state.rs | model | track.fts | built (.35) | .14 |
| `SEQ.tp-gate` | 4 | effects/sampler-panel | sv/project_state.rs | model | track.gate | built (.35) | .14 |
| `SEQ.tp-max-polyphony` | 2 | mixer, effects/track-panels | host_commands/rack.rs | model | track.max-polyphony (the track's own; a rack slot's: .36) | built (.35) | .13 .14 |
| `SEQ.tp-mono-trigger` | 1 | effects/track-panels | sv/project_state.rs | model | track.mono-trigger | built (.35) | .14 |
| `SEQ.tp-mute-group` | 1 | effects/track-panels | sv/project_state.rs | model | track.mute-group (`:int`; label `(nth mute-group-options g)`) | built (.35) | .14 |
| `SEQ.tp-poly` | 12 | effects/track-panels, mixer, effects/instrument-panel | sv/project_state.rs | model | track.poly (the track's own; a rack slot's: .36) | built (.35) | .13 .14 |
| `SEQ.tp-rack-slot-idx` | 5 | effects/track-panels, mixer | sv/project_state.rs | model | selection.rack-slot (-1: no rack) | built (.35) | .13 .14 |
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
| `SEQ.track-mod-output-available` | 2 | mixer | sv/track_and_mixer.rs | model | track.mod-output | built (.35) | .13 |
| `SEQ.track-output-options` | 1 | mixer | host_commands/routing.rs | model | project.output-options (bus instances; nil is sends only) | built (.35) | .13 |
| `SEQ.track-outputs` | 1 | mixer | sv/track_and_mixer.rs | model | track.output (a bus; nil is sends only) | built (.35) | .13 |
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
| `SEQV.rack-clip-center-*` | 1 | mixer | Lisp (reactive-set) | Lisp-owned | mixer view singleton | view-local | .13 |
| `:bindable` | 97 | effects/physical-model-surface, sequencer, effects/drum-surface +24 | - | - | delete (ignored since stage 5) | remove | .11 .12 .13 .14 .20 .21 |
| `<ns-var namespace>` | 3 | bindings | - | - | bindings.lisp generic scopes → kinds | remove | .18 |
| `reactive-value` | 75 | instruments/Synths/Heat/ui, effects/param-controls, scripts/sequencers/graph-neural-variable-reset-demo +27 | - | - | t.x / #'t.x read as a value (§8) | remove | .11 .13 .14 .20 .21 |
| `SEQ.bus-ids` | 10 | mixer, drum-rack-v2, seq-core-state +1 | sv/track_and_mixer.rs | model | instance identity | remove | .11 .13 .19 |
| `SEQ.delete-target-version` | 4 | mixer, browser, application-menus +1 | reactive_tick.rs | model | implicit (fields re-render) | remove | .13 .14 .17 .18 |
| `SEQ.num-patterns` | 6 | transport, macros, scene-banks | sv/topology_and_visualization.rs | model | (len (scenes)) | remove | .12 .18 |
| `SEQ.num-tracks` | 38 | mixer, track-collapse, sequencer +10 | reactive_sync.rs | model | (len (tracks)) | remove | .11 .13 .14 .17 .18 .19 |
| `SEQ.rack-panel-view-generation` | 1 | effects/state | sv/project_state.rs | model | implicit | remove | .14 |
| `SEQ.scene-bank-view-generation` | 1 | scene-banks | sv/project_state.rs | model | implicit (collections re-render) | remove | .12 |
| `SEQ.track-ids` | 30 | sequencer, arrangement, mixer +1 | reactive_sync.rs | model | instance identity (subtree :key t) | remove | .11 .13 .15 .20 |
| `SEQ.instances` | 3 | mixer, browser, packages/alez.neural/src/variable-reset | lisp_host/eseq/process_dsl_parse.rs | model | package instances (live_instances) | keep | .13 .17 .20 |
| `THEME.buffer_bg` | 1 | sequencer | - | model | THEME stays (theme namespace, not host state) | keep | .11 |
| `THEME.plock_base` | 2 | effects/panel-bodies, effects/track-panels | - | model | THEME stays (theme namespace, not host state) | keep | .14 |
| `THEME.scene_clip_bg` | 1 | arrangement | - | model | THEME stays (theme namespace, not host state) | keep | .15 |
| `GRAPH` via `bind-graph` / `bind-graph-config` (103 calls) | 103 | scripts/sequencers/graph-*, packages/alez.neural | lisp_host/eseq/graph_authoring.rs | model | graph-node.‹field› | .33 | .20 |
| `reactive-set "GRAPH"` (52 writes) | 52 | scripts/sequencers/graph-* | Lisp | Lisp-owned | graph-node :set | .33 | .20 |
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
