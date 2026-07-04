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
`create_track_shell` (`crates/sequencer/src/ui/graph.rs`):

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

### Runtime audio nodes (`crates/sequencer/src/ui/mod.rs` + `ui/graph.rs`)
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

## 5. Audio graph construction (`ui/graph.rs`)

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

Two views over the same container (`crates/sequencer/src/ui/…`, custom widgets
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

**T3 — Audio graph: per-slot sub-mixer + multi-instrument build** (`ui/graph.rs`)
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

## 11. Open questions

1. Pick the concrete per-slot `max_polyphony` clamp and default values for
   broadcast racks vs drum racks. Drum rack pads should default low; instrument
   rack layers may default higher but must make cost visible.
2. Should a `Single` track be literally a one-slot rack internally (less code,
   more churn) or stay a separate flat path (more code, zero migration risk)?
   Spec recommends keeping `Single` flat and adding `Rack` as a sibling.
3. Per-slot FX chains in Phase 1 or defer to Phase 3? (Defer — slots get
   gain/pan/mute/solo only at first; the track FX chain is shared.)
4. Drum-rack pads: fixed grid size or growable? (Start fixed 16, revisit.)
