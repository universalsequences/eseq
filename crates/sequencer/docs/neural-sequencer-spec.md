# Neural Sequencer Spec

## Goal

Add a neural-sequencer layer that sits alongside the step sequencer. The step sequencer keeps its current role as the deterministic, punch-in surface. The neural layer takes step events as seeds, propagates them through a weighted neuron matrix, and emits additional delayed, possibly transposed events back into the audio pipeline. The two layers compose so that any track can be:

- pure step sequencer (matrix empty / track not routed) — identical behavior to today
- pure neural (matrix seeded once at pattern start, no step events) — generative
- hybrid (steps seed the matrix and play directly; the matrix expands them) — the primary intended mode

The neural layer is a pure transformation over a value-shaped event type. Today's pipeline emits pointer-shaped triggers and resolves them late; this spec also covers the refactor needed to make the neural layer cleanly insertable, and to fix a class of long-standing p-lock-by-index bugs along the way.

## Non-Goals (v1)

- Learning / training weights from data. Weights are user-authored.
- Per-neuron parameter overrides beyond delay, threshold, transpose, quantization, dampening, route.
- Per-neuron chord-mode (root vs full chord vs voicing extract). Chords always propagate intact.
- Audio-rate neural propagation. The neural layer ticks on the sequencer clock (one tick = one step at the finest enabled timebase).
- Pattern morphing / interpolation. Patterns swap atomically; cross-pattern weight crossfades are future work.
- Multi-network composition. There is exactly one neural network per pattern in v1.
- UI for editing the weight matrix. v1 ships with a data model + audio-side implementation; editing UI is a follow-up.

## Concepts

### Neuron

A neuron is a member of a fixed-size pool (`NUM_NEURONS = 16` in v1, configurable in storage). Each neuron has:

- `weight[N]`: outgoing weights, one per other neuron. Row in the NxN matrix.
- `threshold: f32` — energy level at which the neuron fires.
- `delay: u32` — ticks between fire-resolution and "set my last_trigger=1" (the propagation step).
- `transpose: f32` — semitone offset applied to events when they fire from this neuron.
- `quantize: Quantization` — optional grid snap. When a neuron's threshold is met, the fire event is deferred to the next quantization boundary before its `delay` countdown begins.
- `dampening: f32` — multiplier (0..1) applied to subsequent triggers after firing; recovers over time. Models "neuron fatigue."
- `route: Option<TrackId>` — destination track. `None` = hidden neuron (participates in matrix propagation, no audio output).

A track can be the route of multiple neurons (many-to-one). A neuron can have at most one route (one-to-one in the other direction). This asymmetry is the key compositional primitive: one track can host multiple neural "voices" each with its own transpose / delay / quantize.

### Matrix state

Per-tick state vectors over the neuron pool:

- `last_triggers: [f32; N]` — most recent fire indicator (0 or 1; floats to allow future fractional triggers).
- `energy: [f32; N]` — accumulated incoming weight contributions.
- `pending_delays: [u32; N]` — countdown until a fired neuron's propagation lands.
- `dampening_level: [f32; N]` — current fatigue per neuron (0 = fully recovered, 1 = fully suppressed).

### Tick semantics

On each sequencer tick at the network's chosen base resolution:

1. `incoming = weights · last_triggers` (NxN · Nx1 matmul).
2. `energy += incoming * (1 - dampening_level)`.
3. For each neuron where `energy[i] >= threshold[i]`:
   - Build a `StepEvent` (see "StepEvent" below) from this firing.
   - If `quantize[i].is_some()`, defer the fire to the next quantization boundary; once reached, continue.
   - Schedule the propagation: `pending_delays[i] = delay[i]`. The propagation itself sets `last_triggers[i] = 1` after that countdown elapses.
   - If `route[i].is_some()`, the audio event is emitted at the *moment of firing* (after quantization), not at propagation time. (The user's mental model: "trigger sounds when the neuron fires; the matrix sees it later.")
   - Apply dampening: `dampening_level[i] = min(1.0, dampening_level[i] + neuron.dampening_amount)`.
   - Zero `energy[i]`.
4. Decrement `pending_delays`; where a counter hits zero, set `last_triggers[i] = 1`. Other entries clear to `0`.
5. Dampening recovers: `dampening_level *= dampening_recovery_factor` (per-network constant, e.g. 0.98 per tick).

### Reset / seeding

The matrix has a reset interval (configurable, default: pattern length in bars). At each reset boundary:

- `energy`, `pending_delays`, `last_triggers`, `dampening_level` are zeroed.
- Optionally, a seed vector is loaded into `last_triggers` (e.g. neuron 0 = 1) to kick off generation in pure-neural mode.

A second reset mechanism: any track step with `aux_a = clear-state-sentinel` (TBD, likely max value) triggers a reset when fired. This lets the step sequencer program higher-order behavior — "every 8 bars on the kick, restart the network."

### Step → Network seeding

When a step fires on a track, every neuron with `route == Some(that_track)` has its `last_triggers` set to 1 simultaneously. This is the fan-out described in earlier design discussion: a punch-in on the kick seeds *all* neurons routed to the kick at once, each of which then propagates independently with its own delay/transpose/weights.

The seed step **also plays directly** at its scheduled time — it does not wait for the network. The network only ever *adds* events; it never delays or suppresses step-driven hits. This is the "parallel mode" decision: the matrix is purely additive.

### Velocity accumulation

When multiple neurons routed to the same track fire on the same audio sample, the resulting hits **accumulate velocity** into a single hit rather than producing N stacked note-ons. The semantic: same-tick coincidence is treated as accent, not polyphony.

Pseudocode for the merge:

```
collect all StepEvents at sample S for track T
if multiple events, merge into one:
  velocity = min(1.0, sum(velocities))
  transpose = min-by-fire-order (or: pick the lowest-indexed neuron's transpose)
  duration, pan, aux, chop, sync, chord = from the lowest-indexed firing neuron
```

Different transposes on the same sample do **not** produce chords — that path goes through actual chord data on the seed step. This keeps the audio thread simple and avoids accidental polyphony from runaway matrix coincidence.

### Chord propagation

If a seed step has a chord attached (via existing `ChordData`), the chord travels intact through the network as part of the propagated `StepEvent`. Per-neuron transpose shifts the whole chord uniformly. There is no per-neuron chord mode in v1; the chord is part of the event payload, not a network-modifiable field.

### Note model

Notes remain transpose-deltas against the destination instrument's base note. Per-neuron transpose composes additively with step transpose:

```
final_transpose = step_transpose + neuron_transpose
```

There is no absolute-note resolution at the network layer. The instrument applies its base note as it does today.

## Required Refactor: StepEvent

The current event pipeline is pointer-shaped:

```
Trigger { track, step, offset, cycle_start_beats }   // data.rs:1029
   ↓
late resolve via ResolvedStep::from_step_data         // accumulator.rs:19
   ↓
ScheduledEvent { kind: ResolvedTrigger { track, step, resolved, ... } }  // scheduled_event.rs:42
```

For the neural layer, propagation must operate on value-shaped events, because:

- A network-emitted event has no source step (hidden neurons, transposed copies, events scheduled N ticks after the seed step was edited).
- The step a propagation came from may be edited or deleted by the time the delayed fire lands.

### New type

```rust
pub struct StepEvent {
    pub track: usize,
    pub resolved: ResolvedStep,           // existing struct, used as the value payload
    pub chord: Option<ChordData>,         // chord travels intact through propagation
    pub instrument_plocks: SmallVec<[ParamOverride; 4]>,
    pub effect_plocks: SmallVec<[EffectParamOverride; 4]>,
    pub source: EventSource,
}

pub enum EventSource {
    Step {
        track: usize,
        step: usize,
        instrument_fingerprint: u64,
    },
    Network {
        seed: Option<(usize, usize)>,     // (track, step) of the original seed, if any
        neuron: usize,
        instrument_fingerprint: u64,      // captured at seed time
    },
}

pub struct ParamOverride {
    pub param_node: ParamNodeId,          // stable identity, not index
    pub param_index: usize,               // cache; validated against identity at apply time
    pub value: f32,
}
```

### Pipeline shape

```
Trigger (pointer)
   ↓ resolve at scheduling time
StepEvent (value, p-locks baked in by identity)
   ↓ neural layer: in → 0..N out (pure function over StepEvent)
ScheduledEvent (one event = one note-on)
```

The neural layer is a pure function `StepEvent → Vec<StepEvent>`. When the matrix is empty (no neurons configured, or no neurons routed to this event's track), the function is identity. This preserves byte-for-byte behavior for projects without a configured network.

### ScheduledEventKind change

Add a sibling variant rather than overloading `step: usize` with a sentinel:

```rust
pub enum ScheduledEventKind {
    ResolvedTrigger { /* unchanged */ },
    NetworkTrigger {
        track: usize,
        source_neuron: usize,
        seed: Option<(usize, usize)>,
        resolved: ResolvedStep,
        chord: ScheduledChordData,
        effect_params: Vec<ScheduledEffectParam>,
        instrument_params: ScheduledInstrumentParams,
        instrument_fingerprint: u64,
    },
    InstrumentParams { /* unchanged */ },
}
```

`NetworkTrigger` carries no `step` field. The compiler enforces that step-indexed p-lock paths (e.g. `resolve_instrument_plocks` at scheduler.rs:392) cannot be invoked on a network event — those p-locks must have already been baked into `resolved` / `effect_params` / `instrument_params` by the neural layer at seed time.

## Required Refactor: P-locks by Identity, Not Index

The neural layer is the trigger for this refactor, but the underlying bug is pre-existing and has caused issues in non-neural contexts: p-locks are stored and resolved by parameter *index*, which silently breaks when:

- the instrument is swapped on a track (param at index 7 changes meaning)
- a new param is inserted into an instrument's param list (all subsequent indices shift)
- an event from one track is applied to another (p-locks reference a different instrument's param list)

The cross-track risk is what the neural layer exposes most aggressively, because propagation routinely crosses tracks (neuron 1 → kick, neuron 5 → snare).

### Rule

P-locks are values addressed by **identity**, not by position. The index is a cache, not the source of truth.

### Mechanism

1. Every parameter (instrument param, effect param) has a stable `ParamNodeId` derived from a hash of `(instrument_id, param_path)` or equivalent stable identifier. This ID survives reordering, renaming follows a versioned mapping.
2. P-lock storage continues to be index-keyed for fast atomic access (`AtomicU32` per step×param), but each stored p-lock also persists its `ParamNodeId` in a parallel structure used for resolution and serialization.
3. At resolution time (`resolve_instrument_plocks` and equivalents), each p-lock value is gated by `ParamNodeId` match against the current destination instrument's param at that index:
   - Match → apply.
   - Mismatch → drop, log once at debug level, and (in editor) surface a "stale p-lock" warning on the affected step.
4. On instrument swap: existing index-keyed p-lock values become "stale"; they remain in storage but resolve to `None` until the user explicitly migrates or clears them.

### Scope of the gate

The gate applies on every path that resolves a p-lock to a runtime value:

- `resolve_instrument_plocks` (scheduler.rs:392)
- effect param p-lock resolution (locations under `effects.rs` p-lock paths)
- `TimebasePLockData` and `SwingPLockData` — track-level p-locks, validated against the track itself (not param identity, but track identity matches the step's source track)
- `ScheduledEventKind::InstrumentParams` (scheduled_event.rs:53) — audit pass to apply the same identity gating

### What propagates through the neural layer

When a `StepEvent` is constructed at seed time, p-locks are baked into the event by **resolved value**, keyed by `ParamNodeId`. Propagation then has three cases:

| Case                                      | Step-level params | Instrument p-locks | Effect p-locks  |
|-------------------------------------------|-------------------|--------------------|-----------------|
| Same-track propagation (route = seed track) | carry             | carry, re-validate | carry, re-validate |
| Cross-track propagation                   | carry             | **drop**           | **drop**         |
| Hidden neuron → (never reaches audio)     | irrelevant        | irrelevant         | irrelevant       |

"Carry, re-validate" means the p-lock travels with the event, but on apply the destination's `ParamNodeId` must still match. The seed-time fingerprint may have gone stale if the user swapped the instrument mid-propagation.

Step-level params (velocity, duration, transpose, pan, aux_a/b, speed, chop, sync) are universal — they always carry, regardless of destination, because their meaning is track-agnostic.

## Required Refactor: aux_a Wiring Audit

`aux_a` is already consumed (e.g. accumulators). The `#[allow(dead_code)]` annotation in `ResolvedStep` (accumulator.rs:11) is a compiler quirk. Before the neural layer introduces a new aux_a consumer (the reset-state mechanism), audit all consumers to confirm:

- `aux_a` reaches every place a `ResolvedStep` is used as a final event.
- The "reset state" sentinel value chosen for the neural network does not collide with existing aux_a semantics on any track type.
- The clear-state behavior is opt-in per-track (a track preference), not a global aux_a interpretation.

## Data Model

### Patterns own the network

The neural network is **per-pattern**, not per-project. In practice the weight matrix *is* the pattern — it's the main editing surface that shapes generated output, alongside the per-neuron parameters. Pattern slots already own step data, track params, p-lock snapshots, and chord snapshots (`ProjectPattern` at project.rs:190); the neural network config slots into the same container.

Switching the active pattern atomically swaps:
- step sequencer state (existing behavior)
- weight matrix
- per-neuron config (threshold, delay, transpose, quantize, dampening, route)
- reset interval, base resolution, seed-on-reset vector

This lets the user write a network for the verse pattern and a completely different network for the chorus pattern, switched in the same gesture they use to switch step patterns today.

### Runtime state on pattern switch

Pattern switching is the audio thread's hook to also reset runtime state — `energy`, `last_triggers`, `pending_delays`, `dampening_level`, `quantize_pending`. Without this, the old pattern's accumulated energy bleeds into the new pattern with the new pattern's (possibly very different) weights, producing chaotic transients.

Default behavior: **reset runtime state on pattern switch**. The seed vector for the *new* pattern is loaded into `last_triggers` immediately so the new pattern starts cleanly.

A future opt-in flag (`carry_state_on_switch`, per-pattern) can keep runtime state alive across switches for the "groove continues, weights morph" use case, but that's not v1.

### Project storage

The new struct lives inside `ProjectPattern`:

```rust
pub struct ProjectPattern {
    // ... existing fields (step_data, track_params, plocks, chords, ...)
    #[serde(default)]
    pub neural_network: Option<ProjectNeuralNetwork>,
}

pub struct ProjectNeuralNetwork {
    pub num_neurons: usize,                       // 16 in v1
    pub weights: Vec<Vec<f32>>,                   // NxN
    pub neurons: Vec<ProjectNeuron>,              // length = num_neurons
    pub reset_interval_bars: f32,
    pub base_resolution: Timebase,
    pub seed_on_reset: Vec<f32>,                  // length = num_neurons
}

pub struct ProjectNeuron {
    pub threshold: f32,
    pub delay_ticks: u32,
    pub transpose: f32,
    pub quantize: Option<Timebase>,
    pub dampening_amount: f32,
    pub dampening_recovery: f32,
    pub route: Option<usize>,                     // track index, None = hidden
}
```

`neural_network` is `Option<...>` and `#[serde(default)]` so older projects (and patterns where the user hasn't configured a network) load cleanly with no network — preserving the empty-network passthrough behavior.

### Runtime state

Owned by the audio thread, allocated once at project load (no allocation during processing). The runtime struct is **separate** from the per-pattern config: pattern switching swaps config pointers and resets the runtime state, but the runtime struct's buffers are reused.

```rust
pub struct NeuralNetwork {
    weights: [[f32; N]; N],
    neurons: [NeuronConfig; N],
    last_triggers: [f32; N],
    energy: [f32; N],
    pending_delays: [u32; N],
    quantize_pending: [Option<u64>; N],  // sample of next grid boundary
    dampening_level: [f32; N],
    reset_interval_ticks: u32,
    ticks_since_reset: u32,
    base_resolution: Timebase,
}
```

`N` is a compile-time constant in v1 (16). Growth to runtime-sized requires a separate allocation strategy and is out of scope.

## Pipeline Integration Point

The neural layer slots into the scheduler:

```
scheduler::process_block:
  for each Trigger emitted by the clock:
    let step_event = resolve_to_step_event(trigger, snapshot);  // new
    let events = neural_network.process(step_event);            // new; identity if empty
    for event in events:
      schedule(event);
  for each (sample, neuron) the network would fire autonomously this block:
    let step_event = neural_network.synthesize_event(neuron);
    schedule(step_event);
```

Two emission sources merge into the same scheduled-event stream:
1. **Step-seeded events** — a step fires, the network may emit additional events (delayed, transposed) in addition to the seed.
2. **Autonomously generated events** — neurons whose energy crossed threshold this block fire of their own accord (only possible if seeded, directly or transitively, at some prior tick).

Both paths end at `ScheduledEvent`. The clock and the network share the same sample-clock so their timing aligns.

## Behavior in Empty / Default State

A project with no configured neural network behaves identically to today:

- No neurons exist → `process(step_event)` is identity.
- No state is allocated for the network beyond the empty struct.
- No scheduling overhead is added per trigger.

This property is load-bearing: the refactor itself must be neutral, and the neural feature must be opt-in per project.

## Open Questions

1. **Velocity accumulation cap** — `min(1.0, sum(velocities))` clamps hard; should there be a soft compression curve? Defer to v1 testing; clamp is fine to start.
2. **Quantize grid for neuron**: project-relative or track-relative? Lean project-relative (transport grid) for consistency.
3. **Multiple seed tracks in one tick**: if step events on track A and track B both seed neurons N1 (A) and N3 (B) on the same sample, do their propagations interleave deterministically? Determinism comes for free if seeding is ordered by track index.
4. **Reset on aux_a sentinel — visibility**: when a step is "the reset-state step," does the UI show that distinctly? Probably yes — it's load-bearing for a song's structure.
5. **Per-neuron mute/solo for editing**: not in v1, but worth keeping data model future-compatible (one bit per neuron in storage).

## Implementation Phases

The phases below order the work so that each is independently shippable and the audio pipeline is never broken.

### Phase 1: P-lock identity refactor (no new features)
- Add `ParamNodeId` to params.
- Persist `ParamNodeId` alongside index in p-lock storage.
- Gate all p-lock resolution paths by identity.
- Audit `aux_a` end-to-end.
- Stale p-lock UI surface.

### Phase 2: StepEvent + ScheduledEventKind::NetworkTrigger
- Introduce `StepEvent` as the intermediate value type.
- Reshape scheduler to pass through `StepEvent`.
- Add `NetworkTrigger` variant (unused for now).
- Verify identity behavior — no neural code yet.

### Phase 3: Neural network audio-thread implementation
- `NeuralNetwork` struct, tick loop, matrix math, dampening, quantization, delays.
- Integration into scheduler at the documented hook point.
- Empty-network passthrough verified.

### Phase 4: Pattern integration + storage
- `ProjectNeuralNetwork` lives in `ProjectPattern`.
- Pattern switch swaps active network config and resets runtime state.
- Seed-on-reset for the incoming pattern loads immediately on switch.
- Reset-on-aux_a wiring.
- JSON schema, load/save, backward-compat (`Option` + `#[serde(default)]`).

### Phase 5: Editor UI (out of spec, follow-up)
- Matrix editor, per-neuron config panel, route picker, real-time fire visualization.

## Out of Scope, Logged for Future

- Learned weights / weight evolution from playing patterns.
- Per-neuron chord-mode (root / full / voicing extract).
- Network-as-track (a neural network as a routable instrument with its own track lane).
- Multiple networks per pattern.
- Cross-pattern weight morphing / interpolation.
- `carry_state_on_switch` flag — preserve runtime state across pattern switches.
- Audio-rate (per-sample) propagation.
- MIDI export of network-generated events as a flattened pattern.
