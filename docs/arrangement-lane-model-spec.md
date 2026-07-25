# Arrangement Lane Model — Author in Lanes, Compile to Rows

Status: draft (rev 1, 2026-07-25)
Supersedes: the *authoring/storage* portions of docs/song-mode-spec.md §5
(`ProjectSong` as the stored model, §5.6 row primitives). Playback (§7-§9),
takes phase model (takes spec §7), and capture *semantics* (§7.4, takes spec
§8-§9) are unchanged in behavior; their implementation targets move.

## 1. Summary

The stored arrangement model changes from a flat list of complete launch
states (`ProjectSongRow` = scene + full override set) to **lanes**:

- a **scene lane**: an ordered list of scene *changes* at beats, and
- one **track lane per track**: an ordered list of **clips**, each a
  first-class object with its own identity, span, source, and phase offset.

The row model does not disappear — it becomes a **compiled, derived
representation** used only for playback. A pure function compiles lanes into
exactly today's `ProjectSong`, so `preflight_runtime_song`, `RuntimeSong`,
`SongPlaybackRuntime`, the lookahead row-transition engine, and the transport
machine are untouched.

Why: the row model is a great playback representation (O(1) resolve, no
merge rules, gapless by construction) but a poor authoring one. The thing
the user edits — a clip with a lifetime, sitting on a scene backdrop — is
smeared across every row it overlaps. Symptoms already shipped: the scene
lane fragments at every track-clip boundary ("jagged scenes"), deleting a
scene span swallows the clips riding in that row, and a growing projection
layer (merged lane view, `paint_source_region` multi-row surgery,
`collapse_phase_continuation_rows`) exists solely to reconstruct clip
identity the model discarded. In the lane model each of those is either a
non-problem or a one-object edit.

## 2. Current facts this spec builds on (verified 2026-07-25)

- `ProjectSong { rows, end_beat, loop_enabled, next_row_id }` is stored,
  serialized (`ProjectFile.song`), and is the undo memento
  (`SongStructurePatch`). `sequencer/state/song.rs`.
- `project_lanes` already derives per-track `LaneClip` spans from rows for
  the UI and region ops — one span per (track, row), never merged.
- The takes spec §7 phase model (`p(beat) = steps(beat - S) + o`) is
  implemented: overrides carry `offset_steps`; `paint_source_region` stamps
  `anchor_offset + steps(row.start - anchor)` per governed row.
- Editing primitives are row surgery: `song_row_insert/remove/move/
  set_state`, `song_track_paint[_anchored]` → `paint_source_region` +
  `normalize` + `collapse_phase_continuation_rows`.
- Capture stages `CapturedSongState` events (`consolidate`) and commits
  through `song_replace` / `commit_capture_with_takes`; each launch event
  knows whether it was a scene launch or a per-track launch.
- `preflight_runtime_song` reads `committed_song()`, resolves lanes
  (override else scene cell), expands take chunks, prebuilds snapshots.
- Serialization: `ProjectFile.song: Option<ProjectSong>` in the
  "scene index + 1" id domain via `song_for_serialization`; whole-file
  `version: u32`; no per-song version.

## 3. Terminology

- **Arrangement** — the new stored authoring model (scene lane + track
  lanes + end/loop). Replaces stored `ProjectSong`.
- **Scene event** — `(start_beat, scene)`: from this beat, tracks without a
  clip play this scene's cells. Spans are derived (event → next event).
- **Clip** — a spanned object on one track lane: `[start_beat, end_beat)`,
  a source (pattern / take / explicit-empty), and `offset_steps`.
- **Compile** — the pure function `Arrangement → ProjectSong`. Compiled
  rows are cached, never stored, never edited, never serialized.
- **Row** — unchanged meaning at playback: a complete launch state span.

## 4. Goals

1. Clip identity in the model: move/resize/split/delete a clip is a
   one-object edit, no row surgery, no phase-continuation cleanup.
2. Scene lane contains only scene *changes*. The jagged lane is
   structurally impossible; deleting a scene event never touches clips.
3. Clips survive scene changes beneath them by construction (the override
   semantics the UI already implies).
4. Zero behavior change at playback: compiled rows are byte-equivalent in
   meaning to what the same edits produce today (same resolve, same phase,
   same accumulator-reset diffing, same take expansion).
5. Serialization v2: store lanes. Old `song` field is **ignored on load**
   (project loads with no arrangement). No migration code.

## 5. Non-goals

- Overlapping clips within one lane (rejected by validation, like today's
  duplicate-track overrides).
- Clip envelopes / per-clip loop regions / audio clips. The clip object is
  deliberately minimal; these become possible later precisely because clips
  now exist.
- Changing session (non-song) playback, scenes, pattern pools, or takes
  storage (`take_pools` serialization is unchanged).
- Renaming user-facing "song" commands wholesale; existing
  `seq-song-*` transport/status natives keep their names.

## 6. Data model

```rust
// sequencer/state/arrangement.rs
pub struct ClipId(pub u64);          // stable, monotonic, never reused

pub struct SceneEvent {
    pub start_beat: f64,             // first event must be at 0.0
    pub scene: usize,
}

pub struct ArrClip {
    pub id: ClipId,
    pub start_beat: f64,
    pub end_beat: f64,               // exclusive; > start_beat
    pub pattern_id: Option<u64>,     // same encoding as ProjectSongTrackOverride:
    pub take_id: Option<u64>,        //   take excludes pattern; both None = explicit-empty
    pub offset_steps: f64,           // takes spec §7 anchor; 0 = starts at source step 0
}

pub struct ProjectArrangement {
    pub scene_lane: Vec<SceneEvent>, // sorted by start_beat, strictly increasing
    pub track_lanes: Vec<Vec<ArrClip>>, // index = track; sorted, non-overlapping
    pub end_beat: f64,
    pub loop_enabled: bool,
    pub next_clip_id: u64,
}
```

`ArrClip::source()` mirrors `ProjectSongTrackOverride::source()` and reuses
`LaneSource` verbatim.

### 6.1 Validation (replaces song spec §5.3 as the stored-model rules)

Errors, never clamps (same philosophy as `ProjectSong::validate`):

- `scene_lane` non-empty when the arrangement exists; first event at
  exactly 0.0; strictly increasing; `scene < scene_count`.
- `end_beat` finite, `> 0`, `>= ` every clip `end_beat` and `> ` the last
  scene event's `start_beat`.
- Per lane: clips sorted by `start_beat`, `end_beat > start_beat`, no
  overlap (`clip[i].end_beat <= clip[i+1].start_beat`); gaps are fine.
- Per clip: pattern exists in the track pool; take exists; take excludes
  pattern; `offset_steps` finite, `>= 0`, and for takes
  `< total_len_steps` (takes never wrap).
- `track_lanes.len() == track_count`; clip ids unique, `< next_clip_id`.
- Adjacent same-source clips are legal (unlike adjacent identical rows) —
  two back-to-back clips of the same pattern are distinct objects the user
  made; only *compile* output gets normalized.

### 6.2 Resolution semantics (unchanged, now stated on lanes)

At beat `b`, track `t` plays:

1. the clip on lane `t` containing `b`, if any — pattern loops with the §7
   phase formula, take plays until its end then silence, explicit-empty is
   silence; otherwise
2. the governing scene event's cell for track `t`; otherwise
3. nothing.

A clip is opaque: while it spans `b`, scene events beneath it are inert for
that track. This is precisely the "overrides are supposed to be overrides"
semantics; in the row model it required re-stamping overrides into every
new row.

## 7. Compile: `Arrangement → ProjectSong`

```rust
pub fn compile_arrangement(arr: &ProjectArrangement, ctx: &impl SongProjectContext)
    -> Result<ProjectSong, String>
```

1. **Boundary set** = every `SceneEvent.start_beat` ∪ every clip
   `start_beat` and `end_beat` (dropping `>= end_beat`), sorted, deduped
   (epsilon-free: beats compare exactly; gestures already quantize).
2. For each boundary `B`: `scene` = governing scene event; `overrides` =
   for each lane whose clip contains `B`, one `ProjectSongTrackOverride`
   with `offset_steps` stamped by the takes spec §7 split rule —
   pattern: `(clip.offset + steps(B - clip.start)) mod L`;
   take: `clip.offset + steps(B - clip.start)`, becoming explicit-empty
   past `total_len_steps` (mirrors `split_row_state`).
3. `normalize()` (adjacent identical rows collapse — e.g. a clip boundary
   that lands where the compiled state doesn't change).
4. **Deterministic row ids**: `SongRowId(index)`, `next_row_id = len`.
   Rows are derived, so ids only need stability *given equal input*; a
   content-only project edit recompiles to an identical row layout, keeping
   `replace_song_in_place` edit-through and row mirroring working. (The
   `active_runtime_song` already tolerates full refresh when layout
   changes.)
5. `validate()` in debug builds (compile output is correct by construction;
   the check guards the compiler itself).

The compiled song is cached on `SequencerState` next to the arrangement and
rebuilt whenever the arrangement is set (`set_committed_arrangement`
recompiles and calls today's `set_committed_song`, bumping the existing
`song_revision`). Every current consumer of `committed_song()` —
preflight, transport, mirroring, `state_at_beat` for the UI cursor —
continues to work unmodified.

Dead after compile exists: `paint_source_region`, `split_song_row_at`,
`split_row_state`, `collapse_phase_continuation_rows`, `song_row_insert/
remove/move/set_state` (the primitives; the *struct* `ProjectSong` and its
validate/normalize/`state_at_beat` all stay, minus serde).

## 8. Editing primitives (replace song spec §5.6)

All guarded by `require_song_edit_unlocked`, all committing one
`EditPatch::Arrangement(ArrangementStructurePatch { before, after })`
(whole-object memento incl. `next_clip_id`, like today's song patch), all
ending with validate → recompile → `set_committed_song`.

Clip ops (per track lane):

- `arr_clip_create(track, start, end, source, offset_steps)` — truncates
  whatever it lands on, Ableton-style: overlapped clips are trimmed at the
  new clip's edges (left-trims re-stamp `offset_steps` by the split rule);
  a clip fully covered is removed; a clip the new span lands strictly
  inside is split around it.
- `arr_clip_delete(clip_id)`
- `arr_clip_move(clip_id, new_start)` — rigid: offset unchanged (takes
  spec §7.4). Truncates overlapped clips the same way (the moved clip
  always wins).
- `arr_clip_resize(clip_id, new_start, new_end)` — left-trim adjusts
  `offset_steps += steps(d)` (§7.4); right edge pure occlusion, clamped
  for takes. Growing over a neighbor truncates it.
- `arr_clip_split(clip_id, beat)` — right half: `start = beat`,
  `offset += steps(beat - start)`.
- `arr_clip_set_source(clip_id, source)` — swap content in place.

Scene ops:

- `arr_scene_event_insert(beat, scene)` / `arr_scene_event_move(beat)` /
  `arr_scene_event_set(scene)`.
- `arr_scene_event_remove(beat)` — **merges into the predecessor's scene**
  (removing the event at 0.0 is rejected, like row 0 today). This is the
  user-visible fix: deleting in the scene lane removes the scene *change*
  and cannot touch clips.

Whole-arrangement ops: `arr_set_end`, `arr_set_loop`, `arr_replace`,
`arr_clear` — direct ports of today's equivalents.

Region ops (`song_region.rs`) become clip surgery: copy collects clips
intersecting the region (split at edges, offsets re-stamped by the split
rule) **plus scene events inside a scene-lane region**; paste/duplicate
insert clips (splitting/trimming whatever they land on — paste is the one
op that *does* truncate, matching today's paint-over behavior); delete
removes/trims clips in the region. No ripple row insertion, no
`normalize` cleanup passes.

`def-song` keeps its surface but lowers to `arr_replace` (rows in the
declarative form translate 1:1: each row's scene → scene event when it
changes, each override → a clip spanning to the next row that changes that
lane — the inverse compile, unambiguous because declarative input has no
clip identity to preserve). "Changes that lane" is *phase-continuity
equivalence*, not source equality: a later row's override merges into the
open clip only when `stamped_clip_override(clip, row.start_beat) ==
declared`. Merging on source equality alone would swallow a deliberate
retrigger — the same pattern re-anchored to step 0 mid-cycle — turning it
into an uninterrupted clip. The same lowering runs wherever a row list has
to become lanes (`set_committed_song` while the row primitives survive).

## 9. Capture and take recording

Capture *semantics* (song spec §7.4, takes spec §8-§9) are unchanged; only
the commit target changes. `consolidate` and the staging types stay.

Decomposition rule at commit (`finish_song_capture_take`): walk the
consolidated states/events in order —

- a **scene launch** emits a `SceneEvent` at its audible beat;
- a **per-track launch** *ends* the lane's open clip (if any) at that beat
  and opens a new clip (pattern with free-run offset stamped per takes
  spec §7.2, i.e. `steps(beat) mod L`); back-to-song ends the open clip
  with no successor;
- at punch-out, open clips close at `Q`; the restore tail is whatever the
  *previous arrangement* had — splice = trim/split existing clips and scene
  events at `[P, Q)` and insert the captured ones, replacing
  `stamped_captured_row_spec` + `split_row_state` restore-row construction
  with ordinary clip trimming (offsets re-stamped by the same split rule).

Free-run inheritance disappears as a special case: an *untouched* lane is
simply not written — the scene backdrop keeps playing through the region
because no clip covers it. Only *touched* lanes (takes spec §9.2) get
clips. (Today this requires materializing nonzero-offset overrides on
every captured row.)

Take recording (`register_pending_takes` → `CommittedTakeLane`) paints one
take clip per lane at `[punch_in, punch_out)` with `offset 0` — replacing
`paint_take_region`'s row governance with a single `arr_clip_create` (plus
trim of whatever it overlaps).

## 10. Serialization v2

- `ProjectFile` gains `arrangement: Option<SerializedArrangement>`
  (serde default). The legacy `song` field remains **as a parse-tolerated
  dead field**: still deserialized structurally (or into
  `serde_json::Value`) so old files load, but its content is discarded —
  old projects open with no arrangement. Save never writes it.
- Bump `ProjectFile.version`. Loaders at the new version reject nothing
  extra; the version exists so future tooling can tell eras apart.
- `SerializedArrangement` uses the same "scene index + 1" pattern-id
  domain via a sibling of `song_for_serialization` (`arrangement_for_
  serialization`), same constraint: a clip referencing a pattern assigned
  to no scene cell errors on save. Takes serialize by stable take id,
  unchanged.
- On load: build `SerializedSongContext` as today and **validate** the
  arrangement against it (id domain, take lengths), then **compile against
  the live `ProjectScenes`**, after `replace_pattern_repository` and the
  take-pool install have rebuilt the pools. Not against the serialized
  context: it answers "unknown" for every scene cell and timebase, so
  compiling there materializes no backdrop phase overrides at all and
  silently loses scene-backdrop phase continuity.
- `use_arrangement`, `record_armed`, `scene_cell_presence`, `take_pools`
  fields are untouched.

## 11. Structural remaps

Direct ports of the row versions, simpler on lanes:

- Scene delete: drop scene events referencing it? No — same policy as
  today: **reject** delete while referenced
  (`song_rows_referencing_scene` → `arrangement_scene_references`), then
  `remap_after_scene_delete` decrements higher indices.
- Pattern delete: reject while any clip references it (per-lane scan
  replaces `song_rows_referencing_track_pattern`).
- Track delete/move: drop / permute `track_lanes` entries
  (`track_delete_remap.rs` ports 1:1, minus the normalize call).

## 12. UI read surfaces

- **`SEQ.song-lanes`** now serializes the stored clips directly:
  `{clip-id, start-beat, end-beat, pattern-id, take-id, offset-steps}` —
  real identity, already merged, `from-override` obsolete (every lane item
  IS a clip). A second derived surface **`SEQ.scene-spans`** replaces the
  scene portion of `SEQ.song-rows`: one span per scene event
  (`start-beat`, derived `end-beat`, `scene`). `SEQ.song-rows` (the row
  maps) is retired from the UI; anything still reading it moves to the two
  lane surfaces. The `song-current-row` scalar family stays (it reads the
  compiled song via `state_at_beat`, still meaningful for transport).
- **Backdrop rendering**: track lanes additionally need the scene-provided
  content for gaps (today `project_lanes` bakes it in). Expose it as
  derived ghost spans (per gap: governing scene's cell), rendered dimmer —
  which the UI already does via the `from-override` tint; the distinction
  becomes structural instead of a flag.
- `arrangement.lisp`: scene lane renders `SEQ.scene-spans` (one block per
  actual scene change — the jagged lane is gone without any label
  suppression tricks); `arrangement-scene-row-label`'s dedup hack dies.
  Clip gestures lower to the new clip primitives in
  `arrangement_actions.rs` (move/resize/create/delete map 1:1 now —
  no more "resize = move the *next* row").
- `SEQ.song-lane-events` (note previews) keys off clips instead of rows;
  logic otherwise unchanged.

## 13. Phasing

Each phase lands green on `arrangement-timeline` (or a child branch);
scoped tests per the test-workflow rules.

1. **Model + compile** (pure, no wiring): `arrangement.rs` with types,
   validation, `compile_arrangement`, plus a property-style test that
   compiles known lane fixtures and asserts equality with hand-built
   `ProjectSong`s (reusing the existing song.rs test vocabulary).
2. **Storage + serialization v2**: `committed_arrangement` +
   compiled-song cache on `SequencerState`; `ArrangementStructurePatch`;
   ProjectFile v2 (write arrangement, tolerate-and-drop legacy `song`);
   `arr_replace`/`arr_clear`; `def-song` lowering. Row primitives still
   exist but everything routes through the arrangement.
3. **Editing primitives + region ops**: clip/scene-event ops, region
   copy/paste/duplicate/delete on clips, `arrangement_actions.rs`
   re-lowering, host commands + natives. Delete the row primitives and
   `paint_source_region` machinery.
4. **Capture + take recording**: event→lane decomposition, splice as clip
   trimming, `register_pending_takes` painting clips.
5. **UI surfaces**: `SEQ.song-lanes` from stored clips, `SEQ.scene-spans`,
   backdrop ghost spans, `arrangement.lisp` scene lane + gesture updates,
   retire `SEQ.song-rows`.

Phases 1-2 are purely additive; the app behaves identically. The risky
diffs are 3 (many tests to port — song_edit's 25, song_region's 22) and 4
(capture splice, 7 + 7 tests).

## 14. Locked decisions

- Rows stay as the compiled playback representation; scheduler/runtime/
  transport code is out of scope for changes.
- Old projects' arrangements are dropped on load (no migration). The rest
  of the project (scenes, patterns, takes pools) loads normally.
- Clip wins over scene while it spans a beat; explicit-empty is a clip.
- Scene-event delete merges into the predecessor; it can never remove
  clips.
- No overlapping clips per lane as an *invariant*; every write op
  (create/move/resize/paste/capture) truncates what it lands on,
  Ableton-style — the incoming/edited clip always wins. Nothing rejects on
  overlap.
- Deterministic compile row ids (index-based) so unchanged layout keeps
  edit-through `replace_song_in_place` valid.
- `LaneSource`, the takes phase formula, and take chunk expansion are
  reused verbatim.

## 15. Open questions

- Does the scene lane need explicit-empty ("no scene") spans, or is scene
  0-at-beat-0 always live? v1: always a governing scene (matches today).
- Clip end past `end_beat`: validation currently rejects; the content-
  length handle may want clips to survive a shortened song. v1: reject and
  have `arr_set_end` refuse to shrink past the last clip (UI clamps).
