# Realtime Arrangement Feedback — Recording You Can See, Editing While It Plays

Status: rev 1, 2026-07-27 — **design, nothing built.** Raised while testing
clip move (`docs/arrangement-region-editing-spec.md` §6): edits and recordings
only become visible when they commit, which for arrangement capture means
*after you stop*. This spec covers the two halves of that: seeing recorded
material as it is recorded, and editing the arrangement while it plays.

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
`crates/sequencer/ui/arrangement.lisp`

## 1. Summary

Three slices, in dependency order. 1 is read-only and independently shippable;
2 is the model/scheduler work; 3 is optional.

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
3. **Incremental capture commit** — long takes commit per chunk rather than
   only at Stop, so a 5-minute recording is not one all-or-nothing entry.
   Open (§8); slices 1 and 2 do not depend on it.

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
- **Content edit-through already exists.** `SongPlaybackCommand::Refresh`
  (`song_runtime.rs:112`) hands the scheduler re-preflighted rows;
  `SongPlaybackRuntime::replace_song_in_place` (`song_runtime.rs:407-423`)
  swaps them **only if the row layout is identical** — same row count, same
  `end_beat`, same ids and start beats — and returns `false` otherwise.
  Driver: `App::invalidate_song_rows_for_pattern` (`sound_binding.rs`), for
  device edits landing on a pool pattern the playing song resolves (takes spec
  §16.7).
- So the machinery for "make the future correct without disturbing the
  cursor" is built and proven. What is missing is the **layout-changing** case
  — which is exactly what every arrangement edit is.

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

## 5. Phasing & tests

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
3. **Incremental capture commit** (§8) — only if taken.

## 6. Proposed decisions

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

## 7. Why this is worth doing

Recording without feedback is the harshest failure mode the arrangement has:
the performer plays four bars into what looks like an empty timeline and finds
out at Stop whether the punch-in landed. And the edit lock makes the
arrangement the one surface in the program you cannot touch while it plays,
which is precisely when you know what you want to change.

## 8. Open questions

- **Incremental capture commit.** Committing per chunk gives crash safety and
  a visible, undoable trail, but changes capture from one atomic entry into
  many — and the stop-commit's splice semantics (takes spec §8.5) assume it
  owns `[P, Q)` at the end. Worth it, or is slice 1's feedback enough?
- **Editing during capture.** §4.1 keeps it locked. Is there a subset (edits
  strictly outside the punch region) worth allowing, or does that just move
  the race?
- **Latched lanes.** A track under manual override (takes spec §10) already
  ignores the song's authority. Should an edit to a latched lane's clips
  refresh at all, or wait for Back to Song?
- **Rebuild rate.** A commit per gesture is fine; a live-coded script that
  rewrites the arrangement in a loop is not. Does `Rebuild` need coalescing
  (one per frame, latest wins) or is the bounded command channel's back
  pressure enough?
