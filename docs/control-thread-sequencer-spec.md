# Control-Thread Sequencer and Snapshot Scheduling Spec

## Goal

Move sequencing decisions out of the audio callback and into a dedicated scheduler thread while preserving sample-accurate triggering.

This architecture is intended to unlock:

- Lisp-defined accumulators
- Lisp-defined MIDI FX
- event pipelines such as `sequencer -> MIDI FX -> scheduler -> RT event queue`
- coherent lookahead scheduling
- cleaner separation between musical logic and RT audio execution

The audio thread should become a pure event consumer and DSP executor. It should not perform pattern traversal, accumulator logic, or arbitrary scripted control evaluation.

## Non-Goals

- Replacing the existing DSP/render path
- Moving voice rendering off the audio thread
- Making the audio thread depend on locks, heap allocation, or the Lisp runtime
- Introducing sample-inaccurate control timing

## Current Problem

The current design performs sequencing work directly in the callback path. The accumulator call site in `fire_step_trigger()` runs on the audio thread and reads track state directly before dispatching note actions.

That is workable for fixed Rust function pointers and small built-in state machines, but it makes the following difficult or unsafe:

- calling embedded Lisp per step
- chaining programmable MIDI FX
- scheduling future events with a lookahead horizon
- ensuring control logic sees a coherent view of pattern state

The core issue is that arbitrary musical logic and RT execution have conflicting requirements.

## High-Level Architecture

Introduce three distinct layers:

1. State authoring layer
2. Scheduler layer
3. RT execution layer

### State Authoring Layer

The UI, scratch Lisp, project loading, and any future automation editing tools mutate the sequencer model. They do not schedule directly into the audio thread.

All authored state changes produce a new logical snapshot version.

### Scheduler Layer

A dedicated scheduler thread:

- reads a coherent immutable snapshot of sequencer state
- advances musical time using transport information
- evaluates accumulators, MIDI FX, and Lisp control logic
- emits timestamped events into a lock-free queue for the audio thread
- schedules ahead by a bounded lookahead horizon

### RT Execution Layer

The audio callback:

- reads the current sample time / block window
- consumes due events from the queue
- applies them at exact sample offsets within the current buffer
- renders audio

No pattern walking or Lisp execution occurs here.

## Design Principles

- RT thread must remain allocation-free, lock-free, and bounded
- scheduler must operate on coherent snapshots, not live torn state
- event delivery must be sample-accurate within each render block
- edits must not require hard synchronization with the callback
- the system must tolerate frequent live edits while playing
- transport changes must invalidate stale future events deterministically

## Snapshot Model

## Summary

The scheduler never reads mutable live track/pattern state directly. Instead it reads an immutable `SequencerSnapshot` published by the authoring side.

Each published snapshot gets a monotonically increasing `snapshot_version`.

The scheduler only schedules against one snapshot version at a time.

## Snapshot Contents

The snapshot should contain all state required to produce musical events without touching live atomics:

- global transport-relevant settings
  - BPM
  - swing settings
  - current pattern selection
  - play mode / loop mode
- track parameters
  - gate
  - attack/release if scheduler-relevant
  - timebase
  - accumulator selection
  - accumulator limit/mode
  - FTS scale
  - mute / solo if present
- step activity bitsets
- step parameter payloads
  - duration
  - velocity
  - speed
  - aux values
  - transpose
  - pan
  - chop
  - sync
- chord data
- timebase plocks
- effect/instrument plocks only if required by scheduler decisions
- instrument routing or track destination metadata needed for event dispatch
- references to scripted accumulator / MIDI FX programs by stable id

The snapshot should contain plain owned Rust data, not atomics.

## Snapshot Publication

Recommended publication model:

- authoring side builds a fresh immutable snapshot
- snapshot is wrapped in `Arc<SequencerSnapshot>`
- a single atomic pointer or `ArcSwap`-style holder publishes the latest snapshot
- `snapshot_version` increments on each publication

This gives:

- lock-free reads by the scheduler
- coherent all-at-once state handoff
- easy version tagging on scheduled events

## Snapshot Frequency

Do not publish a fresh snapshot for every tiny internal callback read. Publish on user-visible state edits:

- step toggle
- step param change
- plock change
- track param change
- pattern switch
- project load
- Lisp registration of accumulators / MIDI FX
- transport mode changes that alter scheduling semantics

Batching multiple edits into one published snapshot is acceptable and desirable.

## Transport Ownership

Transport needs a split model:

- authoritative transport timeline for the audio thread
- readable transport state for the scheduler

The scheduler should not invent sample time. The audio engine remains the source of truth for actual render position.

Recommended model:

- audio thread maintains a monotonic rendered sample counter
- scheduler reads a shared transport clock state
- scheduler converts its lookahead horizon into absolute target sample timestamps
- events are stamped in the audio clock domain

### Required Shared Transport Fields

- playing/stopped
- transport generation
- current BPM map or current BPM if fixed
- current absolute sample position
- loop range / pattern restart generation
- sample rate

`transport_generation` increments on disruptive transport events:

- stop
- restart
- seek
- pattern reset
- loop range change if it invalidates future events
- BPM jump if existing queued timestamps should be discarded

## Scheduler Thread

## Responsibilities

The scheduler thread is responsible for:

- watching for new snapshots
- watching for transport generation changes
- maintaining per-track scheduling state
- computing future musical triggers inside a lookahead window
- evaluating accumulators and MIDI FX
- translating logical triggers into timestamped execution events
- enqueuing events for RT consumption

## Thread Cadence

The scheduler does not need to run per sample or per callback. It should wake often enough to keep the queue filled beyond the callback horizon.

Suggested starting point:

- wake every `1-2 ms`, or
- wake on a condition/event plus a fallback periodic poll

The scheduler should always maintain a minimum queued future horizon.

## Lookahead Window

Use two tuning values:

- `lookahead_target_samples`
- `lookahead_min_samples`

Example initial policy:

- target: `2-4` audio buffers
- minimum: `1-2` audio buffers

If the audio callback buffer size is 256 samples, a reasonable initial target is 512 to 1024 samples.

The scheduler loop:

1. read current audio sample position
2. read the last queued sample timestamp
3. if queued horizon is below `lookahead_target_samples`, schedule forward
4. stop when queued horizon reaches target or a generation/version boundary invalidates the pass

This keeps control-thread jitter away from RT execution while preserving responsiveness.

## Scheduling State

The scheduler needs its own runtime state separate from authored snapshots.

### Per-track runtime state

- next step index to evaluate
- next step absolute sample time
- current accumulator runtime state
  - value
  - reversed
  - future custom state if needed
- per-track loop counters / phrase counters if later needed
- per-track scripted processor state if MIDI FX are stateful

This state is not part of the authored project snapshot. It is runtime scheduling state.

## Event Queue

## Requirements

The event queue between scheduler and audio thread must be:

- lock-free
- bounded capacity
- ordered by timestamp
- cheap to read from RT
- able to tag events with snapshot/transport generations

## Event Shape

Suggested event structure:

```rust
pub struct ScheduledEvent {
    pub sample_time: u64,
    pub transport_generation: u64,
    pub snapshot_version: u64,
    pub track_idx: u16,
    pub event_id: u64,
    pub kind: ScheduledEventKind,
}

pub enum ScheduledEventKind {
    NoteOn {
        step_idx: u16,
        resolved: ResolvedStepEvent,
    },
    NoteOff {
        voice_hint: Option<u32>,
    },
    ParamChange {
        target: ParamTarget,
        value: f32,
    },
    Midi {
        bytes: [u8; 3],
        len: u8,
    },
    Transport {
        op: TransportEventOp,
    },
}
```

`ResolvedStepEvent` should be a plain POD-like structure containing the values the RT side needs to trigger exactly what the scheduler decided.

The RT thread should not need to consult the pattern snapshot to interpret a due event.

## Queue Ordering

Events should be produced in non-decreasing `sample_time` order.

That allows the audio thread to:

- peek the queue head
- stop once the head is beyond the current buffer window
- compute `offset = event.sample_time - block_start_sample`
- apply the event at that exact intra-buffer sample offset

## Event Invalidation

There are two kinds of invalidation:

- hard invalidation
- soft coexistence

### Hard invalidation

For transport resets, seek, stop, or disruptive pattern changes:

- increment `transport_generation`
- scheduler clears its local future state as needed
- audio thread discards queued events whose `transport_generation` is stale

### Soft coexistence

For normal live edits:

- publish a new `snapshot_version`
- scheduler starts using the new snapshot for events not yet scheduled
- already queued near-future events may either:
  - remain valid for simplicity, or
  - be discarded if version mismatch exceeds a configured policy

Recommended initial policy:

- use hard invalidation for transport changes
- allow near-future stale snapshot events to play out unless the edit explicitly requests flush

This avoids audible churn from constant queue flushing during live edits.

## Future Considerations

Keep the initial invalidation model simple:

- full reset only for obvious transport-level changes such as play, stop, seek, and pattern switch
- ordinary live edits should apply on the next lookahead fill rather than forcing immediate queue flushes

This is intentionally conservative. A short lookahead window makes many edits feel immediate enough without introducing more scheduler complexity.

Only revisit this if concrete failures appear, for example:

- duplicate or missing triggers after specific live edits
- audible lag that is too long for structural edits
- edits that clearly should rebuild future timing, not just future payload

If that happens later, introduce a small invalidation taxonomy instead of a broad resync system:

- `PayloadOnly`: update future scheduled payloads without touching timing
- `RebuildQueueFromNow`: rebuild future events while preserving musical phase
- `FullResync`: reset scheduler timing state for transport-level changes

## Sample Accuracy

## Core Rule

The scheduler decides *what* should happen and *when in absolute sample time* it should happen.

The audio thread only applies due events at the exact offset inside the current block.

This is what preserves sample accuracy even though sequencing moved off the callback.

## Buffer-Level Execution

For an audio callback handling `[block_start, block_end)`:

1. peek queue head
2. if `head.sample_time >= block_end`, stop consuming
3. if `head.sample_time < block_start`, consume as late event and log underrun
4. otherwise compute sample offset within the block
5. trigger voice/param action at that offset

The renderer must support intra-buffer event application rather than only block-edge triggering.

If the current engine only applies note events at block boundaries, that must be fixed as part of this migration.

## Late Event Policy

The audio thread should count late events. They indicate scheduler starvation or lookahead too short.

Suggested metrics:

- `scheduled_events_total`
- `late_events_total`
- `max_lateness_samples`
- `queue_depth_events`
- `queue_horizon_samples`
- `scheduler_wake_jitter_us`

## Accumulators in the New Model

Accumulators should move entirely into the scheduler layer.

That means:

- `acc fn` is evaluated on the scheduler thread
- accumulator runtime state lives in scheduler per-track runtime
- accumulator output is translated into one or more scheduled events
- limit/mode is applied during scheduling, not during RT execution

This solves the current problem where accumulators must be RT-safe function pointers.

## Lisp-Defined Accumulators

Once accumulators run on the scheduler thread, Lisp-defined accumulators become feasible.

Recommended model:

- scratch Lisp defines a named accumulator
- registration publishes a compiled/validated script descriptor into the snapshot
- tracks reference that accumulator by stable id or name
- scheduler invokes the corresponding Lisp closure or script object off the RT thread

Even on the scheduler thread, avoid looking up mutable editor runtime state directly. Use a dedicated script registry owned by the control side and referenced by stable ids in the snapshot.

## MIDI FX Pipeline

The scheduler should support a logical pipeline:

```text
step trigger
-> accumulator stage
-> note/control event stream
-> zero or more MIDI FX processors
-> scheduled timestamped events
```

Each MIDI FX stage should:

- receive timestamped logical events plus context
- be allowed to emit zero, one, or many future events
- maintain its own per-track runtime state

This architecture is a better fit than trying to encode MIDI FX inside the RT callback.

## Proposed Script API Direction

The eventual script contract should be event-oriented, not step-storage-oriented.

For example:

- accumulator scripts receive:
  - current step payload
  - track context
  - current accumulator state
  - current logical trigger timestamp
- MIDI FX scripts receive:
  - incoming event
  - processor state
  - timing context

They return:

- updated processor state
- emitted events with relative or absolute timing

This is more composable than exposing direct mutation of live engine state.

## Interaction with Existing Engine

## Current `fire_step_trigger()` Role

The current callback-side `fire_step_trigger()` combines:

- step resolution
- accumulator execution
- trigger dispatch

In the new design it should be split.

### Scheduler-side responsibilities

- resolve step data from snapshot
- evaluate accumulator and MIDI FX
- construct `ScheduledEvent`s

### RT-side responsibilities

- consume `ScheduledEvent`s
- invoke `fire_resolved()`-like trigger code using already-resolved event payloads

`fire_resolved()` or a similar function can remain on the RT side if it only applies already-decided trigger data and pushes params/note-ons into the audio graph.

## Snapshot vs Runtime State Split

The new architecture depends on a strict separation:

### Snapshot state

- authored musical data
- immutable while being scheduled

### Scheduler runtime state

- next step times
- accumulator values
- processor state
- cursor position

### RT execution state

- voices
- envelopes
- graph nodes
- block-local event application

Do not blur these layers.

## Data Model Changes

## Track accumulator reference

Today tracks store `accumulator_idx`. That is tied to a static registry order and is not a good fit for dynamic scripted accumulators.

Replace with a stable reference model:

```rust
pub enum AccumulatorRef {
    Builtin(u16),
    Scripted(ScriptId),
}
```

Project serialization should store stable names or ids, not transient dropdown indices.

## Script Registry

Introduce a control-side registry for scripted processors:

- accumulator scripts
- MIDI FX scripts

Suggested properties:

- stable `ScriptId`
- human-readable name
- script kind
- validated callable handle / compiled representation
- metadata for UI dropdowns and parameter exposure

Snapshots should include only the stable ids and any immutable metadata needed by the scheduler.

## Authoring Pipeline

Authoring operations should no longer write directly to scheduling runtime state.

Recommended write flow:

1. mutate editable model
2. rebuild or incrementally derive `SequencerSnapshot`
3. publish `Arc<SequencerSnapshot>` with new version
4. notify scheduler thread

This applies to:

- UI edits
- project load
- scratch Lisp state changes
- script registration

## Migration Plan

## Phase 1: Introduce Snapshot Types

Add new snapshot data structures without changing scheduling yet.

Tasks:

- define `SequencerSnapshot`
- define immutable `TrackSnapshot`, `StepSnapshot`, `PatternSnapshot`
- add snapshot builder from existing live state
- publish latest snapshot behind atomic shared ownership
- add `snapshot_version`

Success criteria:

- snapshot can be built and published on every edit
- no behavior change yet

## Phase 2: Introduce Scheduled Event Queue

Add the scheduler-to-RT queue and make the audio thread able to consume timestamped events.

Tasks:

- define `ScheduledEvent`
- add lock-free queue
- add RT consumer path
- add intra-buffer sample offset triggering
- add metrics for queue depth and late events

Success criteria:

- RT can consume externally scheduled test note events sample-accurately

## Phase 3: Add Dedicated Scheduler Thread

Introduce the scheduler thread, but initially schedule only built-in step triggers without Lisp.

Tasks:

- maintain transport-aware scheduling cursor
- schedule step triggers from snapshots
- translate them into `ScheduledEvent::NoteOn`
- keep RT callback as fallback behind a feature flag or config switch

Success criteria:

- playback works with sequencing disabled in the audio callback
- queued events remain sample-accurate

## Phase 4: Move Built-In Accumulators to Scheduler

Tasks:

- move accumulator runtime state out of callback state
- evaluate existing built-in accumulators on scheduler thread
- emit resolved events identical to current behavior

Success criteria:

- built-in accumulators behave the same as before
- callback no longer touches accumulator registry/state

## Phase 5: Introduce Script Registry and Lisp Accumulators

Tasks:

- add `def-accumulator`
- add stable script ids / names
- expose scripted accumulators in UI dropdown
- publish script references into snapshots
- run scripted accumulators on scheduler thread

Success criteria:

- a Lisp accumulator can transform a track's triggers without RT involvement

## Phase 6: Add MIDI FX Pipeline

Tasks:

- define event-processor interface
- add processor chains per track
- support timestamp-preserving and timestamp-generating FX
- add Lisp-defined MIDI FX registration

Success criteria:

- scheduler can process `sequencer -> MIDI FX -> scheduled events`

## Phase 7: Remove Callback-Side Sequencing

Tasks:

- delete or fully retire callback-side step walking
- keep only RT event execution
- simplify callback state to transport, queue consumption, and DSP dispatch

Success criteria:

- no musical sequencing logic remains in the audio callback

## Failure Modes and Mitigations

## Scheduler Falls Behind

Symptom:

- late events observed in RT

Mitigations:

- increase lookahead target
- reduce scheduler wake interval
- cache script handles
- cap expensive script execution
- add watchdog metrics

## Too Much Queue Churn on Live Edits

Symptom:

- excessive invalidation and rescheduling

Mitigations:

- use snapshot versioning without hard flush for minor edits
- keep hard flush only for transport-generation changes
- batch UI edits

## Snapshot Build Cost Too High

Symptom:

- editing becomes expensive

Mitigations:

- start with whole-snapshot rebuilds
- later optimize with per-pattern or per-track structural sharing
- publish only when meaningful state changes occur

## Script Runtime Too Slow

Symptom:

- scheduler starvation

Mitigations:

- scheduler-side execution budget metrics
- optional max emitted events per call
- cached parsed/compiled closures
- later: lower hot script paths to a cheaper IR if needed

## Open Design Questions

- Should the scheduler own the authoritative playhead, or should it derive all timing from an audio-owned sample clock?
- Do live step edits flush already-scheduled future notes on the same track, or only affect not-yet-scheduled horizon?
- Should note-off events be scheduled explicitly, or derived on RT from note-on plus duration?
- How much future horizon should scripts be allowed to emit?
- Should script state survive transport stop/restart, or reset based on transport generation?
- Is one global scheduler thread enough, or should scripted processor work later be partitioned?

## Recommended Initial Decisions

- audio thread owns actual sample clock
- one dedicated scheduler thread is enough for now
- whole-snapshot rebuilds are acceptable for MVP
- near-future queued events survive ordinary edit churn
- disruptive transport changes increment `transport_generation` and flush stale events
- note-offs should be scheduled explicitly for deterministic behavior
- scripted accumulators and MIDI FX run only on the scheduler thread

## Testing Plan

## Unit Tests

- snapshot builder produces coherent immutable copies
- scheduler emits expected timestamps for fixed BPM/timebase
- accumulator state evolution matches prior built-in behavior
- transport-generation invalidation drops stale events
- snapshot version updates switch future scheduling source

## Integration Tests

- live edit during playback updates newly scheduled events without RT glitches
- stop/start flushes stale future events
- seek/pattern reset realigns scheduler and RT
- high-density event tracks remain sample-accurate
- scripted accumulator emits deterministic output under lookahead scheduling

## Instrumentation

Add observable counters/logging for:

- current snapshot version
- current transport generation
- queue depth
- queue horizon in samples
- scheduler loop duration
- late event count
- max lateness
- snapshot rebuild duration

## Final State

When this migration is complete:

- the callback no longer sequences
- the scheduler sequences ahead from immutable snapshots
- the audio thread consumes exact timestamped events
- Lisp can safely participate in sequencing and MIDI FX
- live edits remain coherent
- triggering remains sample-accurate
