# Realtime Arrangement Feedback — Recording You Can See, Editing While It Plays

Status: rev 3, 2026-07-27 — **design, nothing built; all questions resolved.**
Raised while testing clip move (`docs/arrangement-region-editing-spec.md` §6):
edits and recordings only become visible when they commit, which for
arrangement capture means *after you stop*. This spec covers the three halves
of full realtime feedback: seeing recorded material as it is recorded, editing
the arrangement while it plays, and step-sequencer note edits reaching the
playing song and the timeline. Rev 2 added slice 3 (note edit-through) after
tracing the step-commit path and finding the "content edit-through already
exists" claim covered device edits only. Rev 3 resolves the open questions
(§9) — notably cutting incremental capture commit.

Related: `docs/song-mode-spec.md` (§7 transport authority, §9/§10 runtime and
row transitions, §13 the mode machine), `docs/arrangement-lane-model-spec.md`
(§6 model, §8 primitives), `docs/takes-and-additive-arrangement-recording-spec.md`
(§8 take recording, §10 latch, §16.7 edit-through),
`docs/arrangement-region-editing-spec.md` (the ghost-preview pattern),
`crates/sequencer/src/app/song_edit.rs`,
`crates/sequencer/src/app/song_transport.rs`,
`crates/sequencer/src/app/song_capture.rs`,
`crates/sequencer/src/app/take_recording.rs`,
`crates/sequencer/src/app/sound_binding.rs`,
`crates/sequencer/src/sequencer/state/song_runtime.rs`,
`crates/sequencer/src/sequencer/state/sequencer_state/song_playback.rs`,
`crates/sequencer/src/scheduler/lookahead.rs`,
`crates/sequencer/src/ui/state_values/song_state.rs`,
`content/ui/arrangement.lisp`

## 1. Summary

Three slices. 1 and 3 are independently shippable; 2 is the model/scheduler
work.

1. **Recording feedback** — while arrangement capture runs, the pending takes
   and the launches captured so far render as **provisional** items in the
   arrangement lanes, growing under the playhead. No model change: a new
   read-only reactive surface over state that already exists in memory.
2. **Editing while it plays** — the blanket song-edit lock becomes a
   classified one. Every clip/region primitive works during `SongPlayback`,
   commits normally (one undo entry), and reaches the running scheduler
   through a layout-tolerant re-preflight. The row under the playhead is never
   re-entered: an edit ahead of the playhead is inaudible now and correct when
   reached; an edit under it takes effect at the next boundary.
3. **Note edit-through** — a step-sequencer edit to a pattern the playing song
   resolves becomes audible (the step-commit tail gains the same
   `invalidate_song_rows_for_pattern` call the device path has — a note edit
   never changes row layout, so the existing `Refresh` path carries it) and
   visible (the timeline's note dots gain a pool-content revision so they
   refresh without a committed-song change). Independent of slice 2: step
   edits never went through the song-edit lock.

A fourth slice — **incremental capture commit** (per-chunk commits during a
long take) — was considered and **rejected** (§9): slice 1's provisional
feedback removes the flying-blind problem, and the atomic stop-commit's
ownership of the whole `[P, Q)` splice is worth keeping simple.

## 2. Current facts (verified 2026-07-27)

**The lock is a single boolean.**

- `App::song_edits_locked()` returns `song_transport_locks_edits`, set in
  `set_song_transport_mode` for exactly `SongPlayback | ArrangementCapture`
  (`song_transport.rs:63-71`). `require_song_edit_unlocked` rejects with
  `SONG_EDITS_LOCKED_ERROR` — "song editing is unavailable during song
  playback/capture" (`song_edit.rs:17`).
- Every `arr_*` primitive (`arr_edit.rs`) and every region primitive
  (`song_region.rs`) calls it first, so the lock is one seam, not fifty.
- The UI already surfaces the rejection: the arrangement error banner renders
  `SEQ.song-edit-error` (`arrangement.lisp`). Gestures themselves are not
  blocked — ghosts preview fine while playing; only the commit is refused.

**The scheduler consumes a preflighted, immutable song.**

- `RuntimeSong` / `RuntimeSongRow` (`song_runtime.rs:30-63`) is the preflight
  product: every row resolved against the live project, carrying a prebuilt
  `Arc<SequencerSnapshot>`. A row transition on the scheduler thread is an
  `Arc` pointer switch — no mutexes, no cloning, no allocation, no asset
  loading (`lookahead.rs`, the song-playback branch).
- **Content edit-through exists for device edits only.**
  `SongPlaybackCommand::Refresh` (`song_runtime.rs:112`) hands the scheduler
  re-preflighted rows; `SongPlaybackRuntime::replace_song_in_place`
  (`song_runtime.rs:407-423`) swaps them **only if the row layout is
  identical** — same row count, same `end_beat`, same ids and start beats —
  and returns `false` otherwise. The complete caller set of
  `invalidate_song_rows_for_pattern` (`sound_binding.rs:417`) is the
  device-value fan-out (`edit.rs`, `fan_out_device_values_to_take_chunks`),
  its gesture-deferred flush in `finish_active_gesture`
  (`pending_song_row_invalidation`), and device-state copy
  (`sound_binding.rs`). No note/step path calls it (takes spec §16.7 was
  about device edits).
- So the machinery for "make the future correct without disturbing the
  cursor" is built and proven. What is missing is the **layout-changing** case
  — which is exactly what every arrangement edit is — and the **note-edit**
  driver (slice 3), which needs no new scheduler machinery at all.

**Step edits are pool-correct but song-invisible.**

- Every step/note edit resolves to the **effective current-scene pattern**
  (`ensure_effective_track_pattern`, `edit.rs`; there is no
  arbitrary-`PatternId` step-edit path), commits as a `StepCells` /
  `PatternGeometry` patch, and writes both the pool pattern and the live
  lanes (`restore_pattern_step_cells_no_publish`, `step_edit.rs`). During
  song playback the control mirror re-points the live lanes to each row's
  resolved scene (`apply_song_row_control`), so on a non-latched lane the
  performer is editing **exactly the pool pattern the sounding row
  resolved**. The intent is right; only the plumbing stops short.
- The commit publishes `publish_scheduler_track` — but the scheduler plays
  the preflight-cloned row snapshot (`lookahead.rs`, `row_snapshot`), so the
  edit is **inaudible for the rest of the song, including loop wraps**.
  `pending_gesture_publishes_scheduler` returns `false` for
  `StepCells | PatternGeometry`, so gesture end does not rescue it either.
- Exception: **manually latched lanes already hear edits.** The lookahead
  merges the live snapshot over the row snapshot per chunk for every latched
  bit (`lookahead.rs:304-323`), consistent with the Seq UI only blocking
  pointer gestures on take-governed lanes (state `1` in
  `SEQ.song-track-governed`; ordinary lanes are never dimmed or blocked
  mid-playback — `song_state.rs:389`).
- The timeline never sees note edits either: `SEQ.song-lane-events` rebuilds
  on `pattern_epoch`, which **no step edit bumps** — and an invariant test
  (`song_transport.rs:965`) forbids the control-side mirror from bumping it,
  so the dots refresh only incidentally (scene launch, playback start,
  restore).

**Recording is invisible by construction.**

- Take recording keeps content in **detached** `TrackPatternData` chunks
  (`PendingTakeLane`, `take_recording.rs:23-48`), registered as real takes
  only inside the stop-commit. Cancel is a plain drop.
- Launch capture accumulates lightweight `CaptureLaunchEvent`s on the control
  thread and consolidates them at Stop (`song_capture.rs` header,
  `finish_song_capture_take`), committing scene events and clips in one entry.
- Neither is published to any reactive surface, so the arrangement view has
  nothing to draw. The lanes stay empty until Stop, then everything appears.

**The read surface is revision-gated.**

- `sync_song_state` (`song_state.rs`) rebuilds `SEQ.song-lanes` and
  `SEQ.scene-spans` only when `committed_song_revision()` moves, and
  `SEQ.song-lane-events` only when the lanes or the pattern epoch change. A
  provisional surface therefore needs its own change counter — it must not
  ride the committed revision, and it must not publish per frame.
- `SEQ.song-position-beats` already publishes at render rate while a panel
  that renders it is visible, and every arrangement lane already draws a
  playhead from it. The time base for live feedback exists.

## 3. Slice 1 — recording feedback

### 3.1 What the performer sees

While `ArrangementCapture` is active:

- Each **armed track that has punched in** shows a provisional clip starting
  at its punch-in beat `P` and ending at the record head (the current song
  position), growing under the playhead, with its notes drawn through the
  existing dot pipeline.
- Each **captured launch** shows as a provisional scene event / clip at the
  audible beat it was captured at, so the scene lane fills in as you perform.
- Provisional items are visually distinct (recording tint, no title bar) and
  are **inert**: not selectable, not draggable, not deletable. They are not
  clips yet — there is no `ClipId` to name — and a gesture that appeared to
  edit one would have nothing to lower to.
- At Stop they are replaced, in the same frame, by the real committed clips.
  At Cancel they vanish.

### 3.2 The surface

One new reactive binding, `SEQ.song-pending`, published only while capture is
active and cleared on every exit path (stop, cancel, failure):

```
{ :origin-beat f64            ; capture origin in song beats
  :head-beat   f64            ; record head (clamped to the published position)
  :lanes (list {:track i :start-beat P :end-beat head :dots (...)})
  :scene-events (list {:start-beat b :scene s}) }
```

As built (2026-07-27), two refinements to that shape:

- Lanes and launches carry **raw events**, not `:dots` —
  `{:num-steps :length-beats :events ((time transpose velocity duration)...)}`,
  the same shape `song-lane-events` publishes — so the view normalizes them
  through the committed clips' own `arrangement-windowed-dots` pipeline
  instead of a second one. A take lane's dots window over the DRAWN span, not
  the recorded length: the item grows to the head while the notes stay put,
  and normalizing over the content would stretch the same dots wider every
  frame.
- A fourth key, `:track-events (list {:track i :start-beat b ...})`, carries
  what each captured launch put on each TRACK lane: a clip launch's own
  override, and a captured scene change expanded to the scene's cell pattern
  on every lane it claims (take lanes excluded, matching `consolidate`).
  Without it a captured scene change drew in the scene lane while the clips
  it implies — the ones the splice actually writes — stayed invisible.

Sourced from `TakeRecordingSession` (the pending lanes' `punch_in_beat`,
`step_beats`, `chunks`, `max_end_steps`) and the capture's
`CaptureLaunchEvent` list. Both live on `App`, on the control thread, so this
is a plain read — no new cross-thread traffic.

### 3.3 Cost control

A per-frame rebuild of note dots for a growing take is the obvious way to make
this expensive. The publisher must:

- keep a `pending_revision: u64` on `App`, bumped by `take_record_note` and
  `record_song_capture_launch` — the only two writers;
- rebuild the dots only when that revision moves, and rebuild the **span**
  (head beat) on the existing position cadence, quantized to the lane's step
  so a 1px-per-frame crawl does not re-diff the whole value;
- publish nothing at all when capture is inactive, so the common path pays one
  boolean.

### 3.4 Rendering

`arrangement.lisp` composes provisional items into each lane's `:items` after
the committed clips, with a distinct `:color` and no `:label`. They must not
appear in `arrangement-track-clips` (the gesture source of truth), so
`arrangement-track-action` can never address one: the split is items-for-
drawing vs clips-for-editing, the same separation the ghost already uses.

## 4. Slice 2 — editing while it plays

### 4.1 The classification

`song_edits_locked()` becomes a per-operation question, not a mode boolean:

| Mode | Clip/region primitives | Capture-owned edits |
| --- | --- | --- |
| `Stopped` / `SessionPlayback` | allowed (today) | n/a |
| `SongPlayback` | **allowed (new)** | n/a |
| `ArrangementCapture` | **still refused** | the capture's own splice |

Capture stays locked on purpose: it is about to splice `[P, Q)` into the
arrangement at Stop (takes spec §8.5), and a concurrent structural edit would
race that splice for the same span. The error message narrows accordingly —
"song editing is unavailable during arrangement capture" — so the common case
stops lying about playback.

### 4.2 The commit path

Every primitive already funnels through `commit_arrangement_edit` /
`commit_region_edit` → `state.set_committed_arrangement`, which recompiles the
song and bumps the revision. One line is added at the end of that funnel:

> if song playback authority is active, `preflight_runtime_song()` and send
> the result to the scheduler.

This is the same two calls `invalidate_song_rows_for_pattern` already makes;
the difference is what the scheduler does with a layout that moved.

### 4.3 The scheduler side

`replace_song_in_place`'s all-or-nothing layout check is replaced by a
**cursor re-map**, delivered as a new command so the existing content path
keeps its cheap identity check:

`SongPlaybackCommand::Rebuild { song }` — applied by the scheduler as:

1. Compute the playhead's current song beat from the runtime's own clock
   (`clock_beat_offset` + accumulated beats) — never from the row index, which
   is exactly what an edit can invalidate.
2. Find the row governing that beat in the NEW song
   (`RuntimeSong::row_index_at_beat`).
3. If that row's `resolved_sources` and `lane_offsets` equal the currently
   sounding row's, swap immediately: nothing audible changes, and the future
   is now correct.
4. Otherwise hold the new song as **pending** and swap at the next row
   boundary. The sounding row is never re-entered mid-flight: no clock reset,
   no retrigger, no anchor change (takes spec §7.3).
5. If the playhead is now past the new `end_beat`, follow the existing
   end/loop rule rather than inventing a third behavior.

Every allocation stays on the control thread: the `Arc<RuntimeSong>` is
prebuilt, the scheduler only stores a pointer and compares small slices.

### 4.4 What the performer sees

- Dragging a clip that is ahead of the playhead: it moves, it commits, and it
  plays from its new position when reached. No audible seam.
- Dragging the clip that is **currently sounding**: the audible row keeps
  playing to its boundary, then the edit takes over. This is the only rule the
  UI needs to explain, and it is the same rule Ableton's launch quantization
  teaches already.
- Undo during playback follows the identical path — it is just another commit.

### 4.5 Non-goals

Tempo/timebase changes mid-flight, scene-cell edits (a different subsystem
with its own rebuild), and any attempt to crossfade the audio across a swap.
The contract is "the future becomes correct", not "the present morphs".

## 5. Slice 3 — step edits reach the playing song and the timeline

The DAW expectation: edit notes in the step sequencer while the arrangement
plays and (a) hear the change wherever the song plays that pattern, (b) see
the timeline's dots update. §2 established that (a) fails because nothing
drives `Refresh` from a note commit, and (b) fails because nothing the commit
touches gates the lane-events rebuild. Both fixes are one-seam.

### 5.1 Audible: drive the existing Refresh from the step-commit tail

After `apply_recorded_step_mutation` commits (and inside `replay_step_patch`,
so undo/redo ride the same seam), call
`invalidate_song_rows_for_pattern(track, target_pattern)` with the
`StepCells` target — the same one-liner the device path has. While a history
gesture is active, defer through the existing `pending_song_row_invalidation`
slot exactly as device drags do.

Why this is sufficient, not a simplification:

- A note edit **never changes row layout** — rows are beat spans over
  resolved sources; note content lives inside the snapshot. So
  `replace_song_in_place`'s identity check passes and the existing `Refresh`
  command carries the whole slice. Slice 2's `Rebuild` is not needed here,
  which is why this slice ships independently.
- The **sounding row updates too**: `replace_song_in_place` swaps the row
  `Arc`s and the lookahead reads `row_snapshot(row)` per chunk, so steps
  ahead of the playhead *within the current row* become audible at the next
  lookahead chunk. No retrigger, no clock disturbance — the row is not
  re-entered, its snapshot pointer moves.
- `PatternGeometry` (length) commits keep row layout identical too — the
  song's beat math comes from the arrangement, not the pattern length — so
  they take the same path. (Verify with a test rather than by assertion:
  a length change alters `resolved` content, not row spans.)
- Latched lanes keep their live-merge behavior; after this slice, latched
  and song-governed lanes simply agree.

Cost: each call is a full `preflight_runtime_song`. Step toggles commit
per-gesture-less click, so rapid mouse work re-preflights per click. Start
with that (device edits already accept it at gesture end); if profiling
objects, coalesce to at most one flush per reactive tick, latest-wins per
track — the deferred slot already models this.

### 5.2 Visible: a pool-content revision for the lane dots

Do **not** bump `pattern_epoch` from step commits — the invariant test
(`song_transport.rs:965`) exists because the epoch drives scene-launch-scale
resyncs, and a per-note bump would stampede unrelated caches. Instead:

- Add a `pool_content_revision: u64` bumped in the one funnel every step
  write shares — `restore_pattern_step_cells_no_publish` /
  `restore_pattern_num_steps_no_publish` (undo, redo, and live edits all
  pass through `replay_step_patch` into these).
- `sync_song_state` adds it to the lane-events gate:
  `lanes_changed || pattern_epoch moved || pool_content_revision moved`.
  The existing value-diff against `cached_lane_events` still suppresses
  publishes when the edited pattern is not one any lane resolves.

Take-governed lanes (state `1`) stay pointer-blocked in the Seq grid — an
edit there would target the scene pattern the lane is *not* playing, which is
the lie the block exists to prevent. Editing a take clip's notes is a
separate, clip-addressed gesture and stays out of scope.

## 6. Phasing & tests

1. **Recording feedback** (§3) — read-only.
   - `state_values`: `SEQ.song-pending` publishes while capturing, clears on
     stop/cancel/failure, and does NOT re-publish on a frame with no new
     notes (revision gate).
   - UI-script: provisional items render in the lane, carry no ids that
     `arrangement-track-clips` returns, and a `:select` on one selects
     nothing.
   - Capture round trip: the provisional span for a lane equals the committed
     clip's span after Stop (feedback did not lie).
2. **Editing while it plays** (§4).
   - `song_edit`: clip/region primitives succeed under `SongPlayback` and are
     still refused under `ArrangementCapture`, with the narrowed message.
   - `song_runtime`: `Rebuild` keeps the playhead beat across a row-layout
     change; identical current row → immediate swap; changed current row →
     swap at the next boundary and NOT before; past-end → existing end/loop
     rule.
   - Scheduler: editing a clip ahead of the playhead changes what plays there,
     with no retrigger of the sounding row (assert the row-applied notices).
   - History: an edit committed during playback is one undo entry, and undo
     during playback re-refreshes the scheduler.
3. **Note edit-through** (§5) — independent of 2, can land first.
   - `song_runtime`: a step commit on a pattern a playing row resolves ends
     with a `Refresh` whose swap succeeds (layout identical), including for a
     `PatternGeometry` length change; a pattern no row resolves triggers no
     preflight (the `affected` check).
   - Scheduler: a note added ahead of the playhead in the **sounding** row is
     audible in that row, with no retrigger; the edit survives loop wrap.
   - Undo/redo of a step edit re-drives both the Refresh and the dots.
   - `state_values`: a note edit bumps `pool_content_revision` and refreshes
     `SEQ.song-lane-events` with no committed-song revision change; an edit
     to a pattern no lane resolves publishes nothing (value-diff holds).
   - Gesture-coalesced step edits flush one invalidation at gesture end
     (`pending_song_row_invalidation` path).

## 7. Proposed decisions

- Provisional recording state is **read-only and inert**: drawn, never
  editable. It has no `ClipId`, so it is not addressable by any gesture.
- Provisional state rides its own revision counter, never the committed song
  revision — a recording must not invalidate the committed-lane caches on
  every note.
- Arrangement capture keeps the edit lock; song playback loses it. The lock's
  job is to protect the capture's pending splice, not to protect playback.
- The sounding row is never re-entered. Edits ahead of the playhead apply
  immediately (inaudibly); edits under it apply at the next boundary.
- Every rebuild is preflighted on the control thread; the scheduler thread
  only swaps pointers and compares resolved sources.
- Nothing here changes what a commit IS: same primitives, same single undo
  entry, same history.
- Note edit-through reuses `Refresh`, never `Rebuild`: a step edit cannot
  change row layout, and keeping the two commands' contracts distinct is what
  keeps the identity check on the hot content path cheap.
- The lane-dot refresh rides a new `pool_content_revision`, never
  `pattern_epoch` (mirror invariant) and never the committed song revision.
- Take-governed lanes stay pointer-blocked in the Seq grid; ordinary and
  latched lanes are editable and, after slice 3, audible.
- Every commit refreshes the scheduler's rows **even when every affected lane
  is latched**: the refresh is inaudible there (the lookahead merge masks it)
  but the rows are already correct the moment Back-to-Song releases the
  latch. No latch-aware special case anywhere in the commit path.
- No coalescing up front for `Rebuild` or slice 3's per-click preflight —
  device edits already pay this cost at gesture end; measure before adding a
  per-tick latest-wins flush (the deferred slot already models it).

## 8. Why this is worth doing

Recording without feedback is the harshest failure mode the arrangement has:
the performer plays four bars into what looks like an empty timeline and finds
out at Stop whether the punch-in landed. And the edit lock makes the
arrangement the one surface in the program you cannot touch while it plays,
which is precisely when you know what you want to change.

And note edit-through closes the most confusing half-truth in the current
build: during song playback the Seq grid shows the right pattern, accepts the
edit, writes the right pool pattern — and the song ignores it until the next
full restart. An edit that is 90% plumbed and 0% audible reads as a bug, not
a limitation.

## 9. Resolved questions (2026-07-27)

All open questions from rev 1/2 were resolved with the user; the outcomes are
folded into §7 and recorded here with their reasoning.

- **Incremental capture commit — REJECTED.** Slice 1's provisional feedback
  removes the flying-blind problem that motivated it; per-chunk commits would
  break the stop-commit's ownership of the whole `[P, Q)` splice (takes spec
  §8.5) for crash-safety no one has asked for yet. Capture stays one atomic
  undo entry.
- **Editing during capture — stays fully locked.** The punch region grows as
  you record, so "edits strictly outside it" is a moving target and the
  carve-out only relocates the race. Capture sessions are short; the narrowed
  error message (§4.1) is the whole fix.
- **Latched lanes — always refresh.** An edit affecting only latched lanes
  still re-preflights and swaps rows. Inaudible now (lookahead merge), correct
  at Back-to-Song, zero special cases.
- **Rebuild/preflight rate — ship without coalescing, measure first.** Device
  edits already re-preflight at gesture end without complaint. If a
  live-coded arrangement-rewriting loop objects in profiling, add a per-tick
  latest-wins flush on the `pending_song_row_invalidation` pattern.
- **Take clip note editing — deferred to a follow-up spec.** Take-governed
  lanes stay pointer-blocked in the Seq grid. The clip-addressed version
  (select a take clip, edit its chunk patterns; which chunk a step lands in
  is the only new question, since device writes already fan out) is its own
  design and must not gate realtime feedback.
