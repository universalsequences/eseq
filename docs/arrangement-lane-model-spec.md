# Arrangement Lane Model — Author in Lanes, Compile to Rows

Status: draft (rev 2, 2026-07-25 — the scene *backdrop* is removed; see 6.2)
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
the user edits — a clip with a lifetime — is smeared across every row it
overlaps. Symptoms already shipped: the scene
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
- **Scene event** — `(start_beat, scene)`: a marker plus the gesture that
  **stamps** the scene's cells as real clips across its span. It governs
  nothing at playback. Spans are derived (event → next event).
- **Clip** — a spanned object on one track lane: `[start_beat, end_beat)`,
  a source (pattern / take — never sourceless), and `offset_steps`.
- **Stamp** — write a scene's cells onto every track lane over a span, as
  ordinary clips, truncating whatever was there.
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
    pub take_id: Option<u64>,        //   take excludes pattern; both None is REJECTED
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
- Per clip: **a source is mandatory** — a clip with neither a pattern nor a
  take is rejected, because silence is the absence of a clip; pattern exists
  in the track pool; take exists; take excludes pattern; `offset_steps`
  finite, `>= 0`, and for takes `< total_len_steps` (takes never wrap).
- `track_lanes.len() == track_count`; clip ids unique, `< next_clip_id`.
- Adjacent same-source clips are legal (unlike adjacent identical rows) —
  two back-to-back clips of the same pattern are distinct objects the user
  made; only *compile* output gets normalized.

### 6.2 Resolution semantics

At beat `b`, track `t` plays:

1. the clip on lane `t` containing `b`, if any — pattern loops with the §7
   phase formula, take plays until its end then silence; otherwise
2. **nothing**.

That is the whole rule. **Everything audible is a visible clip** — one the
user can select, move, or delete. A span with no clip is silent: deleting a
clip is exactly "the clip stops playing", with nothing revealed underneath.

Scene events resolve nothing. What a scene event does is **stamp**: placing
one (or repointing or moving it) writes the scene's cells onto every track
lane across its span as ordinary clips. A track whose cell in that scene is
empty gets no clip and is silent there. Stamping truncates what it lands on,
like every other clip write (§14).

#### 6.2.2 Stamping is anchored on the global timeline

A stamped clip carries the **free-run** offset `steps(start) mod L` (takes
spec §7.2) — the phase the pattern would have if it had been looping since
beat 0 — *not* a phase measured from the scene event. So source step 0 always
falls on the same absolute beats, whatever the boundary does: **modifying
scenes never changes the flow of rhythm of the clips below.** Dragging a
boundary changes how *much* of a pattern you hear, never *when* its steps
fall.

Anchoring on the event instead (tried first) restarted the pattern at step 0
at the boundary, so shortening a scene onto a beat that is not a whole number
of pattern cycles jumped every downstream hit off the grid. It also
disagreed with capture, which has always stamped performed launches free-run;
one rule now covers both. The two conventions agree exactly when the boundary
is pattern-aligned, which is why it is easy to miss.

The one place that still anchors on the event is `legacy_backdrop_spans`,
used only by the v5 → v6 migration (§10) — because that is what v5 *sounded*
like, and the migration's job is to preserve it.

#### 6.2.1 Tried and rejected: the scene backdrop

Rev 1 resolved a beat as *clip, else the governing scene's cell, else
nothing* — the "backdrop". It shipped and was removed. Two reasons:

- **Delete looked like a no-op.** Deleting a clip revealed the scene cell
  underneath, usually the same pattern the clip had been playing: the
  timeline changed and the music did not. The core workflow — select a
  track's clips, delete them, record a take into the empty space — was
  impossible.
- **Sparse arrangements were unauthorable.** Silence had to be spelled as an
  "empty clip" occluding the backdrop: an object that renders as a gap and
  behaves as content. You could not look at a lane and know what it played.

The cost of removing it is that a scene event materializes clips instead of
being a cheap marker, and that removing a scene event re-stamps its merged
span rather than leaving clips alone. That is the trade: bigger on disk,
completely legible on screen.

## 7. Compile: `Arrangement → ProjectSong`

```rust
pub fn compile_arrangement(arr: &ProjectArrangement, ctx: &impl SongProjectContext)
    -> Result<ProjectSong, String>
```

1. **Boundary set** = every `SceneEvent.start_beat` ∪ every clip
   `start_beat` and `end_beat` (dropping `>= end_beat`), sorted, deduped
   (epsilon-free: beats compare exactly; gestures already quantize).
2. For each boundary `B`: `scene` = the scene *marked* at `B` (kept because
   transport and session UI ask which scene is current; it no longer affects
   what any lane plays); `overrides` = **one per track, always** —
   - a lane whose clip contains `B`: `offset_steps` stamped by the takes spec
     §7 split rule — pattern `(clip.offset + steps(B - clip.start)) mod L`;
     take `clip.offset + steps(B - clip.start)`, becoming explicit-empty past
     `total_len_steps` (mirrors `split_row_state`);
   - a lane **no clip covers**: `ProjectSongTrackOverride::new(track, None)`,
     an explicit-empty override. This is the crux of the compile and is not
     optional: `preflight_runtime_song` resolves an *absent* override from
     the row's scene cell, which is exactly the backdrop §6.2.1 removed, so
     an uncovered lane has to say "silent" out loud.
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

There is no `backdrop_override` step. Rev 1 had one — a materialized phase
override keeping a scene-filled gap phase-continuous across a boundary some
*other* track's clip created. Its entire purpose was scene-filled gaps, and
there are none: a gap is silence, and everything else is a clip carrying its
own anchor.

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

- `arr_clip_create(track, start, end, source, offset_steps)` — a source is
  mandatory; `LaneSource::Empty` is refused, because creating nothing is
  meaningless. (The *gesture* is not refused: the `arrangement-clip-create`
  host command sees an empty source as "silence this span" and routes to
  `arr_clip_clear_span`.) Truncates
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
- `arr_clip_set_source(clip_id, source)` — swap content in place. `Empty`
  **deletes the clip**: "make this span silent" and "remove this clip" are
  the same operation in this model.
- `arr_clip_clear_span(track, start, end)` — silence a span by removing and
  trimming the clips it covers, storing nothing. What "draw an empty clip
  here" lowers to.

Scene ops:

The first three **stamp** (§6.2), each in one undo entry; remove does not:

- `arr_scene_event_insert(beat, scene)` — stamps the new event's whole span.
- `arr_scene_event_set(scene)` — re-stamps that event's span with the new
  scene's cells.
- `arr_scene_event_move(beat)` — re-stamps the span it vacates (now the
  predecessor's) together with the one it lands on. Shortening a scene span
  IS this operation, and §6.2.2 is what keeps it musically safe.
- `arr_scene_event_remove(beat)` — **removes only the marker.** Its span
  merges into the predecessor's label and **the clips stay** (removing the
  event at 0.0 is rejected, like row 0 today).

The asymmetry is deliberate. Insert/set/move all mean "launch this scene
here", so replacing the content under them is the intent. Remove means "clean
up this marker": the complaint that started the whole pivot was that deleting
scene changes to tidy the scene row destroyed the pattern changes riding
beneath them. Clips are the truth, so a removal leaves the predecessor's
label spanning clips a since-removed scene stamped — which reads honestly,
because what plays is the clips.

Whole-arrangement ops: `arr_set_end`, `arr_set_loop`, `arr_replace`,
`arr_clear` — direct ports of today's equivalents.

Region ops (`song_region.rs`) become clip surgery: copy collects clips
intersecting the region (split at edges, offsets re-stamped by the split
rule) **plus scene events inside a scene-lane region**; paste/duplicate
insert clips (splitting/trimming whatever they land on — paste is the one
op that *does* truncate, matching today's paint-over behavior); delete
removes/trims clips in the region. No ripple row insertion, no
`normalize` cleanup passes.

`SongRegionSelection` carries a `scene_lane` bit saying the marquee was
swept in the scene lane; only then do the region ops touch the scene lane.
Copy stores `(rel_beat, scene)` for every event inside the span, led by an
entry at `rel 0` restating the scene governing the span's start. Paste and
delete both clear the scene lane over the destination span (never the
mandatory event at 0.0) and then **restore the scene that governed the
span's end** by inserting an event there if the edit changed it — so a
scene-lane region op is local to its rectangle and nothing after it moves.

`def-song` keeps its surface but lowers to `arr_replace`. Each row's scene
becomes a scene event when it changes, and those events **stamp** their cells
across the whole arrangement (free-run anchored, §6.2.2); the per-track overrides then truncate on top
(a row that drops an override lets the stamped scene clip resume; an
explicit-empty override carves a silent hole). Rows translate 1:1 (each
override → a clip spanning to the next row that changes that lane — the inverse compile, unambiguous because declarative input has no
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

A scene launch during capture **stamps clips across every lane it claims**:
a claimed lane's captured resolution is its explicit launch else the captured
scene's cell, at the free-run phase `steps(beat) mod L` (takes spec §7.2),
and that resolution becomes a clip. A lane resolving to nothing (an empty
cell) gets no clip and is silent.

Free-run inheritance disappears as a special case, and so does every restore
mechanism at `Q`: an *untouched* lane is simply **not modified** — its
existing clips already cover the punch region and keep playing through it —
while a touched lane's pre-existing clips are left-trimmed and re-stamped by
`occlude_span`, so they resume at `Q` by construction. Only *touched* lanes
(takes spec §9.2) are written at all.

Take recording (`register_pending_takes` → `CommittedTakeLane`) paints one
take clip per lane at `[punch_in, punch_out)` with `offset 0` — replacing
`paint_take_region`'s row governance with a single `arr_clip_create` (plus
trim of whatever it overlaps).

## 10. Serialization v2

**Version 5 → 6 migration (required).** A version-5 file's lane gaps really
did sound: they played the governing scene's cell. Loading one untouched
under §6.2 would silently gut the arrangement. So on load of a file at
version < 6, `migrate_legacy_backdrops` freezes what every gap sounded like
into real clips (`legacy_backdrop_spans` — the old derivation, kept for
exactly this) and drops the old explicit-empty clips, whose spans were
deliberate silence and are now spelled as gaps. The result compiles to the
same `ProjectSong`, phase offsets included, and saves back as version 6. The
migration runs in `finish_project_load`, after the pattern pools, scene
cells, and take pools are rebuilt — it needs the live scenes for the cells
and the timebases. The deserialize-time structural check looks past
explicit-empty clips for version < 6, since validation now rejects them.

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
  arrangement against it (id domain, take lengths), then migrate if needed
  and **compile against the live `ProjectScenes`**, after
  `replace_pattern_repository` and the take-pool install have rebuilt the
  pools. Not against the serialized context: it answers "unknown" for every
  scene cell and timebase, so a clip crossing a boundary another lane created
  would keep its start-of-clip phase instead of the phase it had reached —
  the music would retrigger mid-cycle.
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
- **No backdrop surface.** Rev 1 published **`SEQ.song-backdrops`** — derived
  ghost spans per lane gap, rendered dimmer — and it is gone with the model it
  served (§6.2.1). Track lanes render clips and nothing else; a gap renders as
  empty lane. With no ghosts there is no dim/bright distinction either: one
  clip tint.
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
   `arrangement.lisp` scene lane + gesture updates, retire `SEQ.song-rows`.
6. **Backdrop removal** (this revision): silent gaps, scene events stamp,
   explicit-empty overrides for uncovered lanes, v5 → v6 migration, and the
   ghost-span surface deleted.

Phases 1-2 are purely additive; the app behaves identically. The risky
diffs are 3 (many tests to port — song_edit's 25, song_region's 22) and 4
(capture splice, 7 + 7 tests).

## 14. Locked decisions

- Rows stay as the compiled playback representation; scheduler/runtime/
  transport code is out of scope for changes.
- Pre-lane-model (version <= 4) arrangements are dropped on load. Version-5
  arrangements — written under the backdrop rule — ARE migrated (§10).
- A span with no clip is silent. There is no fallback of any kind.
- A clip always has a source; `LaneSource::Empty` survives only as a
  *compiled override*, never as a stored clip.
- A scene event stamps clips and decides nothing at playback. Insert, set,
  and move re-stamp their span; **remove does not** — it deletes the marker
  and never touches a clip (rev 1's guarantee, kept).
- Stamped clips free-run against the global clock (§6.2.2), so a scene edit
  can never shift the rhythm of the patterns below it.
- `LaneSource::Empty` on a *write* op means silence, and silence is an
  absence: set-source deletes, the clip-create gesture clears the span, and
  only the bare `arr_clip_create` primitive errors.
- No overlapping clips per lane as an *invariant*; every write op
  (create/move/resize/paste/capture) truncates what it lands on,
  Ableton-style — the incoming/edited clip always wins. Nothing rejects on
  overlap.
- Deterministic compile row ids (index-based) so unchanged layout keeps
  edit-through `replace_song_in_place` valid.
- `LaneSource`, the takes phase formula, and take chunk expansion are
  reused verbatim.

## 15. Open questions

- Does the scene lane need explicit-empty ("no scene") spans? Much less
  pressing now that the scene lane governs nothing — an event is a marker
  plus a stamp gesture. v1: always a marked scene (matches today).
- Clip end past `end_beat`: validation currently rejects; the content-
  length handle may want clips to survive a shortened song. v1: reject and
  have `arr_set_end` refuse to shrink past the last clip (UI clamps).
