# Takes, Clip Phase Anchoring, and Additive Arrangement Recording Spec

Status: draft / design, 2026-07-23
Related: `docs/song-mode-spec.md`, `docs/arrangement-timeline-ui-spec.md`,
`docs/record-quantize-spec.md`,
`crates/sequencer/src/sequencer/state/song.rs`,
`crates/sequencer/src/sequencer/state/scenes.rs`,
`crates/sequencer/src/sequencer/state/song_runtime.rs`,
`crates/sequencer/src/app/song_capture.rs`,
`crates/sequencer/src/app/song_transport.rs`,
`crates/sequencer/src/scheduler/lookahead.rs`,
`crates/sequencer/src/ui/input.rs`

## 1. Summary

The current song model is loop-driven: every clip on the timeline is a scene
cell or a pool pattern that free-runs against the global clock. This spec adds
the timeline-native half of the system:

1. **Clip phase anchoring** — every lane clip gains a start offset so that
   "where the clip sits on the timeline" and "where inside the pattern it
   starts" are independent. This subsumes today's free-running phase as a
   special case and is the foundation for everything below.
2. **Takes** — linear, non-looping, chunked recordings owned by a track but
   hidden from the mixer clip grid. A take is a thin ownership/indexing layer
   over ordinary pool patterns, not a new content type.
3. **Take recording** — arm a track, hit record during song playback, and
   punch a performance into the arrangement at the exact beat recording
   started.
4. **Additive arrangement capture** — recording scene/track launches in song
   mode splices the captured region into the existing song instead of
   replacing the whole song.
5. **Bare tracks** — new tracks default to empty everywhere; the timeline
   shows an empty lane to record takes onto, not a pre-populated clip per
   scene launch.
6. **Back to Song** — manual launches during song playback put the affected
   scope into a manual-override state with an explicit control to re-follow
   the timeline (Ableton's "Back to Arrangement").

Everything here is pattern/MIDI arrangement. Audio clips remain out of scope.

## 2. Current state (facts this spec builds on)

- Patterns live in a flat per-track pool: `TrackPatternPool { patterns:
  HashMap<PatternId, TrackPatternData>, next_id }` (`scenes.rs:29`). Scenes
  are views: `Scene.cells: Vec<Option<PatternId>>` referencing pool ids.
  `PatternId`s are minted monotonically per track.
- `ProjectSongRow { id, start_beat: f64, scene: usize, overrides }` with
  `ProjectSongTrackOverride { track, pattern_id: Option<u64> }`
  (`song.rs:30-45`). `None` is the explicit-empty override. Lane resolution
  is override-else-scene-cell-else-`None` (`project_lanes`, `song.rs:287`).
- A pattern holds at most `MAX_STEPS = 256` steps
  (`sequencer/data.rs:19`); note timing finer than a step lives in
  `chord_delays` / step delay params (see `docs/record-quantize-spec.md`).
- Patterns free-run against the global clock through non-wrap row
  boundaries. Only a song loop wrap resets `clock_beat_offset` and all
  runtimes (`song_runtime.rs:428-501`). The diff-aware accumulator reset
  (`lookahead.rs:250-266`, `mark_song_row_accum_resets`) flags per-track
  accumulator resets only when a track's resolved pattern id changes across
  a row boundary.
- Arrangement capture (`song_capture.rs`) records every audible launch via
  the single seam `App::apply_pattern_launch`, then on stop consolidates and
  commits via `song_replace` — a wholesale replacement of the committed
  song.
- Live note recording (`ui/input.rs` release branch) writes into the current
  live pattern of the pressed track, quantized per `RecordQuantize`.

## 3. Terminology

**Clip**
: One resolved lane span on the timeline: a source (pattern or take), a
  timeline start beat, and a start offset into the source.

**Take**
: A linear, non-looping recording on one track, stored as an ordered list of
  chunk patterns claimed from that track's pool. Referenced from the song by
  `TakeId`, drawn and edited as one clip.

**Chunk**
: One ordinary `TrackPatternData` in the pool that a take owns. All chunks
  of a take are `MAX_STEPS` long except possibly the last.

**Start offset**
: Steps (fractional) into the source at which the clip's timeline start
  maps. `offset = 0` means the clip begins at source step 0.

**Anchored phase**
: Pattern position derived from the clip (`start_beat` + offset), not from
  the global clock.

**Punch-in / punch-out**
: The song beats at which additive recording begins and ends affecting the
  committed song.

**Armed track**
: A track whose record-arm flag is set; note input during song-mode
  recording is written into a take on that track.

## 4. Goals

- One phase model (`start_beat` + `offset`) that exactly reproduces today's
  free-run behavior for captured scene launches and gives painted clips and
  takes deterministic clip-relative phase.
- Takes reuse `TrackPatternData`, the pattern write paths, and pattern
  serialization unchanged. No second content format.
- The mixer clip grid never shows take chunks; the timeline shows a take as
  a single clip.
- Arrangement recording never destroys song content outside the punched
  region. One undo entry per commit, atomic, validation-failure leaves the
  committed song intact (same guarantees as `song_capture.rs` today).
- Recording a take and capturing launches are the same transport gesture
  (record during song mode) and commit together.
- New tracks are bare by default.

## 5. Non-goals

- Audio takes / waveform clips.
- Comping (multiple takes layered on one region with pick-a-winner UI). The
  model must not preclude it — takes are already distinct entities — but no
  comping UI is specified here.
- Automation recording into the arrangement.
- Post-record destructive quantization of takes.
- Tempo automation.
- Cross-track take moves (a take's chunks live in one track's pool; moving a
  take to another track is a copy, and is deferred entirely).

## 6. Data model

### 6.1 Take entity

Per track, alongside the pattern pool:

```rust
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TakeId(pub u64);

#[derive(Clone, Serialize, Deserialize)]
pub struct TrackTake {
    pub id: TakeId,
    pub name: String,              // "Take 1", user-renamable
    /// Ordered chunk patterns in this track's pool. Invariant: every chunk
    /// except the last covers MAX_STEPS steps; chunks are exclusive to one
    /// take and never referenced by any scene cell.
    pub chunks: Vec<PatternId>,
    /// Playable length in steps (fractional tail is allowed conceptually,
    /// but note timing finer than a step lives in chord delays, so u32).
    pub total_len_steps: u32,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct TrackTakePool {
    pub takes: Vec<TrackTake>,     // small; linear scan by id is fine
    pub next_take_id: u64,         // monotonic per track, never reused
}
```

`ProjectScenes` gains `take_pools: Vec<TrackTakePool>` (serde-default,
grown in `save_scene_snapshot` exactly like `track_pools`).

**Hidden is derived, not stored.** A pool pattern is hidden from the clip
grid iff some take's `chunks` contains its id. There is no flag on
`TrackPatternData`; ownership is the single source of truth. Provide
`TrackTakePool::claimed(&self) -> impl Iterator<Item = PatternId>` and a
cached set where the grid needs it per frame.

**Non-looping is derived, not stored.** Looping is a property of how a lane
resolves its source (7.3). Takes never wrap; the lane is silent past
`total_len_steps`.

Chunk indexing at resolve time, given clip-local position `p` in steps:

```text
chunk  = floor(p / MAX_STEPS)
step   = p mod MAX_STEPS
source = take.chunks[chunk]        // out of range => silent
```

Sizing note: `MAX_STEPS = 256` is 16 bars at 16 steps/bar. A 5-minute take
at 120 BPM (~150 bars) is ~10 chunks. `chunks` stays tiny.

### 6.2 Lane source and offset on the override

`ProjectSongTrackOverride` grows two serde-compatible fields:

```rust
#[derive(Clone, Serialize, Deserialize)]
pub struct ProjectSongTrackOverride {
    pub track: usize,
    /// Legacy field, unchanged: Some(pattern) or explicit-empty None.
    pub pattern_id: Option<u64>,
    /// If Some, this override plays a take and pattern_id is ignored
    /// (validation requires pattern_id == None when take_id is Some).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub take_id: Option<u64>,
    /// Start offset into the source, in fractional pattern steps of this
    /// track's timebase. Default 0.0. Meaningful for both patterns and
    /// takes.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub offset_steps: f64,
}
```

In-memory, resolution exposes an enum so downstream code never juggles the
two options:

```rust
pub enum LaneSource {
    Pattern(PatternId),
    Take(TakeId),
    Empty,
}
```

Existing projects deserialize unchanged (`take_id = None`,
`offset_steps = 0.0` ⇒ exactly today's semantics). The legacy
bare-number serde form for `pattern_id` keeps working.

**Locked decision: offset lives on the override only.** `ProjectSongRow`
and scene cells do not carry offsets. Any lane whose phase must deviate
from offset-0 anchored playback gets a materialized override. In
particular, capture (section 9) materializes overrides with stamped
offsets for scene-resolved lanes where continuity requires it. This keeps
rows and scenes untouched and makes the override the single place phase is
stored.

`LaneClip` (the `project_lanes` output, `song.rs:62`) gains
`source: LaneSource` and `offset_steps: f64` so the timeline UI and the
scheduler consume identical resolution.

### 6.3 Validation additions (extends song spec 5.3)

- `take_id` must name an existing take in that track's `TrackTakePool`.
- `take_id.is_some() ⇒ pattern_id.is_none()`.
- `offset_steps >= 0`. For pattern sources, `offset_steps` is taken modulo
  pattern length at resolve time (storing it un-normalized is legal); for
  take sources, `offset_steps < total_len_steps` (a clip starting past the
  end of its take is invalid).
- Take invariants: chunk list non-empty; every chunk id exists in the
  track's pool; no chunk is referenced by any scene cell or by another
  take; `total_len_steps <= chunks.len() * MAX_STEPS`.

### 6.4 Lifecycle

- Takes are created only by take recording (section 8) — no UI mints empty
  takes in V1.
- Deleting a take deletes its chunk patterns from the pool and removes every
  song override referencing it (those lanes fall back to scene-cell
  resolution unless the user had layered explicit-empty elsewhere). One undo
  entry.
- Deleting the last song clip that references a take does **not** delete the
  take (it may be re-placed later; this also keeps comping open). Orphaned
  takes are visible in a per-track take list (UI, 11.3) where they can be
  deleted explicitly. No silent GC.
- Deleting a track drops its pool and take pool together (existing column
  semantics).

## 7. Phase model

### 7.1 The formula

For a lane clip with timeline start `S` (beats), source length `L` (steps),
offset `o` (steps), and the track's beat→step mapping `steps(x)` (timebase /
swing domain, the same mapping the free-running clock uses today):

```text
p(beat) = steps(beat - S) + o          // clip-local position, steps

Pattern source:  play step  p mod L            (loops)
Take source:     play chunk/step per 6.1 if p < total_len_steps, else silent
```

### 7.2 Free-run is an offset value, not a mode

Today a scene-launched pattern's position is `steps(beat) mod L`.
Algebraically:

```text
steps(beat) mod L == (steps(beat - S) + (steps(S) mod L)) mod L
```

i.e. the anchored formula with `o = steps(S) mod L`. Therefore:

- **Capture stamps** `offset_steps = steps(row.start_beat) mod L` on the
  materialized override for every lane where the performance free-ran into
  the row (section 9.4). Committed playback is then identical to what was
  performed — launching Scene 2 three-quarters through a bar plays the last
  quarter first, forever, even after the clip is dragged elsewhere.
- **Painted clips** get `offset_steps = 0`: painting a clip at bar 9 means
  it starts at step 0 at bar 9. (This is the deferred "phase-anchor fix"
  from the track-clip editing work, now specified.)
- **Takes** get `offset_steps = 0` by construction: recording writes
  clip-relative positions, so source step 0 *is* the punch-in moment.

There is exactly one runtime phase mechanism. No free-run flag survives.

### 7.3 Runtime changes

`lookahead.rs` / `song_runtime.rs` derive each track's step position from
the active `LaneClip` (`start_beat`, `offset_steps`, source) instead of the
shared global clock position:

- Per-track step position = formula in 7.1, evaluated in the track's
  existing timebase/swing domain. The global clock still drives beats;
  only the beat→pattern-step projection becomes per-lane.
- Take sources additionally resolve chunk + local step, and emit nothing
  once `p >= total_len_steps` (the lane is silent, not wrapped, until the
  next row changes the source).
- The diff-aware accumulator reset stays: a row boundary where
  `resolved_pattern_ids` (now: resolved *source*, take-aware) differs
  flags a reset for that track. Crossing chunk boundaries *within* one
  take is **not** a source change — no accumulator reset, no retrig of
  held state; a take is one continuous clip. `resolved` identity for a
  take lane is the `TakeId`, not the current chunk's `PatternId`.
- Song loop wrap behavior is unchanged (full reset), and is now also
  correct-by-construction for phase since every lane recomputes `p` from
  its own anchor.

### 7.4 Editing semantics that fall out

- **Move clip**: change `start_beat`, keep `offset_steps` → content moves
  rigidly with the clip.
- **Resize left** (trim head): `start_beat += d`, `offset_steps +=
  steps(d)` → later content is revealed/hidden at the left edge instead of
  the old occlusion behavior. This upgrades the existing merged-lane resize
  gesture; resize-right remains pure occlusion (for takes, resize-right
  beyond `total_len_steps` is clamped).
- **Split clip** at beat `B`: right half gets `start_beat = B`,
  `offset_steps += steps(B - S)`.

### 7.5 Compatibility with existing songs

None required — no saved projects contain arrangements (locked decision).
Legacy projects deserialize fine mechanically (serde defaults give
`offset_steps = 0`, `take_id = None`), and since no project has song rows,
there is no audible-phase migration to perform. Do not build an
offset-stamping load pass.

## 8. Take recording

### 8.1 Arming

- New per-track `record_armed: bool` (session/UI state, serialized with the
  project like mute/solo; not part of the song model). Mixer track strip
  and Arr-view track header both expose the arm toggle.
- Arming is independent of track selection. Multiple tracks may be armed;
  each armed track records its own take.

### 8.2 Transport gesture

Recording in song mode (Use Arrangement on + record on) is **one mode**
that captures two streams simultaneously:

1. Launch events (scene / track-pattern launches) → spliced rows
   (section 9).
2. Note input on armed tracks → takes.

There is no separate "take mode". If nothing is armed and no launches are
performed, stopping commits nothing (no-op, no undo entry). The song keeps
playing normally while recording — the committed song remains the launch
authority except where the performer overrides it (section 10).

### 8.3 Punch-in

Per armed track, the take is minted lazily on the first recorded note (not
at record-press), so arming a track and playing nothing leaves no debris.
Punch-in beat `P` = the beat of the first note after quantization policy:

- `RecordQuantize::Off`: `P` = the exact performed beat; the note's
  sub-step phase is preserved via chord delay, and `P` is floored to the
  step grid for the row split (the sub-step remainder lives inside the
  take as step-0 delay).
- Grid quantize: `P` = the quantized step boundary.

At punch-in, one pending edit is prepared (not yet committed): split the
song at `P` for that track and point the lane at the new take with
`offset_steps = 0`. During recording this is a live preview override (same
mechanism as capture's non-destructive take state today — the committed
song is untouched until stop).

### 8.4 Writing notes

The existing release-branch recording path (`ui/input.rs`) is retargeted:
when song-mode recording is active and the pressed track is armed, notes
write into the take at clip-relative position `steps(beat - P)` instead of
into the current live pattern:

- Chunk rollover: when the clip-local step crosses `chunks.len() *
  MAX_STEPS`, append a fresh pool pattern to `take.chunks` and continue.
  Chunks are minted from the same `TrackPatternPool::insert`.
- `RecordQuantize` semantics (including `Off` preserving sub-step phase as
  per-note delay, and notes quantizing to a step the playhead has passed
  landing on the *next* cycle — here, simply the correct absolute step)
  apply unchanged; only the target coordinates change.
- Step params (`Transpose`, `Velocity`, `Duration`) and chord data write
  exactly as today, into the chunk pattern.
- Note-off after punch-out (8.5) still records its duration; the take's
  `total_len_steps` extends to cover the final release.
- Monitoring: an armed track sounds the performer's input live (this is
  today's behavior for played notes; nothing new).

An unarmed track pressed during song-mode recording behaves as today
(writes into its current live pattern) — that path is unchanged and out of
scope here.

### 8.5 Punch-out and commit

Punch-out beat `Q` per take = the step after the last recorded note-on
(rounded up to the step grid), i.e. takes end where the performance ended,
not where the transport stopped. On transport stop (or record toggled
off), commit atomically, together with the launch splice (9.5), as **one
undo entry**:

- Finalize `total_len_steps = steps(Q - P)` (plus release tail per 8.4).
- Split rows at `P` and `Q`; for every row in `[P, Q)` set this track's
  override to `{ take_id, offset_steps }` where `offset_steps` for rows
  after the first = `steps(row.start_beat - P)` (so mid-take row splits
  made by simultaneous scene launches don't disturb take playback — each
  spliced row re-anchors into the take correctly).
- At `Q`, restore whatever the lane resolved to before punch-in (the prior
  override if one existed, else no override so the scene cell shows
  through).
- If `Q` exceeds `end_beat`, extend the song (same rule as scene drops
  beyond the song end).
- Take is named `Take {n}` with `n` from `next_take_id`.

Cancel (existing discard gesture) deletes minted takes and chunks and
touches nothing committed.

## 9. Additive arrangement capture (launch splice)

Replaces the wholesale `song_replace` commit in `song_capture.rs`. The
capture *collection* machinery is unchanged: `begin_song_capture_take`,
`record_song_capture_launch` fed from `App::apply_pattern_launch`,
`CaptureLaunchEvent { beat, kind: Scene | Tracks }`, consolidation
(stable sort, scene-clears-overrides, dedup-adjacent-identical).

### 9.1 Punch region

- Punch-in `P` = beat of the **first captured launch event** (not record
  press, not beat 0). Rationale: hitting record and listening for 8 bars
  before your first launch must not erase those 8 bars.
- Punch-out `Q` = beat at transport stop / record off.
- No launches performed ⇒ no splice (takes may still commit).

### 9.2 Splice semantics

Commit = one atomic project mutation:

1. Compute the row state at `P` of the *existing* song (`state_at_beat`).
2. Split existing rows at `P` and `Q` (reusing the row-split primitive from
   track-clip editing).
3. Delete committed rows with `P <= start_beat < Q`.
4. Insert consolidated captured rows (their beats are already absolute:
   `origin_beats`-relative capture maps onto song beats because capture ran
   during song playback — see 9.3).
5. The row beginning at `Q` (created by the split) restores the pre-existing
   arrangement state from `Q` onward. Nothing after `Q` moves; this is
   replace-in-place, not ripple insert.
6. `normalize()` drops adjacent-identical rows at the seams; validation
   failure aborts the whole commit and latches `song_capture_error`,
   leaving the committed song intact (existing guarantee).

Undo: the splice + all takes from this recording pass = one undo entry.

### 9.3 Capture during playback

Today `ArrangementCapture` mutes the committed song (performer is sole
launch authority, capture starts a fresh timeline at beat 0). Under this
spec, recording runs **on top of song playback**: `origin_beats` is the
song-position beat at record press, captured event beats are song-absolute,
and the committed song keeps playing wherever the performer hasn't
overridden it. The old whole-song capture remains reachable as "record from
an empty song" (or after select-all-delete) — no separate nuke mode
survives.

Track-pattern launches capture as `Tracks` events exactly as today and
splice as overrides; a scene launch inside the region clears prior
overrides per the existing consolidation rule.

### 9.4 Offset stamping

For each spliced row, every lane resolved from a launch that free-ran (all
launches do, today — launch quantization or not, the pattern joins the
global clock) gets a materialized override with
`offset_steps = steps(row.start_beat) mod L` per 7.2, so committed playback
reproduces the performance bit-exactly. Lanes inside the region that the
performer never touched inherit the pre-existing arrangement's resolution
(and keep whatever offsets those clips already had) — the splice materializes
them as overrides on the captured rows so scene-clears-overrides
consolidation cannot silence them.

### 9.5 Interim stopgap (pre-spec quick win)

Independent of the rest of this spec and shippable first: change the
current capture commit to splice from the first captured launch onward
(`P` .. song end) instead of replacing from beat 0. This removes the
worst destructive behavior ("head of the song gets nuked") with no data
model change.

## 10. Manual override and Back to Song

Ableton's orange "Back to Arrangement" equivalent. Applies during song
playback whether or not recording:

- A manual scene or track launch during song playback takes effect audibly
  (immediately or at launch quantize, as in session mode) and sets a
  **manual-override latch**: scene launch latches globally; a track launch
  latches that track. While latched, the song's launch authority is
  suspended for the latched scope — row transitions do not fire launches
  for it (other tracks keep following the song).
- A visible transport control ("SONG" button glows amber, or a dedicated
  return arrow beside it) clears the latch: the affected scope snaps back
  to whatever the song resolves at the current beat, with anchored phase
  per section 7.
- While recording, the same launches are additionally captured (section 9);
  the latch clears automatically at punch-out/commit since the song now
  *contains* the performance.
- The latch is transient transport state — never serialized.

This is what makes song playback and live jamming coexist, and it is a
prerequisite for 9.3's "recording runs on top of playback".

## 11. Bare tracks and UI surface

### 11.1 Bare tracks

Adding a track creates no patterns: `None` cells in every scene, empty
pool, empty take pool (`save_scene_snapshot` already grows with `None` /
default — audit the load path `from_pattern_snapshots` and any UI path that
eagerly materializes a pattern per scene, and remove that materialization
for new tracks). The timeline renders a `None` lane as an empty lane; it
must not synthesize placeholder clips at scene-launch boundaries. Patterns
for such a track come into existence when the user first edits a cell
(existing lazy path) or records a take.

### 11.2 Mixer clip grid

Filters out take-claimed pattern ids (6.1). No other change; the grid
remains a pure view of scene cells, which never reference chunks
(enforced by validation 6.3).

### 11.3 Timeline (Arr view)

- A take clip renders as one item spanning `[start, min(end,
  P + total_len - offset)]`, with MIDI-dot content aggregated across
  chunks; visually distinct from loop clips (no loop-repeat tiling; a
  subtle "recorded" affordance, e.g. rounded solid block).
- Per-track take list (context menu or track header disclosure): name,
  length, referenced-by count; rename / delete / re-place-at-playhead.
- Arm toggle on the Arr-view track header (8.1).
- During recording, the growing take renders live in the lane (preview
  override from 8.3).

### 11.4 Transport

- Record + SONG = additive capture + take recording (sections 8–9). The
  destructive arr-rec mode is retired.
- Manual-override latch indicator + Back to Song control (section 10).

## 12. Edge cases

- **Song loop wrap while recording**: recording across the wrap is
  disallowed in V1 — punch-out is forced at `end_beat` when the transport
  wraps (commit what exists; keep recording streams open in session terms
  is out of scope). Rationale: a take spanning a wrap is ill-defined on a
  linear timeline.
- **Take clip dragged so `offset >= total_len`**: invalid per 6.3; the
  gesture clamps.
- **Pattern length changed after offsets were stamped**: pattern offsets
  are interpreted mod current length at resolve time (6.3), so the clip
  stays audible; the stamped continuity is only guaranteed for the length
  at capture time. Acceptable; do not attempt to auto-rewrite offsets.
- **Chunk boundary vs plocks/step params**: nothing special — each chunk is
  a full `TrackPatternData`, so per-step data works everywhere in a take,
  including across chunk boundaries.
- **Timebase/swing plocks inside take chunks**: takes record none of these
  (they come from note input only); the track's live timebase applies. The
  beat→step mapping used at record time and resolve time is the same
  function of track state, so phase stays consistent under later timebase
  edits exactly as loop clips do today (i.e. it re-maps; no per-take tempo
  freeze).
- **Deleting a scene** that spliced rows reference: unchanged from song
  spec (row scene indices are validated); takes are unaffected.
- **Two recording passes over the same region**: second pass splices over
  the first (rows within `[P,Q)` replaced, including take overrides). The
  first take survives as an orphan in the take list (6.4) — cheap manual
  comping.
- **MIDI-dot rendering cost** for a 10-chunk take: aggregate lazily per
  visible viewport, as the timeline already does for lane events.

## 13. Phasing

1. **Phase A — anchored phase.** `offset_steps` on overrides, `LaneClip`
   plumbing, per-lane runtime projection (7.3),
   resize-left/move/split semantics (7.4). No takes yet.
   This alone fixes painted-clip phase (the deferred phase-anchor fix).
2. **Phase B — bare tracks + splice stopgap.** 11.1 and 9.5. Small,
   independent, immediately de-risks daily use.
3. **Phase C — takes.** `TrackTakePool`, `take_id` on overrides, chunked
   resolve, grid filter, take list UI, take deletion/undo. Still no
   recording — validate with a dev-only "convert clip region to take"
   harness or lisp primitive.
4. **Phase D — take recording.** Arming, punch-in/out, retargeted record
   path, live preview, commit (section 8).
5. **Phase E — additive capture + Back to Song.** Full splice (9.1–9.4),
   manual-override latch (10), retire the destructive mode.

A–B unblock the timeline agent immediately; C–E are sequential but each is
independently shippable.

## 14. Locked decisions

- Takes are ownership over ordinary pool patterns; no new content format.
- Hidden and non-looping are derived from take ownership, never flags on
  `TrackPatternData`.
- Offset lives on `ProjectSongTrackOverride` only; rows and scenes carry no
  phase. Deviating lanes get materialized overrides.
- Free-run survives only as a stamped offset value; the runtime has exactly
  one phase formula.
- Punch-in for launch capture = first captured event, never beat 0.
- Splice is replace-in-place between `P` and `Q`; nothing outside the
  region moves or changes.
- One undo entry per recording pass (splice + all takes).
- Take chunks are `MAX_STEPS` each; chunk boundaries are invisible to the
  song model, the accumulator-reset logic, and the user.
- No silent GC of takes; orphans are user-visible and user-deleted.
- No arrangement backward compatibility: no saved projects contain song
  rows, so there is no phase migration for legacy songs (7.5). Serde
  defaults are the only compatibility surface.

## 15. Open questions

- Arm-flag persistence: serialize with the project (like mute/solo) or
  session-only? Leaning serialize, matching mute/solo.
- Should Back to Song also have per-track return (click the track's latch
  indicator) in V1, or global-only first?
- `offset_steps` units under per-step timebase plocks: the spec defines the
  mapping as the track's live beat→step function; confirm during Phase A
  that swing plocks inside a pattern don't make `steps()` non-invertible in
  a way that matters for resize-left stamping.
- Whether the dev harness in Phase C ("region → take") is worth promoting
  to a user feature (consolidate/flatten), which would also be the seed of
  comping.
