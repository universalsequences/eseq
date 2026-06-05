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
- Building the editing UI. v1 ships with a data model + audio-side implementation. The intended editor design is captured in the "Editor UI" section (a tab in the Step sequencer view), but implementing it is a follow-up (Phase 5).

## Concepts

### Neural clock — per-neuron resolution (there are no "ticks" in this engine)

> **Grounding note.** The engine has **no tick unit**. Time is samples and `f64` quarter-note beats. The scheduler (`scheduler.rs`, `SnapshotSequencerClock::process_chunk`) advances a per-sample `total_beats: f64` and emits a `SnapshotTrigger` (scheduler.rs:40) when `total_beats` crosses a step boundary, computed from a per-track/per-step `Timebase` (data.rs:91, an enum of musical divisions — `Sixteenth`, `EighthTriplet`, `Polyrhythm`, etc.). "One tick = the finest enabled timebase" does not map onto anything real, because timebase is per-track and per-step (`StepParam::Sync`, plus `timebase_plock_snapshots` / `swing_*` snapshots in `ProjectPattern`).

**Each neuron has its own clock.** There is no single network-wide neural step. Every neuron carries a `resolution: Timebase` (the per-row `resolution` dropdown in the editor — `1n`, `4n`, `16n`, `16nt`, `32n`, …). A neuron *evaluates* — checks its threshold, possibly fires — once per crossing of **its own** resolution grid. This is the literal "a network of tracks, each a gnarly sequencer" model: a neuron is a track with its own timebase.

Mechanics, expressed in terms of the existing engine:

- The engine already advances a per-sample `total_beats: f64` in `process_chunk` and already detects boundary crossings for arbitrary `Timebase`s (that is exactly what per-track/per-step timebase does today). The neural layer reuses that machinery: for each neuron, detect crossings of `k * neuron.resolution.step_beats()` within the block — handling zero, one, or several crossings per block, and crossings that fall partway through a block, exactly as the step sequencer does (`derive_local_step`).
- A neuron's `delay` is counted in **its own resolution steps** — `resolution = 16n, delay = 2` means "two sixteenth-notes after firing." This is what the per-row number column in the editor sets, and it is exactly the user's "a timebase the delays are expressed in."
- Because there is no global step, energy does not advance in lockstep. Propagation is **event-driven** (see "Neuron evaluation" below): a fire adds that neuron's weight row into the energy vector; each neuron consumes accumulated energy when *its own* resolution boundary comes around. Two neurons at `4n` and `16n` interleave naturally on the shared sample clock.
- Quantization boundaries are expressed in **samples** (`quantize_pending: [Option<u64>; N]`), because audio emission timing is sample-accurate. A neuron's `resolution` (its evaluation/delay grid) and the optional `quantize` snap (output-timing grid, in samples) are two different units; see the quantize ordering note under Open Questions.
- `reset_interval` is still counted in **bars** (a global musical span), independent of any neuron's resolution — it is the network-wide "bar reset" control in the editor.

This keeps every neuron aligned to the transport at its own rate without inventing a unit the rest of the engine doesn't have.

> **No global "stretch" knob in v1.** The sparse/busy control is *per neuron* (its `resolution`). A single global multiplier that scales every neuron's grid at once — "the same network, sparser or busier, in one gesture" — is deferred (see Out of Scope / Open Questions). Per-neuron resolution already gives full control; the global knob is convenience on top.

### Neuron

A neuron is a member of a fixed-size pool (`NUM_NEURONS = 16` in v1, configurable in storage). Each neuron has:

- `weight[N]`: outgoing weights, one per other neuron. Row in the NxN matrix. (Editor: the neuron's row in the 16×16 grid; circle size = weight magnitude.)
- `resolution: Timebase` — the neuron's own clock. It evaluates (checks threshold, possibly fires) once per crossing of this grid, and its `delay` is counted in these units. See "Neural clock." (Editor: the per-row `resolution` dropdown.)
- `delay: u32` — **counted in this neuron's `resolution` steps** — between firing and "set my last_trigger=1" (the propagation step), i.e. how long until this fire propagates to the neuron's targets. (Editor: the per-row number column with the drag arrow.)
- `threshold: f32` — energy level at which the neuron fires when evaluated. Not on the main editor row in the screenshot; exposed in a per-neuron detail view (see UI section / Open Questions).
- `transpose: f32` — semitone offset applied to events when they fire from this neuron.
- `quantize: Quantization` — optional grid snap, in samples. When a neuron's threshold is met, the audio emission is deferred to the next quantization boundary. Distinct from `resolution` (the eval/delay grid).
- `dampening: f32` — multiplier (0..1) applied to subsequent triggers after firing; recovers over time. Models "neuron fatigue." This is per-neuron and distinct from the network-wide `energy_decay` leak (see Matrix state).
- `route: Option<TrackId>` — destination track. `None` = hidden neuron (participates in matrix propagation, no audio output). (Editor: the per-row `trig-N` dropdown.)

A track can be the route of multiple neurons (many-to-one). A neuron can have at most one route (one-to-one in the other direction). This asymmetry is the key compositional primitive: one track can host multiple neural "voices" each with its own transpose / delay / quantize.

### Matrix state

State vectors over the neuron pool. Because each neuron runs on its own resolution, these are not advanced in lockstep — they are read/written as each neuron's resolution boundaries are crossed on the shared sample clock:

- `last_triggers: [f32; N]` — most recent fire indicator (0 or 1; floats to allow future fractional triggers).
- `energy: [f32; N]` — accumulated incoming weight contributions.
- `pending_delays: [u32; N]` — per-neuron countdown until a fired neuron's propagation lands, each counted in **that neuron's own resolution steps**.
- `dampening_level: [f32; N]` — current fatigue per neuron (0 = fully recovered, 1 = fully suppressed).

Network-wide scalars (the global controls in the editor's top-left):

- `energy_decay: f32` — a global leak applied to the whole `energy` vector over time (editor: `energy decay`, e.g. `0.994`). Distinct from per-neuron `dampening`: `dampening` suppresses a single neuron after *it* fires; `energy_decay` continuously bleeds accumulated energy across *all* neurons so a sub-threshold buildup doesn't persist forever. The grid this leak is applied on is specified under "Neuron evaluation" (and flagged in Open Questions).
- `max_poly: u32` — cap on simultaneous voices the network emits per destination track on a single sample (editor: `max poly`, e.g. `2`). See "Velocity accumulation and polyphony."

### Neuron evaluation (event-driven, not a global matmul)

With per-neuron resolution there is no single step where the whole matrix multiplies at once. Instead, **propagation is event-driven**: a fire adds that neuron's weight *row* into the energy vector (the row-at-a-time form of the old `weights · last_triggers` matmul), and each neuron *consumes* energy when its own resolution boundary comes around.

Within `process_chunk`, the per-sample loop advances `total_beats` and, for each neuron `i`, detects whether the block crossed a boundary of `neuron[i].resolution`. Crossings are processed in sample order; within the same sample, neurons are processed in index order `0..N` (the determinism contract — see Open Questions). When neuron `i` crosses its own resolution boundary, **evaluate neuron `i`**:

1. Decrement `pending_delays[i]` (it counts neuron `i`'s own resolution steps). If it just reached zero, this is a *propagation step*: add neuron `i`'s weight row into the energy of its targets — `for j in 0..N: energy[j] += weight[i][j]` — scaled by `(1 - dampening_level[i])`, and set `last_triggers[i] = 1` for this evaluation (else `0`).
2. If `energy[i] >= threshold[i]`, neuron `i` **fires**:
   - Build a `StepEvent` (see "StepEvent" below) from this firing.
   - If `route[i].is_some()`, emit the audio event at the *moment of firing* — at the next `quantize[i]` sample boundary if quantization is set, otherwise this sample. (Mental model: "the trigger sounds when the neuron fires; the matrix sees the propagation `delay` steps later.")
   - Arm the propagation: `pending_delays[i] = delay[i]` (in neuron `i`'s resolution steps). When it elapses, step 1 adds this neuron's weight row to its targets.
   - Apply dampening: `dampening_level[i] = min(1.0, dampening_level[i] + neuron[i].dampening_amount)`.
   - Zero `energy[i]`.
3. Dampening recovers for neuron `i`: `dampening_level[i] *= dampening_recovery_factor` (per-neuron constant, e.g. 0.98 per *this neuron's* resolution step).

Network-wide energy leak: `energy *= energy_decay` (the `energy decay` control). Because there is no global step to hang it on, the concrete grid for this leak must be pinned — apply it once per crossing of the *finest* configured neuron resolution, or per fixed sample interval. Lean "finest configured resolution"; see Open Questions. The intent is unambiguous (slow continuous bleed); only the discretization is open.

### Reset / seeding

The matrix has a reset interval (configurable, default: pattern length in bars). At each reset boundary:

- `energy`, `pending_delays`, `last_triggers`, `dampening_level` are zeroed.
- Optionally, a seed vector is loaded into `last_triggers` (e.g. neuron 0 = 1) to kick off generation in pure-neural mode.

A second reset mechanism lets the step sequencer program higher-order behavior — "every 8 bars on the kick, restart the network." **This must not be wired through `aux_a`.** See "Reset trigger from a step (NOT via aux_a)" under the refactor section: `aux_a`/`aux_b` are genuinely unread today, `AuxA` carries a legitimate authored range (`0..16`), so a sentinel value would collide with real data. Use a dedicated per-step reset flag instead.

### Step → Network seeding

When a step fires on a track, every neuron with `route == Some(that_track)` has its `last_triggers` set to 1 simultaneously. This is the fan-out described in earlier design discussion: a punch-in on the kick seeds *all* neurons routed to the kick at once, each of which then propagates independently with its own delay/transpose/weights.

The seed step **also plays directly** at its scheduled time — it does not wait for the network. The network only ever *adds* events; it never delays or suppresses step-driven hits. This is the "parallel mode" decision: the matrix is purely additive.

### Velocity accumulation and polyphony

When multiple neurons routed to the same track fire on the same audio sample, the resulting hits **accumulate velocity** into a single hit rather than producing N stacked note-ons. The semantic: same-sample coincidence is treated as accent, not polyphony.

**`max_poly`** (editor: `max poly`) bounds how many *distinct-in-time* voices the network keeps alive per destination track. Velocity accumulation handles the same-sample case (collapse to one accented hit); `max_poly` handles the across-time case — when more than `max_poly` network-emitted voices would overlap on one track, the oldest is stolen (released) so no more than `max_poly` sound at once. With `max_poly = 1` the track is monophonic (each new network hit cuts the previous); the screenshot's `max_poly = 2` allows two overlapping tails. The seed step's own direct hit is *not* counted against `max_poly` — the network only ever adds, and voice-stealing applies to network-emitted voices. The exact steal policy (oldest-first vs quietest-first) is an Open Question.

Pseudocode for the merge:

```
collect all StepEvents at sample S for track T
if multiple events, merge into one:
  velocity = min(1.0, sum(velocities))
  transpose = lowest-indexed firing neuron's transpose
  duration, speed, pan, aux_a, aux_b, chop, chord = from the lowest-indexed firing neuron
```

The merge fields are exactly the `ResolvedStep` fields (`duration, velocity, speed, aux_a, aux_b, transpose, pan, chop` — accumulator.rs:5) plus the chord. `Sync` and `Delay` (`StepParam::Sync=8`, `Delay=9`) are **not** part of `ResolvedStep` and therefore do not participate in the merge — sync drives per-step timebase selection upstream, not the resolved event. "Lowest-indexed firing neuron" is well-defined because neurons that fire on the same sample are evaluated in index order `0..N` (see "Neuron evaluation"), so fire order is deterministic; the merge picks the first firing neuron in that iteration.

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

- A network-emitted event has no source step (hidden neurons, transposed copies, events scheduled several resolution steps after the seed step was edited).
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
- per-neuron config (resolution, delay, threshold, transpose, quantize, dampening, route)
- network globals: reset interval (bars), energy decay, max poly, seed-on-reset vector

This lets the user write a network for the verse pattern and a completely different network for the chorus pattern, switched in the same gesture they use to switch step patterns today.

### Runtime state on pattern switch

How switching works today (verified): `switch_pattern` (sequencer/state.rs ~1685) locks the `pattern_bank: Mutex<Vec<PatternSnapshot>>`, captures live state into the outgoing slot, restores the incoming slot, stores `current_pattern` (`AtomicU32`), increments `pattern_epoch` (`AtomicU64`), and calls `publish_scheduler_snapshot()` (Arc-swap + `scheduler_snapshot_version.fetch_add`). The audio thread reads the new state via `latest_scheduler_snapshot()` and drops any in-flight `ScheduledEvent` whose `pattern_epoch` no longer matches (audio.rs epoch check). So "atomic swap" in this spec means *that* mutex + epoch + publish sequence, not a single atomic store.

The neural config swap rides the same path. Pattern switching is also the hook to reset neural runtime state — `energy`, `last_triggers`, `pending_delays`, `dampening_level`, `quantize_pending`, and the reset tracker (`next_reset_beat`). Without this, the old pattern's accumulated energy bleeds into the new pattern with (possibly very different) weights, producing chaotic transients.

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
    pub reset_interval_bars: f32,                 // editor: "bar reset"
    pub energy_decay: f32,                        // editor: "energy decay", global leak (e.g. 0.994)
    pub max_poly: u32,                            // editor: "max poly", per-track voice cap
    pub seed_on_reset: Vec<f32>,                  // length = num_neurons; editor: "clear" row
}

pub struct ProjectNeuron {
    pub resolution: Timebase,                     // editor: per-row "resolution"; the neuron's own clock
    pub delay_steps: u32,                         // counted in THIS neuron's resolution steps; editor: per-row number
    pub threshold: f32,
    pub transpose: f32,
    pub quantize: Option<Timebase>,               // sample-grid output snap; distinct from resolution
    pub dampening_amount: f32,
    pub dampening_recovery: f32,
    pub route: Option<usize>,                     // track index, None = hidden; editor: per-row "trig-N"
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
    neurons: [NeuronConfig; N],          // each carries its own `resolution: Timebase`
    last_triggers: [f32; N],
    energy: [f32; N],
    pending_delays: [u32; N],            // per-neuron, each in that neuron's resolution steps
    quantize_pending: [Option<u64>; N],  // sample of next output grid boundary
    dampening_level: [f32; N],
    energy_decay: f32,                   // network-wide energy leak
    max_poly: u32,                       // per-track voice cap
    reset_interval_beats: f64,           // derived from reset_interval_bars; reset is bar-based, not neuron-clock-based
    next_reset_beat: f64,                // absolute total_beats of the next reset boundary
    seed_on_reset: [f32; N],
}
```

There is **no** network-wide `base_resolution` field — each neuron's clock lives in `NeuronConfig.resolution`. Reset is tracked in beats (bar-based), independent of any neuron's resolution. Per-neuron resolution-boundary detection does not need stored phase: it is recomputed each block from `total_beats` and `neuron.resolution.step_beats()`, the same stateless way the step sequencer derives its boundaries.

`N` is a compile-time constant in v1 (16). Growth to runtime-sized requires a separate allocation strategy and is out of scope.

**`num_neurons` (stored, variable) vs `N` (runtime, fixed 16) needs an explicit load rule.** `ProjectNeuralNetwork.num_neurons` and the `Vec<Vec<f32>>` weights can in principle differ from `N`. On load/snapshot-copy: if `num_neurons == N`, copy directly; if `num_neurons < N`, copy into the leading rows/cols and zero the remainder (a smaller network is a valid sparse case); if `num_neurons > N`, reject the network (log + treat as empty) rather than truncating silently. State this so the load path is unambiguous.

**Per-neuron `resolution` is the sparse/busy control.** Each neuron's `resolution` is a first-class, user-facing knob: it sets that neuron's evaluation rate, and every `delay` for that neuron is counted in its units. Coarser resolution on a neuron makes its contribution sparser and more spread out; finer makes it busier and denser. The matrix weights and the other per-neuron config stay fixed; only the time grid each neuron plays out on changes. The whole network is conceptualized as **a bank of tracks, each itself a gnarly sequencer** — every neuron has its own timebase the way any track does, and its `resolution` is that track's step resolution.

A single **global stretch** multiplier that scales every neuron's grid at once (one gesture to make the entire network sparser/busier) is a natural convenience but is **out of scope for v1** (see Open Questions / Out of Scope) — per-neuron resolution already covers the capability.

## Pipeline Integration Point

The neural layer slots into the scheduler:

```
SnapshotSequencerClock::process_chunk(nframes):   // scheduler.rs
  for each SnapshotTrigger emitted while advancing total_beats this block:
    let step_event = resolve_to_step_event(trigger, snapshot);  // new
    neural_network.process(step_event, &mut scratch);           // new; identity if empty, no alloc
    for event in scratch.drain():
      schedule(event);                              // stamps ScheduledEvent.pattern_epoch
  // no single global neural step; instead, per-neuron resolution boundaries:
  for each (sample, neuron i) boundary of neuron[i].resolution crossed this block, in sample then index order:
    neural_network.evaluate_neuron(i, sample, &mut scratch);  // decrement delay/propagate, threshold, fire, dampen
    for event in scratch.drain():
      schedule(event);
  // network-wide reset boundary (bar-based) and energy_decay leak handled at their own grids this block
```

Note this runs inside the existing per-sample `process_chunk` loop. Each neuron's resolution boundaries are detected the same way per-track/per-step timebase boundaries already are (multiple per block, or spanning blocks) — there is just one detector per neuron instead of one for the network. Ordering across neurons within a block is sample-time first, then neuron index `0..N` for same-sample ties (the determinism contract). `scratch` is a runtime-owned, pre-allocated buffer — see the no-allocation note under "Pipeline shape."

Two emission sources merge into the same scheduled-event stream:
1. **Step-seeded events** — a step fires, the network may emit additional events (delayed, transposed) in addition to the seed.
2. **Autonomously generated events** — neurons whose energy crossed threshold when evaluated this block fire of their own accord (only possible if seeded, directly or transitively, at some prior evaluation).

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
4. **Quantize ↔ delay ordering and clock mixing.** The fire sequence is "threshold met → emit audio (snapped to the neuron's `quantize` sample boundary if set) → propagation arms `delay` in the neuron's resolution steps → propagate." Two things to pin: (a) audio output snaps on the `quantize` *sample* grid while `delay` counts the neuron's *resolution* steps — confirm these two grids compose as written (output timing independent of propagation timing), and (b) what happens if a second threshold crossing occurs for the same neuron while a propagation is still pending (coalesce, ignore, or re-arm?). Spec a concrete answer before Phase 3.
5. **Multiple seed tracks / many neurons firing in one block**: ordering is sample-time first, then neuron index `0..N` for same-sample ties; seeding iterates by track index. Because neurons now run on independent resolutions, confirm in tests that the sample-then-index contract is stable across block boundaries (a neuron whose boundary lands at the very end of one block vs. the start of the next must be deterministic).
6. **`energy_decay` discretization grid.** The leak is conceptually continuous but must be applied on a concrete grid (per finest-configured-resolution crossing, vs. a fixed sample interval). Lean finest-configured-resolution. Decide before Phase 3 — it affects reproducibility across sample-rate/block-size changes.
7. **`max_poly` voice-steal policy.** When more than `max_poly` network voices would overlap on a track, steal oldest-first or quietest-first? Lean oldest-first (predictable). Confirm whether the seed step's direct hit truly stays exempt from the cap.
8. **Where does `threshold` live in the editor?** The screenshot's per-row controls are route / resolution / delay / fire-LED — no visible threshold. Options: a per-neuron detail panel (click a row), a global default threshold with per-neuron overrides, or a second toggleable grid view. Pick before Phase 5; the data model already has per-neuron `threshold` regardless.
9. **Reset-flag step — visibility**: when a step carries the dedicated reset flag (see "Reset trigger from a step"), does the UI show it distinctly? Probably yes — it's load-bearing for a song's structure.
10. **Per-neuron mute/solo for editing**: not in v1, but worth keeping the data model future-compatible (one bit per neuron in storage — same spare-bit budget as the reset flag).
11. **Global stretch multiplier**: a single knob scaling every neuron's resolution at once (sparse/busy in one gesture). Out of scope for v1 (per-neuron resolution covers it); revisit as a convenience layer. If added, is it automatable / p-lockable?

## Editor UI

The neural editor is a **tab inside the existing Step sequencer view** — the same panel gains a `Steps` / `Neural` switch, so a track's deterministic grid and its neural matrix share one surface and one mental "you are editing this pattern" context. Switching tabs does not change which pattern is active; it changes which layer of that pattern you see.

Reference layout (from the patch-editor prototype):

```
┌─ globals (top-left) ────────┐   ┌─ seed row (top) ───────────────────────┐
│  bar reset   [ 2 ]          │   │ [fire LEDs ▔ ▔ ▔ … per column]          │
│  energy decay[ 0.994 ]      │   │ clear ◯   ● ·  ● · ●  · · · · ●  ·  …    │  ← seed_on_reset
│  max poly    [ 2 ]          │   └─────────────────────────────────────────┘
└─────────────────────────────┘        1  2  3  4  5  …                  16   ← column index
┌─ per-neuron rows (left) ─────────────────┐  ┌─ weight matrix (NxN) ─────────┐
│ trig-1 ▾ │ resolution 1n  ▾ │ [2] │ ◯     │  │  ●  ·  ·  ●  ·  ●  ·  …  ●    │ row 1
│ trig-2 ▾ │ resolution 4n  ▾ │ [2] │ ◯     │  │  ●  ·  ●  ·  ·  ·  ·  …       │ row 2
│ trig-1 ▾ │ resolution 16n ▾ │ [5] │ ◯     │  │  …                            │ …
│   …      │      …           │ […] │ …     │  │  (circle size = weight mag)  │
└──────────────────────────────────────────┘  └───────────────────────────────┘
┌─ output timeline (bottom) — generated voices over bars (read-only viz) ──────┐
└─────────────────────────────────────────────────────────────────────────────┘
```

Element → data mapping:

| UI element | Backing field | Notes |
|---|---|---|
| `bar reset` | `reset_interval_bars` | global |
| `energy decay` | `energy_decay` | global leak (0..1) |
| `max poly` | `max_poly` | per-track voice cap |
| `clear` row of sized circles | `seed_on_reset[i]` | circle size = seed magnitude; `clear` zeroes the vector |
| top fire LEDs | runtime `last_triggers` viz | read-only activity indicator |
| per-row `trig-N` dropdown | `neuron[i].route` | `None` / hidden when unset |
| per-row `resolution` dropdown | `neuron[i].resolution` | the neuron's own timebase |
| per-row number + drag arrow | `neuron[i].delay_steps` | counted in that row's resolution |
| per-row circle (right of number) | runtime fire LED for that neuron | read-only |
| 16×16 grid of sized circles | `weights[i][j]` | row = source neuron, col = target; size = magnitude |
| bottom timeline strip | runtime output history | read-only visualization |

`threshold` (and the other per-neuron fields not on the row: `transpose`, `quantize`, `dampening_amount`, `dampening_recovery`) are not in the compact row — they belong in a per-neuron detail view opened from the row (see Open Question 8).

This section is the design target for Phase 5; it does not change the v1 audio-side scope.

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
- `NeuralNetwork` struct; **per-neuron resolution-boundary detection** (one detector per neuron, reusing the existing timebase boundary logic), not a single global step.
- Event-driven evaluation: per-neuron threshold → fire → arm `delay` (in that neuron's resolution) → propagate weight row; per-neuron dampening + recovery.
- Network globals: `energy_decay` leak (on the agreed grid — Open Question 6), `max_poly` voice cap (Open Question 7).
- Per-neuron `quantize` output snap (samples), distinct from resolution.
- Integration into scheduler at the documented hook point; sample-then-index determinism contract.
- Empty-network passthrough verified.

### Phase 4: Pattern integration + storage
- `ProjectNeuralNetwork` lives in `ProjectPattern`.
- Pattern switch swaps active network config and resets runtime state (rides the `switch_pattern` mutex + `pattern_epoch` + publish path; runtime reset on the audio thread keyed off the epoch change).
- `num_neurons` vs `N` load rule (copy / zero-pad / reject).
- Seed-on-reset for the incoming pattern loads immediately on switch.
- Dedicated per-step reset flag wiring (NOT aux_a).
- **Neural-route remapping in the track-deletion protocol** — remap every pattern's `route` indices under the bank mutex before the epoch bump; add to `docs/track-deletion-implementation-checklist.md`.
- JSON schema, load/save, backward-compat (`Option` + `#[serde(default)]`).

### Phase 5: Editor UI (see "Editor UI" section)
- `Steps` / `Neural` tab switch inside the existing Step sequencer view.
- 16×16 weight-matrix grid (size-encoded circles), seed row with `clear`, global controls (bar reset / energy decay / max poly).
- Per-neuron row: route picker, resolution, delay; per-neuron detail view for threshold / transpose / quantize / dampening.
- Real-time fire visualization (top LEDs, per-row LEDs, bottom output timeline).

## Out of Scope, Logged for Future

- Learned weights / weight evolution from playing patterns.
- Per-neuron chord-mode (root / full / voicing extract).
- Network-as-track *as a literal feature* (a neural network appearing as its own track lane / routable instrument with a dedicated track index). The "bank of tracks, each a gnarly sequencer" framing in this spec is a **mental model** for the per-pattern network where each neuron has its own `resolution` timebase — it does **not** mean v1 allocates real track slots for the network or its neurons. Promoting the network to first-class track lanes (so it shows up in the track UI, can be muted/soloed, routed, etc.) is future work.
- Global stretch multiplier (one knob scaling every neuron's resolution at once). Per-neuron resolution covers the capability in v1; the global convenience knob is deferred.
- Multiple networks per pattern.
- Cross-pattern weight morphing / interpolation.
- `carry_state_on_switch` flag — preserve runtime state across pattern switches.
- Audio-rate (per-sample) propagation.
- MIDI export of network-generated events as a flattened pattern.
