# Kind bindings

Status: spec rev 3, 2026-10-04. Stages 1–4 built (§3.1, §3.2, §3.3, §3.4, §4, §7.1, §8, §9 notes). Bead: epic `eseq-0l17` (`bd list --label kind-bindings`).
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
  project`) before evaluating the root.
- **Schema check.** `host_kinds::PUBLISHED`
  (`crates/sequencer/src/ui/host_kinds.rs`) lists every field the host
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
  | `track` | `(index)` | `index :int`, `name :string`, `color :rgb`, `volume :number` [`seq-set-track-volume`], `peak :number`, `muted :bool` [`seq-set-track-mute`], `armed :bool` [`seq-set-record-arm`], `selected :bool`, `preset :string`, `num-steps :int`, `steps (list-of step)`, `devices (list-of device)` |
  | `step` | `(track index)` | `index :int`, `track track`, `active :bool` [`seq-set-track-step`], `playing :bool`, `selected :bool` |
  | `device` | `(track slot)`, slot part = slot + 1 | `track track`, `slot :int` (-1 = instrument), `name :string`, `enabled :bool` |
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
  setter toggles), not the solo-effective mute.
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
- Errors: `kind 'm:k' has no field 'x'; bindable fields: a, b` and
  `#': field 'name' of kind 'm:k' is :string, which is not bindable;
  bindable fields: …` (also for built-in fields). A non-instance head (a
  dict, nil, a string) is an error too.
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
  track `volume muted armed selected peak num-steps steps`, step `active
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
