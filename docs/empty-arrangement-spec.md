# Empty Arrangements by Default — No "No Song" State

Status: BUILT (rev 2, 2026-08-02; decisions in §5 confirmed; implementation
notes in §12 record where the build refined rev 1)
Builds on: docs/arrangement-lane-model-spec.md (lane model, clips-are-explicit
rev 2). Amends its §6.1 structural invariants and the load/new-project paths.
Playback runtime (song_runtime, transport machine) is untouched.

## 1. Summary

Today a project starts with **no arrangement** (`Option<ProjectArrangement>
= None`), and every arrangement edit is gated on it existing. The user must
first "create a song" (ARR REC or def-song) before they can so much as drag
a scene onto the timeline. Once one exists, two structural invariants — the
scene lane is non-empty, and its first event sits at beat 0.0 — make the
arrangement impossible to empty out again: select-all + delete leaves Scene
1 stretched over the wreckage, Backspace on the first scene change is
rejected, and dragging a scene onto beat 0 is rejected as a collision.

This spec removes the gate and the two invariants:

1. **The arrangement always exists.** A new project (and a legacy file with
   no `arrangement`) gets an *empty* arrangement: no scene events, no
   clips, a default length. All 15 `arr_*` primitives work immediately.
2. **The scene lane may be empty, and its first event may start anywhere.**
   Scene events were already pure authoring gestures under the
   clips-are-explicit model — they govern nothing at playback — so nothing
   downstream needs one to exist. The span before the first event (or the
   whole timeline) is **unscened**: unlabeled, unstamped, and silent where
   no clip covers it (which it already is).

The compiled `ProjectSong` keeps its full runtime contract (rows non-empty,
tiling from beat 0) by *synthesis in compile*, not by user-facing rules.

## 2. Current facts this spec builds on (verified 2026-08-02)

- The `None` state lives in `SequencerState { song: Mutex<Option<ProjectSong>>,
  arrangement: Mutex<Option<ProjectArrangement>> }` (state/core.rs:24-30),
  initialized `None` (accessors.rs:104) and reset to `None` by
  `clear_project_arrangement_state` (projects.rs:950).
- `App::require_arrangement()` (arr_edit.rs:46-52) — "The project has no
  arrangement" — fronts every one of the 15 `arr_*` edit primitives via
  `edit_arrangement[_coalesced]`.
- `ProjectArrangement::validate` (arrangement.rs:198-407) enforces:
  scene lane non-empty (:200); first event at exactly 0.0 (:212);
  `track_lanes.len() == track_count` (:241); `end_beat > 0` and `> ` last
  scene start (:252, :258). `ProjectSong::validate` mirrors the first two
  as rows-non-empty (song.rs:235) and row 1 at 0.0 (:247).
- `compile_arrangement` (arrangement.rs:1003-1083) makes a row per
  boundary; `scene_event_at_beat(beat).ok_or(...)` (:1034) hard-errors on
  any boundary before the first scene event. A lane with no clip at a
  boundary emits an **explicit-empty override** (:1049) — since the
  clips-are-explicit pivot, *every* lane gets an explicit override in every
  compiled row, so the row's `scene` field never decides audio on the
  compile-from-arrangement path. The absent-override → scene-cell fallback
  (song_playback.rs:118-127) is reachable only for rows not produced by
  compile.
- Scene events stamp clips on insert/set/move (`stamp_scene_clips`,
  arrangement.rs:726) and do **not** re-stamp on remove (arr_edit.rs:835);
  removing an event leaves clips in place.
- The beat-0 protections being removed: remove-first rejection
  (arr_edit.rs:852-857), move-away-from-0 and move-onto-0 rejections
  (:767-780), insert-collision rejection (:730-738), and the unconditional
  `start_beat == 0.0` exemption in `clear_scene_lane_span`
  (song_region.rs:734) that makes full-span region delete "extend Scene 1".
- ARR REC bootstraps a song-less project through a dedicated `(None, _)`
  arm in `try_finish_song_capture_take` (song_capture.rs:445-465) that
  hand-builds a one-scene-event arrangement; the transport branches on
  `committed_song().is_some()` (song_transport.rs:241) to pick whole-song
  capture vs splice-on-top.
- `reconcile_committed_arrangement_track_lanes` (rack_editing.rs:13-40)
  no-ops when arrangement is `None`; when live, a surplus authored lane on
  track removal is an error (:21).
- `ProjectArrangement::new` (arrangement.rs:131) already builds a minimal
  valid arrangement — but has zero production callers.
- Serialization: ProjectFile v6; `arrangement: Option<ProjectArrangement>`
  (project.rs:111); load passes `None` through unchanged
  (projects.rs:3325-3337). UI: `song-exists` (song_state.rs:646 → :900)
  gates the "No song yet" banner (ui/arrangement.lisp:1518-1523, :1605-1608).

## 3. Terminology

- **Empty arrangement** — `scene_lane: []`, all track lanes empty,
  `end_beat = DEFAULT_ARRANGEMENT_END` (64 beats), `loop_enabled: false`,
  `next_clip_id: 0`. Valid, compilable, plays silence.
- **Unscened span** — the beat range before the first scene event (the
  whole timeline when the lane is empty). No label, no scene spans
  published, no stamping, no scene-cell fallback.
- **Replace (scene drop)** — dropping/inserting a scene event at a beat
  where one already starts *sets* that event's scene instead of rejecting.

## 4. Model changes

### 4.1 `ProjectArrangement` (state/arrangement.rs)

Struct unchanged. `validate` loosens:

- Scene lane **may be empty**. When non-empty: still sorted, strictly
  increasing, each `scene < scene_count`. The first-event-at-0.0 rule is
  **deleted**.
- `end_beat` still finite and `> 0`, and `> ` last scene-event start *when
  events exist*.
- Everything about clips is unchanged (sourced, ordered, non-overlapping,
  within `end_beat`, ids bounded).

Add `ProjectArrangement::empty(track_count) -> Self` (or repurpose `new`)
producing the empty arrangement above.

### 4.2 Compiled `ProjectSong` (state/song.rs)

- `ProjectSongRow.scene: usize` → `scene: Option<usize>`. `None` means "no
  governing scene": UI shows no scene label, and the absent-override
  fallback in `preflight_runtime_song` resolves to **silence** instead of a
  scene cell. (On the compile path this fallback is already unreachable —
  every lane gets an explicit override — so this is belt-and-braces plus
  def-song-lowering correctness.)
- `ProjectSong::validate` keeps rows-non-empty and row-1-at-0.0 **as is**.
  These become compile-output guarantees, not user-model rules.
- `compile_arrangement`: boundaries before the first scene event map to
  rows with `scene: None`; if no boundary lands at beat 0, **synthesize** a
  `scene: None` row at 0 with all-explicit-empty overrides. The
  `scene_event_at_beat(...).ok_or(...)` hard error is deleted. An empty
  arrangement compiles to exactly one silent `scene: None` row spanning
  `[0, end_beat)`.
- Net effect: `SongPlaybackRuntime`, `SongChunkPlan`, `rebuild_song`,
  `state_at_beat`, transport — **zero changes**. They keep receiving ≥ 1
  row tiling from beat 0.

### 4.3 Presence

`SequencerState.arrangement` and `.song` conceptually become always-`Some`
while a project is open. Mechanically the `Option` may stay (load
staging), but:

- `require_arrangement()` and all "The project has no arrangement/song"
  rejections are deleted; call sites read the arrangement directly.
- `clear_project_arrangement_state` / `arr_clear` install the **empty
  arrangement** (and its compiled song), never `None`. `arr_clear` stops
  erroring when already empty — clearing empty is a no-op.
- `set_committed_song` (accessors.rs:978 — project reset and retired
  `EditPatch::Song` undo replay) must install a matching arrangement
  (empty on reset) instead of nulling it.
- Region ops (`song_region.rs` ×5), `arr_empty_take_clip_create`,
  `song_region_to_take` drop their "no song" rejections.

## 5. Edit-semantics changes (decisions confirmed 2026-08-02)

1. **Remove first scene event: allowed.** Same semantics as any remove —
   marker goes, clips stay (no re-stamp). Its former span joins the
   unscened prefix.
2. **Move onto/away from beat 0: allowed.** Beat 0 is an ordinary beat.
   Moving *onto* an occupied beat follows rule 3.
3. **Insert/drop collision becomes replace.** Dropping a scene event at a
   beat where one already starts performs `arr_scene_event_set` on that
   event — and therefore **re-stamps**: the dragged scene's effective
   per-track patterns (its cells) are written as real clips across the
   event's span, truncating what's there, exactly like any set. Dragging
   "Scene 2" to the start of a botched arrangement does what it looks like.
4. **Full-span region delete truly empties.** `clear_scene_lane_span`
   drops the beat-0 exemption; select-all + delete removes every scene
   event and every clip. `restore_scene_tail` behavior for *interior*
   regions is unchanged (the governing scene still resumes at region end —
   when a governing scene exists; if the region start is unscened, nothing
   is restored). Update the pinning test
   `scene_lane_region_ops_never_remove_the_event_at_zero`
   (song_region.rs:2005) to the new rule; audit `paste_scene_events` and
   `set_scene_event`, which share the helpers.
5. **Track deletion deletes its lane** — clips, takes references and all,
   same as Ableton. Track add appends a lane, stamped from the governing
   scene where one exists, empty over unscened spans.
   `reconcile_committed_arrangement_track_lanes` now always runs (the
   arrangement always exists); the "extra authored lanes cannot be removed
   automatically" error (rack_editing.rs:21) is replaced by lane removal.
6. **`end_beat` on empty stays a stored default** (64 beats), not derived
   from content. Any edit or capture landing past `end_beat` auto-extends
   it (round up to the next bar; if the landing beat sits exactly on a
   bar, extend one bar further so `end > ` the content). `arr_set_end`
   still refuses `end <= last scene start` when events exist — but that
   case is now only reachable by dragging the end handle inward, never as
   a side effect of another edit.
7. **Scene-event move/insert past `end_beat` auto-extends** — this is
   rule 6 applied to the scene lane, called out because it's a shipped
   rejection today: "Cannot move the scene change to beat N: the
   arrangement ends at beat M; extend it first". Resizing a scene by
   dragging its boundary (= moving the following scene event) past the
   end must never bounce with "extend it first" — the ordering problem is
   the model's to solve, not the user's. Same for clip move/resize/create
   and paste landing past the end.

## 6. Capture (ARR REC)

- The `(None, _)` bootstrap arm in `try_finish_song_capture_take`
  (song_capture.rs:445-465) is **deleted**. Capture is always a `[P, Q)`
  splice into the existing arrangement — including into the empty one,
  where `P = 0` and the splice lands on a blank timeline. One code path.
- The transport branch keyed on `committed_song().is_some()`
  (song_transport.rs:241) re-keys on **playback state**, not existence:
  playing on top of running song playback → punch-in splice at the current
  beat; starting from stop → capture from the pressed play position
  (beat 0 by default). Existence is no longer a signal — an empty
  arrangement exists.
- `SongCaptureTake::whole_song` semantics are preserved as "splice from 0
  into empty".

## 7. Serialization & migration

- Bump to **ProjectFile v8** (v7 was already taken by the Sounds model,
  takes spec 17/18.1).
- Save: always write `arrangement` (an empty arrangement serializes
  fine — empty scene lane, empty lanes). `use_arrangement` and any
  presence flags become vestigial; keep writing for parse tolerance,
  ignore on load.
- Load v8: `arrangement` expected; if absent (hand-edited file), fall back
  to empty.
- Load v ≤ 7: `arrangement: None` → **empty arrangement**; `Some(a)` is
  already valid under the looser rules — no rewrite needed beyond the
  existing v5 backdrop-freeze migration.
- One-way as usual: v8 files won't open on older builds (they'd reject an
  empty scene lane).

## 8. UI surfaces

- Delete the "No song yet — record an arrangement (ARR REC) or define one
  with def-song." banner (ui/arrangement.lisp:1518-1523) and the
  `SEQ.song-exists` gate (:1605-1608). If a hint is still wanted, key it
  on "arrangement has no content" (no events and no clips), softly, in
  the empty lane area — not as a mode.
- `song-exists` in song_state.rs: keep publishing `true` always for one
  release (lisp compat), then remove; add `arrangement-has-content` if the
  hint above wants it.
- `scene-spans` publishes **no span** over the unscened prefix — the scene
  lane renders blank there. Row/status readouts show no scene name for
  `scene: None` rows.
- `arrangement-content-length-min` (ui/arrangement.lisp:346) currently
  floors on `SEQ.scene-spans`; make it also consider clip extents and the
  stored `end_beat` so an empty arrangement still draws its default length.
- def-song is unchanged (`lower_rows_to_arrangement` still requires ≥ 1
  row — a def-song with no rows stays an error; "empty" is reached by
  clearing, not defining).

## 9. Invariant-site checklist (delete/adjust)

Gates to delete: arr_edit.rs:46-52 (require_arrangement), :989
(arr_clear), song_region.rs:255/:342/:398/:510/:887, take_edit.rs:1022/
:1139/:1146, the arr_edit.rs:852-857 / :767-780 / :730-738 scene-event
rejections, song_region.rs:734 beat-0 exemption, and the two
"extend it first" past-end rejections at arr_edit.rs:726 (insert) and
:784 (move) — both become auto-extend (§5.7).

Sites that keep their assumptions (satisfied by compile synthesis, §4.2):
song.rs:235/:247 (ProjectSong::validate), song_playback.rs:85/:89,
song_runtime.rs:415/:424-426/:489-490, song_transport.rs:285-298,
song.rs:414-431 (state_at_beat).

Sites to re-point at the always-present arrangement:
projects.rs:950/:3325-3337, accessors.rs:978/:1027, rack_editing.rs:13-40,
song_capture.rs:395/:422-466/:482, song_transport.rs:241,
song_state.rs:646/:651-658/:742/:772.

## 10. Clip drag-and-drop (follow-up, decided 2026-08-02)

Not supported today, but it should be, and this model is what makes it
cheap: with the arrangement always present and unscened spans legal, a
drop is a single `arr_clip_create` — no preconditions, no bootstrap.

- **Sources**: a pattern (a track's scene cell from the Seq grid / a
  pattern reference) or a take. The drop target is one track lane at a
  beat; the created clip gets that source, a default length (the source's
  loop length, bar-rounded), and a **free-run offset** (`steps(start) mod
  L`), consistent with the stamping rule — dropping a clip never shifts
  the rhythm of the underlying pattern.
- **Never creates a scene event.** A clip drop on an unscened span
  creates only the clip; the scene lane is untouched wherever the drop
  lands. Scene events enter the lane only via explicit scene-lane
  gestures (drag a *scene*, insert, paste).
- Drops past `end_beat` auto-extend it (§5.6). Overlap with an existing
  clip follows the existing truncation rule (new clip wins its span).
- Moving/resizing existing clips already exists (`arr_clip_move` /
  `arr_clip_resize`); this section only adds the create-by-drop gesture
  and its UI plumbing (timeline drop target + drag payload from the Seq
  grid/browser).

## 11. Implementation notes (rev 2 — where the build refined rev 1)

- **Track deletion already deleted its lane** —
  `remap_arrangement_after_track_delete` (track_delete_remap.rs) predates
  this spec. The reconcile "extra authored lanes" error survives as
  genuine-drift protection only; §5.5's policy needed no new code.
- **`scene: Option<usize>` reached the runtime rows too**:
  `RowStaging.scene` and `RuntimeSongRow.scene` are `Option<usize>`. An
  unscened row recalls no scene at its boundary
  (`apply_song_row_control(None)` keeps the current Seq scene), stamps no
  `transport.current_pattern`, and carries empty scene-owned graph state
  (mod connections, networks, process chain) in its snapshot.
- **Auto-extend granularity**: scene events extend to the next bar
  boundary strictly after the beat (`next_bar_end`, arr_edit.rs — the end
  must exceed the last event's start); clip create/move/resize extend to
  the exact clip end (`end_beat.max(...)`), matching the pre-existing
  move/resize behavior.
- **Zero-length captures are a graceful no-op**, not an error: Stop on the
  very beat of the first launch (`Q <= P`) previously failed `end > last
  start` validation by accident; committing it under the looser rules
  would have written a stray scene event, so the punch region is filtered
  instead and the stop reports "unchanged".
- **Capture into empty keeps the canvas length**: the splice takes
  `end_beat.max(Q)`, so a short take into a fresh 64-beat arrangement
  leaves `end_beat` at 64 rather than shrinking it to the stop beat (the
  old bootstrap arm set `end = Q`).
- **Splice scene baseline is an `Option`**: `splice_scene_lane` measures
  captured scene changes against `previous.scene_at_beat(P)` (`None` on an
  unscened span), so the first scene launch into an empty arrangement
  writes its marker instead of being swallowed by the old
  `unwrap_or(captured scene)` fallback.
- **`arr_clear` preserves the clip-id allocator** (ids are never reused
  within a project) and is a no-op when the arrangement is already empty
  or was never committed.
- **On-demand install**: `require_arrangement` and `start_song_playback_at`
  fall back to `ProjectArrangement::empty(track_count)` when the committed
  slot was never seeded (fresh `SequencerState` in tests, legacy resets),
  so "always exists" holds even for states that skipped
  `start_new_project`/load.
- **`song-exists` still publishes** (now effectively always `true`) for
  lisp compat; the banner and its gate are deleted.

## 12. Non-goals

- No change to the clips-are-explicit rule, stamping semantics (free-run
  offsets), takes phase math, or the compiled-row playback engine.
- No auto-creation of scene events, ever — neither by clip drops (§10)
  nor by capture landing on unscened spans.
- No change to Seq-view scene launching or the scenes model itself.
