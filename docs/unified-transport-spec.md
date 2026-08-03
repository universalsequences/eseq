# One Transport — Killing the SONG/SESSION Mode

Status: BUILT (rev 2, 2026-08-02; §10 records what the build refined)
Builds on: docs/empty-arrangement-spec.md (prerequisite — always-present
arrangement, silent `scene: None` rows, capture always splices) and
docs/song-mode-spec.md / docs/takes-and-additive-arrangement-recording-spec.md
(the latch + capture machinery this spec promotes to the only transport).
The session grid and the arrangement timeline remain distinct *views*;
this spec removes only the transport *mode*.

## 1. Summary

Today the transport has a persisted mode toggle: `App::use_arrangement`
("SONG" vs "SESSION" pill). It selects what Play does — loop the current
scene's live grid forever, or play the arrangement timeline — and, combined
with Record, whether recording writes into looping patterns or captures an
arrangement take ("ARR REC"). The user has to remember which mode they're in;
the canonical failure is a project with an empty arrangement defaulting to
SONG, where Play produces silence.

This spec deletes the mode:

1. **Play always plays the arrangement** from the arrangement cursor. If the
   cursor sits on silence (empty arrangement, unscened gap, past the end),
   the currently selected scene is **auto-latched** — Play always makes the
   sound you're looking at. Session behavior is not a mode; it is song
   playback with the manual-override latch engaged, which already exists.
2. **Playback is open-ended.** Reaching `end_beat` no longer stops the
   transport (unless `loop_enabled`); the playhead runs past the end into
   silence and latched lanes keep looping — jamming is never cut off by an
   arrangement you're ignoring.
3. **Recording keys on the active view, stamped when recording engages:**
   arrangement view → arrangement capture (takes, splice); session/Seq view →
   **loop overdub** into pattern clips (today's session-record path, which
   must survive — it is how patterns get built). No ARR REC concept.
4. `use_arrangement`, the SONG/SESSION pill, `SessionPlayback`, and the
   session free-run playback path are deleted. The pill's spot becomes the
   `->SONG` (Back to Song) affordance, shown only while latched.

The load-bearing observation (code sweep 2026-08-02): `use_arrangement`
gates *only* the Play fork in `song_transport_play`
(song_transport.rs:224-262). Every other song-vs-session behavior difference
keys on the runtime authority predicate `song_playback_authority_active()`
(song_transport.rs:88-94), and that side is already shaped like this spec:
manual scene launches latch every lane plus the scene identity
(mod.rs:1663-1689), clip-grid clicks during playback delegate to
non-destructive launches (host_commands/scenes.rs:304-308), captures record
manual launches, and Back-to-Song exists globally and per track
(song_transport.rs:122-183).

## 2. Current facts this spec builds on (verified 2026-08-02)

- `App::use_arrangement` (app/mod.rs:902), persisted on ProjectFile
  (project.rs:115, default off), toggled by `set_use_arrangement`
  (song_transport.rs:186-201), which is rejected while playing (:192) and
  clears the timeline clip selection when turned off (:196-201).
- `song_transport_play(record)` (song_transport.rs:224-262):
  `!use_arrangement` → plain `start_playback()`, mode `SessionPlayback`
  (:228-231); song + record → capture on top of playback with
  `open_ended=true` (:241-254); song, no record →
  `start_song_playback_at(arrangement_cursor_beat, open_ended=false)` (:260).
- A non-looping song **ends** playback: `SongChunkPlan::Ended`
  (lookahead.rs:291) → `handle_song_playback_ended`. Capture already runs
  open-ended so grooving past the old end extends the arrangement
  (song_transport.rs:241-254).
- Session playback is the only consumer of: the free-running shared clock
  with cleared anchors (lookahead.rs:240-246), scheduler-applied quantized
  **boundary** launches (quantized_launch.rs:238, :306 — demoted to the
  control-side deadline path whenever a song is installed), and
  scheduler-side pattern/epoch resyncs (worker.rs:490, :509 — suppressed
  during song playback in favor of the control-side row mirror).
- The manual-override latch: scene launch latches all lanes except
  take-playing ones plus the scene identity (`latch_song_scene_override`,
  mod.rs:1665-1684); latched lanes get the **live session snapshot** merged
  per chunk with per-track free-run (`clear_track_anchor`,
  lookahead.rs:351-371) — so a fully latched jam hears live edits, exactly
  like session mode today.
- Recording fork at the note level (ui/input.rs:1571-1600): under song
  authority notes go to `take_record_note` and a note that cannot be staged
  is **dropped**, never folded into a pattern (:1591-1597); in session mode
  notes fall through to the live-pattern write (:1600-1625).
  `promote_song_playback_to_capture` (song_transport.rs:107-116) promotes
  playback → capture on the first armed note.
- `manual_launch_rejection()` always returns `None`
  (song_transport.rs:81-83) — the spec-7.3 launch wall is retired; its
  checks (mod.rs:1720, :1737, scenes.rs:462, :632) and
  `MANUAL_LAUNCH_DURING_SONG_ERROR` are dead code.
- `App::arrangement_view_visible` is already maintained every frame from
  `editor_has_visible_buffer("*arrangement*")` (sound-binding rule-1
  dormancy) — the view signal §5 needs already exists.
- Transport UI: SESSION/SONG pill + amber-when-latched + `->SONG`
  (transport.lisp:646-681); clock source forks on the mode (:692-698);
  mode string via `SongTransportMode::binding_str()`
  (song_transport.rs:42-48, `Stopped` and `SessionPlayback` both publish
  `"session"`).

## 3. Terminology

- **Latched / jamming** — song playback with `song_manual_latch` bits
  and/or the scene latch set: the performer owns those lanes; rows do not
  restore them until Back-to-Song / stop.
- **Silent start** — the governing row at the Play position resolves every
  lane to silence (an empty-arrangement or unscened-gap `scene: None` row
  with no clip covering any lane). Defined on the *compiled row*, not the
  raw beat, so it is one check against `resolved_sources`.
- **Recording kind** — `Capture` (arrangement view) or `Overdub`
  (session view), stamped once per recording engagement (§5).

## 4. Playback

### 4.1 Play

`song_transport_play` loses the `use_arrangement` fork. Play always runs
`start_song_playback_at(arrangement_cursor_beat, open_ended)`; the
prerequisite spec guarantees a compilable song always exists.

**Auto-latch on silent start:** after resolving the governing row for the
start beat, if it is a silent start (§3), fire the currently selected scene
as a latched launch — same code path as a manual scene launch at beat 0 of
playback (`apply_pattern_launch_at` → latch all + scene latch), so
downstream behavior (capture recording it, `->SONG`, back-to-song) is
uniform and free. A fresh project's Play is byte-for-byte today's session
Play: the selected scene loops, live edits are audible (latched lanes merge
the live snapshot per chunk), and `->SONG` lights up to show the transport
is overridden.

Play on a *non*-silent start latches nothing: the arrangement plays, and
the performer overrides by firing scenes/clips — the already-shipped
behavior.

### 4.2 Open-ended by default

`start_song_playback_at` is always called with `open_ended=true` (the
capture path already does). Reaching `end_beat`:

- `loop_enabled` → wrap, as today (accum/runtime resets,
  lookahead.rs:324-341).
- otherwise → the playhead continues past the end through silence; latched
  lanes keep free-running. `SongChunkPlan::Ended` stops being reachable on
  this path; keep `handle_song_playback_ended` as a defensive stop.

Stop restores the saved live session snapshot and re-syncs the grid, as the
song stop path already does (song_transport.rs:343-349); nothing new.

### 4.3 SongTransportMode

`SessionPlayback` is deleted. Modes: `Stopped | SongPlayback |
ArrangementCapture`. `binding_str()` publishes `"stopped"` /
`"song-playback"` / `"arrangement-capture"`; lisp consumers of
`"session"` (transport.lisp:692-698 clock fork, tests.rs:20113) update —
the clock always shows the arrangement position while playing and the
parked cursor while stopped, no fork.

## 5. Recording — the view is the mode

Loop overdub is essential (it is how patterns are built) and arrangement
capture is essential; the global toggle between them is replaced by a
spatial rule: **you record what you're looking at.**

- Recording engages at Play-with-record-on, or at the first armed note
  while already playing (the `promote_song_playback_to_capture` site).
  At that instant the active view picks the kind:
  `arrangement_view_visible` → `Capture`, else → `Overdub`.
- **The kind is stamped for the whole recording.** Switching views
  mid-recording changes nothing — flipping to the Seq tab to tweak a
  parameter mid-take must never silently reroute notes. The stamp lives on
  App (`recording_kind: Option<RecordingKind>`), cleared on stop/cancel.
- `Capture` = today's ArrangementCapture, unchanged: staging take,
  latched launches recorded, `[P, Q)` splice on stop, edit lock, forced
  punch-out on wrap. On a silent start, the §4.1 auto-latch fires *inside*
  the capture, so the selected scene is the captured initial state —
  preserving old whole-song-capture behavior on empty projects.
- `Overdub` = today's session-record note path (input.rs:1600-1625):
  quantized live-pattern writes into the armed track's current pattern,
  modulo loop length. Mode stays `SongPlayback` (no edit lock, no splice,
  no take); `recording_kind` is what the input path branches on — the
  `song_authority` check at input.rs:1574 re-keys to the stamp.

### 5.1 Overdub auto-latches the armed lanes

The load-bearing rule. On the first overdubbed note (or at engage, for all
armed tracks), the armed lane is latched via the existing per-track latch —
the same "an intentional clip interaction claims the lane" principle as
`observe_manual_clip_launch`. This buys, with zero new machinery:

- a **stable target** — the row mirror re-points live lanes at every row
  boundary; without the latch the pattern being overdubbed would change
  under the performer's fingers mid-recording;
- **audibility** — latched lanes merge the live snapshot per chunk, so the
  layered notes are heard immediately (non-latched lanes play preflight
  snapshots and live edits are inaudible);
- the exit — `->SONG` / per-track back-to-song returns the lane afterward.

A take-governed lane refuses overdub (notes dropped with a notice), same
spirit as the Seq grid's pointer-edit block on take lanes.

### 5.2 Overdub edits the pool pattern

Stated for the spec record, not new behavior: overdubbed notes land in the
pattern entity, so every arrangement region referencing that clip changes —
identical to step-editing the pattern today (note edit-through already
invalidates song rows). Isolated overdubs are achieved by firing a fresh
clip first.

## 6. Quantized launches while jamming

Scheduler-applied sample-accurate **boundary** launches exist only when no
song is installed (quantized_launch.rs:238, :306); with the session path
deleted, jam launches always take the control-side deadline path.

Phase 1 (this spec): accept the control-side timing — it is the path song
mode has always used for quantized launches, and no user has flagged it.
Phase 2 (follow-up, only if audible): teach the scheduler boundary path to
merge into latched lanes while a song is installed.

## 7. Deletions

- `App::use_arrangement`, `set_use_arrangement`,
  `USE_ARRANGEMENT_WHILE_PLAYING_ERROR`, the `seq-use-arrangement`
  command/native chain, and the SESSION/SONG pill
  (transport.lisp:646-663). `->SONG` takes the pill's slot, rendered only
  while `SEQ.song-manual-latch` (its current visibility rule).
- Clip-selection clearing moves off the toggle (song_transport.rs:196-201):
  the selection clears on project reset and explicit deselect only; view
  dormancy already handles the Seq tab (`arrangement_view_visible` rule).
- `SessionPlayback` and the `!use_arrangement` arm of
  `song_transport_play`; session free-run scheduling and the
  session-only quantized boundary-launch plumbing become dead
  (quantized_launch.rs `song_active` parameter simplifies to a constant —
  fold it out).
- ARR REC as a UI concept; record is one button, `SEQ.song-mode` string
  values shrink per §4.3.
- Serialization: `use_arrangement` becomes vestigial — keep writing it for
  parse tolerance, ignore on load (rides the same ProjectFile v7 bump as
  the empty-arrangement spec; no extra version). `record_armed`
  persistence is unchanged.

## 8. UI surfaces

- Transport: stop / play / record / `->SONG`-when-latched. Play is never
  a question; record's meaning is shown by the view underneath it. While
  recording, the transport shows the stamped kind (small "TAKE" vs "DUB"
  tag or record-button tint) so a mid-recording view switch can't mislead.
- Clock: arrangement position while playing, parked cursor while stopped
  (fork in transport.lisp:692-698 collapses).
- Auto-latch on silent start lights `->SONG` immediately — that is the
  honest signal that the transport is playing "your hands", not the
  timeline, and one click reconnects.
- Seq-view dim/block rules (take-governed lanes), per-track back-to-song
  triangles, and the mixer-grid `active_effective` suppression are
  untouched — all already keyed on runtime authority, which is now simply
  always the arrangement.

## 9. Migration / compatibility

- Projects saved with `use_arrangement=false` open into the unified
  transport; if their arrangement is empty, Play auto-latches the selected
  scene — behavior matches what those projects did before, minus the pill.
- Projects with content and `use_arrangement=false` (user was ignoring an
  existing arrangement): Play now starts the timeline from the cursor. The
  escape hatches are the ones Ableton users know — park the cursor past
  the content, or fire a scene (one click, full latch). Accepted change.
- Tests keyed on the mode string `"session"` and on
  `metal_seq_transport_song_status_shows_session_song_and_arr_rec_states`
  update to the new mode set.

## 10. Implementation notes (rev 2 — what the build refined)

- **Past-end is the LAST ROW, not silence.** The existing `open_ended`
  runtime semantic (song_runtime.rs `SongChunkPlan` planning) keeps the
  last row sounding past `end_beat`; unified playback adopts it verbatim —
  one code path, zero audio-thread changes. §4.2's "silence past the end"
  is amended accordingly; the latched-jam guarantee (never cut off) is
  what mattered and holds.
- **Silent start is defined on the compiled row**: `row.scene.is_none() &&
  resolved_pattern_ids all None`. A *scened* row whose lanes are all
  explicit-empty is an authored gap — intentional silence, never
  auto-latched.
- **The auto-latch fires inside capture too**, after the staging take
  opens, so capture into an empty arrangement records the selected scene
  as its audible initial state (a scene event at P) — restoring the old
  whole-song-capture feel and keeping the capture truthful. The pending
  surface draws it from beat zero.
- **Mode enum**: `SessionPlayback` deleted; `Stopped` publishes
  `"stopped"` (was `"session"`). `RecordingKind` lives on
  `App.recording_kind: Option<RecordingKind>` (a `pub` field —
  metal_seq is a separate bin crate), cleared whenever the mode returns
  to `Stopped`.
- **Overdub claim** = `App::claim_overdub_lane(track)`: no-op when the
  song isn't authority; refuses (drops the note) on a take-governed
  unlatched lane; otherwise latches per-track and bumps
  `song_row_mirror_epoch`. The stamp site is
  `App::stamp_recording_kind_for_note()` at the armed-note release path.
- **Serialization**: rode the empty-arrangement bump (v8, not a separate
  one); `use_arrangement` writes `true` and is ignored on load.
- **The TAKE/DUB tag box is ALWAYS laid out** (blank label when idle) —
  conditional subtree layout misses reruns from nil and re-layouts per
  flip (project rule from the takes work).
- The scheduler-side `song_active` demotion in quantized_launch.rs was
  left in place (phase 2 owns boundary-launch parity, §6).
- **The auto-latch must land BEFORE `start_playback()`** (user-found bug:
  every step-1 trigger was skipped on the first pass and only played after
  the loop wrapped). The scheduler fills its first lookahead window the
  moment the transport atomic flips; a latch applied after that schedules
  beat 0 from the silent row. `song_transport_play` therefore calls
  `prepare_song_playback_at` (everything except the transport start), then
  opens the capture take (capture branch), then auto-latches, then
  `state.start_playback()` last.
- **Overdub persistence needs two extra pieces** (user-found bug: recorded
  trigs vanished at stop). The live keyboard write goes only to the live
  grid, and a latched lane is *stale* to every masked scene save-back, so
  the stop resync re-launched the scene from a pool that never received
  the recording. Fix: (a) `claim_overdub_lane` pins the lane's session
  override to the pattern it is playing (`pin_track_override_to_effective`)
  — even when the silent-start auto-latch already latched it — turning
  every masked save-back into a legal SELF-WRITE; (b) the SongPlayback
  stop arm runs `save_current_pattern_snapshot` BEFORE clearing the latch
  and resyncing. Regression test:
  `overdubbed_steps_survive_transport_stop`.

## 10.1 Rev 3 refinements (2026-08-03, user-driven)

- **The auto-latch capture stamp is the explicit raw beat 0**, never a
  clock read. The record clock extrapolates from the previous run's anchor
  and grows with wall time while stopped; since the auto-latch fires
  before `start_playback()`, reading it stamped the initial scene tens of
  beats late — the capture committed with no scene at beat 0 and a sliver
  event near the stop boundary. Test:
  `auto_latch_capture_stamp_ignores_the_stale_record_clock`.
- **Past-end Play is jam space.** `normalize_start_beat` no longer rejects
  `start >= end_beat` (non-looping); the last row governs past-end starts
  (both row-resolution sites fall back), and a past-end start is ALWAYS a
  silent start → auto-latch. The arrangement timeline's scroll extent
  follows the playhead during past-end playback. This is the "park the
  cursor after the arrangement and play it like hardware" gesture. Test:
  `play_past_the_arrangement_end_is_jam_space`.
- **The latch survives transport stop (Ableton Back to Arrangement).**
  Stop no longer clears `song_manual_latch`;
  `resync_live_grid_to_current_scene` skips latched lanes; `back_to_song`
  works while stopped (clears + resyncs); a project switch still clears.
  Pause/play round-trips keep the performer's overrides (the row apply and
  lookahead merge were already latch-masked). UI: the TAKE/DUB tag and the
  `->SONG` pill are replaced by ONE `back-to-arrangement-icon` button
  (orange tile, play triangle + three lanes, Ableton-style) that lights
  whenever `SEQ.song-manual-latch` is set and clears the latch on click —
  stopped or playing. `SEQ.song-recording-kind` stays published for future
  use. Known edge: `launch_scene` inside the latch-skipping resync still
  clears override pins, so live edits to a still-latched lane made while
  STOPPED don't self-write until re-claimed. Tests:
  `latch_survives_stop_and_the_next_play`,
  `metal_seq_transport_back_to_arrangement_button_follows_the_latch`.

## 10.2 Rev 4 (2026-08-03, user-decided): launches always override

- **Every manual launch latches — playing or stopped, empty arrangement
  included.** One gesture, one meaning: clicking a scene or clip is always
  an override of the arrangement, always lights the back-to-arrangement
  indicator, and Play then plays the override (the latch already survives
  stop). User explicitly chose no empty-arrangement carve-out ("fine with
  it being always on on fresh projects — actually clearer").
- **The take-lane exclusion is scoped to capture.** During
  `ArrangementCapture` a scene launch still never claims lanes playing
  takes (a recorded scene change must not steal a fresh take); outside
  capture a scene launch claims every lane — this fixes "scene change
  can't override a take while jamming".
- **Scene launches pin the claimed lanes' overrides** to the launched
  scene's cells (`pin_track_override_to_effective`): masked scene
  save-backs treat latched lanes as stale unless the override pins a
  self-write, so without the pin, session-style editing on a
  stopped-but-latched scene would silently stop persisting.
  (`launch_scene_tracks` already pins.)
- Row-boundary trigger fix (same session): the scheduler clock's
  step-dedup memory resets per lane when the resolved source changes at a
  row boundary (fractional captured boundaries wrapped silenced lanes into
  step 0 and swallowed the re-entry downbeat); take-governance mask clears
  at stop (the latch survives, take governance doesn't — stale masks
  darkened the clip grid and blocked scene claims).

## 11. Non-goals

- No change to the views themselves — the session grid (scene step
  sequencer) and arrangement timeline both remain, and this spec does not
  touch view switching.
- No change to the latch/back-to-song machinery, take capture math,
  splice semantics, or clips-are-explicit.
- No incremental capture commit, no capture edit-lock change
  (docs/realtime-arrangement-feedback-spec.md owns that).
- Phase-2 boundary-launch parity (§6) is explicitly deferred.
