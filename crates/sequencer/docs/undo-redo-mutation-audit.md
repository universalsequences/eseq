# Undo/Redo Mutation Audit

This is the Slice 8 mutation inventory. Each production write to persisted or
logical sequencer state belongs to exactly one class below. Calls to a low-level
helper inherit the class of the boundary that invokes it; those helpers are not
independent authoring entry points.

## 1. Recorded or coalesced edit transactions

| State | Authoring boundaries |
|---|---|
| Step cells, chords, every p-lock domain, pattern length | `try_apply_command`, recorded piano-roll operations, recording-take transaction |
| Track parameters, sends/output, mixer values, BPM, master volume, global reverb | `try_apply_command`; continuous controls use one active gesture |
| Instrument, MIDI-FX, audio-FX, bus-FX, rack and rack-slot values | device-value and device-p-lock patches in `tui/edit.rs` |
| Instrument/effect/rack source and chain topology | recorded graph/effect wrappers in `tui/edit.rs`; async completion validates stable ids before commit |
| Tracks, rack slots, scenes/Track Patterns, buses and groups | structural wrappers in `tui/edit.rs` around graph/project operations |
| Processes, modulation, neural state and graph overrides | scene-repository transactions; UI and Lisp submit intent before mutation |
| Project macro definitions and mappings | `MacroConfigurationPatch`; live values are excluded below |
| Generated and agent edits | explicit compound authoring request; failure rolls applied patches back |

TUI input, the sample/preset browsers, piano roll, `ui/main.rs`, host commands,
Lisp natives/hooks, recording, and agent actions were audited as callers. Their
persisted mutations enter one of the boundaries above. `AppCommand` is
exhaustively classified by `history_policy`; no current command returns
`HistoryPolicy::Barrier` or `Reset`.

The audit found one bypass: the three serialized global reverb parameters were
written directly by keyboard, numeric-entry, and mouse handlers. Slice 8 routes
them through `AppCommand::SetReverbParam` and the coalesced global-parameter
patch. Project reset/load uses the explicitly unrecorded initialization helper.

## 2. Derived runtime updates driven by mutation effects

- Scheduler publication and current-pattern materialization.
- Audio graph parameter pushes, route rebuilds, node construction/removal, and
  deferred lease retirement.
- Dense device/rack/effect lookup maps and stable-id location indexes.
- Macro target pushes and published macro/neural/graph override snapshots.
- Bus-gate runtime, meters, playheads, voice counts, accumulator resets, and UI
  invalidation data.

Replay functions apply logical state first and derive these resources through
`MutationEffects` or the owning graph controller. Runtime graph node ids,
engine handles, scheduler versions, and lease watermarks are not authoring
identity and are normalized out of the test oracle.

## 3. Explicitly ephemeral performance or UI state

- Transport playing state, playheads/phases/epochs, sounding voices, record
  arming, and record-quantize threshold.
- Live project/rack macro values, releases, scene-push gestures, and meters.
- Cursor, selection, hover, focus, open panels/dropdowns, browser filter and
  scroll, rack pad bank/selected slot, redraw flags, and status messages.
- Text-editor buffer history and scratch cursor. Text editing owns its own undo
  stack and does not enter sequencer history.

## 4. Project initialization/load

Schema migration in `project.rs` transforms detached `ProjectFile` data before
application. New-project reset, project load/finalization, pattern repository
materialization, graph construction, stable-id registry rebuilding, macro
restoration, and reverb restoration occur only in the project replacement
workflow. That workflow resets history after successful replacement and is the
`ApplyMode::ProjectLoad` equivalent; it is not an ordinary authoring barrier.

## Release-gate evidence

- `AuthoringStateSnapshot` serializes the authoritative project representation
  byte-for-byte while normalizing project name, active scene/track, scratch
  editor state, and runtime-only identities.
- Mixed command and deterministic seeded stress tests compare complete oracle
  snapshots at the captured undo and redo revisions.
- History tests prove failed replay leaves stacks/revisions intact, budget
  eviction releases only evicted retained resources, and continuous gestures
  commit one entry.
- Patch shapes are domain deltas: scalar patches contain one stable target;
  step patches contain only affected cells; structural patches retain only the
  affected container or the coherent scene repository required by that edit.
  No command patch contains `App` or `ProjectFile`, so scalar/step editing
  cannot clone the full project.
- `UndoManager` is owned by `tui::App`. The audio callback and scheduler
  snapshots contain no history manager or history patch type, so audio-thread
  processing cannot perform history work.
- Metal and terminal shortcut tests prove sequencer-facing buffers route the
  primary undo shortcut to sequencer history while editable text and focused
  text widgets retain editor undo.

Barrier infrastructure remains as a defensive invariant and for its unit test;
there is no ordinary authoring caller of `commit_history_barrier`.
