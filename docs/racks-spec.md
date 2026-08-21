# Instrument Rack & Drum Rack Spec

Status: draft / design
Author: design pass, 2026-06-30
Related: `docs/track-groups-spec.md`, `MACRO_MAPPING_SPEC.md`

## 1. Goal

Let a single track host **multiple internal instruments** ("slots") so we get
richer tracks without inflating track count. Two user-facing behaviors:

- **Instrument Rack** — one incoming note triggers *all* slots at once (layered
  sounds, per-slot transpose/detune/gain). Ableton's Instrument Rack.
- **Drum Rack** — each MIDI note maps to *one* slot, so a single track plays a
  whole kit; each slot can be a sampler *or* a full custom instrument. Ableton's
  Drum Rack.

**Key design decision:** these are not two features. They are one feature — a
**multi-instrument track container** — distinguished only by a *routing policy*
that decides which slot(s) a note reaches. Build the container once; instrument
rack = `Broadcast` policy, drum rack = `ByPitch` policy. Future policies
(velocity zones, key zones, round-robin) drop in at the same seam.

## 2. Why this is low-risk in our engine

The audio-summing and mixer plumbing already supports this. Today every
instrument feeds a **shared per-track sum bus**, built once in
`create_track_shell` (`crates/sequencer/src/ttui/graph.rs`):

```
voice_sum_id (L) ─┐
voice_sum_r_id(R)─┴─> pan_id ─> fx_out_id(delay_id) ─> track bus (L/R)
```

- `build_sampler_voices` connects each of `MAX_VOICES` sampler voices into
  `voice_sum_id` / `voice_sum_r_id`.
- `add_custom_track` → `connect_engine_to_track` connects a dgen engine into the
  same two sum nodes.

So **multiple instruments already mix correctly** if we just build several
instrument subgraphs and connect each into the same `voice_sum_id`. The pan /
fx / send / bus chain is per-track and shared — racks reuse it unchanged.

What does *not* exist yet, and is the actual work:

1. **Data model** — a track currently has exactly one instrument (one entry in
   each per-track parallel `Vec` in `ProjectPattern`). Racks need an ordered
   list of slots per track.
2. **Trigger routing** — `ResolvedTrigger` targets a track and carries
   `notes: [f32; MAX_VOICES]`. We need to fan/route those notes to slots.
3. **Per-slot voice management** — each slot needs its own voice allocation and
   (for drum racks) choke groups.
4. **UI** — pad grid (drum) / chain list (instrument).

## 3. Current architecture (reference)

### Project / pattern data (`crates/sequencer/src/project.rs`)
- `ProjectTrack` enum (`#[serde(tag="kind")]`): `Sampler { sample_path, color,
  collapsed }`, `Custom { instrument_name, … }`, `Modulator { … }`. This is
  per-track *kind* metadata.
- `ProjectPattern` holds **parallel `Vec`s indexed by track**, including:
  `instrument_types`, `instrument_slots: Vec<ProjectEffectSlot>`,
  `instrument_base_note_offsets: Vec<f32>`, `instrument_run_modes`,
  `sample_paths: Vec<Option<String>>`, `sample_names: Vec<String>`,
  `graph_overrides`, `neural_networks`, `effect_slots`, `midi_fx_slots`,
  `track_params`, `track_sound_states`.

### Instrument type (`crates/sequencer/src/sequencer/data.rs`)
- `enum InstrumentType { Sampler, Custom, Modulator }`, `COUNT = 3`, with
  `to/from` integer flag helpers. `MAX_TRACKS = 64`, `MAX_VOICES = 12`,
  `EXT_MOD_INPUT_COUNT = 4`.

### Runtime audio nodes (`crates/sequencer/src/tui/mod.rs` + `tui/graph.rs`)
- `app.graph.n: Vec<TrackNodeIds>`, one per track. `TrackNodeIds`:
  `sampler_ids`, `sampler_gatepitch_ids`, `sampler_modulator_ids`,
  `voice_sum_id`, `voice_sum_r_id`, `pan_id`, `filter_id`, `delay_id`,
  `send_id`, `mod_out_id`, `mod_in_clip_ids`, `mod_env_id`, `bus_send_ids`.
- Build path: `add_track` / `add_blank_sampler_track` / `add_custom_track` →
  `create_track_shell` (mixer chain) + `build_sampler_voices` or
  `connect_engine_to_track` (instrument) → `finish_track_registration`
  (parallel-vec bookkeeping). `InstrumentRegistration` enum carries the per-kind
  node ids back into `TrackNodeIds`.

### Trigger / voice path (`crates/sequencer/src/scheduler.rs`, `voice.rs`, `audio.rs`)
- `ResolvedTrigger { notes: [f32; MAX_VOICES], … }` enqueued per track via
  `enqueue_resolved_trigger`. `VoicePool` per track (`MAX_VOICES = 12`), held in
  `audio.rs`.
- `max_poly: Option<u32>` + `NeuralMaxPolySelection` (graph.rs) already do
  velocity/note/seed-aware voice stealing at a firing boundary.
- **Fan-out already exists**: MIDI-FX `rebind_midi_fx_event_to_track` re-targets
  a note event to another `target_track` with transpose/velocity. Racks
  internalize this same idea *within* one track across slots.

## 4. Unified design

### 4.1 Track becomes either single-instrument or a rack

Introduce a per-track *container kind*:

```
enum TrackContainer {
    Single,                 // today's behavior — exactly one instrument
    Rack { policy, slots }, // 1..=N instrument slots
}
```

A **rack slot** is conceptually a complete mini-instrument:

```
struct RackSlot {
    instrument_type: InstrumentType,   // Sampler | Custom (Modulator excluded)
    // ── identity/source ──
    sample_path: Option<String>,       // Sampler slots
    instrument_name: Option<String>,   // Custom slots (dgen engine)
    run_mode: CustomInstrumentRunMode, // Custom slots
    base_note_offset: f32,             // per-slot transpose
    // ── routing (drum rack) ──
    pad_note: Option<u8>,              // MIDI note this slot answers to (ByPitch)
    choke_group: Option<u8>,           // 0..=N; same group → mutually cut off
    // ── per-slot mix ──
    gain: f32,
    pan: f32,
    mute: bool,
    solo: bool,
    // ── params/state (mirrors today's per-track instrument data) ──
    instrument_slot: ProjectEffectSlot,
    graph_overrides: ProjectGraphOverrides,
    neural_network: Option<ProjectNeuralNetwork>,
}
```

### 4.2 Routing policy

```
enum RackRouting {
    Broadcast,          // Instrument Rack: note → every (unmuted) slot
    ByPitch,            // Drum Rack: note → the slot whose pad_note == note
    // future: VelocityZones, KeyZones, RoundRobin
}
```

Routing is a pure function applied when a `ResolvedTrigger` is dispatched:

```
fn route(trigger_note, policy, slots) -> SmallVec<SlotIndex>
  Broadcast => all slots where !slot.mute (respect solo set)
  ByPitch   => slots.iter().filter(|s| s.pad_note == Some(note)).take(1)
```

`Broadcast` applies `base_note_offset` per slot to the same incoming note;
`ByPitch` typically ignores incoming pitch for synthesis (pad plays its mapped
sound at `base_note_offset`) — configurable per slot.

### 4.3 Voice budget

- Each slot owns an independent voice allocation. For samplers that means its
  own gatepitch/modulator/sampler voice fan (today's `build_sampler_voices`,
  parameterized by slot). For custom engines, its own engine runtime instance.
- Each underlying instrument follows its own polyphony rule. A slot's
  `max_polyphony` bounds how many voices that slot may sound at once; it is not
  partitioned out of a shared `MAX_VOICES` track budget.
- To bound cost, keep a hard slot cap (`MAX_RACK_SLOTS`, initially 16) and
  clamp every slot's `max_polyphony` to a documented limit. Drum racks default
  most pads to low polyphony (usually 1 for one-shots), while instrument racks
  expose per-slot polyphony clearly because layered synths are the expensive
  case.
- **Choke groups** (drum rack): when a slot in choke group *g* triggers, send a
  fast note-off / gate-cut to all currently-sounding voices of other slots in
  group *g* (classic closed-hat chokes open-hat). Implement as a pre-trigger
  pass in the dispatch step that issues gate-off to the chokee voices.

## 5. Audio graph construction (`tui/graph.rs`)

New builder `add_rack_track(policy) -> usize` and
`add_slot_to_rack(track, slot_spec)`:

1. `create_track_shell(idx, name)` — **unchanged**; gives shared
   `voice_sum_id`/`voice_sum_r_id` → pan → fx → bus.
2. For each slot, build its instrument subgraph and **insert a per-slot
   sub-mixer** so per-slot gain/pan/mute/solo work:
   ```
   slot voices ─> slot_sum_l/r ─> slot_gain ─> slot_pan ─┬─> voice_sum_id (L)
                                                          └─> voice_sum_r_id (R)
   ```
   - Sampler slot: reuse `build_sampler_voices`, but connect voice outputs into
     `slot_sum_l/r` instead of directly into `voice_sum_id`. (Generalize
     `build_sampler_voices` to take target sum nodes — it already takes
     `voice_sum_id`/`voice_sum_r_id` params, so this is a call-site change.)
   - Custom slot: reuse `connect_engine_to_track` targeting `slot_sum_l/r`.
3. Extend `TrackNodeIds` with `slots: Vec<RackSlotNodeIds>` where
   `RackSlotNodeIds { sampler_ids, gatepitch_ids, modulator_ids, engine_id,
   slot_sum_l, slot_sum_r, slot_gain_id, slot_pan_id }`. Keep the existing
   top-level fields for `Single` tracks (back-compat; a single-instrument track
   is the degenerate one-slot rack but we keep the flat fields to avoid churn —
   see §8).

Node-count budget: a rack with `S` slots adds ~`S × (voices×3 + 4)` nodes. With
`MAX_TRACKS = 64` this can grow large; enforce a per-track slot cap (suggest 16
to match a 4×4 drum grid, configurable) and per-slot voice caps as in §4.3.

## 6. Trigger routing (`scheduler.rs`)

- At trigger resolution for a rack track, after computing the incoming note(s),
  apply `RackRouting::route(...)` to map each note to target slot indices.
- Emit one resolved sub-trigger per (slot, note), carrying the slot's
  `base_note_offset` and pointing at that slot's voice fan. This mirrors the
  existing `rebind_midi_fx_event_to_track` retarget logic, but the target is a
  **slot within the same track** rather than another track.
- For `ByPitch`, also run the choke pass (§4.3) before enqueueing.
- Plocks / instrument params / sampler params resolution
  (`resolve_instrument_params`, `resolve_sampler_params`) must become
  slot-aware: index by `(track, slot)` instead of `track`.

## 7. Serialization & migration (`project.rs`)

- Add a new `ProjectTrack` variant rather than mutating existing ones:
  ```
  Rack {
      routing: RackRouting,
      slots: Vec<ProjectRackSlot>,
      color, collapsed,
  }
  ```
  This keeps `Sampler` / `Custom` / `Modulator` tracks loading unchanged.
- `ProjectPattern` parallel `Vec`s: the cleanest path is to make the per-track
  instrument fields hold *either* a single instrument or a rack. Recommended:
  introduce `ProjectTrackInstrument` that is `Single(existing fields)` or
  `Rack(Vec<slot fields>)`, and migrate the parallel vecs
  (`instrument_types`, `instrument_slots`, `instrument_base_note_offsets`,
  `sample_paths`, `sample_names`, `graph_overrides`, `neural_networks`,
  `instrument_run_modes`) behind it. All new fields `#[serde(default)]` so old
  projects deserialize as `Single`.
- Add a `racks` migration test alongside the existing legacy-sampler
  deserialize tests in `project.rs`.

## 8. UI

Two views over the same container (`crates/sequencer/src/tui/…`, custom widgets
under `bin/metal_seq`). Use the `each`-based widget generation convention (see
memory: *Lisp UI: each vs map*, *Graph reactive bindings*) — never `map`.

- **Drum Rack**: 4×4 (or N×M) pad grid. Each pad shows slot name/sample,
  mute/solo, and selects the slot for editing its instrument + params panel
  below (reuse the existing per-track instrument param UI, scoped to the slot).
  Pad → `pad_note` assignment, choke-group selector.
- **Instrument Rack**: vertical chain list of slots; per-slot transpose, gain,
  pan, poly; an "add layer" button.
- Both: a track-level header switch (Single ↔ Rack) and routing-policy toggle.

## 9. Phasing

**Phase 1 — Instrument Rack (validates the container).** Cheapest routing
(`Broadcast`, no pad map, no choke). Forces us to solve the hard shared parts:
multi-slot data model, per-slot sub-mixer + summing into `voice_sum_id`,
slot-aware param resolution, and the voice budget. Ship layered synths/samplers.

**Phase 2 — Drum Rack (the payoff).** Add `ByPitch` routing, `pad_note`
assignment, choke groups, and the pad-grid UI on top of the Phase-1 container.
We already have the `Sampler` instrument type + sample DB feeding it, so this is
the bigger end-user unlock with mostly additive work.

### 9.1 Phase 1 task breakdown (Instrument Rack, `Broadcast`)

**Guiding constraint — each slot owns its voices.** A rack is a collection of
independent underlying instruments. Each sampler slot gets its own sampler pool;
each custom slot gets its own engine runtime. Slot polyphony is enforced per
slot, so a drum rack can keep most pads monophonic while an instrument rack can
still layer several polyphonic instruments. Cost is bounded by `MAX_RACK_SLOTS`
and by clamping each slot's `max_polyphony`, not by partitioning one shared
track voice pool.

Tasks are ordered so each lands compiling + tested before the next.

**T1 — Data model + serialization** (`project.rs`, `sequencer/data.rs`)
- Add `ProjectTrack::Rack { routing: RackRouting, slots: Vec<ProjectRackSlot>,
  color, collapsed }`; leave `Sampler`/`Custom`/`Modulator` untouched. Extend
  `ProjectTrack::color()`/`collapsed()` match arms.
- Add `enum RackRouting { Broadcast }` (one variant now; `ByPitch` in Phase 2),
  `#[serde(rename_all="snake_case")]`.
- Add `ProjectRackSlot { instrument_type, sample_path, instrument_name,
  run_mode, base_note_offset, gain, pan, mute, solo, instrument_slot:
  ProjectEffectSlot, graph_overrides, neural_network }` — drum-only fields
  (`pad_note`, `choke_group`) omitted until Phase 2. All new fields
  `#[serde(default)]`.
- *Checkpoint:* round-trip serde test + a legacy-project deserialize test
  (old project with no racks still loads), alongside the existing legacy-sampler
  tests in `project.rs`.

**T2 — Slot-indexed instrument param resolution** (`scheduler.rs`)
- Generalize `resolve_instrument_params` (`:519`), `resolve_instrument_defaults`
  (`:565`), `resolve_instrument_plocks` (`:655`), `resolve_sampler_params`
  (`:909`) to take a slot index and read `snapshot.tracks[track].slots[slot]`
  instead of the track's single `instrument_slot`. For non-rack tracks, slot 0
  maps to today's behavior (shim so the single-instrument path is literally
  `slot = 0`).
- *Checkpoint:* existing resolver tests pass unchanged (single track = slot 0);
  add one test resolving params for slot 1 of a 2-slot rack.

**T3 — Audio graph: per-slot sub-mixer + multi-instrument build** (`tui/graph.rs`)
- Extend `TrackNodeIds` (`ui/mod.rs`) with `slots: Vec<RackSlotNodeIds>`;
  `RackSlotNodeIds { sampler_pool_id, engine_id, slot_sum_l, slot_sum_r,
  slot_pan_id, ... }`. Keep flat fields for `Single` tracks.
- `build_sampler_voices` (`:2448`) already takes `voice_sum_id`/`voice_sum_r_id`
  — call it once per sampler slot with the slot's sampler pool and
  `max_polyphony`, connecting that slot's voices to `slot_sum_l/r` (which then
  feed the shared `voice_sum_id`). Same call-site change for
  `connect_engine_to_track` (`:2867`).
- New `add_rack_track(routing) -> usize`: `create_track_shell` (unchanged) →
  for each slot build sub-mixer (`slot_sum → slot_gain → slot_pan → voice_sum`)
  + its instrument subgraph → register each slot's sampler pool or engine
  runtime independently.
- Enforce `slots.len() <= MAX_RACK_SLOTS` and clamp/reject out-of-range
  per-slot `max_polyphony`.
- *Checkpoint:* headless `HeadlessEngine` test — a 2-slot broadcast rack
  (sampler + sampler) builds, both slots' voices wire into `voice_sum_id`, each
  slot enforces its own `max_polyphony`, per-slot gain node present.

**T4 — Trigger routing (`Broadcast`)** (`scheduler.rs`)
- At trigger resolution for a rack track, after computing incoming note(s), emit
  per-slot sub-triggers: for `Broadcast`, every unmuted slot (respect solo set),
  each applying the slot's `base_note_offset`, allocating from that slot's
  sampler pool or engine runtime. Reuse the `rebind_midi_fx_event_to_track`
  retarget pattern conceptually, but target a slot within the same track.
- *Checkpoint:* unit test — `Broadcast` on a 3-slot rack produces 3 sub-triggers
  hitting the 3 slots' independent voice allocators; muted slot is skipped; solo
  isolates.

**T5 — UI: instrument-rack chain view** (`ui/…`, `bin/metal_seq` custom widgets)
- Track header: Single ↔ Rack toggle; "add layer" to append a slot.
- Per-slot row: instrument picker (Sampler/Custom), transpose (`base_note_offset`),
  gain, pan, mute/solo, per-slot poly. Build children with `each` (owner
  metadata), never `map` — see memory *Lisp UI: each vs map* and *Graph reactive
  bindings*.
- Reuse the existing per-track instrument param panel, scoped to the selected
  slot (uses T2's slot-indexed resolution).
- Show per-slot polyphony and a rack cost summary so users can see when a layer
  stack is getting expensive.
- *Checkpoint:* `mk_ui` layout test for the chain view; manual audition of a
  2-layer rack (sampler + custom synth) via the dylib harness.

**T6 — Plumbing & polish**
- Track delete/reorder (`delete_track_shell` `:1761`, move logic) must free all
  slot nodes and keep the parallel vecs aligned (`debug_assert_track_vectors_aligned`).
- Project load/save path constructs racks via `add_rack_track` + slot adds.
- Hot-reload (`hot_reload_instrument` `:1012`) made slot-aware for Custom slots.
- *Checkpoint:* create → save → load → delete a rack track; vector-alignment
  assert holds; no leaked graph nodes.

**Out of Phase 1 (deferred):** `ByPitch`/pad map, choke groups, per-slot FX
chains, lifting the 12-voice per-track ceiling, nested racks.

**Phase 3 (optional).** Velocity/key zones, round-robin, per-slot send/FX,
nesting a rack inside a rack slot.

## 10. Testing

- `project.rs`: round-trip + legacy migration tests for the new `Rack` variant
  and `Single` back-compat.
- Routing unit tests: `Broadcast` hits all unmuted slots; `ByPitch` maps note→
  slot and respects choke groups (chokee voices receive gate-off).
- Graph build test (headless `HeadlessEngine`): a 2-slot broadcast rack sums
  both instruments into `voice_sum_id`; a drum rack routes note 36→slot0,
  38→slot1; assert node wiring and that voice counts stay within caps.
- Voice-budget test: per-slot `max_poly` enforced; slot count and per-slot caps
  keep graph size bounded.

## 11. Open questions (original)

1. Pick the concrete per-slot `max_polyphony` clamp and default values for
   broadcast racks vs drum racks. Drum rack pads should default low; instrument
   rack layers may default higher but must make cost visible.
2. Should a `Single` track be literally a one-slot rack internally (less code,
   more churn) or stay a separate flat path (more code, zero migration risk)?
   Spec recommends keeping `Single` flat and adding `Rack` as a sibling.
3. Per-slot FX chains in Phase 1 or defer to Phase 3? (Defer — slots get
   gain/pan/mute/solo only at first; the track FX chain is shared.)
4. Drum-rack pads: fixed grid size or growable? (Start fixed 16, revisit.)

---

## Amendment A — Per-Slot FX Chains & Rack Sounds

Status: draft / design
Author: design pass, 2026-07-15
Supersedes: original open question 3 (per-slot FX was deferred to Phase 3;
this amendment promotes it and specifies it).

### A1. Goal

Give every rack slot its own FX chain, Ableton-style:

```
slot voices ─> slot_sum_l/r ─> slot_pan ─> [slot FX chain] ─┬─> voice_sum_id (L)
                                                            └─> voice_sum_r_id (R)
              (track then applies its own shared FX chain to the sum, as today)
```

The driving product goal is a **"Sounds" tab**: ready-made instrument-rack
presets that bundle instrument(s) + effects. Loading a Sound swaps the rack on
the track and leaves the *track-level* FX chain untouched — the effects that
make the sound live inside the rack's slots, so presets never nuke a user's
track processing. This requires three things that don't exist yet, in
dependency order:

1. Per-slot FX chains (A4–A6).
2. "Convert track → rack" — fold an existing track's instrument + custom FX
   into a one-slot rack (A7).
3. A rack **Sound preset** format + browser tab (A8).

### A2. Current state (what this builds on)

- Rack container (Phase 1/2) is **built**: `RackSlotMixer`
  (`tui/graph.rs:155`) gives each slot `slot_sum_l/r → slot_pan`, with
  `slot_pan` port 0/1 feeding the track's `voice_sum_id`/`voice_sum_r_id`
  mono sums (`tui/graph.rs:4570`). The per-slot FX chain inserts exactly in
  that pan→sum gap.
- Effect chains are **uniform inserts**: `BUILTIN_SLOT_COUNT = 0`
  (`effects.rs:19`) — builtins (OTT, Roar, Space Echo, …) are ordinary chain
  slots. Once a slot can host a chain, it hosts every effect.
- The low-level insert API is **already host-agnostic**:
  `add_effect_to_chain_at` (`lisp_host.rs:1422`) takes arbitrary
  predecessor/successor node ids + a host-local diagnostic `slot_id`;
  `connect_custom_effect_gap` (`tui/graph.rs:3206`) handles mono/stereo
  adaptation.
- DGenLisp effect nodes own an immutable process-function identity in their
  initialized node state. Old and new nodes can therefore coexist while a
  replacement batch crosses the audio-thread boundary; track compaction and
  rack-slot addressing do not require registry-row re-keying.
- But buses were added by **duplication, not generalization**: parallel
  `add_builtin_effect_sync` / `add_bus_effect_sync`, `move_effect_slot_sync`
  / `move_bus_effect_slot_sync`, `delete_custom_effect_slot` /
  `delete_bus_effect_slot`, separate lease vectors
  (`track_effect_leases` / `bus_effect_leases`, `ui/mod.rs:408`), separate
  predecessor/successor finders. A third literal copy is the main thing this
  amendment refuses to do.

### A3. Locked decisions

1. **Generalize before adding the third host.** Introduce an `FxChainHost`
   seam (A4) and port track + bus chains onto it first. No third copy of the
   chain machinery.
2. **Process identity is node-owned; resource retirement is acknowledged.**
   A DGenLisp effect node carries its immutable process-function pointer in its
   initialized state. Loaded-artifact leases remain owned until the audio
   thread's applied-batch watermark reaches the batch that removed the node.
   No timeout or queue-empty estimate participates in correctness.
3. **v1 scope is edit + persist, not modulate.** Slot-FX params are editable
   and saved/loaded. P-locks on slot-FX params, macro mapping into slot FX,
   and sidechain taps *into* slot effects are all deferred (A9).
4. **Serialization is additive.** `ProjectRackSlotPattern` gains
   `effect_slots: Vec<ProjectEffectSlot>` with `#[serde(default)]`. No
   migration needed; no rack projects/presets exist in the wild yet, so this
   window is the cheapest it will ever be.
5. **Sound presets are instrument-rack presets.** The Sounds tab loads racks;
   loading replaces the rack (slots + slot FX + slot mix), never the
   track-level FX chain, sends, or routing.
6. **Rack macros are rack-scoped, not project-global.** (Locked 2026-07-15;
   implemented 2026-07-17.) Every rack owns eight macros
   knobs, the rack's public surface when collapsed) uses **rack-relative
   addressing** (slot index + param within the rack) and **serializes inside
   the rack blob / rack preset** — never as project-global
   `MacroMapping { track, target }` entries pointing into a rack, which break
   the moment a rack preset is loaded elsewhere or swapped between tracks.
   This is what makes a Sounds-tab entry ("closed rack + 6 knobs") portable.
7. **One override mechanism, two scopes.** Rack macros ride the same
   engine live-override layer and effective-value send path as project
   macros (`MACRO_MAPPING_SPEC.md` Phase 1) — a rack-local mapping table
   consulted in the same seam, not a parallel system. A rack macro is itself
   a param once it exists, so project macros can map onto rack macros
   (macro-of-macros) with zero extra machinery.

### A4. Phase R1 — `FxChainHost` generalization (refactor, no new features)

Define one description of "a place a chain lives":

```
struct FxChainHost {
    predecessor_id: i32,           // stereo node feeding the chain
    successor: ChainSuccessor,     // where the chain output lands
    // + accessors for: chain storage (Vec<EffectSlotState>),
    //   effect_descriptors row, lease row, display label
}

enum ChainSuccessor {
    StereoNode(i32),               // track: delay_id; bus: bus tail
    MonoPair { l: i32, r: i32 },   // rack slot: voice_sum_id / voice_sum_r_id
}
```

- Port the track chain and bus chain onto `FxChainHost`: one implementation
  each of add / move / delete / param-push / predecessor-successor search,
  parameterized by host. The 16 bus-specific functions in `tui/effects.rs`
  collapse into the shared impl. Move the shared implementation out of the
  oversized `tui/effects.rs` module as part of the refactor.
- `connect_custom_effect_gap` grows a `ChainSuccessor::MonoPair` arm: last
  effect out ch0 → `l`, ch1 → `r` (mono effect out fans to both).
- Lease storage unifies to one keyed map or a per-host row, replacing the
  `track_effect_leases` / `bus_effect_leases` pair.
- *Checkpoint:* zero behavior change. Existing track-FX and bus-FX tests pass
  unchanged; a save→load round-trip of a project with track + bus effects is
  bit-identical.

### A5. Phase R2 — rack-slot chains (engine)

**Node-owned process identity + acknowledged retirement.** Graph edits are
enqueued and applied at an audio block boundary. Every committed edit batch
has a monotonic serial, and the audio thread publishes the highest serial it
has fully applied. Replacing an effect creates a new node with its own process
pointer; the old node continues to reference only its old process pointer
until its deletion is applied. The host retains the old loaded-artifact lease
until `applied_batch_serial >= deletion_batch_serial`, checked opportunistically
on the UI tick and subsequent chain edits. This is exact, non-blocking, and
does not depend on the usual sub-50-ms drain time.

**Host wiring.** For a leased slot chain:

```
FxChainHost {
    predecessor_id: slots[slot].slot_pan_id,
    successor: MonoPair { l: voice_sum_id, r: voice_sum_r_id },
}
```

`RackSlotNodeIds` (`ui/mod.rs:607`) is unchanged — chain node ids live in the
chain storage, as they do for tracks.

**State + snapshot.** Slot chains get `Vec<EffectSlotState>` storage parallel
to the slot (inside the rack-slot state, not a new top-level parallel vec).
The audio-thread snapshot restore path (`sequencer/state.rs` effect-slot
restore) walks slot chains the same way it walks track chains.

**Lifecycle.**
- Slot delete / rack track delete: free all chain nodes, release the lease,
  drop dylib leases. Extend the `delete_track_shell` path and slot-removal
  path; `debug_assert_track_vectors_aligned` still holds.
- Slot reorder: chains move with their slot (lease map keys update; no node
  surgery).
- Hot-reload of a custom effect (`hot_reload_instrument`-adjacent path for
  fx) must find instances in slot chains too — falls out of A4 if reload
  iterates hosts, verify explicitly.

**Serialization.** `ProjectRackSlotPattern` (+`ProjectRackSlot` if distinct)
gains `#[serde(default)] effect_slots: Vec<ProjectEffectSlot>`; project load
builds slot chains through the same host API. Round-trip + legacy-load tests
alongside the existing rack tests in `project.rs`.

*Checkpoints:*
- Headless test: 2-slot rack, OTT on slot 0, custom dgenlisp fx on slot 1;
  assert wiring `slot_pan → fx → voice_sum_l/r`, both slots audible, slot 1
  chain removable without touching slot 0.
- Lease test: add/remove effects across slots recycles pool rows; exhaustion
  errors cleanly.
- Save → load → delete rack: no leaked nodes, no leaked leases.

### A6. Phase R3 — UI: slot chain view

- In the rack chain list, each slot row gains an FX section: the existing
  track FX-chain panel component, scoped to the slot's `FxChainHost` (add /
  reorder / delete / bypass, param panel for the selected effect).
- Same effect browser/picker as track chains (builtins + custom dgenlisp fx).
- Build children with `each` (owner metadata), never `map` (see memory:
  *Lisp UI: each vs map*).
- Show per-slot chain cost next to the existing per-slot poly/cost summary.
- *Checkpoint:* `mk_ui` layout test; manual audition — layered 2-slot rack,
  different reverb per slot, track-level FX still applied to the sum.

### A7. Phase R4a — Convert track → rack

Command on any `Sampler`/`Custom` track: **"Group to Instrument Rack"**.

1. Serialize the track's instrument (type, sample/instrument name, params,
   graph overrides, neural net) and its custom FX chain to the project-side
   structures (reuse the save path — no live node surgery).
2. Rebuild the track as a `Rack { Broadcast }` with one slot from that
   serialized instrument; load the serialized FX into the **slot's** chain.
3. Track-level chain ends empty (Ableton semantics: devices move into the
   rack). Keep sends/bus routing/track params untouched. Undo restores the
   flat track.
4. *Checkpoint:* convert a playing custom track with 2 effects → identical
   audio output before/after (offline render compare via the audition
   harness), then "add layer" works on the result.

### A8. Phase R4b — Sound presets + Sounds tab

- **Format:** a `.sound` preset = serialized rack (routing, slots incl. per-
  slot FX chains, slot mix, per-slot instrument params) + display metadata
  (name, tags, author). It is exactly `ProjectTrack::Rack`'s payload lifted
  out of a project — one serializer, shared with save/load.
- **Save:** "Save rack as Sound" from the rack header. A flat (non-rack)
  track offers "Save as Sound" via auto-convert (A7) on a temp copy.
- **Load/swap:** dropping a Sound on a track swaps the instrument container
  only — same rebind seam as instrument swap (see
  `docs/instrument-swap-spec.md`): rack slots + slot FX replaced, track FX /
  sends / pattern data preserved. Loading onto a flat track converts it to a
  rack.
- **Browser:** "Sounds" tab in the sidebar tab rail (see
  `docs/browser-tab-rail-spec.md`), flattened tree like builtins; drag-to-
  track + click-to-audition consistent with the instrument browser's
  keep-open audition behavior.
- *Checkpoint:* save a 2-slot rack w/ slot FX as a Sound; load it onto (a) a
  blank track, (b) a track with track-level FX — track FX survives in case
  (b); audition harness confirms the Sound reproduces the original render.

### A9. Deferred (explicitly out of scope for A5–A8)

- P-locks on slot-FX params (needs `(track, slot, fx_slot)` plock keys).
  **Priority follow-up, not a nice-to-have**: p-locks are the defining
  feature of this sequencer, and once rack macro banks exist (A3 #6/#7),
  p-locking a rack macro is the single highest-leverage version — one plock
  lane sweeping a whole curated sound. Design plock keys so a rack macro is
  addressable as a plock target from day one.
- Additional rack-macro curves/range editing beyond the persisted linear/exp/log
  model and captured full parameter range.
- Sidechain routing *into* slot effects (`refresh_effect_sidechain_labels`
  is track-keyed; needs a "Track N / Slot M / FX" naming scheme).
- Per-slot sends; nested racks; drum-rack per-pad return chains.

### A10. Open questions

1. Sound preset location: instrument-style directory
   (`content/instruments/…`) vs a user-level sounds dir? Sounds tab
   likely wants both (factory + user), mirroring the sample DB split.
2. Does "Group to Instrument Rack" also move *builtin* mixer stages (filter/
   delay in `TrackShell`) into the slot chain, or only chain inserts? (Start:
   chain inserts only; the shell stages are per-track mixer furniture.)

### A11. Rack macro bank (implemented 2026-07-17)

- Every rack defaults to exactly eight macros with immutable identifiers
  `:macro_1` through `:macro_8`; names and values are editable independently.
- Definitions, mappings, values, and p-lock rows serialize inside
  `ProjectRackTrackPattern`, including `.sound`/rack presets. Older racks load
  a default eight-macro bank.
- Rack-relative targets cover slot mixer, slot instrument, and slot FX params.
  UI mapping intentionally exposes slot instrument and slot FX controls, never
  track FX. Slot/effect deletion and effect insertion/reordering repair or drop
  affected mappings transactionally.
- Live changes push mapped values without mutating device defaults. At trigger
  time the rack macro is the effective default beneath target p-locks.
- `def-process` can target `(rack-macro :macro_1)`; process Set/Add values are
  carried in scheduled events and remain transient.
- `rack-panel-macros-open` controls the third rack-toolbar button and the 4x2
  bank. Renaming, live values, p-lock authoring, map/unmap, and mapping ownership
  are exposed through host commands and reactive rack state.
