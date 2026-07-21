# Undo/Redo System Spec

## Status

Implemented through Slice 6.

- Slice 0 provides stable track identity, bounded linear history, barriers,
  revision tracking, and global undo/redo shortcuts.
- Slice 1 covers lossless step cells, bulk deletion/paste, drum lanes, and
  piano-roll add/remove/move/resize/note changes.
- Slice 2 covers pattern geometry, track-level p-locks, and variant stamp/clear.
- Slice 3 covers track, mixer, routing, accumulator, base-note, master-volume,
  and BPM edits with stable Track Pattern targets and gesture coalescing.
- Slice 4 covers stable track/rack device identities, scalar and tensor
  defaults, scalar and tensor p-locks, instrument key locks and variants, rack
  strip/instrument/effect values, instrument preset application, and
  convolution IR selection with retained prepared data for fileless redo.
- Slice 5 covers custom-instrument replacement, sampler/custom conversion,
  retained engine and sample resources, stable async swap targeting, and exact
  restoration of every track pattern's instrument state, process bindings,
  project macro mappings, and scene neural instrument overrides. The Slice 5
  rack extension covers appending layer-rack slots and replacing layer-rack
  instrument/sample sources while preserving slot FX and per-pattern state.
  Slice 6 extends the binding transaction to whole rack-container replacement
  from the Sounds and Presets browsers, including rack-slot/effect identities
  and values;
  sampler-to-rack Sound conversion uses the same transaction and restores the
  original flat sampler graph on undo. Custom-instrument-to-rack conversion
  remains a barrier until the graph has the corresponding rack-to-custom inverse.
- Slice 6A covers track audio-effect insertion, deletion, source replacement,
  and reordering. History retains exact compiled source text and asset roots,
  restores scalar/tensor/IR values across every Track Pattern, rebinds process,
  macro, and neural targets, preserves logical effect-instance identity, and
  rejects stale async compile targets.
- Slice 6B covers MIDI-FX insertion, deletion, and reordering with explicit
  stable-instance ordering, per-Track-Pattern defaults and p-locks, process and
  macro target remapping, and scheduler republishing.
- Slice 6C covers bus-effect insertion, deletion, replacement, reordering, and
  coalesced value/p-lock edits. Replay resolves stable bus/effect identities,
  restores the edited scene's values and gate-linked snapshot, retained source
  metadata, sidechain selection, IR data, and bus macro mappings.
- Slice 6D covers rack-slot effect insertion, deletion, replacement, and
  reordering through the shared `FxChainHost`/`MonoPair` path. Per-pattern
  values and rack macro tables are restored transactionally, including mappings
  dropped by deletion.
- Project schema version 4 serializes authoritative stable instance records for
  track audio effects, MIDI effects, bus effects, rack slots, and rack-slot
  effects. Dense pattern slots remain value snapshots and are rebound to those
  records during load. Top-level legacy name vectors are migration projections;
  rack source names remain in the shared Sound/Rack preset payload, while
  project loading treats the instance records as authoritative.
- Recorded takes remain an explicit history barrier until Slice 7F; arming and
  other performance-only controls remain outside authoring history.

Related: `docs/racks-spec.md` (Amendment A: `FxChainHost`, rack-slot chains,
rack macro banks), `docs/MACRO_MAPPING_SPEC.md`, `docs/instrument-swap-spec.md`.
See "Cross-Feature Alignment" below for the required ordering against the
in-flight racks and macros work.

This document defines the complete architecture and staged delivery plan for
sequencer undo/redo. Each slice is intended to be independently shippable and
complete for its declared command set. During the staged rollout, unsupported
authoring mutations form an explicit history barrier; undo must never silently
cross an edit that it cannot restore.

## Goal

Provide deterministic, lossless, session-local undo and redo for sequencer
authoring operations, including:

- Step creation, deletion, movement, rotation, and paste.
- Step parameters, chords, and every p-lock domain.
- Track, mixer, instrument, effect, rack, and global parameters.
- Instrument preset loading and instrument source replacement.
- Audio-effect and MIDI-effect chain insertion, deletion, replacement, and
  movement.
- Later structural operations such as tracks, racks, buses, scenes, processes,
  macros, modulation routes, and neural graph edits.

The motivating hard case is replacing a track's instrument and then restoring
the previous instrument with all of its defaults, scalar p-locks, tensor
p-locks, key locks, preset state, process bindings, macro mappings, and neural
overrides intact across every Track Pattern.

## Definition of "Proper"

An operation is undoable only if the following round-trip law holds:

```text
capture authoring state S0
apply edit       -> S1
undo             -> exactly S0
redo             -> exactly S1
```

"Exactly" means bit-for-bit equality for persisted numeric values and exact
equality for logical identities, ordering, mappings, and source definitions.
Runtime graph node ids, scheduler snapshot versions, transport epochs, active
voices, transient selection, hover state, and redraw counters are derived or
ephemeral and are excluded from authoring-state equality.

The system is not considered complete if it can restore the audible result but
silently loses data that is not currently visible in the UI.

## Non-Goals

- Persisting undo history in project files.
- Undoing playback position, currently sounding voices, hover/focus state, or
  ordinary scene launch actions.
- Reversing individual low-level audio graph queue commands.
- Treating filesystem edits to instrument/effect source files as sequencer
  history. The text editor retains its own buffer history.
- Collaborative or branching history. This is a single-writer, linear history.
- Replacing domain patches with full project save/load snapshots. Project
  reload is too broad, too slow, and does not retain every transient compiled
  resource required for exact redo.

## Current Architecture and Gaps

The repository already provides useful foundations:

- `tui/command.rs` defines `AppCommand` and a central `apply_command` entry
  point for many live pattern and parameter mutations.
- `sequencer/state.rs` defines stable `PatternId`, `TrackPatternData`,
  `PatternSnapshot`, `StepSnapshot`, and `EffectSlotSnapshot` capture/restore
  machinery.
- Instrument replacement already resets the instrument domain across all
  Track Patterns through `reset_instrument_slot_all_patterns`.
- Graph mutations already use capacity preflight in important paths and
  `GraphEditBatchGuard` for batched topology edits.

The following gaps must be addressed rather than worked around:

1. `AppCommand` is not the only mutation boundary. Chord editing, piano-roll
   editing, recording, browser preset application, track/scene operations,
   structural graph edits, and many Lisp host commands mutate state directly.
2. `AppCommand` mixes authoring edits, performance controls, and transport
   actions.
3. Some state helpers publish scheduler snapshots internally while
   `apply_command` publishes again. A transaction must own final publication
   exactly once.
4. `apply_command` does not return `Result`; several arms assume valid indices
   and can panic instead of rejecting an invalid edit before mutation.
5. Commands identify the current dense track index but do not retain the
   stable Track Pattern identity that was edited.
6. Runtime state and logical authoring state are intertwined. Undo must capture
   the latter and derive the former.
7. `StepSnapshot` is not yet a complete step memento. It does not currently
   carry MIDI-FX p-locks or tensor p-lock rows for all device domains.
8. Instrument and effect loading can be asynchronous. History cannot be
   recorded when loading is merely requested; it must be recorded only after
   successful application.
9. Instrument/effect project references are name-based. Exact redo cannot
   depend on the named file remaining unchanged on disk.

## Cross-Feature Alignment

Two feature tracks are landing alongside this system and share seams with it.
The following alignment rules are binding on both specs.

### Racks Amendment A (`docs/racks-spec.md`)

1. **`FxChainHost` (racks A4) precedes Slice 6.** Slice 6's
   `EffectChainPatch` machinery must be designed against the generalized
   `FxChainHost` seam, not against today's duplicated track/bus chain code.
   Chain add/move/delete/param-push exist once, parameterized by host; the
   undo restore path is one implementation over `FxChainHost`, and
   `EffectChainOwner` gains a `RackSlot` arm from day one (see Slice 6).
2. **Lease retirement and history retention share one mechanism.** Racks A3
   locks lease lifetime to the audio thread's applied-batch-serial watermark:
   a loaded-artifact lease is held until the batch that removed its node is
   confirmed applied. H13's history-retained compiled resources are the same
   lifetime problem — a reference-counted handle whose release is gated on
   the watermark. Do not build a second retirement scheme; the resource-handle
   layer introduced for Slice 5/6 wraps the same lease accounting.
3. **Rack macro banks are already implemented and rack-scoped (racks A11).**
   Definitions, mappings, values, and p-lock rows serialize inside
   `ProjectRackTrackPattern` and rack presets. Consequences for this spec:
   rack macro state is captured by rack-track snapshots (`RackTrackSnapshot`
   inside `InstrumentPatternState`, and Slice 7A rack-structure patches) —
   it is never part of the project-global macro history in Slice 7E. Rack
   macro identity is stable by construction (`:macro_1`–`:macro_8` per rack),
   so no new id scheme is needed. A11's transactional mapping repair on
   slot/effect deletion must run inside the same edit transaction so the
   before-state captures mappings prior to repair.
4. **Rack macro p-lock rows are a live lock domain.** They serialize today,
   so `StepCellSnapshot` (Slice 1) must include them from the start, and the
   deferred `(track, slot, fx_slot)` slot-FX p-lock keys (racks A9, a stated
   priority follow-up) must be admissible in the same snapshot shape without
   another Slice 1 revision.
5. **Track → rack conversion (racks A7) is a Slice 5-family operation.** A7
   states "Undo restores the flat track"; that operation must ship with (or
   behind) the Slice 5 instrument-binding machinery — the flat track's
   instrument + FX state is exactly a `TrackInstrumentState` before-snapshot
   plus an `EffectChainState` before-snapshot. Until then it is a barrier.

### Macro mapping (`docs/MACRO_MAPPING_SPEC.md`)

1. **Project macro mappings currently store a dense `track: usize`.** History
   must not; `MacroMappingSnapshot` records `TrackId`. Either the mapping
   store migrates to `TrackId` with the Slice 0 track-identity work, or the
   snapshot layer translates at capture/restore — migrating the store is
   preferred since the macro spec already re-resolves `ParamTarget` against
   live descriptors on load.
2. **P-locked macro values are declared intent (macro spec §9.2).** When
   project- or rack-macro values become p-lock targets, they become a step
   lock domain; the Slice 1 snapshot shape and `without_audio_plocks`
   enumeration must be extended then, not silently miss it. Rack macro p-lock
   rows already exist (point 4 above).
3. **Macro live values remain performance state** for both scopes: project
   `MacroSetValue`/`MacroRelease` and rack macro live pushes are `Ignore`.
   Configuration (create/delete/rename/map/unmap/range/curve) is authoring:
   project-global in Slice 7E, rack-scoped via rack snapshots (point 3 above).

## Core Invariants

The implementation must preserve all of these invariants.

### H1. History records successful authoring transitions

A history entry is committed only after an edit succeeds and a non-empty
before/after patch has been produced. Failed and no-op edits do not clear redo
and do not add history.

### H2. Incoming commands do not carry caller-captured old values

UI and host callers submit intent. The central edit executor resolves targets,
captures the before-state, performs the edit, captures the after-state, and
constructs the history patch. Callers generally cannot know all derived changes
caused by a setter.

### H3. Undo and redo apply exact mementos

Undo applies the stored before-state. Redo applies the stored after-state.
Redo does not rerun a filesystem lookup, re-evaluate an old relative delta, or
repeat a toggle against whatever state happens to exist.

### H4. Authoring state is distinct from runtime state

History may store instrument/effect definitions, parameter data, pattern
lanes, mappings, and stable logical ids. It must not store audio node ids as
restorable identity. Graph nodes, scheduler snapshots, and runtime descriptor
tables are rebuilt or reconciled from authoring state.

### H5. History targets stable identities

Pattern edits target `(TrackId, PatternId)`, not "the selected track in the
current scene." Structural device edits target stable logical device ids, not
slot indices that can shift after deletion or movement.

### H6. Unsupported authoring mutations are barriers

During staged rollout, any authoring mutation not represented by a reversible
patch must invalidate both history stacks as part of its successful commit and
before control returns to the user. A rejected/failed mutation leaves history
unchanged. This prevents an older entry from being undone through an untracked
state transition without sacrificing history for an operation that never
happened.

### H7. Ephemeral actions do not affect history

Playback, cursor movement, selection, hover, panel state, audition notes, and
ordinary scene launch actions neither create entries nor clear history.

### H8. New edits clear redo only after success

If the user undoes and then performs a successful new authoring edit, redo is
cleared. If the new edit fails or is a no-op, redo remains available.

### H9. Undo/redo failure is non-destructive

An entry moves between stacks only after its patch is successfully applied and
runtime reconciliation finishes. On failure, the entry remains on its source
stack and the current authoring state remains valid.

### H10. One user gesture is one history entry

A pointer drag, knob gesture, selected-step multi-edit, preset load, instrument
swap, effect insertion, or recorded take is one transaction regardless of the
number of internal setter calls it generates.

### H11. Publication is centralized

An edit returns a positive description of required side effects. The outer
transaction performs scheduler publication, live parameter pushes, graph
reconciliation, epoch changes, mod resync, accumulator reset, and UI
invalidation exactly once in a defined order.

### H12. Live and stored Track Pattern state remain coherent

If a target `(TrackId, PatternId)` is currently effective, patch application
updates its live mirror and its pool entry. If it is inactive, patch
application updates only the pool entry. Switching scenes before undo must not
redirect the undo to another pattern.

### H13. Resource lifetimes are explicit

History entries may retain reference-counted compiled instrument/effect
resources. Evicting an entry releases its history reference; resources still
referenced by the live project, another entry, or the engine cache remain
alive. Final release of a resource whose node was removed from the live graph
is additionally gated on the audio thread's applied-batch-serial watermark —
the same acknowledged-retirement mechanism racks Amendment A locks for effect
leases (see Cross-Feature Alignment).

### H14. History is bounded

History is limited by both entry count and retained payload size. The initial
defaults are 256 entries and 64 MiB of snapshot payload, configurable in one
place. An entry's `retained_bytes` counts only snapshot allocations the entry
exclusively owns. Compiled resources kept alive solely by history are charged
at budget-evaluation time from the resource cache's history-only reference
counts, so a resource shared by several entries is charged once, not once per
entry. Oldest entries are
evicted until both limits are satisfied. The byte limit is soft for the newest
entry: a single oversized successful edit evicts all older entries but remains
undoable, with a diagnostic recording that the budget was exceeded.

### H15. No authoring mutation bypasses policy classification

Every UI, TUI, Lisp host, agent, recording, browser, and project authoring path
must eventually enter the edit boundary as undoable or explicitly invoke a
barrier. A debug-only mutation audit should make missing classification visible
during development.

## User-Facing Behavior

### Shortcuts and focus

- macOS: `Cmd-Z` undo, `Cmd-Shift-Z` redo.
- Other platforms: `Ctrl-Z` undo, `Ctrl-Shift-Z` redo.
- When a text editor buffer owns keyboard focus, its existing text undo/redo
  wins.
- Modal text fields keep their local editing behavior.
- Otherwise the shortcuts target sequencer history.

The plain `z` key is already consumed by the recording keyboard as an octave
shift. Modifier-qualified undo/redo must be matched before plain-character
handlers such as the recording-key path so `Cmd-Z` is never swallowed as an
octave change.

### Feedback

Successful actions show concise labels:

```text
Undo: Delete 8 steps
Redo: Replace instrument with Drift
```

When unavailable:

```text
Nothing to undo
Nothing to redo
```

When restoration fails, show the error without moving the history cursor:

```text
Could not undo "Replace instrument": <reason>
```

Selection and cursor position are normally not restored. After a structural
restore, invalid selections are sanitized and focus remains in the closest
surviving logical panel.

### History lifetime

- Opening or creating a different project clears both stacks.
- Saving does not clear history.
- History is not serialized.
- A saved-revision marker records the history revision at successful save so a
  future project-dirty indicator can become clean again when undo returns to
  that revision.

## Target Architecture

### Command flow

```text
UI / keyboard / Lisp host / agent
                |
                v
           EditCommand intent
                |
                v
 resolve stable ids + validate + prepare resources
                |
                v
 capture before -> mutate logical state -> capture after
                |
                v
       HistoryEntry + MutationEffects
                |
                v
 reconcile live graph/scheduler/runtime and commit history
```

Playback and pure UI actions use a separate non-history command path.

### Module layout

Start with:

```text
crates/sequencer/src/tui/
  command.rs       # incoming commands and exhaustive history policy
  edit.rs          # resolve/execute/capture transaction boundary
  history.rs       # manager, entries, patches, grouping, budgets
```

Split domain patches into `history/step.rs`, `history/device.rs`,
`history/instrument.rs`, and `history/effect.rs` only when their implementations
become large enough to justify separate files.

State-layer capture/restore primitives that are valid outside the UI remain in
`sequencer/state.rs`. They must not depend on `App` or history stacks.

### Command classification

Every command has an exhaustive policy:

```rust
pub enum HistoryPolicy {
    Record,
    Coalesce(MergeKey),
    Ignore,
    Barrier,
    Reset,
}
```

- `Record`: one successful command becomes one entry.
- `Coalesce`: successful commands join the active gesture/group.
- `Ignore`: performance or UI state; history is unchanged.
- `Barrier`: unsupported authoring mutation; clear both stacks atomically after
  successful apply and before returning control.
- `Reset`: project replacement/new project; clear both stacks after successful
  replacement.

The policy match must enumerate every command variant. It must not use a
negative blacklist in which new variants silently inherit a behavior.

### Stable identities

Add a persisted `TrackId(u64)` and a dense runtime `TrackRegistry`:

```rust
pub struct TrackRegistry {
    order: Vec<TrackId>,
    index_by_id: HashMap<TrackId, usize>,
    next_id: u64,
}
```

Dense indices remain valid for audio/runtime arrays, but history entries store
`TrackId` and resolve the current dense index at application time. Track
addition/deletion/reordering must update the registry atomically with the
existing aligned track collections.

Do not add `id` independently to every variant of the current `ProjectTrack`
enum. Refactor project tracks into a record with shared identity/metadata and a
kind enum:

```rust
pub struct ProjectTrack {
    pub id: TrackId,
    pub color: Option<TrackColor>,
    pub collapsed: bool,
    pub kind: ProjectTrackKind,
}

pub enum ProjectTrackKind {
    Sampler { sample_path: String },
    Custom { instrument_name: String },
    Modulator,
    Rack { routing: ProjectRackRouting, slots: Vec<ProjectRackTrackSlot> },
}
```

Increment the project format version for this model. A legacy loader may assign
ids deterministically by old track order, but preserving the old Rust API or
parallel duplicated metadata is not a requirement.

Pattern targets use:

```rust
pub struct TrackPatternId {
    pub track: TrackId,
    pub pattern: PatternId,
}
```

Before structural effect support, effect-slot indices are safe only while
effect-chain mutations remain barriers. The `EffectInstanceId`,
`MidiFxInstanceId`, and `RackSlotId` newtypes are defined in Slice 0 (they
appear in `MutationEffects` and `DeviceId`), assigned session-stable values
when Slice 4 begins storing device parameter history, and persisted and made
the identity for structural chain references in Slice 6.

Existing stable ids such as `PatternId`, `MacroId`, and `BusId` should be
reused, with two caveats:

- `PatternId` is allocated by a per-track `TrackPatternPool` counter and is
  unique only within its owning track. It must never appear in history without
  its `TrackId` — hence `TrackPatternId` below.
- `TrackPatternPool::insert` only allocates fresh ids. Restoring a deleted
  Track Pattern (Slice 7B, and any patch that recreates a pattern) requires a
  new `insert_with_id` primitive that reinstates the original `PatternId` and
  advances `next_id` past it, so redo never reallocates a different id.

### History entry and patches

```rust
pub struct HistoryEntry {
    pub revision_before: u64,
    pub revision_after: u64,
    pub label: String,
    pub merge_key: Option<MergeKey>,
    pub patch: EditPatch,
    pub retained_bytes: usize,
}

pub enum EditPatch {
    StepCells(StepCellsPatch),
    TrackPattern(TrackPatternPatch),
    TrackParams(TrackParamsPatch),
    TransportParams(TransportParamsPatch),
    DeviceValues(DeviceValuesPatch),
    InstrumentBinding(InstrumentBindingPatch),
    EffectChain(EffectChainPatch),
    ProjectStructure(ProjectStructurePatch),
    Composite(Vec<EditPatch>),
}
```

`Composite` is for a single user gesture spanning domains. Patch application
order is forward for redo and reverse for undo when dependencies require it.
Do not store arbitrary closures; typed patches are inspectable, testable,
size-accountable, and resource-aware.

### Mutation effects

Replace `command_mutates_sequencer_state` with positive effects returned by
execution or patch replay:

```rust
#[derive(Default)]
pub struct MutationEffects {
    pub publish_scheduler: bool,
    pub publish_macro_overrides: bool,
    pub push_track_mixer: BTreeSet<TrackId>,
    pub push_instrument_defaults: BTreeSet<TrackId>,
    pub push_effect_defaults: BTreeSet<EffectInstanceId>,
    pub apply_track_routing: BTreeSet<TrackId>,
    pub rebuild_graph: GraphReconcilePlan,
    pub sync_scratch_descriptors: bool,
    pub schedule_mod_resync: bool,
    pub reset_accumulators: bool,
    pub bump_pattern_epoch: bool,
    pub bump_topology_epoch: bool,
    pub invalidate_ui: UiInvalidation,
}
```

The exact shape may be refined, but it must remain positive and typed. The
outer transaction normalizes duplicates and applies effects in a documented
order.

### Transaction modes

```rust
pub enum ApplyMode {
    UserEdit,
    Undo,
    Redo,
    ProjectLoad,
}
```

Undo and redo replay patches without recording new entries. Nested domain
operations contribute to the current transaction rather than opening their own
history entry.

### Gesture grouping and coalescing

Use explicit gesture ids where an input source has begin/update/end events:

```rust
pub struct GestureId(u64);

pub enum GesturePhase {
    Begin(GestureId),
    Update(GestureId),
    End(GestureId),
    Cancel(GestureId),
}
```

- Pointer down captures the initial state.
- Updates mutate live state without adding entries.
- Pointer up captures final state and commits one entry.
- Cancel restores the initial state and commits nothing.
- If a widget disappears mid-gesture, cancellation is mandatory.

Lisp/custom widgets must propagate gesture identity through host commands.
Keyboard auto-repeat may coalesce by `(target, parameter)` while the key remains
held. A short timeout is only a fallback for sources that cannot expose a
reliable end event.

### Bit-exact equality and no-op detection

Use `f32::to_bits` for snapshot equality and diff construction. Normal
`PartialEq` is insufficient for exact NaN and signed-zero behavior. Values
should already be validated against non-finite input at the mutation boundary,
but history equality must still be deterministic.

### Async and structural transactions

Structural operations use three phases:

1. **Prepare**: resolve stable targets, load source/assets, compile or acquire a
   cached resource, validate descriptors, calculate graph capacity, and capture
   before-state. No live authoring state changes.
2. **Commit**: enqueue a graph batch for which no fallible work remains, then
   apply the corresponding logical state transition.
3. **Finalize**: reconcile runtime mirrors, capture after-state, publish once,
   and commit history.

If the current graph API can fail after its first irreversible command, that
API must be fixed before structural undo is implemented. Undo cannot be built
on a partially transactional topology mutation.

Async completion records the edit at completion time. A pending operation
retains its stable target id and verifies that the target still exists and
still has the expected source binding before commit. It captures before-state
immediately before the successful commit, after resource preparation, so edits
made while compilation was pending are not lost. A user undo performed while
compilation is pending does not cancel or implicitly absorb that future edit;
the UI must either support explicit cancellation or let successful completion
become the newest history entry.

### Compiled resource ownership

Introduce reference-counted logical resource handles rather than retaining raw
library indices in history. `ResourceKey`, `AssetOrigin`, and
`LoadedInstrumentResource` are new types for this system: today a track's
custom instrument binding is a raw `engine_id: usize` index plus a saved name,
so this slice must add the handle layer around the existing engine
registration path rather than assume it exists:

```rust
pub struct InstrumentResource {
    pub key: ResourceKey,
    pub name: String,
    pub source: Arc<str>,
    pub asset_origin: AssetOrigin,
    pub run_mode: CustomInstrumentRunMode,
    pub manifest: Arc<DGenManifest>,
    pub compiled: Arc<LoadedInstrumentResource>,
}
```

Effects use the analogous type. The engine registry, live graph bindings, and
history entries share these handles. This guarantees exact redo even if the
saved source changes on disk after the original operation.

## Initial Policy Matrix

| Operation family | Initial policy | Undo slice |
|---|---|---|
| Playback, cursor, selection, audition, panel state | Ignore | Never/document-independent |
| Project open/new | Reset | N/A |
| Unsupported authoring mutation during rollout | Barrier | Until its slice lands |
| Step toggle/set/clear/move/rotate/paste/chord edit | Record | 1 |
| Pattern length, duplicate/halve, track timebase/swing p-locks | Record | 2 |
| Track/mixer/global parameter edits | Coalesce or Record | 3 |
| Instrument/effect/rack values, all device p-locks, presets | Coalesce or Record | 4 |
| Track instrument replacement and type conversion | Record | 5 |
| Audio/MIDI/bus effect structure | Record | 6 |
| Tracks, racks, scenes, buses, groups, processes, macros, routes | Record | 7 |
| Macro live value/release gestures (project and rack scope) | Ignore by default | Performance state |
| Macro configuration (project-global) | Barrier, then Record | 7 |
| Rack macro bank config (rename/map/unmap/range) | Barrier, then Record | 7A (rack snapshots) |
| Rack macro p-lock set/clear | Barrier, then Record | 1 (step cells) + 4 |
| Recording into steps | Barrier, then one entry per take | 7 |
| Instrument/effect source editor text | Existing editor history | Separate subsystem |

Note the practical consequence during early slices: while most authoring
families are still barriers, common actions such as a mixer tweak wipe the
step-edit history that Slice 1 just made undoable. This is the intended
correctness trade-off, not a bug; each shipped slice widens the set of edits
that survive. The UI should surface a barrier wipe (for example, a status
message naming the operation that cleared history) so the behavior reads as
deliberate.

## Slice 0: History Foundation and Mutation Boundary

### Objective

Establish a trustworthy transaction and policy layer before exposing sequencer
undo shortcuts.

### Required changes

1. Add `TrackId` and `TrackRegistry`; persist ids with project tracks and
   generate ids for older/missing input during project migration.
2. Add `UndoManager` to `App` with undo/redo stacks, revisions, saved revision,
   entry/byte budgets, and an optional active gesture.
3. Split authoring edit intent from performance/UI commands, or otherwise make
   `HistoryPolicy` exhaustive for every existing command.
4. Change the authoring apply API to return `Result<EditOutcome, EditError>`.
5. Add `ApplyMode` and ensure patch replay cannot recursively record history.
6. Add canonical helpers to resolve `(TrackId, PatternId)`, capture active or
   inactive Track Pattern data, and restore it without targeting the wrong
   scene after a switch.
7. Refactor state primitives used by commands so mutation and publication are
   separable. Public convenience wrappers may still mutate-and-publish for
   non-App tests, but edit execution must use no-publish primitives.
8. Add `MutationEffects` and one ordered finalization path.
9. Mark every direct authoring path outside `AppCommand` as a barrier until it
   migrates. This includes TUI input, piano roll, recording, browser preset
   load, graph controllers, project/scene mutation, Lisp host commands, and
   agent actions.
10. Route platform undo/redo shortcuts according to focus, but keep them
    disabled or reporting empty history until Slice 1 is complete.

### Finalization order

Unless a domain requires a stricter order, use:

1. Apply or confirm graph topology batch.
2. Update logical/runtime binding tables.
3. Push restored default/live parameter values.
4. Reapply mixer and routing state.
5. Sync scratch descriptors and macro overrides.
6. Schedule modulation and accumulator resync.
7. Bump topology epoch, then pattern epoch where applicable.
8. Publish one scheduler snapshot.
9. Invalidate and refresh UI state.
10. Commit/move the history entry.

### Acceptance criteria

- Every existing authoring path is classified as `Record`, `Barrier`, or
  `Reset`; no path silently inherits `Ignore`.
- Successful unsupported authoring edits clear history atomically with commit;
  failed unsupported edits leave history intact.
- Failed/no-op commands leave both stacks unchanged.
- Project replacement clears both stacks only after successful replacement.
- Scheduler publication counts prove one publication per migrated edit.
- No history lock or allocation occurs on the audio thread.
- `cargo check -p sequencer --bin metal_seq` and existing sequencer tests pass.

### Required tests

- Stack transitions for edit, undo, redo, new edit after undo, no-op, failure,
  barrier, and project reset.
- Entry-count and byte-budget eviction.
- Bit-exact no-op detection.
- Pattern target remains correct across scene launch.
- Text-editor focus retains text undo while app focus uses sequencer undo.
- A deliberately unclassified test command fails compilation through an
  exhaustive match rather than receiving a default policy.

## Slice 1: Lossless Step Editing

### Objective

Support the first complete musical-edit slice: step toggles, creation,
deletion, chord edits, movement, rotation, and paste.

### Snapshot prerequisite

Extend or replace `StepSnapshot` with a lossless `StepCellSnapshot` containing:

- Active and neural-reset bits.
- Every `StepParam` value.
- Chord notes, durations, and delays.
- Timebase, swing, and swing-resolution p-locks.
- MIDI-FX scalar p-lock rows.
- MIDI-FX tensor p-lock rows.
- Audio-effect scalar p-lock rows.
- Audio-effect tensor p-lock rows.
- Instrument scalar p-lock row.
- Instrument tensor p-lock rows.
- Rack strip p-locks.
- Rack-instrument scalar and tensor p-lock rows.
- Rack macro p-lock rows (racks-spec A11 — these serialize today).

The snapshot shape must also admit the planned `(track, slot, fx_slot)`
slot-FX p-lock keys (racks-spec A9) without a structural revision — model the
lock domains as an enumerable set rather than a fixed field list where that
is cheap to do.

`without_audio_plocks` must clear every MIDI-FX, audio-effect, instrument,
rack device, and rack macro lock, including tensor rows. Clipboard cross-track
sanitation must not accidentally retain a newly added lock domain.

The underlying step clear/copy/move primitives must be corrected to cover the
same complete set of domains before this slice ships. History must not preserve
an existing bug in which an operation moves or clears only scalar/audio-effect
locks while leaving MIDI-FX or tensor locks behind.

The step patch also captures the track's before/after p-lock variant registry
when the edit can change variant membership.

### Commands covered

- `ToggleStep`
- `SetStepActive`
- `SetStepParam`
- `AdjustStepParam`
- `ClearStepPayload`
- `ClearSteps`
- `RotateSteps`
- `PasteSteps`
- `ShiftStepRange`
- Direct chord add/toggle/remove/clear mutations in TUI input
- Piano-roll note create/delete/move/resize operations that change one or more
  step cells

`DuplicateTrackPattern`, `HalveTrackPattern`, and explicit track/p-lock
commands remain barriers until Slice 2.

### Patch shape

```rust
pub struct StepCellsPatch {
    pub target: TrackPatternId,
    pub cells: Vec<StepCellDelta>,
    pub variant_registry_before: Option<PlockVariantRegistry>,
    pub variant_registry_after: Option<PlockVariantRegistry>,
}

pub struct StepCellDelta {
    pub step: usize,
    pub before: StepCellSnapshot,
    pub after: StepCellSnapshot,
}
```

Steps are sorted and deduplicated. Unchanged cells are omitted.

### Affected-set rules

- Single-step edit: that step.
- Multi-clear/rotate: sanitized unique requested steps.
- Paste: all in-range candidate destinations, followed by after-diff removal of
  skipped/no-op destinations.
- Shift: union of source range and destination range, including vacated cells.
- Piano-roll multi-move: union of every old and new cell touched.
- Transpose edit: capture the whole cell because chord notes may move as a
  derived effect.

### Application behavior

- If the target pattern is active, restore cells through one no-publish batch
  and synchronize its pool entry.
- If inactive, edit the pool entry directly.
- Reconcile the variant registry once after the full cell batch.
- Publish only when the target is currently effective.
- Preserve current selection unless it references a deleted piano-roll item;
  then sanitize it.

### Gesture semantics

- One click/toggle is one entry.
- One drag/multi-note move is one entry.
- Key auto-repeat for step parameter adjustment coalesces until key-up.
- Copy does not create history; paste does.

### Acceptance criteria

- Undo of an active-step toggle restores every lock that toggle cleared.
- Delete/move/rotate/paste round-trip all scalar and tensor lock domains.
- Undo after switching to another scene modifies the original `PatternId`, not
  the currently visible pattern.
- Shared Track Patterns reflect undo in every scene referencing the same id.
- Cross-track paste still removes incompatible audio/device locks.

### Required tests

- A maximally populated step containing chords and every p-lock domain.
- Toggle active off, undo, and redo.
- Overlapping shift ranges in both directions.
- Non-contiguous rotate.
- Paste with inactive-source skip behavior.
- Piano-roll multi-note move across multiple cells.
- Tensor p-lock deletion and restore.
- MIDI-FX p-lock deletion and restore.
- Variant registry equality after round trip.
- Generic `S0 -> S1 -> S0 -> S1` law tests for every covered command.

## Slice 2: Pattern Geometry and Track-Level P-Locks

### Objective

Cover changes that affect Track Pattern geometry or track-level step locks.

### Commands covered

- `DuplicateTrackPattern`
- `HalveTrackPattern`
- `SetTrackNumSteps`
- `AdjustTrackNumSteps`
- `SetTimebasePlock`
- `SetTimebasePlockMulti`
- `ClearTimebasePlockMulti`
- `SetTrackSwingPlock`
- `SetTrackSwingPlockMulti`
- `ClearTrackSwingPlockMulti`
- `SetTrackSwingResolutionPlock`
- `SetTrackSwingResolutionPlockMulti`
- `ClearTrackSwingResolutionPlockMulti`
- Variant stamp/copy/clear operations across selected steps (currently direct
  calls to state helpers such as `stamp_variant_key_to_steps` and
  `clear_variant_locks_for_steps`, not `AppCommand` variants; they must enter
  the edit boundary as part of this slice)

### Patch strategy

Use two patch forms:

- Targeted step-cell patches for p-lock-only changes.
- `TrackPatternPatch` for duplicate/halve and any future operation that changes
  pattern length plus a broad range of cells.

```rust
pub struct TrackPatternPatch {
    pub target: TrackPatternId,
    pub before: TrackPatternData,
    pub after: TrackPatternData,
}
```

Runtime descriptor node ids embedded in slot snapshots must be normalized out
of equality and rebound to the current live descriptor when restoring an
active pattern. Logical parameter ids, values, and lock assignments remain
exact.

### Acceptance criteria

- Duplicate and halve restore both `num_steps` and every affected cell.
- P-lock multi-edits are one entry.
- Undo works when the target pattern is inactive.
- Pattern geometry changes do not alter effect/instrument source topology.
- Variant assignments and tensor rows remain coherent.

### Required tests

- Duplicate and halve patterns with non-default data above and below the new
  boundary.
- Undo after scene switch.
- Multi-step timebase/swing/resolution set and clear.
- Variant stamp/clear round trips across every supported device domain.
- Scheduler snapshot observes the restored geometry after one publication.

## Slice 3: Track, Mixer, and Global Parameters

### Objective

Make ordinary track configuration and continuous mixer controls undoable with
correct gesture grouping and live audio synchronization.

### Commands covered

- Gate, polyphony, max polyphony, attack, and release.
- Swing and swing resolution defaults.
- Volume, pan, sends, output routing, and send lists.
- Track timebase.
- FTS scale.
- Accumulator selection, limit, and mode.
- Instrument base-note offset where it is track-scoped.
- Master volume.
- Project BPM.

Record-quantize threshold remains `Ignore` unless it becomes persisted project
authoring state.

### Patch shape

```rust
pub struct TrackParamsPatch {
    pub target: TrackPatternId,
    pub before: TrackParamsSnapshot,
    pub after: TrackParamsSnapshot,
}

pub struct TransportParamsPatch {
    pub before: TransportAuthoringSnapshot,
    pub after: TransportAuthoringSnapshot,
}
```

Using a complete `TrackParamsSnapshot` is intentional: operations such as
accumulator selection may update more than one field. The diff determines
precise `MutationEffects`, so restoring volume does not trigger an unrelated
scheduler publish and restoring output does reapply graph routing.

### Gesture grouping

- Slider/knob drag: one entry from pointer-down to pointer-up.
- Mouse wheel or keyboard repeat: coalesce while the gesture/key is active.
- Discrete enum/toggle: one entry per committed choice.
- Multi-track mixer adjustment, when introduced, uses one composite entry.

### Live effects

Patch replay derives required pushes from changed fields:

- Volume/pan/send/master: push live graph values, normally no scheduler
  snapshot.
- Output/send destinations: reapply graph routing and bus sends.
- Timebase/swing/step count/polyphony and scheduler-observed fields: publish.
- BPM: push all tempo-dependent delays and publish transport state.

### Acceptance criteria

- A 200-update slider drag creates one entry.
- Undo while playing updates the audible control and relevant scheduler state.
- Undoing track output restores both logical output and graph routing.
- Coupled accumulator fields round-trip together.
- No-op clamped edits do not create history.

### Required tests

- One law test per field family.
- Gesture begin/update/end and cancel.
- Clamp-to-same-value no-op.
- Routing and send graph spy assertions.
- Publication-count assertions for mixer-only versus scheduler-observed fields.
- BPM restoration pushes all registered tempo-dependent devices once.

## Slice 4: Device Values, P-Locks, Key Locks, Tensors, and Presets

### Objective

Cover instrument, audio-effect, MIDI-effect, and rack device values without yet
changing device-chain topology.

### Stable device addressing

Assign the stable device instance ids (defined in Slice 0) before storing
device parameter history. Session-stable assignment at project load and
instance creation is sufficient here; ids become persisted with the Slice 6
serialization refactor. Because effect-chain structure remains a barrier until
Slice 6, id-to-slot resolution in this slice is a fixed bijection:

```rust
pub enum DeviceId {
    TrackInstrument(TrackId),
    AudioEffect(EffectInstanceId),
    MidiEffect(MidiFxInstanceId),
    RackSlot(RackSlotId),
    RackInstrument(RackSlotId),
    RackMacro(TrackId, u8), // :macro_1..:macro_8 — stable by construction
}
```

Dense slot/parameter indices are resolved from ids and current descriptors.
Parameter patches store both logical parameter identity and the resolved index
used at capture. Descriptor mismatch is an error unless a structural patch is
simultaneously restoring the matching descriptor.

### Commands covered

- Instrument and effect default scalar parameters.
- Instrument/effect/MIDI-FX scalar p-lock set/clear, including selected-step
  multi-edits.
- Instrument key-lock set, multi-set, clear, stamp, and variant clear.
- Instrument/effect/MIDI-FX tensor default cells and tensor p-lock cells.
- Rack strip gain, pan, mute, solo, max polyphony, choke group, base note, and
  strip p-locks.
- Rack-slot instrument defaults, scalar/tensor p-locks, and key locks where
  supported.
- Rack macro p-lock set/clear (rack macros are stable p-lock targets per
  racks-spec A11; bank configuration itself is Slice 7A rack-snapshot
  territory, not this slice).
- Instrument preset load/save-to-live operations that change authoring values.
  Loading one preset is one history entry; saving a preset file is a filesystem
  action and is not sequencer history.
- Effect instance metadata changes such as convolution IR selection.

### Patch shape

```rust
pub struct DeviceValuesPatch {
    pub target: DeviceId,
    pub pattern: Option<PatternId>,
    pub before: DeviceValueSnapshot,
    pub after: DeviceValueSnapshot,
}
```

`DeviceValueSnapshot` is domain-specific and contains all coupled values
changed by the operation. For a simple scalar setter this is usually a small
list of parameter-address/value pairs. Preset load uses a full device-value
snapshot.

Derived modulation-active parameters must be included in before/after diffs.
Do not assume the requested parameter is the only changed value.

P-lock values use `Option<f32>` or `Option<Vec<f32>>` so clearing a lock is a
first-class reversible value rather than a magic sentinel.

### Preset behavior

- Capture the device value state before applying a preset.
- Resolve and validate the preset against the current descriptor.
- Apply defaults in one no-publish batch.
- Push all changed effective values once.
- Capture after-state and commit one entry labeled with the preset name.
- Undo restores prior dirty/loaded-preset state as well as numeric values.

### Acceptance criteria

- Scalar, tensor, key-lock, and every p-lock domain round-trip exactly.
- Derived active flags restore exactly.
- Multi-selected p-lock edits create one entry.
- Preset load restores previous loaded-preset and dirty markers on undo.
- Structural device changes remain barriers until Slices 5 and 6.

### Required tests

- Instrument/effect modulation depth that changes a shared active flag.
- Default scalar and tensor edits.
- Scalar and tensor p-lock set, overwrite, and clear.
- Key-lock set/clear/stamp across multiple notes.
- Rack solo behavior, including the derived mute refresh for other slots.
- Preset load with partial schema and validation failure.
- Convolution IR reference restore.
- Gesture grouping for custom Lisp instrument/effect controls.

## Slice 5: Instrument Replacement and Track-Type Conversion

### Objective

Make the motivating destructive operation fully reversible across logical
state, stored patterns, compiled resources, and live graph topology.

### Operations covered

- Custom instrument to custom instrument replacement.
- Sampler to custom instrument conversion.
- Custom instrument to sampler conversion.
- Repeated A/B instrument audition through undo/redo.
- Instrument run-mode change when it is part of source replacement.
- Append an instrument or sampler to a broadcast/layer rack.
- Replace a broadcast/layer rack slot's instrument or sampler source without
  replacing its FX chain.

Drum-rack pad topology remains in Slice 7A. Likewise "Group to Instrument
Rack" (racks-spec A7) and Sound-preset load (A8) are this slice's machinery —
the flat track's before-state is a `TrackInstrumentState` plus an
`EffectChainState` — but they additionally move the track FX chain, so they
remain barriers until both this slice and Slice 6A are complete.

### Snapshot shape

```rust
pub struct InstrumentBindingPatch {
    pub track: TrackId,
    pub before: TrackInstrumentState,
    pub after: TrackInstrumentState,
}

pub struct TrackInstrumentState {
    pub source: TrackInstrumentSource,
    pub display_name: String,
    pub pattern_states: Vec<(PatternId, InstrumentPatternState)>,
    pub live_pattern: Option<(PatternId, InstrumentPatternState)>,
    pub macro_mappings: Vec<MacroMappingSnapshot>,
    pub neural_overrides: Vec<SceneNeuralInstrumentOverrideSnapshot>,
}

pub struct InstrumentPatternState {
    pub instrument_slot: EffectSlotSnapshot,
    pub instrument_base_note_offset: f32,
    pub track_sound_state: TrackSoundState,
    pub sample_id: (i32, String, u32),
    pub instrument_type: InstrumentType,
    pub instrument_run_mode: CustomInstrumentRunMode,
    pub rack_track: Option<RackTrackSnapshot>,
    pub process_chain: TrackProcessChain,
    pub plock_variant_registry: PlockVariantRegistry,
    pub key_lock_variant_registry: PlockVariantRegistry,
}
```

`TrackInstrumentSource` contains the exact retained instrument or sample
resource, not merely its saved name. The live-pattern field is present only if
the live mirror contains edits not yet synchronized into its pool entry; the
transaction should normally synchronize first and avoid duplicate divergent
copies.

### Prepare phase

1. Acquire/compile the destination resource with exact source and asset origin.
2. Resolve `TrackId` immediately before commit and verify the current instrument
   binding still matches the operation's expected source.
3. Validate run mode, descriptor, engine capacity, graph queue capacity, and
   type-conversion prerequisites.
4. Capture the complete current before-state across every Track Pattern plus
   mappings and neural overrides. This capture occurs after async preparation
   so intervening parameter edits are included.
5. Build the destination logical state without mutating live state.

### Commit phase

1. Silence/retire active voices using the canonical track source-change path.
2. Ensure the destination engine or sampler voice pool is ready.
3. Commit the graph rebind/conversion as one prevalidated batch.
4. Update logical source binding and descriptor tables.
5. Apply the destination instrument-pattern state across every pattern.
6. Reconcile process bindings, macro mappings, neural overrides, and variant
   registries according to the destination state.
7. Push defaults, resync modulation/accumulators, advance topology then pattern
   epoch, publish once, capture after-state, and commit history.

### Undo/redo

Undo and redo invoke the same `restore_track_instrument_state` operation with
the stored before/after state. They do not call "load instrument by name" and
do not store/replay low-level graph deletion commands.

The restore operation preflights the retained resource and graph capacity
before changing live state. If it fails, history remains unchanged and the
currently bound instrument continues to function.

### Mapping behavior

The snapshot must restore exactly:

- Instrument defaults, p-locks, tensor p-locks, and key locks in every pattern.
- Loaded-preset and dirty state.
- Instrument-scoped p-lock/key-lock variant registry entries.
- Instrument process bindings that replacement dropped.
- Instrument macro mappings that replacement removed. (For rack tracks the
  rack macro bank travels inside `RackTrackSnapshot`; `macro_mappings` here
  covers only project-global mappings targeting this track.)
- Scene neural instrument overrides.
- Sampler id/path/rate state for type conversion.
- Track display name, instrument type, engine binding, and run mode.

Effect state, MIDI-FX state, ordinary step data, track parameters, group
membership, track color, and mod-in routes must remain untouched.

### Acceptance criteria

- The exact motivating workflow succeeds: customize instrument A across
  several patterns, replace it with B, undo to exact A, redo to exact B.
- Another track sharing A's engine remains unchanged throughout.
- Source files may change on disk after replacement without affecting undo or
  redo within retained history.
- Free-patch and ordinary instrument run modes restore correctly.
- Sampler/custom conversion restores the correct runtime voice implementation.
- Loading a Sound rack over a sampler or custom instrument restores the exact
  flat source on undo and the complete rack on redo.
- Dropping a saved instrument or sampler onto a rack replaces the container as
  one transaction; undo restores the rack and redo restores the flat source.
- No partially applied graph state is observable after a failed restore.

### Required tests

- Three-pattern swap with scalar, tensor, key, and variant data.
- Macro mapping, neural override, and process-binding restoration.
- Shared engine isolation.
- Free-patch dedicated resource handling.
- Sampler/custom round trip including sample metadata.
- Flat-instrument/rack round trips from both Sound loads and mixer drops.
- Async target deletion/reordering before compile completion.
- Graph-capacity failure before commit.
- Repeated undo/redo loop to detect leaked routes or duplicated nodes.
- Project save/load after undo and after redo.

Because this slice changes host-side instrument initialization and restoration,
run `instrument_probe` for both representative source types, for example:

```sh
cargo run --bin instrument_probe -- core/drift --frames 4096 --min-peak 0.01 --min-rms 0.001
```

## Slice 6: Effect-Chain Structural Undo

### Objective

Support reversible audio-effect, MIDI-effect, and bus-effect topology changes
without storing runtime node ids as history identity.

### Prerequisite: `FxChainHost`

The racks Amendment A Phase R1 refactor (`FxChainHost` + `ChainSuccessor`,
unified lease storage) must land before this slice. All chain patch
application — prepare, connect, delete, param push, lease accounting — is
written once over `FxChainHost` and works identically for track, bus, and
rack-slot chains. If Slice 6 arrives first for scheduling reasons, it must
still introduce the host seam rather than a third (or fourth) copy of the
chain machinery.

### Identity model

Promote the instance ids assigned in Slice 4 (`EffectInstanceId`,
`MidiFxInstanceId`, `RackSlotId`) from a stable addressing convenience to the
authoritative identity for chain structure. An audio/MIDI chain is an ordered list of logical instances. Slot indices are
derived views. Parameter targets, process bindings, macro mappings, sidechain
references where applicable, and UI selection should resolve through stable
ids.

Refactor project serialization from parallel name/slot vectors into instance
records containing stable id, source definition, and per-instance metadata.
Pattern snapshots may remain dense for scheduler efficiency, but chain restore
must map pattern slot payloads by instance id before producing the dense live
layout. Do not preserve the current index-only project API merely to avoid a
format version change.

### Operations covered

#### Slice 6A: Track audio effects

- Insert built-in or saved effect.
- Delete custom effect.
- Replace effect source.
- Move/reorder custom effects.
- Restore custom resource leases and instance metadata.

#### Slice 6B: MIDI effects

- Insert, delete, replace, and reorder MIDI-FX instances.
- Restore descriptor, defaults, p-locks, and chain position across patterns.

#### Slice 6C: Bus effects

- Insert, delete, replace, and reorder bus effects.
- Restore per-scene bus effect values, gate state references, sidechains, and
  custom resource metadata.

#### Slice 6D: Rack-slot effects

- Insert, delete, replace, and reorder effects in a rack slot's chain
  (racks-spec A5/A6).
- Same patch machinery via `EffectChainOwner::RackSlot`; the only structural
  difference is the `ChainSuccessor::MonoPair` wiring, which lives below the
  patch layer.
- Rack macro mappings that target slot-FX params are repaired or dropped
  transactionally on chain edits (racks-spec A11). The chain patch must
  capture the rack macro mapping table before repair so undo restores dropped
  mappings, not just the chain.

Each sub-slice remains a barrier until it is individually complete.

### Patch shape

```rust
pub struct EffectChainPatch {
    pub owner: EffectChainOwner,
    pub before: EffectChainState,
    pub after: EffectChainState,
}

pub enum EffectChainOwner {
    Track(TrackId),
    Bus(BusId),
    RackSlot(RackSlotId),
}

pub struct EffectChainState {
    pub instances: Vec<EffectInstanceState>,
    pub pattern_slots: Vec<(PatternId, Vec<EffectSlotSnapshot>)>,
    pub process_bindings: Vec<ProcessBindingSnapshot>,
    pub macro_mappings: Vec<MacroMappingSnapshot>,
    pub neural_overrides: Vec<SceneNeuralEffectOverrideSnapshot>,
}
```

MIDI chains use the analogous state type. An `EffectInstanceState` contains
stable id, exact built-in or retained compiled source definition, descriptor,
IR/instance metadata, and logical ordering. Runtime node bindings in captured
slot snapshots are normalized and rebound during restore.

### Graph restore strategy

Reconcile from the logical before/after chain:

1. Prepare every required effect resource and node construction plan.
2. Validate channel counts, sidechain inputs, modulator requirements, and graph
   queue capacity.
3. Build/connect replacement nodes before disconnecting old nodes where the
   graph permits it.
4. Commit one batch that leaves the chain connected at every observable batch
   boundary.
5. Publish the new logical ordering and rebind every pattern's slot data by
   stable instance id.
6. Refresh process/macro/neural parameter identities and push restored values.

Undo does not call the current delete method in reverse and does not infer an
old slot from neighboring indices.

### Cross-pattern behavior

Effect insertion/deletion/movement changes slot layout in every Track Pattern.
The patch must therefore capture every affected pattern lane. Values follow
the stable effect instance when it moves; they do not remain attached to the
old numeric slot.

### Acceptance criteria

- Delete restores exact source, ordering, defaults, scalar/tensor p-locks,
  sidechains, IR metadata, and mappings.
- Move preserves values with the moved instance across every pattern.
- Repeated undo/redo does not leak, duplicate, or cross-connect graph nodes.
- Process and macro targets follow instance identity through reordering.
- Bus and track chains remain isolated.
- A failed prepare/commit leaves the old chain valid and history unmoved.

### Required tests

- Built-in and custom effect insert/delete/move.
- Tensor p-lock and IR-bearing effect restore.
- Multi-pattern slot compaction and expansion.
- Process/macro/neural target restoration.
- Sidechain source restoration after track index changes.
- MIDI-FX routing after undo/redo.
- Bus effect state across several scenes.
- Graph connection-count and route-invariant checks across repeated cycles.

If effect changes touch scheduler MIDI routing or graph-generated event routing,
run the deterministic scheduler lookahead harness required by `AGENTS.md`.

## Slice 7: Remaining Project Structure and Coverage Closure

### Objective

Remove history barriers from the remaining persisted authoring domains and
make "Undo" chronological across the whole project rather than only the
initially requested cases.

This slice should be delivered as independently complete sub-slices.

### Slice 7A: Tracks and rack structure

Operations:

- Add, delete, duplicate, and reorder tracks.
- Add/delete/reorder rack slots and replace rack-slot sources.
- Track colors, collapse state, rack routing mode, pad assignment, and bank
  structure where persisted.

Requirements:

- Use `TrackId`/`RackSlotId` throughout.
- Snapshot/remap group membership, bus routing, mod connections, neural routes,
  sidechains, selections, scene cells, pattern pools, and runtime binding
  definitions.
- Restore graph structure from logical track/rack state.
- Rack macro bank configuration (names, mappings, ranges/curves, persisted
  values) is part of the rack snapshot — it serializes inside
  `ProjectRackTrackPattern` (racks-spec A11) and is restored here, not in
  Slice 7E. Slot deletion's transactional mapping repair must be captured in
  the same entry as the deletion it accompanies.

### Slice 7B: Scenes and Track Patterns

Operations:

- Create/delete/rename/reorder scenes.
- Create/fork/delete/share/assign Track Patterns.
- Change scene cells and track overrides when the change is an authoring action.

Ordinary scene launch remains `Ignore`; editing the scene matrix is undoable.
Stable `PatternId` values must be restored rather than reallocated on redo,
using the `TrackPatternPool::insert_with_id` primitive described under Stable
identities.

### Slice 7C: Buses, sends, and groups

Operations:

- Add/delete/rename/reorder buses.
- Add/delete/edit track groups and membership.
- Structural send and output changes not already handled in Slice 3.

Deleting a bus must snapshot and restore every reference across live and stored
patterns before topology teardown.

### Slice 7D: Processes, modulation, and neural graph

Operations:

- Add/delete/reorder/configure track and project process chains.
- Process lane override edits.
- Mod connection add/delete/edit.
- Neural network/node/edge/override edits.
- Graph override edits.

Use stable logical ids for process slots and graph nodes. If these edits change
scheduler routing, run the deterministic scheduler harness specified by
`AGENTS.md`.

### Slice 7E: Macro configuration

Operations:

- Create/delete/rename macro.
- Map/unmap target.
- Edit range and curve.

Macro ids and mapping ids must remain stable through undo/redo. Live
`MacroSetValue` and `MacroRelease` remain performance actions unless a separate
automation-recording mode explicitly groups them into an authoring take.

This sub-slice covers **project-global** macros only; rack macro banks are
rack state and restore through Slice 7A / rack snapshots. Two alignment
requirements from `docs/MACRO_MAPPING_SPEC.md`:

- Project macro mappings persist a dense `track: usize` today.
  `MacroMappingSnapshot` records `TrackId`; prefer migrating the mapping
  store itself to `TrackId` during the Slice 0 identity work.
- `MacroId` is a persisted monotonic `u32` that is never reused — redo of a
  macro delete/create must reinstate the original id, mirroring the
  `insert_with_id` requirement for `PatternId`.

### Slice 7F: Recording and generated edits

Until this slice ships, a successfully recorded trigger/take is an explicit
history barrier: it is not undoable, and it clears earlier sequencer history so
undo can never cross untracked recorded notes. The UI should report this
limitation when existing history is cleared. Recording must not be presented as
undoable before take-level transaction capture is implemented.

Operations:

- One completed recording take becomes one composite history entry.
- Agent actions and Lisp scripts that perform one requested edit use one
  explicit transaction.
- Neural/generated pattern application records the exact accepted result, not
  the random seed alone.

Cancellation or failed generation restores the initial state and records
nothing. Long-running generators retain stable targets and validate them at
commit just like async instrument/effect loads.

### Acceptance criteria

- Every persisted project authoring path is `Record`/`Coalesce` or intentionally
  documented as non-history performance state.
- No normal authoring action uses `Barrier` in the completed system.
- Track/effect identities survive delete/undo/redo without index confusion.
- Cross-domain references are restored exactly.
- A full project can be edited across several domains and undone/redone in
  strict chronological order.

## Slice 8: Hardening, Audit, and Release Gate

### Objective

Prove completeness and operational safety rather than adding new feature
families.

### Mutation audit

Audit all writes under:

- `state.pattern`
- persisted `state.transport` fields
- `macro_engine`
- `App` project collections (`tracks`, buses, groups, rack metadata)
- graph controller authoring methods
- TUI input and browser handlers
- `src/ui/main.rs` and host-command modules
- piano roll
- recording
- agent actions
- Lisp natives and hooks
- project migration/load/finalization

Every write must be one of:

1. Inside a classified edit transaction.
2. A derived runtime update invoked by `MutationEffects`.
3. Explicitly ephemeral and documented.
4. Project initialization/load under `ApplyMode::ProjectLoad`.

### Test oracle

Add an `AuthoringStateSnapshot` or `AuthoringStateDigest` used only for tests.
It captures all persisted/logical state while normalizing runtime node ids and
other derived fields. Every undoable command family uses the round-trip law
against this oracle.

### Stress and failure testing

- Random deterministic sequences of edits, undos, redos, barriers, scene
  switches, and no-ops.
- History budget eviction while structural resources are retained.
- Repeated structural cycles while playing.
- Graph capacity/preparation failures.
- Missing target and stale async completion.
- Project save at several history revisions.
- Debug assertions for aligned dense runtime collections after structural
  replay.
- Leak checks for compiled resources, leases, graph routes, and node counts.

### Performance gates

- Scalar edit history overhead is negligible relative to the existing UI
  event and parameter-push path.
- No full-project clone for scalar or step edits.
- Step multi-edit capture scales with affected cells.
- Structural snapshots may clone affected Track Pattern lanes but not unrelated
  tracks or buses.
- History work never runs on the audio callback.
- Continuous gesture updates do not allocate a history entry per tick.

### Release gate

The system may be called complete only when:

- No supported authoring path silently bypasses history.
- No ordinary authoring path remains a barrier.
- All domain round-trip suites pass.
- Structural undo failure has been demonstrated non-destructive.
- Resource and graph leak tests pass repeated undo/redo cycles.
- The UI clearly routes text undo versus sequencer undo.

## Implementation Sequence and Rough Effort

For one engineer, after the current UI/TUI modularization settles:

| Slice | Rough effort |
|---|---:|
| 0. Foundation and mutation boundary | 4-7 days |
| 1. Lossless step editing | 3-5 days |
| 2. Pattern geometry and track p-locks | 2-4 days |
| 3. Track/mixer/global parameters | 3-5 days |
| 4. Device values, locks, tensors, presets | 5-8 days |
| 5. Instrument replacement/conversion | 7-12 days |
| 6. Effect-chain structure | 8-15 days |
| 7. Remaining project structure | 10-20 days, delivered as sub-slices |
| 8. Audit and hardening | 5-10 days |

The first useful release through Slice 1 is approximately one to two weeks.
The motivating instrument-replacement case is reached after Slice 5. A fully
project-wide system with no normal authoring barriers is a multi-month effort,
primarily because structural mutations currently span logical state, stored
Track Patterns, compiled resources, and live graph topology.

## Definition of Done for Every Slice

Before a slice is complete:

1. Every included operation has an explicit history label and policy.
2. Every excluded authoring operation in the same area is an explicit barrier.
3. Before/after snapshots include all coupled and hidden state.
4. Undo and redo use stored mementos, not inferred inverse deltas.
5. Failed/no-op edits obey stack invariants.
6. Active and inactive Track Pattern behavior is tested.
7. Runtime synchronization and publication counts are tested.
8. Generic round-trip law tests pass.
9. Relevant AGENTS.md harnesses are run.
10. No fragile workaround, raw runtime identity, or unbounded resource
    retention is introduced.
