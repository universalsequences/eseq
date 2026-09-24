# Instance kinds

Status: spec rev 1, 2026-09-24. Stages 1-7 built (see the
"Built" notes under §5, §6, §7, §8.1, §8.3, §9, §10 and §11). Bead: see the `instance-kinds`
epic (`bd list --label instance-kinds`).

## 1. Problem

A package script can only exist once per project. `alez.neural.variable-reset`
is the motivating case: loading two drum-rack kits that each carry a neural
sequencer drives both configurations into one sequencer, when the user wants two
independent ones.

The engine is not the problem. Graph overrides are keyed by `sequencer_id`, and
`graph_instance_id` (`src/lisp_host/eseq/graph_manifest.rs`) already namespaces
a rack-owned sequencer as `name@rack:<gid>`. The singleton lives in the Lisp and
host layers around it:

1. **One owner per module.** `RACK_OWNER_BY_MODULE` maps a module name to ONE
   group id (`publish_rack_owner_modules`, `src/app/rack_sequencers.rs`). Two
   racks recording `(import alez.neural.variable-reset)` collapse to whichever
   was written last, and project open imports the module once.
2. **Module globals.** `(def gvr-name (def-sequencer …))` and `gvr-owner-rack`
   are bound once per module; a second evaluation rebinds them, so rack A's UI
   drives rack B.
3. **Global `defstate`s**: `gvr-weights`, `gvr-expanded-node`,
   `gvr-selected-neuron`, `gvr-proc-version`, …
4. **Literal buffer.** `effect-buffer` needs a compile-time name
   (`compile_effect_buffer_form`, `crates/eseqlisp/src/lang/compiler.rs`).
5. **Tab registry** dedupes by buffer name
   (`seq-register-script-step-sequencer-tab`, `content/ui/seq-step-tabs.lisp`).
6. **Fixed widget keys** (16 `"graph-variable-reset-…"` strings, including
   subtree keys, which cache).
7. **Node patch-bay namespace** is `1024 + node` with no graph in it, so node k
   of two graphs share a lane-patch namespace.
8. **Rack ownership is at most one instance per (rack, name)**, so even a single
   rack cannot own two neural sequencers.

The usual workaround, string-concatenated `reactive-set` keys, is untyped,
fails silently on a typo and never gets cleaned up.

## 2. Model

Three kinds of state, each with one home:

| State | Examples | Home |
|---|---|---|
| Definition | manifest, node params, update rule | package code, shared |
| Document | routes, weights, node params, process chains | graph overrides per instance (exists), undoable, saved |
| View | expanded node, selected neuron, add-class picker | per-instance `:state` cells, not undoable, not saved (v1) |

Most of today's `gvr-*` `defstate`s are caches of document state that exist
only because the `graph-*` read natives are not reactive (§6). They disappear;
they are not ported.

A package defines **kinds**. The host owns **instances** of kinds. Importing a
module registers its kinds and creates nothing.

## 3. `def-kind`

```lisp
(def-kind neural
  :sequencer (:shape (line :default 8 :min 1 :max 16)
              :energy-decay 0.992 …
              (def-node nrn …)
              (edges …))
  :state ((expanded-node -1) (selected-neuron -1) (add-class 0))
  :view gvr-panel)
```

v1 slots:

- `:sequencer`: a graph `def-sequencer` body without the name, captured as data
  exactly like graph-mode `def-sequencer` today. The host publishes it under the
  instance's id. Names need not be unique.
- `:state`: `(field default)` pairs. Defaults are evaluated per new instance.
- `:view`: a function of one argument, the instance.

The registry, `self`, ownership, buffers, tabs and lifecycle (§4–§8) are
kind-agnostic from the start. `:sequencer` is the first **resource slot** (what
the instance owns in the engine); a kind may have none (a view-only tool) and
later kinds may add others (effect, process). Only `:sequencer` is built now;
further slots arrive with a real second user.

Deliberately NOT in v1: `:label`/`:tab` (the tab shows `self.label`; a new
instance is labelled `<kind> <n>`), `:persist` state,
`(instances :kind …)` queries, nested records. `:on-create` was added for the
one case §11 names (see there).

Also built with the port (stage 7): `:on-create f` (a function of the
instance) and the `(instance-ref id)` native, which returns the live instance
`id` as a value (nil otherwise) so scripts and capture fixtures can address a
host-created instance.

## 4. `self`

`self` is an ordinary value, `Value::Instance(id)`, not a magic binding. The
host passes it to `:view`; helpers take it as their first parameter, and
closures capture it like any value, so an event handler that fires later still
addresses the instance that rendered it.

```lisp
(def gvr-weight-matrix (self)
  (let ((active-count (graph-node-count self)))
    (matrix :key "weight-matrix"
      :rows active-count :cols active-count
      :on-cell-press  (lambda (r c) (set! self.selected-neuron c))
      :on-cell-change (lambda (r c v) (graph-edge self :from r :to c :weight v)))))
```

Fields:

| Field | Source | Writable |
|---|---|---|
| `self.id`, `self.kind`, `self.owner` | host | no |
| `self.label` | host | `set!` = rename, an undoable project edit |
| declared `:state` fields | cells | yes |

- Reads go through the existing `x.field` compilation (load + `GetField`,
  `compiler.rs`). `GetField` on an instance looks the field index up in the
  kind's schema, reads the cell `(instance id, field index)` and records a DAG
  dependency on that cell. `SetField` accepts only `:state` fields and `label`,
  and dirties only that cell's readers.
- An unknown field is an error naming the kind and its fields.
- A stale `self` (instance deleted) reads defaults; writes are no-ops.
- There is no `self.sequencer`: every `graph-*` native accepts an instance
  wherever it accepts a handle. Raw handles remain for kind-less scripts.
- No implicit "current instance". An implicit binding is wrong when an event
  handler runs outside render, which is exactly where most writes happen.

## 5. Instances

```
Instance { id, kind: "alez/neural:neural", owner: Project | Rack(gid), label }
```

- **Kind id** = `<package name>:<kind name>`, never a module path, so code can
  move between modules without breaking saved projects and kits.
- **Instance ids** are host-assigned and stable within a project. The published
  sequencer id IS the instance id; `graph_instance_id` naming no longer applies
  to kind instances.
- Instances are listed in the project (project-owned) and in each rack's config
  (rack-owned). Create, delete, duplicate, rename and move-owner are recorded,
  undoable project edits.
- Duplicate copies the graph overrides to the new id. Delete drops overrides,
  published sequencer, buffer, tab and state cells.
- Hot reload keeps ids, so view state survives; new `:state` fields start at
  their default and removed ones are dropped.
- An instance whose kind is not registered (package missing) is kept as a
  placeholder `neural (package alez/neural missing)` and revives when the
  package is installed and attached.

Deleted by this design: `RACK_OWNER_BY_MODULE`, `set_rack_owner_modules`,
`publish_rack_owner_modules`, `with_graph_owner_rack`, the import-string parsing
in `rack_sequencer_module`, and the "re-run the script after move/detach" paths
in `ui/host_commands/drum_rack_v2.rs`.

Built (stage 3, part 1: `def-kind`, registry, project model):

- `def-kind` is a compiler form (`compile_def_kind_form`) calling the
  `def-kind` native (`lisp_host/eseq/kinds.rs`). `:sequencer` is captured
  exactly like graph-mode `def-sequencer`: only a TOP-LEVEL `,x` escapes.
  `:state` defaults are evaluated once when `def-kind` runs and deep-copied
  into each new instance (deviation from "per new instance": identical for
  literal defaults; `:on-create` remains the answer if a kind ever needs
  per-instance computation). `(field)` or a bare `field` defaults to nil.
- Kind id: `<package name>:<kind>` from the installed package whose
  namespace holds the defining module; outside any package `<module>:<kind>`,
  headerless code `scratch:<kind>`. The host registry (`KindDefinition`:
  parsed manifest template, state field names, module, package) is
  process-wide; the evaluating VM gets the `:state`/`:view` schema
  (`InstanceKindSchema::view`). Registering publishes nothing.
- Deviation from "listed in each rack's config": every instance lives in
  ONE project list, `ProjectFile::instances` (`ProjectInstances { list,
  next_id }`, absent in older files) with `owner: project | {rack: gid}`, so
  every lifecycle edit is one kind of recorded edit
  (`EditPatch::InstanceStructure`). Kits (§9) copy rack-owned ones out.
- Ids count up from 1, skipping any id an instance or published sequencer
  holds, or that still keys graph overrides in any scene (rack clips
  included). Overrides are not instance edits, so undoing a create can leave
  some behind under the undone id; `next_id` never moves backwards on undo
  and the override check survives a reload that drops `next_id`, so a new
  instance never inherits them. Project open / new project bumps
  `ProjectInstances::generation`, and the UI drops every live record before
  re-syncing, so `:state` view cells never carry across projects even when
  ids and kinds coincide. `def-kind` validates the `:state` schema (host
  field names, duplicates, `.`) before touching the host registry, and parses
  `:sequencer` with no rack owner in scope. A rename to the current label is
  a quiet no-op. The published manifest is the kind's with id = instance id, owner
  from the instance, and name `<kind>#<id>`: unique on purpose, because
  `GraphManifest::matches_overrides` falls back to a name match within one
  owner and two instances sharing the kind name would share overrides.
- `App::{create,delete,duplicate,rename}_instance_recorded`. Create/rename
  snapshot only the list; delete/duplicate/move/kit load also record the
  overrides keyed by the instances they touch (`InstanceOverridesState`:
  per site, i.e. each scene's own list and every clip of every rack bank,
  launched or not). Replay splices just those ids back, never the whole scene
  bank, so an unrecorded `graph-*` edit to any OTHER sequencer made after the
  instance edit survives its undo and redo. Delete drops every override keyed
  to the id (every scene and every clip) and unpublishes; duplicate copies
  them to the new id wherever they are and inserts the copy after its source.
  The copies' node process slots get fresh slot ids through the node-process
  id high-water (`GraphNodeProcessReminter`), with `ProcessInlet` wires and
  fan-out into them rewritten to match: the scheduler keys a node slot's state
  by slot id alone, so a verbatim copy would share counters and rand streams
  (and the patch bay's slot selection) with its source. New and duplicated
  instances get `<kind> <n>` with the smallest unused `n`. `next_id` is saved
  whenever an id was ever handed out (`ProjectInstances::is_unused`), even
  with an empty list.
- The UI mirrors `(ProjectInstances::revision, kind_registry_version())` in
  the reactive tick: publish instance sequencers, then
  `sync_instance_records` (create/drop VM records, push `owner` = `:project`
  or the rack gid, and `label`). An instance whose kind is not registered
  keeps no record and no sequencer until the kind registers (placeholder UI
  is §8 work). `(set! x.label v)` queues the `instance-rename` host command.
- Host commands (`ui/host_commands/instances.rs`): `instance-create
  {:kind [:group-id] [:label]}`, `instance-delete {:id}`,
  `instance-duplicate {:id}`, `instance-rename {:id :label}`.
- Every `graph-*` native resolves an instance value to its sequencer
  (`resolve_graph_manifest`, and the homeostat natives).
- Built (part 2): **move-owner** is `App::move_instance_owner_recorded`
  (recorded with the instance's overrides). The instance keeps its id; its
  overrides expand through the old rack's members
  (`expand_member_routes_to_tracks`) and contract to the new rack's
  (`contract_track_routes_to_members`, a track outside the rack goes off),
  and `owner_rack` follows. Each scene's view of the instance is read once
  before anything moves (two scenes sharing a clip both keep it). Into a
  clip-bearing rack every clip of the bank gets the entry of the first scene
  pointing at it, else the current scene's; out of one, a scene that launched
  no clip of it takes the current scene's entry; entries in the old rack's
  unlaunched clips have no destination and go (decision, §11). Host command `instance-move {:id [:group-id]}`;
  the rack menu's existing "Attach … to rack" / "Detach …" entries route an
  instance's sequencer id to the same move, with nothing re-run. "Each rack
  lists its instances" is the derived `App::rack_instances(gid)`; any number
  of instances of one kind can share a rack, and member removal remaps the
  routes of a rack that owns only instances.
- A rack's instances live and die with it (decision, §11). Dissolving a rack
  that keeps its tracks (`delete_group_recorded`, `ungroup_tracks_recorded`)
  first moves its instances back to the project (routes expand to the
  tracks); deleting it with its tracks (`delete_group_with_members_recorded`,
  `replace_rack_with_sound`) deletes them with their overrides. Either way
  it is one undo entry with the group edit. New group ids also skip every
  rack an instance still names (`App::next_group_id`), so a new rack can
  never silently adopt an orphan.
- Deleted: `RACK_OWNER_BY_MODULE`, `set_rack_owner_modules`,
  `publish_rack_owner_modules`, `rack_owner_for_module`,
  `graph_owner_for_module`, `parse_graph_manifest_in_module` /
  `published_sequencer_from_def_args_in_module` and
  `rack_sequencer_module`. Deviation: `with_graph_owner_rack` stays, but
  only for LEGACY plain scripts a rack records (`(load …)`/script text):
  those have no kind, so the scope is still how they publish rack-owned,
  and `replay_rack_sequencer_sources` now evaluates every recorded source
  under its rack. Kinds never read it (templates parse with
  `parse_graph_manifest_owned(args, None)`); the legacy re-run paths in
  `drum_rack_v2.rs` run for plain scripts only.
- `alez.neural.variable-reset` declares `(def-kind neural …)` with a copy of
  its graph body so the manifest's `kinds` entry passes the attach check;
  the stage-7 port removes the legacy `def-sequencer`.

## 6. Reactive graph reads

`graph-edge-value`, `graph-node-value`, `graph-config` reads and the node
process-chain read become tracked per instance, like `bind-graph` already is for
node params. Writes dirty their readers. This removes the `reactive-set "GRAPH"`
echoes after writes, the cache `defstate`s and `gvr-proc-version`. It lands
before the `variable-reset` port so the port is done once.

Built (stage 2):

- Reads inject a dependency on a host-owned `__graph` source per
  (graph instance id, field): `id|n<node>|<field>` (`graph-node-value`),
  `id|p<node>|<param>` (`graph-param-value`), `id|e<from>_<to>|<param>`
  (`graph-edge-value`), `id|cfg|<field>` (`graph-config-value`) and
  `id|proc<node>` (`graph-node-process-chain`, `graph-node-lane-patch`).
  Reads outside a rendering effect keep no dependency.
- A source's generation is the resolved value (a fingerprint for lists and
  errors; the chain plus the process-library version for `proc`), so
  re-resolving to the same value dirties nothing. The first reader seeds it.
- Lisp writes (`graph-node`, `graph-param`, `graph-edge`, `graph-config`,
  every `graph-node-process-*` edit) re-resolve the subscribed reads they can
  change (node / edge / node patch; config re-resolves the whole graph) before
  the handler returns. Anything else (pattern switch, kit load, Rust-side
  edits, another VM) is swept by `queue_graph_read_invalidations` from the UI
  tick when the scheduler snapshot version, published sequencer version or
  pattern moves.
- The same writes echo the new value into numeric `bind-graph*` handles
  that someone bound. Handles bound through an options list hold a dropdown
  index, so they are never echoed. A view that needs an enum field kept in
  step reads it with `graph-node-value` / `graph-config-value` instead.
- `variable-reset.lisp` echoed and had `gvr-proc-version` until the §10
  port (stage 7), which deleted both.

## 7. Views, buffers, tabs

For each instance of a kind with a `:view`, the host creates a buffer
(`*neural · Kit A*`) and a step-sequencer tab labelled `self.label`, and
renders `(view self)` in it. This needs:

- a runtime buffer bound to a closure, alongside the literal `effect-buffer`;
- tab registry entries keyed by instance, not buffer name;
- implicit key scoping: an instance's view renders inside a key scope, so
  `:key "weight-matrix"` is unique per instance with no package code;
- the node patch-bay namespace derived from the instance id (fixes §1.7; the
  Rust projection builder duplicated in the lib crate must match);
- an optional `:keymap` slot later, replacing `set-buffer-mode-for`.

The tab × closes the tab only. Deleting an instance is a menu action (§8).

Built (stage 4):

- **Closure-bound buffers.** `VM::bind_view_buffer(target, view, key_scope)`
  (`crates/eseqlisp/src/lang/vm/view_buffers.rs`, wrapped by `Runtime`)
  makes an ordinary top-level named effect whose body is a synthesized
  chunk (`EffectBegin; CallBoundView; EmitTree; EffectEnd`, one new opcode),
  so read tracking, subtree caching, hidden-buffer deferral and error
  reporting are `effect-buffer`'s. `BoundView::Instance(id)` resolves the
  kind's `:view` at every render (a hot-reloaded `def-kind` re-renders
  its instances' buffers through the new function);
  `BoundView::Call { callable, args }` covers any closure (from Lisp:
  `(bind-view-buffer name f arg ...)` / `(unbind-view-buffer name)`). Also
  `unbind_view_buffer`, `retarget_view_buffer` (rename) and
  `bound_view_buffers`; bindings have no owner buffer or source file, so
  neither a layout reset nor a module reload clears them, and they roll
  back with a VM state snapshot. A stale instance renders nil.
- **Host sync.** `lisp_host::sync_instance_view_buffers` binds a buffer per
  live instance record whose kind has a `:view`, named
  `*<kind> · <label>*` (two instances that would share a name get
  ` #<id>` appended), retargets on rename, unbinds on delete / lost record /
  lost `:view`, and with `reset` (project open, `generation` moved) starts
  every view afresh. The UI (`host_commands::instances::sync_instance_views`,
  run from the reactive tick after `sync_instance_records` and from every
  instance host command) mirrors the changes: new buffer (with the kind's
  `:keymap`) and tab; rename = `Editor::rename_buffer` in place plus a tab
  update; removal = tab, then buffer. Create and duplicate open the new
  instance's tab; new host command `instance-open {:id}` (re)opens one.
- **Tabs** (`content/ui/seq-step-tabs.lisp`): instance tab records are
  `(label buffer :instance id)`, keyed by id (`seq-register-instance-tab`,
  `seq-update-instance-tab`, `seq-unregister-instance-tab`,
  `seq-open-instance-tab`, `seq-instance-tab-buffer`). A rename updates the
  tab in place and never reopens a closed one; the × only unregisters the
  tab. Instance tabs are neither script tabs (no sequencer delete on ×,
  not cleared by `seq-clear-project-script-tabs`) nor source tabs.
- **Key scoping.** A binding's key scope (the host uses `instance:<id>`)
  applies while anything renders into that target, top level or a subtree
  rerun: `qualify_widget_stable_key` writes `__stable-key` =
  `instance:<id>::<module-qualified key>` and subtree keys (`subtree`,
  `subtree-owner`) get the same prefix before they hash into root ids.
  The authored `:key` prop is untouched.
- **Patch-bay namespace.** `graph_node_lane_patch_namespace(graph id, node)`
  = `1024 + slot * 4096 + node`, where `slot` is the graph id itself below
  2^23 (instance ids) and a fold into `[2^23, 2^24)` above (legacy name
  hashes; two can collide only if equal mod 2^23), keeping every port id
  under 2^53. `graph-node-lane-patch` mints ids in it; the new native
  `graph-node-patch-namespace` exposes it, and
  `eseq.sequencer/lane-patch-node-namespace` now takes `(graph node)`. The
  UI bin's track projection (`build_track_lane_patch_value`) is unchanged:
  tracks keep namespace = track index below 1024.
- **`:keymap`** is a `def-kind` slot (a mode name; a bare symbol is its
  name), carried on `InstanceKindSchema::keymap` and `KindDefinition`; the
  host gives it to every instance buffer, replacing `set-buffer-mode-for`.

## 8. Packages tab

### 8.1 Manifest declares kinds

The tree is built from manifests without evaluating code, so kinds are declared
there:

```json
{
  "name": "alez/neural",
  "version": "0.1.0",
  "entry": "alez.neural.variable-reset",
  "kinds": [ { "name": "neural", "module": "alez.neural.variable-reset" } ]
}
```

On attach: a manifest kind the module never `def-kind`s is an error; a
`def-kind` missing from the manifest is a warning (it works once loaded but
cannot be offered before attach).

Built: `PackageManifest::kinds` (`[{name, module}]`, validated: names are
identifier segments without `:`, unique, modules inside the package
namespace). Attaching to the project loads the module, then
`check_manifest_kinds` runs for that module's declared kinds: a missing
`def-kind` fails the attach before the import line is written (the module
stays loaded for the session, like any failed attach); undeclared
`def-kind`s are appended to the status as warnings.

### 8.2 Tree

```
LOADED
  ▾ alez.neural.variable-reset          ✓ (3)
        neural 1               project
        neural 2               Kit A
        neural 3               Kit B
    alez.tracker                         ✓ (1)
    alez.sig                             ✓
```

- The count badge sits after the check and counts every instance of the
  module's kinds, whatever owns it. No badge at zero.
- The row expands (collapsed by default) to one row per instance with its owner.

### 8.3 Actions

Module row:

- **Double-click**: no kinds → attach (today). Exactly one kind and zero
  instances → create the first instance, attaching first. Otherwise toggle the
  row open.
- **Menu**: `New <kind>` per kind (attaches first if needed), then the existing
  Attach / Detach / Always load / Open source / Copy to Local.
- **Detach with instances**: confirm "Detach alez/neural and delete its 3
  instances?"; one undoable edit. `import` cannot unload, so the kind stays
  registered until reopen, as today.

Instance row: `Open` (double-click), `Rename`, `Duplicate`, `Move to rack ▸` /
`Give back to project`, `Delete`.

Rack panel: "attach sequencer" becomes a kind picker that creates an instance
owned by that rack.

Built (stage 5):

- **Data.** The reactive tick publishes `SEQ.instances` (`{:id :kind :label
  :owner-rack :owner-label :registered?}` per instance, republished on a
  fingerprint change, so rack renames and kind registration show up); the
  tab passes it to `(seq-package-tree query instances)`, which is how the
  tree re-renders on instance edits. Module kinds come from every installed
  manifest's `kinds` plus registered `def-kind`s no manifest declares
  (`module_kinds`); `(seq-instance-kinds)` lists the same set (plus
  project-code kinds) for the rack picker.
- **Tree.** Every module row (tier and Loaded) of a kind-defining module
  carries `:kinds`, `:instance-count` and, above zero, `:badge`: the tree
  widget's new `:badge` item field draws a round chip with the count at the
  right edge, the status check just before it. Its children are one
  `instance` row per instance (label, owner as the detail, `· not loaded`
  for a placeholder; identity `instance:<id>` so expansion survives
  renames). A search hit on a module keeps all its instance rows.
  Decision: instances no module row claims (package missing, or a kind
  defined in project code) are grouped per kind at the end of Loaded as an
  `orphan` row, `thing (package gone/pkg missing)` / `thing (project code)`:
  this is where the §5 placeholder is visible and deletable.
- **Double-click.** The tree uses `:activate-parents true`: the first click
  of a double-click already toggles a parent row, so a module row WITH
  instances does nothing more (the toggle is the gesture). Decision: a
  module with several kinds and no instances attaches, like a kind-less one
  (there is nothing to toggle). Package rows with source children only
  toggle.
- **Menus.** Module: `New <kind>` per kind, then Attach/Remove, Always
  Load, View Source, Copy to Local. Instance: Open, Rename (an inline text
  field above the tree), Duplicate, one `Move to <rack>` per other rack
  (context menus have no submenus), `Give back to project` when
  rack-owned, Delete; a placeholder offers Rename, the moves and Delete.
- **Host.** `packages-new-instance {:module :kind [:group-id]}` attaches
  the module when the evaluated scratch does not import it, checks the kind
  registered, then runs `instance-create` (records, view buffer, tab).
  `packages-detach` of a module whose kinds have instances opens
  `eseq.file-dialogs/open-confirm` ("Detach alez/neural and delete its 3
  instances?"); Continue re-sends it with `:confirmed true`, and
  `App::delete_instances_recorded` deletes them AND drops the module's
  import from the evaluated scratch in one recorded edit. History records
  only that import's presence (`InstanceStructureState::scratch_import`),
  never a scratch snapshot: undo re-adds that one line (after the leading
  imports) to whatever the evaluated scratch is by then, and redo removes it
  again, so scratch edits made after the detach (other attaches, evaluated
  code) survive both. The draft scratch buffer and the module's `override`
  toggle live outside history; the event loop's undo/redo replays
  `EditPatch::replayed_scratch_imports` into both
  (`packages::apply_replayed_scratch_imports`), so after an undo the draft
  imports the module again (the next scratch evaluation keeps it) and its
  overrides are back on. Detach switches the overrides off whenever either
  scratch text lost the import (the draft may already lack it).
- **Rack panel.** The rack's context menu (mixer) gains `New <kind> in rack`
  per kind (the same `packages-new-instance` with `:group-id`); the
  existing Attach/Detach entries stay for legacy scripts and now show an
  instance's label instead of `neural#<id>`.
- `metal_seq capture` applies `instance-*` host commands from a fixture
  and publishes `SEQ.instances`, so the tab can be rendered headlessly.

## 9. Kits

- A saved kit records instances as data: `{kind, label, overrides}`, replacing
  `ProjectRackSequencer { sequencer_id, sequencer_name, source }`.
- Loading a kit always assigns **fresh** instance ids and re-keys the saved
  overrides and rack-clip references to them (extends the existing
  old-id → new-id map in `register_kit_sequencers`,
  `src/app/break_kits.rs`). Two kits, or one kit loaded twice, give independent
  instances.
- A kind whose package is installed but detached attaches it; an uninstalled one
  becomes a placeholder (§5).

Built (§9, stage 6): `ProjectKitPreset::instances` (kit format 4) holds
`ProjectKitInstance { id, kind, label, overrides }`; `sequencers` keeps only
legacy plain scripts.

- `id` is a kit-local key (the exporting instance id), needed because the
  kit's clip overrides are keyed by it; loading never reuses it.
- `overrides` is the instance's override set in the current scene at export,
  routes in pad space. It is applied only when the kit has no clips (a break
  kit's clips carry per-clip overrides): into every scene when the target
  rack has no clip bank, else into every clip of its bank.
- `register_kit_sequencers` returns kit id -> (new id, published name);
  each instance gets `allocate_instance_id` in one recorded edit inside the
  kit load's undo entry, and `install_kit_clips` re-keys clip overrides'
  id and name and re-mints their node process slot ids (one identity per
  new sequencer across clips), so one kit loaded twice shares no process
  state. A clip override whose id the kit does not record is dropped on load,
  and export (`capture_kit_content`) keeps only overrides of the rack's
  instances and recorded sequencers: instance ids are small counters, so a
  stale one would match an unrelated instance of the importing project. The label is kept unless another instance already uses it
  (then the kind's next default label).
- Auditioning a kit onto a rack (`load_kit_onto_rack`) first deletes the
  rack's instances and their overrides (recorded), like its sequencers.
- Old kits: `migrate_kit_sequencers` turns an `(import m)` whose module
  declares exactly one kind into an instance record keyed by the recorded
  `sequencer_id`, so it gets a fresh id and its clip overrides follow; a
  module with several kinds is reported and kept as a script.
- Host side (`load-kit`): for each kind the rack's instances use, the module
  its installed package declares (`declared_module_for_kind`) is attached to
  the project scratch (idempotent). A kind no installed package declares is
  reported; its instance is a placeholder (kept, overrides waiting under the
  fresh id, unpublished) that publishes when the kind registers.
- Export warns about an instance of a `scratch:` kind (project code, not a
  package): it only comes back where that code is evaluated.

## 10. Migration

- A recorded `(import <module>)` rack source, and a project-owned import of a
  module that declares a kind, become one instance of that module's kind that
  **keeps its old `sequencer_id`**, so existing overrides still match. A module
  declaring more than one kind is reported, not guessed.
- Kits with source-string sequencers migrate the same way on load (fresh ids per
  §9).
- `alez.neural.variable-reset` is ported: `gvr-name` → `self`, view `defstate`s
  → `:state`, cache `defstate`s deleted (§6), buffer/tab tail deleted.

Built (§10, stage 7, the port): `variable-reset.lisp` is now only helpers
plus `(def-kind neural … :view gvr-panel :keymap
eseq.sequencer-keys/sequencer-keys :on-create gvr-init-ring-defaults)`; the
legacy `def-sequencer "variable-reset"`, `gvr-name` and `gvr-owner-rack`
globals, and the `effect-buffer` / `set-buffer-mode-for` /
`seq-register-script-step-sequencer-tab` tail are gone. Every helper takes
the instance first. `:state` is `expanded-node selected-neuron add-class
map-slot map-port piano-depth` (map arming moved from globals too, so arming
in one instance does not light another). Deleted caches: `gvr-weights`,
`gvr-threshold`, `gvr-global-transpose`, `gvr-dur-factor`,
`gvr-proc-version`, `gvr-delay-factor-index`, `gvr-timebase-factor-index`, and
every `reactive-set "GRAPH"` echo (route colors included). Reads: the weight
matrix and edge strips read `graph-edge-value` per cell (the matrix in its own
subtree so a drag does not re-run the rows); enum dropdowns (route,
resolution, quantize, poly mode) read `graph-node-value` /
`graph-config-value`, as §6 prescribes; numeric fields keep `bind-graph*`
handles (the batch threshold / global transpose / dur x pickers bind node 0,
which every batch write echoes); the node patch reads
`graph-node-process-chain` / `graph-node-lane-patch` directly. The owner chip
reads `self.owner`. Exported for scripts/fixtures: `gvr-panel`,
`gvr-init-ring-defaults`, `gvr-expand-node`, `gvr-map-arm`,
`gvr-edit-config`, `gvr-node-count`, all taking the instance first. The
fixtures `graph-node-patchbay.lisp` / `graph-node-map-arm.lisp` create an
instance with `(host-command "instance-create" …)` at top level and address it
with `(instance-ref 1)`; capture now runs fixture instance commands through
the live path (records, view buffers, tabs, `:on-create`), so they capture
with `--buffer "*neural · neural 1*"`. The standalone demo
`content/scripts/sequencers/graph-neural-variable-reset-demo.lisp` stays a
kind-less legacy script (its own `*variable-reset*` buffer): it is the
reference plain rack script that §5's retained `with_graph_owner_rack` path
and its tests exercise, so it was not converted.

Built (§10, rack and project records): `app::migrate_legacy_kind_sources`
runs on the project file in `queue_loaded_project`, from package manifests
alone (no evaluation). Project file version 14 marks files that have
instances.

- A rack record `(import m)` whose module declares exactly one kind becomes
  an instance owned by that rack with the recorded `sequencer_id`; the
  record is dropped, and `(import m)` is added to the project scratch (like
  attaching the package) because nothing else would register the kind.
- In files below v14 only, a scratch `(import m)` of a one-kind module that
  no rack records becomes a project instance with id
  `graph_instance_id(legacy name, None)`. The legacy name comes from the
  manifest entry's new optional `legacy_sequencer` field (the old
  `def-sequencer` name; the kind name when absent). From v14 on a scratch
  import alone creates nothing, so a deleted instance does not come back.
- A module declaring more than one kind is reported in the open status and
  its rack record stays a legacy script. Migration notes are appended to the
  open status.
- Kits migrate on load (§9 "Built"), with fresh ids.

## 11. Open questions

- ~~Can the `edges` form express the ring default (weights computed from
  from/to)? If yes, a fresh instance is already correct and kinds need no
  `:on-create` hook; if not, add `:on-create` only for this.~~ Resolved
  (stage 7): no. `edges :params` defaults are scalars and `:topology` only
  knows `all-to-all`, so a ring would need a new per-edge default expression
  in the graph DSL. `def-kind` got a minimal `:on-create f` instead: stored
  on `InstanceKindSchema::on_create` (VM-side only; the host registry does not
  carry it), run by `lisp_host::run_instance_on_create` in the UI VM with the
  new instance after `instance-create` has published its sequencer and
  created its record, so `graph-*` writes land on that instance's overrides
  in the current pattern. It runs ONLY for `instance-create` (Packages tab
  "New …", rack "New … in rack", `packages-new-instance`): never for a
  duplicate, kit load, migration or project open, which already carry their
  document. Its writes are ordinary override edits, not part of the create's
  undo entry (like every `graph-*` edit today); undoing the create leaves
  them under an id the allocator never hands out again (§5). A failing hook
  keeps the instance and reports the error in the status line. The neural
  hook writes only the ring cells (n → n+1 at weight 1) and turns on node 0's
  seed-from-route; the old explicit demo setup that seeded nodes 0–3 from
  tracks 1, 2, 3, 5 is gone.
- ~~What happens to a rack's instances when the rack goes?~~ Decided (final
  review): they never outlive it as `Rack(gid)`. A dissolve that keeps the
  tracks gives them back to the project; a delete that takes the tracks
  deletes them (§5).
- ~~Where do a moved instance's overrides land when the old or new rack has
  a clip bank?~~ Decided (final review): per-scene views are read before the
  move; every clip of a clip-bearing destination gets one; entries in the old
  rack's unlaunched clips are dropped (§5).
- Map keys: instance handles are numbers; if `:state` ever holds maps keyed by
  lists, `Value` needs a hashable form.
- ~~Whether `(set! self.x v)` already compiles through the existing dotted
  `set!` path or needs a `SetField` opcode for instances.~~ Resolved (stage 1):
  the existing dotted `set!` path (load + `StoreField`) suffices; `StoreField`
  and `GetField` dispatch on `Value::Instance` (`vm/instances.rs`). No new
  opcode. Each (instance, field) cell is a `NamespaceField` DAG source under a
  reserved `%instance/<id>` namespace, so subtree caching and effect
  scheduling need no new machinery. Writes of an equal value dirty nothing.
  Without a host label hook, `(set! self.label v)` writes the label cell
  locally; with one (`VM::set_instance_label_hook`) it is forwarded to the host,
  which pushes the accepted label back. A never-created id is an error, not a
  stale handle.

## 12. Stages

1. Reactive instance records in eseqlisp (`Value::Instance`, tracked
   `GetField`/`SetField`, schema check).
2. Reactive graph reads (§6).
3. Instance registry + `def-kind` + manifest `kinds` on the host; project and
   rack lists; delete the module-owner machinery; migration (§10).
4. Host-created buffers and tabs per instance, key scoping, patch-bay namespace.
5. Packages tab: badge, instance rows, menus, double-click, detach confirm.
6. Kits save/load instances as data with fresh ids; regression test: two kits
   with a neural sequencer each → two tabs, editing a weight in one leaves the
   other unchanged.
7. Port `alez.neural.variable-reset`.
