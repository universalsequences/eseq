# MIDI FX Chain Spec

## Problem

The current Lisp MIDI FX prototype is implemented through the track accumulator slot. That was useful for proving the scheduler-side event processing model, but it conflates two different concepts:

- **Accumulator**: a visible per-track sequencing state machine, such as the builtin transpose ramp accumulator.
- **MIDI FX**: one or more event processors that transform scheduled note events into zero, one, or many downstream note events.

Because the current prototype occupies the visible accumulator slot, a track cannot use a builtin accumulator and a Lisp arpeggiator at the same time. The long-term model should allow both, and should allow several MIDI FX to be composed in series.

## Goals

- Keep the existing visible accumulator slot available for builtin accumulator workflows.
- Add a separate per-track MIDI FX chain.
- Make the chain scriptable from `*scratch*` before adding UI.
- Support multiple MIDI FX in order, where output from one stage becomes input to the next.
- Support event expansion, suppression, retiming, routing, and parameter mutation.
- Preserve the scheduler-thread execution model. Lisp MIDI FX must not run on the audio thread.
- Reuse the note-span grouping model proven by the arpeggiator prototype.
- Keep the first authoring path simple enough for scratch-buffer iteration.

## Non-Goals

- A full UI for editing MIDI FX chains.
- A plugin marketplace or preset browser.
- Audio FX processing. This spec is only for pre-instrument note/event scheduling.
- Replacing the builtin accumulator system immediately.
- Supporting arbitrary cross-track feedback loops in the first implementation.

## Conceptual Pipeline

For each raw sequencer trigger:

```text
pattern step trigger
  -> pre-accumulator MIDI FX chain, optional
  -> visible track accumulator slot
  -> post-accumulator MIDI FX chain, optional
  -> ...
  -> scheduled resolved trigger events
```

The accumulator slot remains a single visible UI concept. It mutates the initial `ResolvedStep` or emits builtin accumulator actions. MIDI FX are separate hidden/scripted chains. A track should be able to choose whether its MIDI FX chain runs before or after the visible accumulator slot. The default should be post-accumulator, because it preserves the current mental model: the visible accumulator shapes the track, then MIDI FX transform the result.

## Core Types

### `MidiFxEvent`

Internal scheduler-side event object passed between MIDI FX stages.

```rust
pub struct MidiFxEvent {
    pub offset_beats: f32,
    pub track: usize,
    pub step: usize,
    pub samples_per_step: f32,
    pub resolved: ResolvedStep,
    pub chord: Vec<f32>,
    pub chord_durations: Vec<f32>,
    pub chord_step_transpose: f32,
    pub effect_params: Vec<ScheduledEffectParam>,
    pub instrument_params: Vec<ScheduledInstrumentParam>,
    pub note_spans: Option<Vec<AccumulatorNoteSpan>>,
}
```

Notes:

- `offset_beats` is relative to the original scheduler trigger sample time.
- `track` is the current target track for the event.
- `step` remains the source step unless explicitly retargeted.
- `note_spans` is a general time-span note group, not an arpeggiator-specific type.
- An empty `Some(vec![])` note span set means the current trigger participates in an existing note group but should not start its own group.
- `None` means no explicit grouped note context was supplied, so helpers may fall back to the event chord/resolved note.

### `MidiFxOutput`

Each stage receives one `MidiFxEvent` and returns zero or more events.

```rust
pub struct MidiFxOutput {
    pub events: Vec<MidiFxEvent>,
}
```

A suppressed input event is represented by returning `events = []`.

## Note Spans

The scheduler computes note spans per track from the immutable `SequencerSnapshot`.

```rust
pub struct AccumulatorNoteSpan {
    pub transpose: f32,
    pub start_beats: f32,
    pub end_beats: f32,
}
```

This type should probably be renamed again when moved fully out of accumulator code, for example:

```rust
pub struct MidiNoteSpan { ... }
```

The current behavior:

- A trigger whose note starts a new held group receives all notes in that group.
- Later notes that start before the held group ends are included in the first trigger's `note_spans`.
- Later triggers inside an already-running group receive `Some(vec![])`.
- A note that starts exactly at or after the previous group end starts a new group.

This supports arps, strums, chord gates, ratchets, and other processors that need a time-varying note set.

## Scratch Lisp API

### File Layout

The MIDI FX system should eventually mirror custom instruments:

```text
midi-fx/
  octave-every-n/
    dsp.lisp
    ui.lisp
    presets/
```

This follows the existing instrument pattern:

- `instruments/emulations/digitone/dsp.lisp` declares DSP inputs and `param` metadata.
- `instruments/emulations/digitone/ui.lisp` declares a `defsynth-ui` body that renders controls by looking up params from the current instrument instance.

For MIDI FX:

- `midi-fx/<name>/dsp.lisp` declares the MIDI FX definition, params, and event-processing body.
- `midi-fx/<name>/ui.lisp` declares the editor UI for one MIDI FX chain instance.
- Presets can later store default instance param values.

Scratch definitions remain useful for iteration, but folder-based MIDI FX should be the path for reusable effects and custom UI.

### Defining MIDI FX

```lisp
(def-midi-fx "arp-16"
  (do
    (fx-suppress)
    (for-each |i|
      (fx-arp-emit :16 i :vel 0.8)
      (range 0 (fx-arp-count :16)))))
```

`def-midi-fx` registers a named scheduler-side closure in the scratch sequencing runtime. The body runs once per input `MidiFxEvent`.

Folder-based `dsp.lisp` can use the same form, with metadata:

```lisp
(def-midi-fx "octave-every-n"
  (:params
    (param every  @default 2  @min 1   @max 16 @unit trig)
    (param amount @default 12 @min -48 @max 48 @unit st))
  (do
    (let ((n (fx-state-get :count 0)))
      (fx-state-set :count (+ n 1))
      (fx-emit 0
        :transpose (+ fx-transpose
                      (* (fx-param amount)
                         (floor (/ n (fx-param every)))))))))
```

This intentionally resembles instrument `param` declarations. The params are metadata plus defaults; instance values live in Rust project state and are passed into the scheduler invocation context.

### Assigning a Chain

Initial UI-less assignment:

```lisp
(seq-use-midi-fx 0 "arp-16")
(seq-use-midi-fx 0 "humanize" "arp-16" "delay")
```

Proposed variants:

```lisp
(seq-clear-midi-fx 0)
(seq-midi-fx-chain 0) ; => ("humanize" "arp-16" "delay")
```

The chain is ordered left to right. The first processor receives the accumulator output event. Each emitted event from a stage is fed into the next stage.

### Event Helpers

The `fx-*` API mirrors the current `acc-*` prototype but is no longer tied to accumulator state.

```lisp
(fx-suppress)
(fx-emit offset :note 7 :vel 0.8)
(fx-emit :16 2 :note 12 :duration 1.0)
```

`fx-emit` emits a derived event from the current input event. It should support the same override keys as `acc-emit`:

- `:track`
- `:note`
- `:transpose`
- `:duration`
- `:vel` / `:velocity`
- `:speed`
- `:pan`
- `:chop`

### Timing Model

All MIDI FX timing is represented internally as `offset_beats`, where one beat is one quarter note. The scheduler converts beat offsets to absolute sample times using the current snapshot BPM. Lisp code should never calculate samples.

`fx-emit` should support three timing forms:

```lisp
(fx-emit 0.5 :note 7)       ; source-step units
(fx-emit :8t 3 :note 7)     ; explicit musical timebase units
(fx-emit :beats 0.037 :note 7) ; direct quarter-note beat offset
```

The default numeric form is source-step units. If the input event came from a 16th-note step, `(fx-emit 1 ...)` means one 16th note later. If it came from an 8th-triplet step, `(fx-emit 1 ...)` means one 8th triplet later. This keeps simple effects coupled to the source track feel.

The explicit timebase form decouples the effect from the source step timebase:

```lisp
(fx-emit :8t 0 :note (fx-note 0))
(fx-emit :8t 1 :note (fx-note 1))
(fx-emit :8t 2 :note (fx-note 2))
```

That is the intended way to say "arpeggiate in 8T timebase" even if the source track is running at 16ths, 32nds, or polyrhythm.

The direct beat form is for arbitrary microtiming curves and algorithms:

```lisp
(def-midi-fx "exp-flam"
  (do
    (fx-suppress)
    (for-each |i|
      (fx-emit :beats (* 0.012 (- (pow 1.35 i) 1))
        :vel (* 0.92 (pow 0.88 i)))
      (range 0 8))))
```

This creates eight events with exponentially increasing microtiming offsets. The numbers are still musical beats, not seconds or samples, so the effect remains tempo-relative.

Timing helpers should exist so Lisp does not have to encode constants manually:

```lisp
(fx-time :16 1)     ; => beat length of one 16th-note unit as a plain number
(fx-time :8t 3)     ; => beat length of three 8th-triplet units as a plain number
(fx-source-time 1)  ; => beat length of one current source-step unit
```

`fx-time` and `fx-source-time` should return plain beat numbers. This keeps generated timing values easy to feed into `:beats` and easy to combine mathematically:

```lisp
(fx-emit :beats (* i (fx-time :8t 1)) :note (fx-note i))
```

This is the intended low-friction path for effects such as "8T triplet arpeggiator" and for algorithmic timing curves.

### Note Span Helpers

General helpers:

```lisp
(fx-note-count)
(fx-note 0)
(fx-note-start 0)
(fx-note-end 0)
(fx-notes)
```

Functions:

- `fx-note-count`: returns the number of note spans available to the current event. If the current event has explicit note spans, this is the span count. If no explicit spans are present, it falls back to the event chord count, or `1` for a single-note event.
- `fx-note`: returns the transpose value for note index `i`. With explicit note spans, this reads `note_spans[i].transpose`. With no explicit spans, it reads the event chord or single resolved transpose. Out-of-range should return `nil`.
- `fx-note-start`: returns the note span start time in beats relative to the current input event. For fallback chord/single-note data, start is `0`. Out-of-range should return `nil`.
- `fx-note-end`: returns the note span end time in beats relative to the current input event. For fallback chord/single-note data, end is the note duration converted to beats. Out-of-range should return `nil`.
- `fx-notes`: returns all available notes as structured data, preserving ordering.

`fx-notes` returns a list of maps:

```lisp
((:note 0 :start 0 :end 3)
 (:note 7 :start 0 :end 1)
 (:note 12 :start 1 :end 3))
```

These are in beats relative to the current input event's trigger time.

Examples:

```lisp
; Strum all notes using their current ordering.
(def-midi-fx "strum-up"
  (do
    (fx-suppress)
    (for-each |i|
      (fx-emit :beats (* i 0.025) :note (fx-note i))
      (range 0 (fx-note-count)))))

; Emit only notes that are active at this exact event start.
(def-midi-fx "only-initial-notes"
  (do
    (fx-suppress)
    (for-each |i|
      (if (= (fx-note-start i) 0)
        (fx-emit 0 :note (fx-note i)))
      (range 0 (fx-note-count)))))
```

### Arp Convenience Helpers

Arp helpers are implemented on top of note spans:

```lisp
(fx-arp-count :16)
(fx-arp-note :16 0)
(fx-arp-emit :16 0 :vel 0.8)
```

Example:

```lisp
(def-midi-fx "arp-16"
  (do
    (fx-suppress)
    (for-each |i|
      (fx-arp-emit :16 i :vel 0.8)
      (range 0 (fx-arp-count :16)))))
```

### Compatibility

The current `acc-*` helpers can remain for existing accumulator prototypes. New MIDI FX should use `fx-*`.

Possible implementation shortcut:

- Internally implement `fx-*` first.
- Keep `acc-*` as aliases when running in an accumulator context.
- Remove or de-emphasize accumulator-based MIDI FX examples from docs once `def-midi-fx` exists.

## MIDI FX UI Lisp

Custom MIDI FX UI should follow the same split as `instruments/emulations/digitone/ui.lisp` versus `dsp.lisp`.

Digitone's `dsp.lisp` declares params:

```lisp
(param algorithm @default 2 @min 1 @max 8)
(param amp_attack @default 4 @min 1 @max 5000 @unit ms)
```

Digitone's `ui.lisp` does not own those values. It looks up the current instance param by name:

```lisp
(let ((p (inst-param synth-ui-current-inst "algorithm")))
  ...)
```

and writes changes through a host native:

```lisp
(fx-set-instrument-value p v)
```

MIDI FX should use the same shape with MIDI-FX-specific names:

```lisp
(def-midi-fx-ui
  (h-stack :width :fill :gap 0.4
    (mfx-param-number "every" "every" 0 "trig")
    (mfx-param-number "amount" "amt" 0 "st")))
```

The expanded lower-level version would look like:

```lisp
(def-midi-fx-ui
  (let ((every (mfx-param midi-fx-ui-current-instance "every"))
        (amount (mfx-param midi-fx-ui-current-instance "amount")))
    (h-stack :width :fill :gap 0.4
      (number-picker :value (get every :value)
        :min (get every :min)
        :max (get every :max)
        :decimals 0
        :unit "trig"
        :on-change (lambda (v) (mfx-set-param-value every v)))
      (number-picker :value (get amount :value)
        :min (get amount :min)
        :max (get amount :max)
        :decimals 0
        :unit "st"
        :on-change (lambda (v) (mfx-set-param-value amount v)))))))
```

Runtime responsibilities:

- The UI runtime evaluates `ui.lisp` and renders controls.
- The UI runtime receives a `midi-fx-ui-current-instance` map that contains param metadata and current values for one chain slot.
- `mfx-set-param-value` mutates Rust authoring/project state, not scheduler Lisp memory.
- Rust publishes a scheduler snapshot or control update.
- The scheduler runtime sees new values through `(fx-param every)` / `(fx-param amount)` on future MIDI FX invocations.

This preserves the two-runtime boundary. The UI runtime never reaches directly into scheduler closure state. It edits authored parameters. Scheduler state such as counters and phase remains private to the scheduler-side MIDI FX instance.

As with custom instruments, simple generated controls should be possible. A loader can transform:

```lisp
(params every amount)
```

into a stack of default controls, while custom `ui.lisp` can hand-author richer layouts.

## Chain Evaluation Semantics

For each raw trigger:

1. Resolve the base step into `ResolvedStep`, chord data, effect params, and instrument params.
2. If the track is configured for pre-accumulator MIDI FX, build initial `MidiFxEvent`s and run the chain.
3. Run the visible track accumulator slot on the resulting event stream, or on the raw resolved step when no pre-chain is configured.
4. If the track is configured for post-accumulator MIDI FX, convert accumulator result/actions into `MidiFxEvent`s and run the chain.
5. Convert final `MidiFxEvent`s into `ScheduledEventKind::ResolvedTrigger`.

Pseudo-code:

```rust
let events = base_trigger_to_midi_fx_events(...);
let events = if track.midi_fx_position == PreAccumulator {
    run_midi_fx_chain(track, events)?
} else {
    events
};
let events = run_visible_accumulator_slot(track, events)?;
let events = if track.midi_fx_position == PostAccumulator {
    run_midi_fx_chain(track, events)?
} else {
    events
};
enqueue_final_events(events);
```

Chain runner:

```rust
let events = chain.iter().try_fold(events, |events, stage| {
    let mut next = Vec::new();
    for event in events {
        next.extend(runtime.invoke_midi_fx(stage, event)?);
    }
    Ok(next)
})?;
```

### Cross-Track Routing

If a MIDI FX stage emits to another track with `:track`, the event should enter the target track's MIDI FX processing from the start of that target track's configured chain.

Example:

```lisp
(fx-emit 0 :track 1 :note 12)
```

If track 1 has MIDI FX configured, the routed event enters track 1 as if it were a fresh input event for that track. This makes cross-track routing compositional: target tracks remain responsible for their own MIDI FX sound.

To avoid feedback loops:

- keep a per-event route depth counter
- cap cross-track route depth, for example at `4`
- drop or pass through events that exceed the route depth limit, with debug logging

First implementation can disallow routing back to a track already present in the current route path. Later, explicit feedback-style MIDI routing can be designed separately.

## Live Instrument Triggers

Live playing must use the same MIDI FX path as sequenced playback. If a track has an arpeggiator in its MIDI FX chain, playing that track from the computer keyboard or a MIDI keyboard should produce the arpeggiated output, not bypass the chain.

### Current Path

Today, computer-keyboard note input is routed separately:

```text
UI key press
  -> KeyboardTrigger channel
  -> audio callback
  -> direct voice allocation / trigger
```

That is intentionally low latency, but it bypasses the scheduler and therefore bypasses scheduler-side Lisp MIDI FX. The audio callback also currently resolves some live keyboard behavior directly. That should not be the long-term path for MIDI FX.

### Target Path

Live input should enter the scheduler as timestamped musical input events:

```text
computer keyboard / MIDI keyboard
  -> LiveInputEvent queue
  -> scheduler live input state
  -> visible accumulator slot, if applicable
  -> MIDI FX chain
  -> ScheduledEventQueue
  -> audio callback voice allocation
```

The audio callback remains the final event consumer and DSP executor. It should not run MIDI FX Lisp and should not decide arpeggiator behavior.

### Live Input Events

The UI thread or MIDI input thread should send events such as:

```rust
pub enum LiveInputEventKind {
    NoteOn,
    NoteOff,
}

pub struct LiveInputEvent {
    pub source_id: u64,
    pub track: usize,
    pub transpose: f32,
    pub velocity: f32,
    pub kind: LiveInputEventKind,
    pub received_sample_time: u64,
}
```

Notes:

- `source_id` identifies the physical held note, not the output note after MIDI FX.
- Computer keyboard input can allocate `source_id` from key identity plus a generation counter.
- MIDI keyboard input can allocate `source_id` from device, channel, note number, and generation counter.
- `received_sample_time` lets the scheduler place immediate live output as close as possible to the real input time while still using the scheduler queue.

### Scheduler Live State

The scheduler owns live held-note state per track:

```rust
pub struct LiveHeldNote {
    pub source_id: u64,
    pub transpose: f32,
    pub velocity: f32,
    pub start_sample_time: u64,
}

pub struct LiveTrackState {
    pub held_notes: Vec<LiveHeldNote>,
}
```

This state is used to build note spans for live MIDI FX invocations:

```rust
note_spans = held_notes.map(|note| MidiNoteSpan {
    transpose: note.transpose,
    start_beats: 0.0,
    end_beats: live_horizon_beats,
})
```

For live input, note ends are not known until release. Inside one scheduler lookahead window, held notes can be treated as active through the end of that window. On the next scheduler pass, note spans are rebuilt from the current held-note set.

### Immediate Live MIDI FX

Some MIDI FX are event-local and do not need a running clock:

- transpose
- velocity mapping
- chord expansion
- strum on note-on
- fixed echo/delay from note-on

These can be supported first by invoking the MIDI FX chain when a live `NoteOn` arrives.

For a simple pass-through chain, `NoteOn` produces one scheduled trigger immediately. `NoteOff` releases the corresponding held source note and sends note-off/cancel messages for any output voices that are still held.

This requires the audio/scheduler event model to distinguish live note identity. A live output should carry enough identity to release it later:

```rust
pub struct LiveOutputVoiceKey {
    pub input_source_id: u64,
    pub fx_instance_path: SmallVec<[u64; 4]>,
    pub output_index: u32,
}
```

The exact type can be simpler, but the important rule is that note-off should not guess by transpose alone once MIDI FX can duplicate, transpose, or reorder notes.

### Clocked Live MIDI FX

Arpeggiators are not just note-on transforms. A held live chord needs repeated output while the chord is held, and the held note set can change while the arp is running.

The scheduler should support clocked live chain invocation:

```text
while track has held live notes:
  at each relevant scheduler lookahead pass:
    build current live note spans
    invoke MIDI FX chain over the next lookahead horizon
    enqueue generated events inside that horizon
```

For the first live arpeggiator implementation, the scheduler can invoke the chain once per live lookahead window with:

```rust
MidiFxEvent {
    source: LiveHeldGroup,
    offset_beats: 0.0,
    note_spans: Some(current_held_notes_as_spans),
    live_horizon_beats,
    ...
}
```

Then `fx-arp-count` in live mode should return the number of ticks inside the current live horizon, not infinity. `fx-arp-emit` emits ticks relative to the live horizon start.

Example:

```lisp
(def-midi-fx "live-arp-16"
  (do
    (fx-suppress)
    (for-each |i|
      (fx-arp-emit :16 i :vel 0.8)
      (range 0 (fx-arp-count :16)))))
```

The same Lisp can work for sequenced and live input:

- Sequenced note spans have known end beats from piano-roll durations.
- Live note spans are active through the current scheduler horizon and are recomputed every pass.

### Avoiding Duplicate Live Events

Clocked live FX must not emit the same arp tick on every scheduler pass. The scheduler needs a per-track/per-chain live cursor:

```rust
pub struct LiveMidiFxClockState {
    pub scheduled_until_sample: u64,
}
```

For each scheduler pass:

1. Determine the new live scheduling horizon.
2. Invoke clocked live MIDI FX only for the unscheduled interval.
3. Advance `scheduled_until_sample`.
4. On note set changes, keep the clock phase but rebuild note spans from current held notes.

This gives stable timing while allowing notes to join and leave the running arp.

### Note-Off and Future Event Cancellation

A live note release can happen after some future arp notes are already queued. The first implementation can handle this conservatively:

- Keep the live scheduler lookahead short enough that stale queued arp events are rare.
- On note-off, update the held-note set immediately.
- Future scheduler passes stop emitting released notes.

Longer term, queued scheduled events should carry a live generation or note-set version:

```rust
pub struct LiveEventStamp {
    pub track: usize,
    pub generation: u64,
}
```

The audio callback can drop stale live events whose generation no longer matches the current live state. This avoids needing random removal from the lock-free scheduled event queue.

### Recording

Live recording should remain separate from live monitoring:

- Live monitoring sends `LiveInputEvent`s through the MIDI FX chain.
- Recording writes the user's physical input notes into the piano roll, unless the user explicitly chooses "record MIDI FX output."

Default behavior should record pre-FX input. This makes takes editable and avoids baking arpeggiator output accidentally.

Later, add a mode:

```text
Record: Input | MIDI FX Output
```

### Latency

Moving live input through the scheduler adds a risk of extra latency. The scheduler should support a low-latency live path:

- live input events wake the scheduler immediately
- live note-on events are scheduled at `max(received_sample_time + safety_samples, rendered_sample + minimum_lead_samples)`
- clocked live MIDI FX use a short lookahead horizon

The direct audio-thread keyboard path can remain as a fallback while this is implemented, but once MIDI FX are expected for live input, armed-track live monitoring should route through the scheduler path.

### Error Handling

If a MIDI FX stage errors:

- Log a bounded scheduler debug error.
- Do not crash the scheduler.
- For first implementation, pass through the input event unchanged.
- Later, add a per-stage bypass/error state for UI.

### Output Limits

Each stage needs a hard output limit to prevent runaway scripts.

Suggested defaults:

- max events per stage invocation: `256`
- max events after full chain per input trigger: `1024`

If a limit is exceeded:

- truncate additional events
- set a scheduler debug status
- continue scheduling the truncated output

## Track State and Snapshots

Track params need a separate MIDI FX chain field.

Authoring-side state:

```rust
pub struct TrackParams {
    pub accumulator_idx: AtomicU32,
    pub script_accumulator_name: Mutex<Option<String>>,
    pub midi_fx_chain: Mutex<Vec<String>>,
    pub midi_fx_position: AtomicU32, // pre-accumulator or post-accumulator
}
```

Snapshot-side state:

```rust
pub struct TrackParamsSnapshot {
    pub accumulator_idx: usize,
    pub script_accumulator_name: Option<String>,
    pub midi_fx_chain: Vec<String>,
    pub midi_fx_position: MidiFxPosition,
}
```

The scheduler only reads the snapshot. Lisp assignment helpers mutate authoring state and publish a scheduler snapshot.

Initial Lisp controls:

```lisp
(seq-use-midi-fx 0 "arp-16")
(seq-set-midi-fx-position 0 :post-accumulator)
(seq-set-midi-fx-position 0 :pre-accumulator)
```

## Runtime Registry

The scratch sequencing runtime should own two independent registries:

```rust
registered_accumulators: Vec<RegisteredAccumulator>
registered_midi_fx: Vec<RegisteredMidiFx>
```

`def-accumulator` and `def-midi-fx` should not share a namespace requirement. Name collisions are allowed across registries but not within a registry.

The scheduler reload path should log both registries when `TINYSEQ_DEBUG_ACCUM=1` or a renamed debug env is enabled.

Possible debug rename:

```text
TINYSEQ_DEBUG_SCRIPT_FX=1
```

Keep `TINYSEQ_DEBUG_ACCUM=1` as a compatibility alias while this is in flux.

## Persistence

Project save/load should include per-track MIDI FX chain names.

Initial persistence can store names only:

```json
{
  "track_params": {
    "midi_fx_chain": ["humanize", "arp-16", "delay"],
    "midi_fx_position": "post-accumulator"
  }
}
```

This assumes the scratch source or project source defines those names. Later, named MIDI FX definitions can become saved project assets.

## UI Direction

First implementation is UI-less:

```lisp
(seq-use-midi-fx 0 "arp-16")
```

Later UI:

- separate "MIDI FX" track lane/panel from the accumulator selector
- ordered chain list with bypass/delete/reorder
- per-stage editor/status
- chain can reference builtin MIDI FX and scratch-defined FX

The visible accumulator slot should remain exactly what it is today.

## Builtin Accumulators in Chains

Long term, builtin accumulators could be adapted as MIDI FX stages.

Example future Lisp:

```lisp
(seq-use-midi-fx 0
  (builtin-accumulator "transpose ramp")
  "arp-16"
  "delay")
```

This should not be first implementation work. The first split should preserve builtin accumulators in the existing slot and add a separate script MIDI FX chain after it.

### Current UI-Less Implementation

The scratch-buffer path now supports the first version of this split:

```lisp
(def-midi-fx "arp-16"
  (do
    (fx-suppress)
    (for-each |i|
      (fx-arp-emit :16 i :vel 0.8)
      (range 0 (fx-arp-count :16)))))

(seq-use-midi-fx 0 "arp-16")
```

Implemented scratch functions:

- `def-midi-fx`
- `seq-use-midi-fx`
- `seq-clear-midi-fx`
- `seq-midi-fx-chain`
- `seq-set-midi-fx-position`
- `fx-suppress`, `fx-emit`, `fx-time`, `fx-source-time`
- `fx-note-count`, `fx-note`, `fx-note-start`, `fx-note-end`, `fx-notes`
- `fx-arp-count`, `fx-arp-note`, `fx-arp-emit`
- `fx-state-get`, `fx-state-set`

Current scheduler behavior runs the MIDI FX chain after the visible accumulator slot. `midi_fx_position` is stored and persisted, but the pre-accumulator scheduler path is still future work.

`fx-arp-*` phase is derived from continuous transport beat time and the requested arp timebase. Pattern changes should not reset MIDI FX arp phase; playback start can still establish a fresh transport origin.

Computer-keyboard live input is routed through the scheduler when the target track has a MIDI FX chain. The first live implementation ticks held notes on a 1/16-note grid and advances an arp phase so the scratch arp example rotates through held chord notes. While live notes are held on a playing track, the live tick owns that track's MIDI FX output and combines `currently active sequenced notes + live held notes` into one note pool. Future pre-expanded sequenced MIDI FX output is flushed on live note-on/off so the sequenced arp and live arp do not run as separate processors. Tracks without a MIDI FX chain keep the existing direct audio callback path. External MIDI input and a configurable live tick source are still future work and are covered below.

## Implementation Plan

### Phase 1: Split Data Model

- Add `midi_fx_chain` to track params and snapshots.
- Add `midi_fx_position` to track params and snapshots.
- Add `seq-use-midi-fx`, `seq-clear-midi-fx`, and `seq-midi-fx-chain` scratch natives.
- Add `seq-set-midi-fx-position`.
- Add a `RegisteredMidiFx` registry beside registered accumulators.
- Keep old accumulator-based MIDI FX working during transition.

### Phase 2: Shared Event Context

- Rename or extract accumulator event structs into neutral MIDI FX structs.
- Move `AccumulatorNoteSpan` to a neutral type such as `MidiNoteSpan`.
- Implement `fx-*` helpers.
- Keep `acc-*` helpers as aliases or wrappers where practical.

### Phase 3: Scheduler Chain

- Build initial `MidiFxEvent`s before or after the visible accumulator slot according to `midi_fx_position`.
- Evaluate the per-track MIDI FX chain in order.
- Route cross-track emitted events into the target track's chain from the start.
- Enqueue final events.
- Add output limits and error behavior.

### Phase 4: Docs and Examples

- Rewrite `docs/lisp-midi-fx.md` around `def-midi-fx`.
- Keep a short migration note explaining that the accumulator prototype was the first implementation.
- Add examples for arp, strum, delay, humanize, and note gate.

### Phase 5: UI

- Add a MIDI FX chain panel after the scratch-driven path feels stable.
- Keep accumulator UI separate.

## Open Questions

- Should note spans be recomputed after a stage retargets `:track` or `:step`, or should spans be frozen from the original event?
- Should chain definitions eventually be first-class saved project assets instead of scratch source only?
