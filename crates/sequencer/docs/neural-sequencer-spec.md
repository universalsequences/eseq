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
- Audio-rate neural propagation. The neural layer advances on a *neural clock* derived from the transport (see "Neural clock" below), not per audio sample.
- Pattern morphing / interpolation. Patterns swap atomically; cross-pattern weight crossfades are future work.
- Multi-network composition. There is exactly one neural network per pattern in v1.
- UI for editing the weight matrix. v1 ships with a data model + audio-side implementation; editing UI is a follow-up.

## Concepts

### Neural clock (there are no "ticks" in this engine)

> **Grounding note.** The engine has **no tick unit**. Time is samples and `f64` quarter-note beats. The scheduler (`scheduler.rs`, `SnapshotSequencerClock::process_chunk`) advances a per-sample `total_beats: f64` and emits a `SnapshotTrigger` (scheduler.rs:40) when `total_beats` crosses a step boundary, computed from a per-track/per-step `Timebase` (data.rs:91, an enum of musical divisions — `Sixteenth`, `EighthTriplet`, `Polyrhythm`, etc.). "One tick = the finest enabled timebase" does not map onto anything real, because timebase is per-track and per-step (`StepParam::Sync`, plus `timebase_plock_snapshots` / `swing_*` snapshots in `ProjectPattern`).

The neural layer defines its **own** clock:

- The network owns a single `base_resolution: Timebase`. A **neural step** is one crossing of `total_beats` past `k * Timebase::step_beats()` for that resolution — detected inside `process_chunk` using the same boundary-crossing logic the step sequencer already uses (`derive_local_step`), including the cases where a block contains zero, one, or several boundaries, and where a boundary falls partway through a block.
- Everywhere this spec previously said "tick," read **neural step**. The neuron `delay` field counts neural steps; `reset_interval` counts neural steps (derived from bars); `ticks_since_reset` becomes `steps_since_reset`.
- Quantization boundaries, by contrast, are expressed in **samples** (`quantize_pending: [Option<u64>; N]`), because audio emission timing is sample-accurate. The neural clock (neural steps) and the quantize grid (samples) are two different units; see the quantize ordering note under Open Questions.

This keeps the neural layer aligned to the transport without inventing a unit the rest of the engine doesn't have.

### Neuron

A neuron is a member of a fixed-size pool (`NUM_NEURONS = 16` in v1, configurable in storage). Each neuron has:

- `weight[N]`: outgoing weights, one per other neuron. Row in the NxN matrix.
- `threshold: f32` — energy level at which the neuron fires.
- `delay: u32` — neural steps between fire-resolution and "set my last_trigger=1" (the propagation step). See "Neural clock."
- `transpose: f32` — semitone offset applied to events when they fire from this neuron.
- `quantize: Quantization` — optional grid snap. When a neuron's threshold is met, the fire event is deferred to the next quantization boundary before its `delay` countdown begins.
- `dampening: f32` — multiplier (0..1) applied to subsequent triggers after firing; recovers over time. Models "neuron fatigue."
- `route: Option<TrackId>` — destination track. `None` = hidden neuron (participates in matrix propagation, no audio output).

A track can be the route of multiple neurons (many-to-one). A neuron can have at most one route (one-to-one in the other direction). This asymmetry is the key compositional primitive: one track can host multiple neural "voices" each with its own transpose / delay / quantize.

### Matrix state

Per-neural-step state vectors over the neuron pool:

- `last_triggers: [f32; N]` — most recent fire indicator (0 or 1; floats to allow future fractional triggers).
- `energy: [f32; N]` — accumulated incoming weight contributions.
- `pending_delays: [u32; N]` — countdown (in neural steps) until a fired neuron's propagation lands.
- `dampening_level: [f32; N]` — current fatigue per neuron (0 = fully recovered, 1 = fully suppressed).

### Step semantics

On each **neural step** (one boundary crossing at the network's `base_resolution`; see "Neural clock"):

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
5. Dampening recovers: `dampening_level *= dampening_recovery_factor` (per-network constant, e.g. 0.98 per neural step).

### Reset / seeding

The matrix has a reset interval (configurable, default: pattern length in bars). At each reset boundary:

- `energy`, `pending_delays`, `last_triggers`, `dampening_level` are zeroed.
- Optionally, a seed vector is loaded into `last_triggers` (e.g. neuron 0 = 1) to kick off generation in pure-neural mode.

A second reset mechanism lets the step sequencer program higher-order behavior — "every 8 bars on the kick, restart the network." **This must not be wired through `aux_a`.** See "Reset trigger from a step (NOT via aux_a)" under the refactor section: `aux_a`/`aux_b` are genuinely unread today, `AuxA` carries a legitimate authored range (`0..16`), so a sentinel value would collide with real data. Use a dedicated per-step reset flag instead.

### Step → Network seeding

When a step fires on a track, every neuron with `route == Some(that_track)` has its `last_triggers` set to 1 simultaneously. This is the fan-out described in earlier design discussion: a punch-in on the kick seeds *all* neurons routed to the kick at once, each of which then propagates independently with its own delay/transpose/weights.

The seed step **also plays directly** at its scheduled time — it does not wait for the network. The network only ever *adds* events; it never delays or suppresses step-driven hits. This is the "parallel mode" decision: the matrix is purely additive.

### Velocity accumulation

When multiple neurons routed to the same track fire on the same audio sample, the resulting hits **accumulate velocity** into a single hit rather than producing N stacked note-ons. The semantic: same-sample coincidence is treated as accent, not polyphony.

Pseudocode for the merge:

```
collect all StepEvents at sample S for track T
if multiple events, merge into one:
  velocity = min(1.0, sum(velocities))
  transpose = lowest-indexed firing neuron's transpose
  duration, speed, pan, aux_a, aux_b, chop, chord = from the lowest-indexed firing neuron
```

The merge fields are exactly the `ResolvedStep` fields (`duration, velocity, speed, aux_a, aux_b, transpose, pan, chop` — accumulator.rs:5) plus the chord. `Sync` and `Delay` (`StepParam::Sync=8`, `Delay=9`) are **not** part of `ResolvedStep` and therefore do not participate in the merge — sync drives per-step timebase selection upstream, not the resolved event. "Lowest-indexed firing neuron" is well-defined because step 3 iterates neurons `0..N` in order, so fire order is deterministic; the merge picks the first firing neuron in that iteration.

> Two things still need pinning down — see Open Questions: (a) *where* this merge runs (a post-scheduling pass over the block's events vs. at voice-trigger time in `audio.rs`), and (b) the soft-vs-hard velocity cap.

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

The current event pipeline is pointer-shaped. The active path uses `SnapshotTrigger`, not the (effectively dead) `Trigger` at data.rs:1096:

```
SnapshotTrigger { track, step, offset, cycle_start_beats, absolute_beats, samples_per_step }  // scheduler.rs:40
   ↓
resolve: ResolvedStep::from_step_data(step_data, step)  // accumulator.rs:18
   ↓
ScheduledEvent { kind: ResolvedTrigger { track, step, samples_per_step, resolved,
                 chord: ScheduledChordData, effect_params, instrument_params,
                 instrument_fingerprint } }            // scheduled_event.rs:42
```

For the neural layer, propagation must operate on value-shaped events, because:

- A network-emitted event has no source step (hidden neurons, transposed copies, events scheduled N neural steps after the seed step was edited).
- The step a propagation came from may be edited or deleted by the time the delayed fire lands.

### New type

```rust
pub struct StepEvent {
    pub track: usize,
    pub resolved: ResolvedStep,           // existing struct (accumulator.rs:5), the value payload
    pub chord: ScheduledChordData,        // VALUE form (scheduled_event.rs:11) — NOT ChordData.
                                          // ChordData (data.rs:900) is a non-Clone atomic struct;
                                          // ScheduledChordData with count=0 is the "no chord" case.
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
    pub param_node: ParamNodeId,          // stable identity, not index (see P-lock refactor)
    pub param_index: usize,               // cache; validated against identity at apply time
    pub value: f32,
}
```

> **`ParamNodeId` should reuse the identity the engine already has, not invent a hash.** The runtime already addresses params by a node-relative stable index: `resolve_instrument_plocks` (scheduler.rs:409) maps a p-lock's `param_idx` to `EffectSlotState::param_node_indices[param_idx]` (effects.rs) ← `ParamDescriptor::node_param_idx`, then resolves to a `(logical_id = node_id, node_param_idx)` pair (scheduler.rs:317-335, with `MOD_PARAM_BASE` separating modulator targets). Define `ParamNodeId` as that existing `(node_id, node_param_idx)` identity rather than `hash(instrument_id, param_path)`. The gate then becomes "does the destination slot's current `param_node_indices[param_index]` still equal the stored `node_param_idx`?" — a far smaller, already-half-built change.

### Pipeline shape

```
Trigger (pointer)
   ↓ resolve at scheduling time
StepEvent (value, p-locks baked in by identity)
   ↓ neural layer: in → 0..N out (pure function over StepEvent)
ScheduledEvent (one event = one note-on)
```

The neural layer is a pure function `StepEvent → 0..N StepEvents`. When the matrix is empty (no neurons configured, or no neurons routed to this event's track), it is identity. This preserves byte-for-byte behavior for projects without a configured network.

> **No allocation on the audio thread.** Write the signature as `process(&mut self, input: StepEvent, out: &mut SmallVec<[StepEvent; K]>)` (push into a caller-owned, pre-allocated buffer), not `-> Vec<StepEvent>`. The hot path today is allocation-free (atomic reads + an `Arc::clone` of the snapshot); returning a fresh `Vec` per trigger would allocate on the realtime thread and contradicts the "no allocation during processing" rule in Runtime state. The same applies to `synthesize_event` and the velocity merge — reuse scratch buffers owned by the runtime struct.

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

`NetworkTrigger` carries no `step` field. The compiler enforces that step-indexed p-lock paths (`resolve_instrument_plocks` at scheduler.rs:409) cannot be invoked on a network event — those p-locks must have already been baked into `resolved` / `effect_params` / `instrument_params` by the neural layer at seed time. Note the field types mirror the existing `ResolvedTrigger` variant (scheduled_event.rs:42): `chord: ScheduledChordData`, `effect_params: Vec<ScheduledEffectParam>`, `instrument_params: ScheduledInstrumentParams`.

## Required Refactor: P-locks by Identity, Not Index

The neural layer is the trigger for this refactor, but the underlying bug is pre-existing and has caused issues in non-neural contexts: p-locks are stored and resolved by parameter *index*, which silently breaks when:

- the instrument is swapped on a track (param at index 7 changes meaning)
- a new param is inserted into an instrument's param list (all subsequent indices shift)
- an event from one track is applied to another (p-locks reference a different instrument's param list)

The cross-track risk is what the neural layer exposes most aggressively, because propagation routinely crosses tracks (neuron 1 → kick, neuron 5 → snare).

### Rule

P-locks are values addressed by **identity**, not by position. The index is a cache, not the source of truth.

### Mechanism

1. Every parameter (instrument param, effect param) has a stable `ParamNodeId`. **Reuse the engine's existing identity** — the `(node_id, node_param_idx)` pair already produced by `param_node_indices` / `ParamDescriptor::node_param_idx` (see the note under `ParamOverride`) — rather than introducing a new `hash(instrument_id, param_path)`. This ID survives p-lock-index reordering because it is node-relative, not position-relative.
2. P-lock storage continues to be index-keyed for fast atomic access (`AtomicU32` per step×param), but each stored p-lock also persists its `ParamNodeId` in a parallel structure used for resolution and serialization.
3. At resolution time (`resolve_instrument_plocks` and equivalents), each p-lock value is gated by `ParamNodeId` match against the current destination instrument's param at that index:
   - Match → apply.
   - Mismatch → drop, log once at debug level, and (in editor) surface a "stale p-lock" warning on the affected step.
4. On instrument swap: existing index-keyed p-lock values become "stale"; they remain in storage but resolve to `None` until the user explicitly migrates or clears them.

### Scope of the gate

The gate applies on every path that resolves a p-lock to a runtime value:

- `resolve_instrument_plocks` (scheduler.rs:409)
- effect param p-lock resolution (`resolve_effect_params`, scheduler.rs ~317-342; storage `SlotPLockData` in effects.rs, indexed `step * max_params + param_idx`)
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

## Reset trigger from a step (NOT via aux_a)

The original draft proposed encoding the network-reset signal as an `aux_a` sentinel value. **Don't.** Verified against the code:

- `aux_a`/`aux_b` are *not* read in voice synthesis today. The `#[allow(dead_code)]` on `ResolvedStep::aux_a`/`aux_b` (accumulator.rs:9,11) is **load-bearing**, not a cosmetic quirk — the fields are genuinely unconsumed in the final-event path. The earlier claim that "aux_a is already consumed (e.g. accumulators)" is false as written.
- `StepParam::AuxA` (data.rs:253) carries a legitimate authored range (`0..16`). A "sentinel = max value" reset signal would collide with real user data on any track that uses aux_a for its intended purpose.

Instead, the reset signal is a **dedicated per-step flag**, independent of the step's parameter values:

- Add a per-step reset bit. The pattern already stores per-step bitfields (`ProjectPattern.track_bits: Vec<[u64; TRACK_PATTERN_WORDS]>`, project.rs:190) and the data model already anticipates spare per-neuron bits (see Open Question 5) — a parallel per-step reset mask fits the same shape.
- Behavior is **opt-in per track** (a track preference), not a global interpretation of any parameter.
- When a step with the reset flag fires, the network performs the same zero-and-reseed as a reset-interval boundary.

If `ResolvedStep` is later extended to actually carry `aux_a`/`aux_b` to voices, that is a *separate* change with its own audit — it must not be conflated with the network reset mechanism.

## Data Model

### Patterns own the network

The neural network is **per-pattern**, not per-project. In practice the weight matrix *is* the pattern — it's the main editing surface that shapes generated output, alongside the per-neuron parameters. Pattern slots already own step data, track params, p-lock snapshots, chord snapshots, mod connections, and bus patterns (`ProjectPattern` at project.rs:190); the neural network config slots into the same container as one more `#[serde(default)]` field.

Switching the active pattern atomically swaps:
- step sequencer state (existing behavior)
- weight matrix
- per-neuron config (threshold, delay, transpose, quantize, dampening, route)
- reset interval, base resolution, seed-on-reset vector

This lets the user write a network for the verse pattern and a completely different network for the chorus pattern, switched in the same gesture they use to switch step patterns today.

### Runtime state on pattern switch

How switching works today (verified): `switch_pattern` (sequencer/state.rs ~1685) locks the `pattern_bank: Mutex<Vec<PatternSnapshot>>`, captures live state into the outgoing slot, restores the incoming slot, stores `current_pattern` (`AtomicU32`), increments `pattern_epoch` (`AtomicU64`), and calls `publish_scheduler_snapshot()` (Arc-swap + `scheduler_snapshot_version.fetch_add`). The audio thread reads the new state via `latest_scheduler_snapshot()` and drops any in-flight `ScheduledEvent` whose `pattern_epoch` no longer matches (audio.rs epoch check). So "atomic swap" in this spec means *that* mutex + epoch + publish sequence, not a single atomic store.

The neural config swap rides the same path. Pattern switching is also the hook to reset neural runtime state — `energy`, `last_triggers`, `pending_delays`, `dampening_level`, `quantize_pending`, `steps_since_reset`. Without this, the old pattern's accumulated energy bleeds into the new pattern with (possibly very different) weights, producing chaotic transients.

Default behavior: **reset runtime state on pattern switch**, performed on the audio thread when it observes the `pattern_epoch` change (the same signal that invalidates stale events). The seed vector for the *new* pattern is loaded into `last_triggers` immediately so the new pattern starts cleanly.

**Future neural events and the epoch.** A neuron with `delay > 0` (or a quantize defer) schedules a `NetworkTrigger` into the future. At a pattern switch, the epoch bump means any already-scheduled neural event from the old pattern is dropped by the existing epoch check — which is exactly the desired "reset on switch" behavior. For this to work, `ScheduledEvent.pattern_epoch` (scheduled_event.rs:60) must be stamped on `NetworkTrigger` events the same way it is on `ResolvedTrigger`. The `NetworkTrigger` variant itself carries no epoch field; it inherits the outer `ScheduledEvent.pattern_epoch`.

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
    pub delay_steps: u32,                         // neural steps; was "delay_ticks"
    pub transpose: f32,
    pub quantize: Option<Timebase>,
    pub dampening_amount: f32,
    pub dampening_recovery: f32,
    pub route: Option<usize>,                     // track index, None = hidden
}
```

`neural_network` is `Option<...>` and `#[serde(default)]` so older projects (and patterns where the user hasn't configured a network) load cleanly with no network — preserving the empty-network passthrough behavior.

### Track routing and the track-deletion protocol (REQUIRED — currently a latent corruption bug)

`route: Option<usize>` is a **transient track index**, not a stable id. Tracks are dynamic (`MAX_TRACKS = 64`, `num_tracks: AtomicU32`) and indexed `0..num_tracks`. This engine already has an established protocol for keeping index references valid across track deletion/reorder: on delete, **every pattern snapshot is normalized under the bank mutex before the epoch bump** — `mod_connections` are filtered (refs to the deleted track dropped) and shifted (higher indices decremented), and sidechain references are remapped (see `state.rs` remap paths and `docs/track-deletion-implementation-checklist.md`).

**Neural routes must join that protocol.** When a track is deleted or reordered:

- Each neuron's `route: Option<usize>` is remapped exactly like a `mod_connection` target: drop (→ `None`, i.e. becomes a hidden neuron) if it pointed at the deleted track; decrement if it pointed above it.
- The **weight matrix and seed vector are neuron-indexed, not track-indexed**, so they are *not* touched by track deletion — only `route` is. Make this distinction explicit so nobody tries to reshape the matrix on track delete.
- This remap must happen for **every pattern's** `ProjectNeuralNetwork`, synchronously, before `pattern_epoch` is bumped — same as the existing remaps.

Add neural-route remapping to `docs/track-deletion-implementation-checklist.md` as part of Phase 4. Without it, a neuron silently points at the wrong instrument (or out of bounds) after a delete.

### Getting runtime config from the snapshot

The `NeuralNetwork` runtime struct is owned by the audio thread, but its *config* (weights, neuron params, resolution, seed) originates in `ProjectNeuralNetwork` inside the active pattern, which reaches the audio thread through `SequencerSnapshot` (snapshot.rs; Arc-swap published by `publish_scheduler_snapshot`, versioned by `scheduler_snapshot_version`). The spec must say how the runtime picks up config changes: on observing a new snapshot version (or epoch), copy the `num_neurons`-sized config into the fixed `[N]` runtime arrays (see the `num_neurons` vs `N` rule below). This copy is the only write to the runtime config and happens at block boundaries, not mid-block.

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
    reset_interval_steps: u32,   // neural steps, derived from reset_interval_bars + base_resolution
    steps_since_reset: u32,
    base_resolution: Timebase,
    seed_on_reset: [f32; N],
}
```

`N` is a compile-time constant in v1 (16). Growth to runtime-sized requires a separate allocation strategy and is out of scope.

**`num_neurons` (stored, variable) vs `N` (runtime, fixed 16) needs an explicit load rule.** `ProjectNeuralNetwork.num_neurons` and the `Vec<Vec<f32>>` weights can in principle differ from `N`. On load/snapshot-copy: if `num_neurons == N`, copy directly; if `num_neurons < N`, copy into the leading rows/cols and zero the remainder (a smaller network is a valid sparse case); if `num_neurons > N`, reject the network (log + treat as empty) rather than truncating silently. State this so the load path is unambiguous.

**`base_resolution` is a first-class, user-facing control.** Because every `delay` and the reset interval are expressed in neural steps at this resolution, changing `base_resolution` *stretches or compresses the whole network in time* — coarser resolution makes the generated output sparser and more spread out; finer makes it busier and denser. This is intended as a live performance/editing knob ("the same network, sparser or busier"), not just a storage field. The matrix and per-neuron config stay fixed; only the time grid they play out on changes. The neural network can be conceptualized as **a track that is itself a gnarly sequencer** — it has its own timebase the way any track does, and `base_resolution` is that track's step resolution.

## Pipeline Integration Point

The neural layer slots into the scheduler:

```
SnapshotSequencerClock::process_chunk(nframes):   // scheduler.rs
  for each SnapshotTrigger emitted while advancing total_beats this block:
    let step_event = resolve_to_step_event(trigger, snapshot);  // new
    neural_network.process(step_event, &mut scratch);           // new; identity if empty, no alloc
    for event in scratch.drain():
      schedule(event);                              // stamps ScheduledEvent.pattern_epoch
  for each neural-step boundary crossed this block:
    neural_network.advance_step(&mut scratch);      // step loop: matmul, threshold, delay, quantize
    for event in scratch.drain():
      schedule(event);
```

Note this runs inside the existing per-sample `process_chunk` loop, where neural-step boundaries are detected the same way step boundaries already are (multiple per block, or spanning blocks). `scratch` is a runtime-owned, pre-allocated buffer — see the no-allocation note under "Pipeline shape."

Two emission sources merge into the same scheduled-event stream:
1. **Step-seeded events** — a step fires, the network may emit additional events (delayed, transposed) in addition to the seed.
2. **Autonomously generated events** — neurons whose energy crossed threshold this block fire of their own accord (only possible if seeded, directly or transitively, at some prior neural step).

Both paths end at `ScheduledEvent`. The clock and the network share the same sample-clock so their timing aligns.

## Behavior in Empty / Default State

A project with no configured neural network behaves identically to today:

- No neurons exist → `process(step_event)` is identity.
- No state is allocated for the network beyond the empty struct.
- No scheduling overhead is added per trigger.

This property is load-bearing: the refactor itself must be neutral, and the neural feature must be opt-in per project.

## Open Questions

1. **Velocity accumulation cap** — `min(1.0, sum(velocities))` clamps hard; should there be a soft compression curve? Defer to v1 testing; clamp is fine to start.
2. **Where does the velocity merge run?** Two candidate sites: (a) a post-scheduling pass over the block's `ScheduledEvent`s grouped by `(sample_time, track)`, or (b) at voice-trigger time in `audio.rs`. (a) keeps the merge in the scheduler where ordering/determinism is easy; (b) avoids a second pass but pushes merge logic into the voice path. Lean (a). Decide before Phase 3.
3. **Quantize grid for neuron**: project-relative or track-relative? Lean project-relative (transport grid) for consistency.
4. **Quantize ↔ delay ordering and clock mixing.** The fire sequence is "threshold met → defer to next quantize boundary (samples) → *then* start `delay` countdown (neural steps) → propagate." Two things to pin: (a) after a quantize defer, does `delay` count neural steps from the deferred moment, and (b) what happens if a second threshold crossing occurs for the same neuron while it is mid-defer (coalesce, ignore, or re-arm?). Spec a concrete answer before Phase 3.
5. **Multiple seed tracks in one block**: if steps on track A and track B both seed neurons in the same block, propagations interleave deterministically because seeding and the tick loop both iterate by index (track index for seeding, neuron `0..N` for firing). Confirm the ordering contract in tests.
6. **Reset-flag step — visibility**: when a step carries the dedicated reset flag (see "Reset trigger from a step"), does the UI show it distinctly? Probably yes — it's load-bearing for a song's structure.
7. **Per-neuron mute/solo for editing**: not in v1, but worth keeping the data model future-compatible (one bit per neuron in storage — same spare-bit budget as the reset flag).
8. **`base_resolution` as a live knob**: changing it stretches/compresses the network in time (see Runtime state). Is it automatable / p-lockable, or edit-time only in v1? Lean edit-time only for v1; flag automation as future work.

## Implementation Phases

The phases below order the work so that each is independently shippable and the audio pipeline is never broken.

### Phase 1: P-lock identity refactor (no new features)
- Define `ParamNodeId` in terms of the existing `(node_id, node_param_idx)` identity (effects.rs `param_node_indices` / `ParamDescriptor::node_param_idx`), not a new hash.
- Persist `ParamNodeId` alongside index in p-lock storage.
- Gate all p-lock resolution paths by identity (`resolve_instrument_plocks` scheduler.rs:409, `resolve_effect_params`, `InstrumentParams`, track-level timebase/swing p-locks).
- Stale p-lock UI surface.

### Phase 2: StepEvent + ScheduledEventKind::NetworkTrigger
- Introduce `StepEvent` as the intermediate value type.
- Reshape scheduler to pass through `StepEvent`.
- Add `NetworkTrigger` variant (unused for now).
- Verify identity behavior — no neural code yet.

### Phase 3: Neural network audio-thread implementation
- `NeuralNetwork` struct, neural-step loop, matrix math, dampening, quantization, delays.
- Integration into scheduler at the documented hook point.
- Empty-network passthrough verified.

### Phase 4: Pattern integration + storage
- `ProjectNeuralNetwork` lives in `ProjectPattern`.
- Pattern switch swaps active network config and resets runtime state (rides the `switch_pattern` mutex + `pattern_epoch` + publish path; runtime reset on the audio thread keyed off the epoch change).
- `num_neurons` vs `N` load rule (copy / zero-pad / reject).
- Seed-on-reset for the incoming pattern loads immediately on switch.
- Dedicated per-step reset flag wiring (NOT aux_a).
- **Neural-route remapping in the track-deletion protocol** — remap every pattern's `route` indices under the bank mutex before the epoch bump; add to `docs/track-deletion-implementation-checklist.md`.
- JSON schema, load/save, backward-compat (`Option` + `#[serde(default)]`).

### Phase 5: Editor UI (out of spec, follow-up)
- Matrix editor, per-neuron config panel, route picker, real-time fire visualization.

## Out of Scope, Logged for Future

- Learned weights / weight evolution from playing patterns.
- Per-neuron chord-mode (root / full / voicing extract).
- Network-as-track *as a literal feature* (a neural network appearing as its own track lane / routable instrument with a dedicated track index). The "track that is a gnarly sequencer" framing in this spec is a **mental model** for the per-pattern network with its own `base_resolution` timebase — it does **not** mean v1 allocates a real track slot for the network. Promoting the network to a first-class track lane (so it shows up in the track UI, can be muted/soloed, routed, etc.) is future work.
- Multiple networks per pattern.
- Cross-pattern weight morphing / interpolation.
- `carry_state_on_switch` flag — preserve runtime state across pattern switches.
- Audio-rate (per-sample) propagation.
- MIDI export of network-generated events as a flattened pattern.
