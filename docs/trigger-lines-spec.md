# Trigger lines: named destinations with per-scene listeners

Status: spec rev 1, 2026-10-24. Bead: `eseq-ka5t.7` (epic `eseq-ka5t`, the
rig). Extends `docs/rig-spec.md` §11. Nothing here is built.

**Built-in UI first**, as in the rig spec: everything ships in the built-in
UI (route menus, sequencer rows, drum rack panel, a small lines panel) and is
fully usable there. A rig view consumes the same host fields and commands.

## 1. Goal

Autechre's cascade sequencer sends to named trigger lines (`drumtrig 1`,
`drumtrig 2`), and whatever listens plays. eseq sequencers route to a track
index, or to a pad (member) index inside a kit. To change what a sequencer
plays in bank C, the user has to edit its routes in bank C's patterns, and
for a project-owned sequencer whose routes are shared across scenes, that
edit leaks into other banks.

A **trigger line** is a named destination. A sequencer routes a row, node or
emission to a line instead of a track. Each scene says which tracks **listen**
to each line. The sequencer keeps its routing; the bank changes what listens.
This is the rig's one rule again: who plays is scene state (rig-spec §3,
§10).

**User story.** A project-owned jaki "cascade" sends rows 0-3 to lines
`drumtrig 1`..`drumtrig 4`. In banks A and B the lines are heard by the
Jungle-ology kit's kick, snare and two hat pads. The user makes bank C, keeps
the cascade untouched, and in bank C points `drumtrig 1` and `drumtrig 2` at
the Gotham Piano kit's low pads and `drumtrig 3` at a bass synth track, then
parks the Jungle kit (rig-spec §6). Banks A and B are not touched.

Not a goal: a general event bus. Lines carry note triggers from sequencers to
tracks. Values stay in channels (`defchan`, `send`, `chan-get`); §11 says why
lines are not channels.

## 2. What exists today

- **No shared destination type.** Graph node overrides store
  `ProjectGraphRouteOverride { None, Track(usize), Generator(u64),
  GeneratorRestart(u64) }` (`runtime/graph.rs:495`); the runtime node holds
  `route: Option<usize>` plus `gate_target`. Generators (`seq-emit :track n`)
  and processes (`emit :track n`) produce `EmittedAccumulatorEvent { track:
  Option<usize>, .. }` (`lisp_host/shared_state.rs:116`). Jaki rows store an
  integer in the instance document: `n >= 0` track or pad, `-1` Off, `-2`
  chords (`alez.jaki/src/doc.lisp`). The legacy `ProjectNeuralNetwork` stores
  `ProjectNeuron.route: Option<usize>`.
- **Rack-owned sequencers address members.** Graph routes are member indices
  resolved through `resolve_rack_member_routes` (`graph.rs:790`); generator
  emissions through `map_rack_member_emissions` (`scheduler/lookahead.rs`).
  Kit export converts member to pad space for graph routes only; jaki
  documents travel verbatim.
- **One meeting point.** Graph, generator (jaki included) and process hits all
  reach `enqueue_emitted_network_event_with_midi_fx`
  (`scheduler/enqueue.rs:625`), which calls
  `EmittedNetworkEventSource::resolve_track` and then
  `enqueue_step_event_with_midi_fx`. The legacy neural path
  (`enqueue_neural_output_with_midi_fx`) and step ratchets do not. All of it
  runs on the `sequencer-scheduler` thread, which already allocates; only
  mod-input triggers (`audio/mod_trigger.rs`) run on the audio thread, and
  they have no route.
- **Channels** are a flat, string-named, project-wide keyspace declared in
  Lisp source and not saved as data (`ProcessRuntime.channels`,
  `runtime/process.rs`). A `send` runs listening processes in the same
  cascade; `chan-get` in a generator reads a once-per-chunk snapshot.
- **Scene-owned swap state.** `Scene.slot_enable` and `Scene.parked_kits` live
  in `SceneSwapState` (`sequencer/state/slot_enable.rs:38`), so every scene
  creation path inherits them by adding a field to `of` / `apply_to`.
  `SequencerSnapshot::park_kits` applies the parked set of the scene a
  snapshot plays, in all four capture cases (live, boundary launch, per-track
  launch, song row). Bank-scoped commands follow `set-kit-parked`
  (`app/kit_parking.rs`): one `apply_recorded_scene_structure_mutation` over
  the bank's scene range, then `publish_scheduler_snapshot`.

## 3. Model

### 3.1 Lines

```rust
// sequencer/state/trigger_lines.rs, owned by ProjectScenes
pub struct TriggerLine {
    pub id: TriggerLineId,   // stable u64, minted from next_trigger_line_id, never reused
    pub name: String,        // unique in the project, case-insensitive
}
// ProjectScenes gains: trigger_lines: Vec<TriggerLine>, next_trigger_line_id: u64
```

- **Project-level registry.** One flat list per project, in display order.
  Lines are few (tens), so no namespacing.
- **Identity is the id.** Stored routes hold the id; renaming changes only
  the label, so every route follows a rename. Code names lines by string
  (§4.2) and resolves the name to an id.
- **Names.** Default `line 1`, `line 2`, …; the user renames freely. A
  duplicate name is refused with a status message.
- **Lives in `ProjectScenes`**, next to the scenes that hold its listeners, so
  the scene-structure undo memento and the scenes revision (which drives
  bank-usage recompute) cover it with no new mechanism.
- **Deleting a line** removes it from the registry and its listeners from
  every scene. Routes that name it are left alone and resolve to nothing; the
  route control shows `(deleted line)`. No route scan, and undo restores the
  line with every route still pointing at it. Because ids are never reused, a
  dangling route can never land on a later line.

### 3.2 Listeners are scene state

```rust
pub struct LineListener {
    pub track: usize,        // a track index; a kit pad is its member track
    pub transpose: i8,       // semitones added to the incoming note, default 0
    pub velocity: f32,       // scale on the incoming velocity, default 1.0
}
// Scene gains: line_listeners: BTreeMap<TriggerLineId, Vec<LineListener>>
```

- **One line, many listeners.** A hit on a line plays once on each listener.
  Listeners for the same track are deduplicated when the scheduler table is
  built (§5); two different transforms on one track are two notes.
- **No entry means nobody listens.** A scene with no listeners for a line
  plays nothing for it, like an arrangement span with no clip. There is no
  project-wide default listener set: creation-path inheritance (below) is
  what makes a new scene sound like its bank.
- **Pads are tracks.** A kit pad is stored as its member track index, the
  same index everything else uses, and is shown as "Kit / pad name". Track
  delete and move remap it like `slot_enable` (`remap_optional_track_after_delete`,
  `move_track_slot_enable`); a deleted track's listeners are dropped.
- **Transform is minimal.** Pass-through note plus transpose, velocity
  scale. A drum pad ignores the note the way it does for a track route today
  (pad notes are transposes, 0 = C4). A fixed-note mode is an open question
  (§12).
- **Inheritance.** `line_listeners` joins `SceneSwapState`, so
  `clone-scene!`, `copy-scene-to-new-bank!` and the blank `+` scene copy it
  exactly as they copy `slot_enable` and `parked_kits` (rig-spec §3.3).
- **Parked kits.** A listener whose track belongs to a kit parked in that
  scene plays nothing (rig-spec §6: a parked kit plays nothing). The listener
  stays stored, so turning the kit back on restores it.

### 3.3 Edit scope

Listeners are stored per scene and edited per **bank**, the rule rig-spec §4
settled for devices ("a change applies to the viewed bank and never to other
banks"):

| Edit | Writes |
|---|---|
| Add listener *track* to *line* | every scene of the viewed bank gains it (deduplicated) |
| Remove listener | removed from every scene of the viewed bank |
| Change a listener's transform | every scene of the viewed bank that has that track on that line |
| "In all banks" (menu item, says so in its label) | the same edit over every scene |
| Create / rename / delete line | registry; delete also clears listeners everywhere |

A **mixed bank** (scenes differ, e.g. after copying scenes between banks)
shows the listener with a small dot, like the kit toggle; clicking it sets
the whole bank. Each edit is one undo step.

## 4. How sequencers target lines

A route value gains a third variant everywhere a route is stored: **off |
track (or pad) | line**.

### 4.1 Stored routes

| Kind | Today | Adds |
|---|---|---|
| Graph node override (`ProjectGraphRouteOverride`) | `None`, `Track(n)`, `Generator(id)`, `GeneratorRestart(id)` | `Line(TriggerLineId)`, JSON `{"line": id}`, Lisp `(graph-node n i :route (list :line id))` |
| Graph runtime node | `route: Option<usize>` | `route: Option<EmitTarget>`, `EmitTarget::{Track(usize), Line(TriggerLineId)}` |
| Jaki row (document) | `:route n` (`-1` off, `-2` chords) | `:route (list :line id)`; integers keep their meaning |
| Generator `seq-emit` | `:track n` | `:line "name"` (or an id); exclusive with `:track` |
| Process `emit` | `:track n` | `:line "name"`; exclusive with `:track` |
| Legacy `ProjectNeuralNetwork` | `route: Option<usize>` | none: no rack mapping either, superseded by the neural kind |

`EmittedAccumulatorEvent` gains `line: Option<TriggerLineId>`; `track` and
`line` are never both set.

### 4.2 Names in code

`seq-emit :line "drumtrig 1"` resolves the name against the snapshot's line
table on the scheduler VM (one hash lookup per emit, on a thread that already
allocates). An unknown name emits nothing and logs once under
`ESEQ_DEBUG_ROUTING`, like an out-of-range track. Code that names a line
breaks on rename, like a `defchan` name; documents (graph overrides, jaki
rows) hold ids and do not.

### 4.3 Rack-owned sequencers

A line is outside the kit. A rack-owned sequencer may route to a project line;
the member mapping (`resolve_rack_member_routes`,
`map_rack_member_emissions`) skips line targets, so a member route still
means "my pad" and a line route means "whoever listens". That lets gotham's
rack-owned neural sequencer feed a line as well as its own pads.

### 4.4 What does not change

Gate targets (`Generator`, `GeneratorRestart`) stay separate: a line plays
notes, it does not gate generators (§12). A track's own pattern steps, step
ratchets and mod-input triggers keep playing their own track. Every existing
route keeps its stored value and meaning, so no migration is needed (§9).

## 5. Scheduler resolution

**Where.** In `enqueue_emitted_network_event_with_midi_fx`, before
`resolve_track`: an event whose `line` is set is expanded into one event per
resolved listener (note + transpose, velocity × scale), and each copy goes
down the existing per-track path (`enqueue_step_event_with_midi_fx`), so it
runs the listener track's MIDI fx and process cascade exactly as a direct
track route would. Generator gates opened by a hit (`seed_generator_gates_from_hit`)
and `seq-emit :lock` holds key off each resolved track.

**Which scene's listeners.** The scene the snapshot plays, by the same rule
`park_kits` uses:

| Snapshot | Listeners from |
|---|---|
| Live capture | the playing scene |
| Boundary (scene) launch | the target scene |
| Per-track launch | the playing scene |
| Song row | the row's scene |

A hit resolves against the snapshot active for the chunk it is enqueued in.
A hit scheduled before a scene boundary for a sample past it keeps its old
listeners, the rule a direct track route already follows.

**The table.** At snapshot capture (control side, under the scenes lock in
`capture_scene_track_inputs`, and as a required parameter of
`capture_from_track_pattern_data` so no builder can forget it, as with
`parked_kits`), build

```rust
pub struct LineTable {
    pub by_id: HashMap<TriggerLineId, Range<u32>>,  // into listeners
    pub by_name: HashMap<Box<str>, TriggerLineId>,   // for code (§4.2)
    pub listeners: Vec<ResolvedListener>,            // track, transpose, velocity
}
// SequencerSnapshot gains: trigger_lines: Arc<LineTable>
```

with parked-kit members, out-of-range tracks and duplicates already removed.
The scheduler only reads it: expansion is a range walk with no lookups past
the id. The event clone per listener is the same allocation the existing path
makes for a direct route (chord vectors). Nothing runs on the audio thread.

**Timing.** Zero added latency: every copy keeps the hit's beat and
`sample_time`. A line is not a channel and has no one-chunk delay.

**Loops.** Lines end at tracks and tracks do not re-emit to lines, except a
process on a listening track calling `emit :line`, which the existing
`PROCESS_EVENT_CASCADE_LIMIT` bounds.

## 6. Built-in UI

- **Route menus** (graph `content/ui/graph-kit.lisp`, jaki
  `alez.jaki/src/kind.lisp`, process lane inlets in `content/ui/sequencer.lisp`)
  gain a **Lines** group after the tracks: every line, labelled with the
  viewed bank's listeners (`drumtrig 1 → Kick, Snare`), then `New line…`,
  which creates `line N`, sets the route and opens the name for editing.
  Lines are not filtered by bank: there are few of them and a line with no
  listeners in this bank is still a valid target. A route to a line shows the
  line's name in the control; a deleted one shows `(deleted line)`.
- **Subscribe a track.** A sequencer row's context menu and the drum rack
  panel's pad menu gain `Listen to line ▸`, a check list of lines; each
  toggle is bank-scoped and says so ("in bank C"). A mixed bank shows a dot.
- **Badge.** A track's sequencer row shows `← drumtrig 1` (first line, then
  `+N`) when it listens in the current scene; clicking it opens the lines
  panel at that line.
- **Lines panel** (a `*lines*` buffer, File/View menu and the badge): one row
  per line with an editable name, the viewed bank's listeners as chips (×
  removes, `+` adds from a bank-filtered track/pad menu, click a chip to edit
  transpose and velocity), the banks where it has listeners, the sequencers
  routed to it, and delete. "In all banks" variants live in each row's menu.
- **Lisp surface** (`eseq.kinds`): `(trigger-lines)`; fields `line.id`,
  `line.name`, `line.listeners` (current scene), `line.used-banks`,
  `line.mixed-banks`; `create-trigger-line!`, `rename-trigger-line!`,
  `delete-trigger-line!`, `set-line-listener!` (line, track, bank-or-nil,
  on), `set-line-listener-transform!`. Host fields are pushed on scenes
  revision change, like `bank_usage` and `kit_parking`.

## 7. Interactions

- **Bank display (rig-spec §9).** A scene uses a track when it lists the track
  as a listener of any line, so a synth that plays only through a line in
  bank C shows in bank C. `ProjectScenes::bank_usage` reads `line_listeners`
  next to `cells` and `parked_kits`. A listener on a parked kit's pad does
  not count. Lines themselves are not hidden (§6).
- **Parking (rig-spec §5, §6).** Covered in §3.2 and §5. CPU parking needs
  nothing new: a parked kit's pads get no line triggers, the same as no
  pattern triggers.
- **Purge.** A line with no listeners in any scene and no route naming it is
  listed by purge alongside unused slots and kits, never deleted
  automatically. Finding routes means a scan of graph overrides and jaki
  documents; purge is rare, so that is acceptable there and nowhere else.
- **Controlled import (rig-spec §8).** Lines travel **by name**. An imported
  route or listener whose line name exists in the master project joins that
  line; otherwise the line is created. Imported listeners land only in the
  target bank's scenes, with track indices remapped through the import's
  track map (`remap_project_routes`); a listener on a track that was not
  picked is dropped and listed as unconnected in the import summary. A
  master line that gains imported listeners keeps its other banks unchanged.
- **Kit export/load.** A rack-owned route to a line is exported as the line's
  name (pad routes keep their pad-space conversion) and resolved by name on
  load, creating the line if missing. Kit presets carry no listeners: what
  listens is the master project's scene state.
- **Undo.** Every edit in §3.3 is one `apply_recorded_scene_structure_mutation`;
  the memento is the whole `ProjectScenes`, so registry and listeners need no
  new history code. Track topology undo restores listeners with
  `slot_enable` (keyed by `SceneId`).
- **Arrangement.** A song row plays its scene's listeners (§5). The
  arrangement view's track list spans banks and shows every track, as now.
- **Recording.** Record-as-heard captures what tracks play; whether a hit
  that arrived through a line records onto the listener track is an open
  question (§12). v1 records nothing new.

## 8. Serialization

Next project file version (20 at the time of writing; take whatever number
is free when this lands, since older builds must refuse a file whose routes
they cannot read):

```rust
#[serde(default, skip_serializing_if = "Vec::is_empty")]
pub trigger_lines: Vec<ProjectTriggerLine>,          // { id, name }
#[serde(default)]
pub next_trigger_line_id: u64,
#[serde(default, skip_serializing_if = "Vec::is_empty")]
pub scene_line_listeners: Vec<Vec<ProjectLineListeners>>, // per scene: { line, listeners: [{ track, transpose, velocity }] }
```

Added in the same places as `scene_parked_kits` (file struct, wire struct,
conversion, default, save in `app/projects.rs`, install after rack clips,
slot enable and parked kits). `ProjectGraphRouteOverride` gains
`Line(TriggerLineId)` (`{"line": id}`); jaki rows gain the `(list :line id)`
literal. A project with no lines writes none of the new fields, but still
the new version number.

## 9. Migration

None. Existing routes are track, pad, off or generator and keep their stored
form and meaning. A legacy file has no lines and no listeners; every scene
loads with an empty `line_listeners`.

## 10. Test plan

State (`sequencer/state/trigger_lines.rs` tests):
- create, rename (duplicate refused), delete; ids never reused.
- `line_listeners` inherited by clone, copy-to-new-bank and the blank `+`
  scene (extend `slot_enable.rs` inheritance tests).
- track delete drops and remaps listeners; track move remaps.
- bank-scoped add/remove/transform touches only the bank's scenes; mixed
  bank detection.

Scheduler (`scheduler/tests.rs`):
- a graph node routed to a line with two listeners plays two notes at the
  same `sample_time`, each through its track's MIDI fx.
- the same project, launched into a bank-C scene, plays the bank-C listener
  set; per-track launch keeps the playing scene's; song row uses the row
  scene's.
- jaki row `(list :line id)`, `seq-emit :line "name"` and process
  `emit :line` each reach listeners; unknown name and deleted line emit
  nothing.
- a listener on a parked kit's pad is silent; unparking restores it.
- a rack-owned graph's line route is not member-mapped.
- duplicate listeners on one track fire once; transpose and velocity apply.
- the snapshot is built with the table once per capture (assert on a capture
  counter, as the park-kits tests do).

Project and import:
- save/load round-trip of lines and listeners; a v19 file loads unchanged and
  plays identically (gotham fixture: same scheduled events before and after).
- controlled import matches lines by name, creates missing ones, lands
  listeners only in the target bank, reports dropped listeners.
- kit export/load of a rack-owned line route.

UI (`ui/host_kinds/tests/`):
- `line.listeners` / `used-banks` / `mixed-banks` follow scene edits.
- a listener-only track shows in its bank (`track.used-banks`).
- route menus list lines with the viewed bank's listener summary.

## 11. Why not channels

Channels are values with listener *processes*, declared in source and not
saved; a `chan-get` reads one chunk late. Lines are saved project data, hold
per-scene listener sets that the user edits in the UI, and must expand at the
hit's own sample into track note-ons with each track's MIDI fx. Sharing the
keyspace would make every line a source-code declaration and every channel a
saved object. A process can still bridge them: `(on (chan "x") (fn (v) (emit
:line "drumtrig 1" …)))`.

## 12. Open questions

1. **Fixed note.** Should a listener be able to play its own note instead of
   the incoming one plus transpose (useful for a melodic track listening to
   a drum line)? Proposed: v1 pass-through + transpose only.
2. **Lines that gate generators.** Autechre chains cascades; should a
   listener be a generator gate (a jaki instance) as well as a track?
   Proposed: not in v1; graph gate routes already cover direct chaining.
3. **Rename vs code.** Code names lines by string and breaks on rename.
   Warn when a renamed line is named in loaded source, or accept it like
   `defchan`?
4. **Kit-scoped lines.** Should a kit carry its own lines (listeners inside
   the kit, travelling with it), or are pad routes enough for that?
5. **Tracks as sources.** Should a track's own pattern steps be able to send
   to a line (a track as a trigger source), or only sequencers?
6. **Recording.** Should a hit that arrives through a line record onto the
   listener track during record-as-heard?
7. **Per-scene variation.** Bank scope only (as rig-spec §4 decided for
   devices), or a per-scene edit too?

## 13. Build order

| Bead | Title | Scope |
|---|---|---|
| `.7.1` | Line registry and scene listeners | `TriggerLine` registry in `ProjectScenes`, `Scene.line_listeners` in `SceneSwapState`, track delete/move remap, bank-scoped commands (create, rename, delete, set listener, transform) with undo, host fields, `bank_usage` input, serialization version bump. No routing yet. |
| `.7.2` | Scheduler expansion and line routes | `LineTable` on every snapshot (four capture cases, parked filter, dedupe), `EmitTarget::Line` in graph overrides and runtime, `seq-emit :line` / `emit :line`, expansion in `enqueue_emitted_network_event_with_midi_fx`, rack member mapping skips lines; scheduler tests. |
| `.7.3` | Built-in UI | Lines group and `New line…` in graph, jaki and lane route menus; jaki `(list :line id)` rows; `Listen to line ▸` on sequencer rows and drum rack pads; row badge; `*lines*` panel. |
| `.7.4` | Import, export and purge | Controlled import and kit export/load by line name, import summary for dropped listeners, purge lists unused lines. |

`.7.1` and `.7.2` can land together behind no UI (tested from Lisp); `.7.3`
makes them usable; `.7.4` needs `.7.1` and `.7.2`.
