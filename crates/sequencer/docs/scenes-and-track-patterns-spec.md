# Scenes and Track Patterns Spec

## Goal

Introduce Ableton-style "clips" into the sequencer without abandoning the
groovebox-style global pattern workflow. Concretely:

- Keep project-wide pattern switching (load pattern 3 → every track loads its
  slice of pattern 3).
- Add **per-track** pattern selection: while the project is on pattern 5, let
  track 5 play pattern 3 — independently of the other tracks.
- Surface this as a small grid in each mixer channel strip, and (later) as a
  full tracks × scenes matrix panel — i.e. an Ableton Session view.

The insight driving the data model: today "project pattern *n*" and "track
pattern *n*" are the **same index**. The moment a track can diverge, that
implicit identity must become an **explicit indirection table**. That table is
exactly Ableton's clip/scene matrix.

## Terminology

Two explicit terms (this is the vocabulary used everywhere below and should be
used in code/UI):

- **Scene** — a project-wide row. Selecting a Scene sets, per track, which Track
  Pattern that track plays. The current global "pattern" becomes a Scene.
- **Track Pattern** — a track-specific pattern: one track's step sequence plus
  its per-track preset state (params, effects, instrument, chords, p-locks).
  Each track owns an independent **pool** of Track Patterns.

A Scene references **one Track Pattern per track**. The mixer/Session grid cell
`(scene, track)` is literally that reference.

## Non-Goals (first pass)

- Free-running, non-quantized clip launch per track with arbitrary clip lengths
  and warp markers. Track Patterns keep the existing fixed-width step model.
- Per-cell clip envelopes / automation lanes beyond the existing p-lock system.
- Follow actions, clip-launch legato modes, scene tempo/time-signature changes.
- Refcounted garbage collection of unused Track Patterns (see Orphans below — we
  start by keeping orphans).

These can layer on later. This spec is about the data model + the two launch
paths + the grid UI.

## Current Model (what exists today)

Patterns are already **hierarchical per track**, but addressed by a single
shared index.

- `ProjectFile.patterns: Vec<ProjectPattern>` and
  `ProjectFile.current_pattern: usize` — `project.rs:25,35`.
- `ProjectPattern` (`project.rs:208`) is a bundle of **parallel per-track
  lanes**: `track_bits: Vec<[u64; TRACK_PATTERN_WORDS]>`,
  `step_data: Vec<Vec<[f32; NUM_PARAMS]>>`, `track_params`, `effect_slots`,
  `midi_fx_slots`, `instrument_slots`, `chord_snapshots`, `*_plock_snapshots`,
  etc. Lane index = track index.
- Runtime mirror: `PatternState` (`state.rs:1032`) with
  `pattern_bank: Mutex<Vec<PatternSnapshot>>` and a single global
  `current_pattern: AtomicU32` (`state.rs:1041`).
- `PatternSnapshot` (`state.rs:58`) is the runtime equivalent of
  `ProjectPattern` — the same parallel per-track lanes.
- Switching: `switch_pattern` (`state.rs:1993`) captures the current snapshot,
  then `bank[new_idx].restore(self)` restores **all tracks at once**
  (`restore` loops `for t in 0..num_tracks`, `state.rs:489`), sets
  `current_pattern`, and bumps `transport.pattern_epoch`.
- Cloning: `clone_pattern` (`state.rs:2032`) snapshots current, pushes a full
  clone, points `current_pattern` at it. This is the "new pattern clones the
  current one" UX we preserve.

**What blocks per-track divergence:** there is exactly one `current_pattern`
index shared by all tracks, and `restore`/`switch_pattern` operate on the whole
snapshot. There is no notion of "track *t* is on a different pattern than the
rest."

**What already helps us:** the snapshot is fully lane-decomposed and there are
ready-made per-track primitives — `PatternSnapshot::clone_track_lane_from`
(`state.rs:582`) copies one track's entire lane from another snapshot, and the
`restore` body is already a per-track loop that can be factored into a
`restore_track(t)`. The per-track machinery mostly exists; it just isn't
addressable yet.

## Target Data Model

Flip ownership: separate the **per-track pattern pool** from the **scene
reference table**.

```rust
// A single track's pattern = one lane-slice of today's PatternSnapshot.
// (Same fields, but for ONE track instead of Vec-per-track.)
pub struct TrackPatternData {
    pub track_bits: [u64; TRACK_PATTERN_WORDS],
    pub neural_reset_bits: [u64; TRACK_PATTERN_WORDS],
    pub step_data: Vec<[f32; NUM_PARAMS]>,
    pub track_params: TrackParamsSnapshot,
    pub effect_slots: Vec<EffectSlotSnapshot>,
    pub midi_fx_slots: Vec<EffectSlotSnapshot>,
    pub instrument_slots: EffectSlotSnapshot,
    pub instrument_base_note_offset: f32,
    pub track_sound_state: TrackSoundState,
    pub sample_id: (i32, String, u32),
    pub chord_snapshot: ChordSnapshot,
    pub timebase_plock_snapshot: [Option<u32>; MAX_STEPS],
    pub swing_plock_snapshot: [Option<u32>; MAX_STEPS],
    pub swing_resolution_plock_snapshot: [Option<u32>; MAX_STEPS],
    pub instrument_type: InstrumentType,
    pub instrument_run_mode: CustomInstrumentRunMode,
}

pub struct PatternId(pub u64); // stable id, NOT a pool index

// Each track owns its own pool of patterns, keyed by stable id.
pub struct TrackPatternPool {
    pub patterns: HashMap<PatternId, TrackPatternData>, // or Vec<(PatternId, _)>
    pub next_id: u64,
}

// A Scene is a row of references: for each track, which pattern it selects.
// None = "this track has no pattern in this scene" (see launch semantics).
pub struct Scene {
    pub name: String,
    pub cells: Vec<Option<PatternId>>, // indexed by track
}
```

Project-level state becomes:

```rust
pub struct ProjectScenes {
    pub track_pools: Vec<TrackPatternPool>, // indexed by track
    pub scenes: Vec<Scene>,
    pub current_scene: usize,
    // Per-track override of the scene's cell — set when a track is launched to a
    // different pattern than the current scene dictates. None = follow scene.
    pub track_overrides: Vec<Option<PatternId>>,
}
```

The **effective pattern for track `t`** is:
`track_overrides[t].or(scenes[current_scene].cells[t])`.

Project-wide entities that today live on `ProjectPattern` but are not really
per-track-pattern data (`mod_connections`, `neural_networks`, `graph_overrides`,
`bus_patterns`) stay attached to the **Scene**, not the Track Pattern, unless we
later decide buses/mods should also be per-track-launchable. Document the choice
explicitly in code; do not silently drop them.

### Mapping to runtime

`PatternState` gains:

- `track_pools: Vec<Mutex<TrackPatternPool>>` (or one `Mutex` over all pools).
- `scenes: Mutex<Vec<Scene>>`.
- `current_scene: AtomicU32` (replaces `current_pattern`'s meaning).
- `current_track_pattern: Vec<AtomicU64>` — the resolved `PatternId` currently
  loaded per track (so the audio/scheduler side reads one atomic per track, no
  locking on the hot path). This is the per-track index you confirmed wanting.

`pattern_epoch` (`state.rs:1057`) keeps signalling "pattern state changed" to the
scheduler and is bumped by both launch paths.

## Behavior Spec

### B1. Shared Track Patterns — edits propagate

A Track Pattern is **owned once** in its track's pool. Scenes and overrides hold
`PatternId`s, never copies. Therefore:

- If `scenes[2].cells[3] == scenes[3].cells[3] == PatternId(4)`, editing any
  step/param of track 3's pattern while either scene is active mutates the single
  pool entry. Both scenes reflect the change. There is no per-scene duplicate to
  drift.

This is deliberately the **opposite** of Ableton-strict (every cell its own
clip). Sharing is the default behavior of references; divergence is explicit
(see B2).

Implementation note: live edits already flow into the current snapshot via the
existing `step_data`/`track_params` mutators. With the pool model, edits target
`track_pools[t].patterns[effective_id(t)]`. There is exactly one writer location
per track per field, so propagation is automatic.

### B2. Two explicit verbs: Share and Fork

- **Share** — point a cell (or override) at an **existing** `PatternId`. Used
  when the user assigns "track 3 plays pattern 4" in a scene where it already
  exists elsewhere. Edits propagate (B1).
- **Fork** — clone the current Track Pattern into a **new** `PatternId`, append
  to the track's pool, and repoint the cell/override at the new id. This is the
  deliberate "I want this to diverge now" action.

No hidden copy-on-write. Editing a shared pattern never auto-forks. What is
referenced is what changes.

### B3. Creating a new Track Pattern clones the current one

When the user spawns a new Track Pattern for a track (clicking an empty/next grid
cell in that channel), the default is **clone the track's currently effective
pattern** into a fresh `PatternId` (deep copy of all lanes via the existing
`clone_track_lane_from` logic, adapted to single-track), then launch the track
onto it (B5). "New empty" is a secondary option; **clone-current is the
default**, mirroring today's `clone_pattern` UX.

### B4. Creating a new Scene

Creating a new project-wide Scene **forks the current effective pattern in each
track** (allocate next `PatternId` per track, clone-current), and sets the new
scene's cells to those new ids. This reproduces the worked example:

- Start: 1 scene, 2 tracks. `track_pools = [[A], [A]]`,
  `scenes[0].cells = [A, A]`.
- User forks track 2 only (B3): `track_pools = [[A], [A, B]]`; track 2 override →
  B.
- User creates Scene 2 → fork current in each track: track 1 gets its 2nd
  pattern, track 2 gets its 3rd. `track_pools = [[A, A2], [A, B, B3]]`,
  `scenes[1].cells = [A2, B3]`.

That is the "preset 2 in track 1, preset 3 in track 2" outcome. The old
`project pattern n == track pattern n` assumption is gone; allocation is per-track
and the Scene only records references.

Note: a Scene's cells may also be **shared** with another scene by explicit
assignment (B2 Share) — new-scene defaults to fork, but the grid lets you set any
cell to any existing pattern.

### B5. Launch semantics

- **Launch Scene `s`** (`launch_scene`): for each track `t`, clear
  `track_overrides[t]`, resolve `id = scenes[s].cells[t]`, and if `Some(id)`
  restore that track's lane from the pool into live state; if `None`, **silence
  the track** for this scene (strict Session-view semantics — an empty cell means
  nothing plays on that track; **resolved decision**). Set `current_scene = s`,
  bump `pattern_epoch`. Replaces today's `switch_pattern`.
- **Launch Track Pattern `(t, id)`** (`launch_track_pattern`): set
  `track_overrides[t] = Some(id)`, restore only track `t`'s lane (factor the
  per-track body out of `restore`, `state.rs:489`), update
  `current_track_pattern[t]`, bump `pattern_epoch`. This is the single-cell /
  per-track launch — the lisp native and the mixer grid both call this.
- Before any restore that switches *away* from an effective pattern, **capture**
  the live edits back into that pattern's pool entry (mirror the existing
  capture-before-switch in `switch_pattern`/`clone_pattern`, but per track). This
  is what makes B1 edits durable.

### B6. Quantization (project-wide option)

Today pattern switches are **immediate** (no bar quantization). Add a
project-wide **Launch Quantization** setting (like Ableton's transport-bar
quantize menu): `Off | 1 bar | 1/2 | 1/4 | ...`. When non-Off, `launch_scene`
and `launch_track_pattern` do not apply immediately — they enqueue a **pending
launch** that the scheduler commits at the next quantize boundary.

- Store pending launches in transport state (e.g.
  `pending_scene: AtomicU32` sentinel + `pending_track_pattern: Vec<AtomicU64>`).
- The scheduler, which already consumes `pattern_epoch`, applies pending launches
  on the boundary and then bumps `pattern_epoch`.
- `Off` preserves today's immediate behavior. **Resolved default: 1/4 (beat)** —
  launches commit on the next beat, giving fast response while still snapping.
  The menu still offers `Off | 1 bar | 1/2 | 1/4 | ...`.

**Silencing an empty-cell track (B5).** "Silence" must not lose the track's
state — it means: mute the track's audio output / suppress its triggers for the
duration the scene's cell is `None`, without overwriting its pool patterns.
Implement via a per-track `scene_silenced: Vec<AtomicBool>` consulted by the
scheduler/voice trigger path (separate from the user-facing mute in
`track_params`, so a user un-mute doesn't fight it). Launching a scene whose cell
is `Some(id)` clears the flag and restores; `None` sets it.

This is a transport/scheduler concern layered on top of B5; the data model does
not change.

### B7. Orphans / delete semantics

Because patterns are shared references, removing a pattern from a scene is
**dropping a reference, not deleting data**. First pass:

- Clearing a cell sets `scenes[s].cells[t] = None` (and clears any matching
  override). The `PatternId` stays in the pool even if nothing references it
  ("orphan"). This matches a groovebox "your patterns persist" feel and lets you
  re-point an orphan later.
- Provide an explicit **"purge unused Track Patterns"** action (and optional
  per-track pattern list UI) instead of automatic GC.
- Refcounting + auto-free is a later optimization, only if pool memory becomes a
  real problem. If we ever auto-delete, it must check no scene/override
  references the id.

Deleting a **track** must remove that track's pool and its column from every
scene/override (extend the existing `PatternSnapshot::remove_track` lane logic,
`state.rs:87`).

## Migration (lossless, identity mapping)

Existing projects map into the new model as the degenerate identity case:

- For each existing `ProjectPattern` index `p` and each track `t`, create a
  `TrackPatternData` from that pattern's lane `t` → assign a `PatternId`.
- `scenes[p].cells[t] = ` that pattern's track-`t` id, for all `t`.
- `current_scene = old current_pattern`. All `track_overrides = None`.

Same audible behavior, new representation. Bump `PROJECT_FILE_VERSION`
(`project.rs:16`) and keep a `#[serde]` path that reads the old
`patterns: Vec<ProjectPattern>` + `current_pattern` and constructs
`ProjectScenes` on load. Write only the new format. Round-trip test: load an old
project, save, reload → identical step/param/effect state and same active scene.

## Runtime / Code Changes

1. **Snapshot decomposition** — factor `PatternSnapshot` lane access into a
   single-track view. Add `TrackPatternData` (the lane slice) and:
   - `PatternSnapshot::restore_track(&self, state, t)` — extract the body of the
     `for t` loop in `restore` (`state.rs:489`).
   - `capture_track(&self, t) -> TrackPatternData` and the inverse — reuse the
     field list from `clone_track_lane_from` (`state.rs:582`).
2. **PatternState** — add `track_pools`, `scenes`, `current_scene`,
   `current_track_pattern`, `track_overrides`. Keep `pattern_bank` only as long
   as needed for migration, then remove.
3. **Launch paths** — implement `launch_scene` (replaces `switch_pattern`) and
   `launch_track_pattern`; both capture-before-switch and bump `pattern_epoch`.
4. **Allocation** — `fork_track_pattern(t)` (clone current lane → new id) and
   `new_scene()` (fork per track, B4). `clone_pattern` (`state.rs:2032`) is
   superseded by `new_scene`.
5. **Quantization** — pending-launch fields in transport + scheduler commit on
   boundary (B6).
6. **Scheduler** — `publish_scheduler_snapshot` already runs after switches;
   ensure it reflects per-track effective patterns.
7. **Project (de)serialization** — `ProjectScenes` + migration (above).

Keep each step independently compilable; land the snapshot decomposition and the
identity migration first so behavior is unchanged before any UI exists.

## Lisp Natives

Same shape as existing natives (`natives.rs`, `runtime.register_native(name,
closure)`):

- `(seq-launch-scene s)` — project-wide launch (B5). Replaces the existing
  switch-pattern native.
- `(seq-launch-track-pattern track id)` — per-track launch (B5). The user's
  "load pattern N for track M".
- `(seq-fork-track-pattern track)` → returns new `PatternId` (B3).
- `(seq-new-scene)` → returns new scene index (B4).
- `(seq-set-cell scene track id)` / `(seq-clear-cell scene track)` — Share /
  orphan a cell (B2/B7).
- `(seq-launch-quantize "1bar" | "off" | ...)` — B6.

All of them ultimately call the same `launch_*` / allocation functions the grid
UI calls. No engine logic lives in lisp.

## UI

Two views over the same `cells` / override state:

1. **Per-strip mini selector** (mixer channel). The strip is tight (see the
   `main` dropdown / A-B macros / sends layout), so per channel show **one
   column**: a small vertical/4×4 selector of that track's pool, highlighting the
   effective pattern. Click an existing pattern → `launch_track_pattern` (Share +
   launch). Click "next/empty" → `fork_track_pattern` + launch (B3). This is the
   quick "bump this track to pattern 3" affordance.
2. **Session grid panel** (later). The full tracks × scenes matrix — the actual
   Ableton Session view. Cell click = launch that cell; row launch = launch
   scene; right-click = Share/Fork/Clear/duplicate. The Max "preset grid" image
   is this panel, not the per-strip widget.

Mixer widgets are built per track in `ui/projects.rs` (`push_track_volume`,
`push_track_pan` around `:715-770`) with state in `ui/mod.rs`
(`BusChannelState`, `TrackNodeIds`). The mini selector slots into the same
per-track widget construction. Follow the project rule of generating widget
children with `each` (owner metadata), never `map` (see memory
[[lisp-ui-each-vs-map]]).

## Resolved Decisions

All five open decisions are settled (locked for the first implementation pass):

1. **Empty-cell launch policy (B5)** — **Silence the track.** `None` cell ⇒ the
   track plays nothing for that scene (strict Session-view semantics), via a
   `scene_silenced` flag that does not overwrite pool state. See B5/B6.
2. **What a Track Pattern includes (B1/B5)** — **Full preset.** A Track Pattern
   carries steps + instrument + effects + chords + p-locks, exactly as today's
   `restore` does. A "notes-only" mode is explicitly out of scope for now.
3. **Mod/neural/graph/bus scope (data model)** — **Scene level only.**
   `mod_connections`, `neural_networks`, `graph_overrides`, `bus_patterns` belong
   to the Scene, not the Track Pattern; per-track launch does not touch them.
   `TrackPatternData` therefore omits these fields.
4. **Launch quantization default (B6)** — **1/4 (beat).** Menu still exposes
   `Off | 1 bar | 1/2 | 1/4 | ...`; the project default is beat-quantized.
5. **Orphan strategy (B7)** — **Keep + manual purge.** Cleared/re-pointed
   patterns stay in the pool and remain re-shareable; add an explicit "purge
   unused" action. No automatic refcount GC in the first pass.

## Test Plan

- **Migration round-trip** — old project loads, saves, reloads identical;
  `current_scene` == old `current_pattern`.
- **Identity equivalence** — with no overrides set, `launch_scene` reproduces
  the exact behavior of the old `switch_pattern` (golden snapshot of live state).
- **Per-track launch isolation** — launch track `t` to a different pattern; all
  other tracks' live state byte-identical; only track `t` changed.
- **Sharing propagation (B1)** — two scenes share a track id; edit in one; assert
  the other scene's launch reflects the edit.
- **Fork divergence (B2/B3)** — fork a shared pattern; edit; assert the original
  (still referenced elsewhere) is unchanged.
- **New scene allocation (B4)** — reproduce the 2-track worked example; assert
  pool sizes and cell ids match.
- **Orphan persistence (B7)** — clear a cell; assert the pattern remains in the
  pool and can be re-shared.
- **Quantize (B6)** — with 1-bar quantize, a launch issued mid-bar commits on the
  next bar boundary, not immediately.
- **Capture-before-switch** — edit, launch away, launch back; assert the edit
  persisted into the pool.
