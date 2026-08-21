# Clip Edit Target — Unified Focus, Double-Click-to-Piano-Roll, Clip Panel

Status: rev 4, 2026-07-29 — **all four slices shipped** on branch
`clip-edit-target` (unmerged), each through a multi-agent review gate with
fixes applied. Slice A: `app/focus.rs` (EditFocus over the sound binding),
`FocusStepGesture`/`begin_for` in `app/edit.rs` (pool-first, take chunks as
one Composite undo entry), `PianoRollLanes` in `ui/piano_roll.rs` (the one
note reader/writer over live/pool/take storage), `pattern_play_step` in
`state/arrangement.rs`, `SEQ.focus-num-steps`/`focus-label`/
`piano-roll-playhead`. Slice B: `handle_double_click` `ItemTitleBar` arm
(opt-in `:double-click-items`, track lanes only), `:double-click-item` →
`seq-open-piano-roll-bottom-for-track`, focus header row. Slice C:
`set_pinned_pattern_num_steps` (pattern-addressed `TrackParamsPatch`,
merge-key coalesced), `arr_clip_slide_offset` band slide, window overlay
(`:window-marker`/`:window-span`/`:window-repeat`). Slice D: clip panel
column inside the `*piano-roll*` buffer (`focus_clip_fields`,
`arr_clip_set_offset`, `focus-clip-resize`/`focus-set-offset`).

Rev 3 adds the arrangement authoring follow-up: double-clicking empty space
in a track lane mints a silent take-backed clip over the widget's default
create span, selects it as the existing sound/edit binding, and opens the
piano roll. Take chunks, the clip, and any required song-end extension are
one undo entry. This deliberately uses a real take rather than weakening the
arrangement invariant that silence is the absence of a clip.

Rev 4 makes the lower panel an explicit Ableton-style mode while arrangement
is visible. Track-header double-click always enters FX/device mode. Clip-title
or empty-space double-click enters arrangement piano-roll mode (the latter
first creates a silent take clip). While that piano roll is open, a single
click anywhere on a clip retargets it and an empty-space click keeps the mode
open with a measured “No clip selected” state. In FX mode clip bodies remain
cursor/region surfaces, and body double-click never changes modes.

Implementation notes vs rev 1 (code won where they differed):

- §3.1's two-arm resolution is implemented as the FULL 16.3 binding
  (including rule 2, playback) — locked decision 1 ("the edit target IS the
  sound binding, no parallel state") takes precedence, so in follow mode
  during song playback the editors track what the row is sounding, like the
  device panel.
- The clip-shaped surfaces (window overlay, clip panel fields, band slide)
  key off the ACTIVE clip selection, not the resolved focus: a pinned clip
  whose pattern happens to be the effective one resolves `Live` for the
  WRITE path (capture rule) but is still a pinned clip for display. A new
  `SEQ.focus-clip-kind` gates those; `SEQ.focus-kind` routes writes.
- §4.1's widget action is emitted only when the instance opts in
  (`:double-click-items`), because the scene lane also has a title bar and
  an unconditional emission consumed its double-click-then-drag gesture.
- Band-slide is hit-tested against the drawn loop-band rows (spec 5.1
  "band-body"), not the whole header — the ruler keeps scrubbing.
- The pinned loop-bar resize re-preflights a playing song once per drag
  (at the gesture seal), not per frame.

Related: `docs/takes-and-additive-arrangement-recording-spec.md` (§16 sound
binding — this spec **extends** it), `docs/arrangement-lane-model-spec.md`
(clip model), `docs/arrangement-region-editing-spec.md` (§clip anatomy;
its take-paste-clones decision at spec line ~348 anticipated this feature),
`docs/realtime-arrangement-feedback-spec.md` (slice 2 — this spec provides
its safe write path),
`crates/sequencer/src/app/sound_binding.rs`,
`crates/sequencer/src/ui/piano_roll.rs`,
`crates/sequencer/src/app/edit.rs` (`StepGestureTransaction`),
`crates/sequencer/src/sequencer/state/sequencer_state/step_edit.rs`,
`crates/sequencer/src/sequencer/state/takes.rs`,
`content/ui/piano-roll.lisp`,
`content/ui/arrangement.lisp`,
`crates/eseqlisp/src/widget_render/timeline.rs`

## 1. Summary

One concept, one sentence: **whatever clip is focused is what the lower
panel shows — its notes in the piano roll, its sound in the instrument/fx
panel — what the monitor plays, what a punch-in records, and what every
editor edits.**

Today the app has three overlapping "what am I pointed at" notions:

1. `SEQ.current-track` atomic — what the piano roll, step grid, and loop
   bar edit (always the *live mirror* of that track's effective pattern).
2. `App::song_clip_selection` (sound binding, takes spec §16) — what the
   device panels show, the monitor plays, and a record clones.
3. `App::song_region_selection` — the copy/paste/delete span.

This spec merges 1 and 2 into a single **focus** (edit target) with a
uniform resolution rule, then builds on it:

- **Slice A** — the focus model: resolution rule, lifecycle, read/write
  plumbing for pattern targets. Session mode behavior is unchanged *by
  construction* (the target is derived, not stored).
- **Slice B** — arrangement double-click on a clip title bar opens the
  piano roll pointed at that clip's source (pattern or take).
- **Slice C** — loop bar retarget + clip window overlay.
- **Slice D** — clip panel: Ableton-style numeric column left of the piano
  roll (Start/End, start offset, length, source identity).

Region selection (3) stays a separate concept — a span is not a pointer to
a sound. Clip-click continues to set both at once.

## 2. Current facts (verified 2026-07-28)

**Piano roll is welded to the live mirror of the current track.** No
pattern id exists anywhere in its path:

- Read: `build_piano_roll_items_value` (`ui/piano_roll.rs:279`) →
  `piano_roll_step_note_entries` (`:169-204`) reads
  `state.pattern.step_data[track]` / `chord_data[track]` /
  `patterns[track]` directly. Track comes from the current-track atomic
  (`ui/event_loop/reactive_tick.rs:177`).
- Write: single lisp entry `seq-piano-roll-action`
  (`piano-roll.lisp:231`, native `ui/natives.rs:3688`) derives track from
  the atomic (`:3692`); mutations land in
  `set_piano_roll_step_note_entries` (`piano_roll.rs:207-251`) on the live
  lanes.
- Loop bar: `:resize-content-length` → `seq-set-track-param :num-steps`
  (`piano-roll.lisp:210-213`) → `AppCommand::SetTrackNumSteps` on the live
  `track_params` (`ui/natives.rs:5238-5252`, `app/command.rs:2958`).
  Applied per drag-frame; `:finish-resize-content-length` is unhandled.
- `SEQ.tp-num-steps` is published for the current track only
  (`ui/state_values/project_state.rs:503-507`).

**But the addressed machinery already exists, unused by the piano roll:**

- `StepGestureTransaction::begin` (`app/edit.rs:2984-3010`) already
  resolves `effective_track_pattern_id(track)` into an explicit
  `TrackPatternId` target and bails if it changes mid-gesture
  (`edit.rs:3029`). The focus model is this derivation, promoted.
- `capture_pattern_step_cells` (`step_edit.rs:409-457`) reads a pattern id
  from the live lanes **when it is effective, else from the pool** — the
  exact mirror-consistency rule targeted writes need. Restore twin at
  `step_edit.rs:1562`; num-steps twins at `:461-479` / `:1529`.
- `TrackTake::chunk_step_at` (`takes.rs:32-41`) maps take-step →
  (chunk, local step); `song_state.rs:485-538` already concatenates chunk
  events onto a continuous step axis for arrangement dots.

**Sound binding (takes spec §16) is the natural host.**
`App::song_clip_selection: Option<SongClipSelection { track, clip_id,
source: BoundSource }>` (`app/mod.rs:949`, `sound_binding.rs:94-99`), set
by `seq-song-select-clip` on clip click (`arrangement.lisp:834-839` →
host command `song-select-clip`). Consumers today: device panel state,
monitor sound, record-clone template (`sound_binding.rs` rules 1-3,
`bound_read_pattern` `:220`). Published as `SEQ.song-bound-clip`.

**Double-click plumbing exists end to end.** Editor synthesizes it (350ms
/ 1.5-cell slop, `editor/widget_interaction.rs:1124-1222`), dispatched
*before* `begin_widget_gesture` so a consumed double-click suppresses the
drag. `TimelineWidget::double_click_event` (`timeline.rs:755`,
`handle_double_click` `:3074-3093`) currently handles only
`HitRegion::Background` (create-item); **every item hit returns `None`**.
Arrangement clips already have an `ItemTitleBar` hit region
(`:title-bar-height 0.9`, `arrangement.lisp:147`) — "the top part of a
clip" is exactly that region.

**Lower panel already unifies the screen area.** `*piano-roll*` and
`*fx*` alternate in the same lower slot
(`main.lisp:554-571`, `seq-apply-lower-panel-layout`). The focus rule
gives that slot one meaning: *the focused clip's editor, whichever face
it's showing.*

**Clip → note data:**

- Pattern clip: `pattern_id` indexes the track's own pool
  (`ProjectScenes::track_pools[track]`). Playback is `mod num_steps`
  (three sites: `arrangement.rs:495`, `song_playback.rs:283`,
  restamp `:559-577`).
- Take clip: `take_id` → `TrackTake { chunks: Vec<PatternId>,
  total_len_steps }` (`takes.rs:19-29`); full-width `MAX_STEPS` chunks,
  exclusive to the take, device state duplicated across chunks and must
  never diverge (`take_chunk_device_state_agrees`, `:180-208`). Silent
  past `total_len_steps`, never wraps.
- `ArrClip` carries **no loop metadata** — only `start_beat` / `end_beat`
  / `offset_steps`. Loops are always `[0, num_steps)` of the pattern.

## 3. The focus model

### 3.1 Resolution rule

```
focus(track) =
    explicit binding for track (song mode, user-set)   -- pinned
    else effective_pattern_id(track)                   -- follows launches
```

- **Session mode:** nothing is stored. The focus *is* the effective
  pattern (track override, else current scene cell), recomputed at use
  time — the step sequencer's behavior is unchanged by construction.
- **Song mode / arrangement view:** clicking a clip pins an explicit
  focus (the existing `song_clip_selection`). It deliberately does NOT
  follow playback — the song can move through scenes while you keep
  editing the pinned clip.
- Follow mode is simply `song_clip_selection == None`.

The focus resolves to a `FocusSource`:

```rust
enum FocusSource {
    Pattern(TrackPatternId),          // session-derived or pattern clip
    Take { track: usize, take: TakeId },  // take clip
}
```

For pattern clips this is `BoundSource` reused; the enum name change is
cosmetic — do not introduce a parallel type if `BoundSource` already
covers both arms.

### 3.2 Unification with sound binding

The edit target **is** `song_clip_selection`. It gains editors as its
fourth and fifth consumers; the invariant extends from
"panel = monitor = record-clone" to:

> **panel = monitor = record-clone = piano roll = step grid = loop bar.**

There must be no second "which clip" state that can disagree with the
binding — that asymmetry class is what produced the scene-switch resync
bug (`track-pattern-switch-paths`).

Interaction summary:

| lower-panel mode | gesture | effect |
|---|---|---|
| FX | single-click clip title | bind clip; keep FX mode |
| FX | single/double-click clip body | park cursor / start region; keep FX mode |
| FX | **double-click clip title** | bind clip + enter arrangement piano-roll mode |
| FX | **double-click empty space** | create and bind a silent take clip + enter arrangement piano-roll mode |
| Piano roll | single-click anywhere on any track's clip | bind/retarget that clip and track; keep piano-roll mode |
| Piano roll | click empty space | unbind and render “No clip selected”; keep piano-roll mode |
| Piano roll | **double-click track header** | enter FX mode for that track |
| Either | leave arrangement / enter session mode | leave arrangement clip mode (§4.2) |

### 3.3 Lifecycle rules

1. **Invalidation.** Clip deleted, source pattern retired, take
   spliced/consolidated/retired → binding clears to follow mode. The
   clip-id-based binding already must handle this for the panel; editors
   inherit the same clearing sites. Never dangle.
2. **Mode transitions.** Leaving the arrangement view or exiting song
   mode always drops the explicit binding. A pinned target silently
   persisting into session mode would make the step grid "mysteriously
   not edit what's playing" — the worst failure mode this feature can
   have. Locked: always clear, no sticky option.
3. **Mid-gesture change.** `StepGestureTransaction`'s existing bail-out
   generalizes: abort the gesture if the *resolved* focus changes under
   it — whether from a scene launch (session, derived target moved) or a
   re-bind/invalidation (song, explicit target moved).
4. **Playhead display.** Editors show a playhead only when the focused
   source is what's actually sounding. For a focused clip during song
   playback, the playhead is clip-relative:
   `(song_pos_steps - clip_start_steps + offset_steps) mod num_steps`
   while the song position is inside the clip span; hidden otherwise.

### 3.4 Write path (the load-bearing part)

All targeted writes go **pool-first with mirror sync**, following
`capture_pattern_step_cells`'s rule:

- If the target pattern id is currently effective for its track → write
  the live lanes (today's behavior) and let `save_effective_track_pattern`
  persistence work as it does now.
- Else → write the pool `TrackPatternData` directly. No live-mirror
  touch, no scheduler interaction.

Concretely:

- `StepGestureTransaction` gains an explicit-target constructor
  (`begin_for(target: FocusSource)`); the existing `begin` becomes
  `begin_for(resolve_focus(track))`.
- `set_piano_roll_step_note_entries` gains a pool-addressed twin (or a
  lane-handle abstraction over live vs pool storage — implementor's
  choice, but one writer, not two diverging copies).
- **Take writes:** map the take-axis step through `chunk_step_at`, write
  the owning chunk's pool pattern. Note edits don't touch device state,
  so `take_chunk_device_state_agrees` is unaffected — assert it in debug
  builds anyway. A note whose duration crosses a chunk boundary stays in
  its start chunk (chunks are `MAX_STEPS` wide; durations already fit —
  verify and clamp).
- Undo: same history commands, with the target id recorded in the
  payload instead of re-derived at undo time (the focus may have moved
  between do and undo).

This write path is exactly what realtime-feedback slice 2 (dropping the
SongPlayback edit lock) needs: pool-first writes don't race the
scheduler's live mirrors. Landing slice A de-risks that spec; edit-through
for the *currently playing* target during song playback remains gated on
that spec's own mirror/splice design and is out of scope here.

### 3.5 Read surfaces

- `build_piano_roll_items_value` / `piano_roll_step_note_entries` gain a
  target-aware variant: pattern target reads pool (or live when
  effective — reuse the capture rule); take target reads chunks
  concatenated onto the take-step axis (adapt `song_state.rs:485-538`).
- `SEQ.tp-num-steps` gets a focus-aware sibling (`SEQ.focus-num-steps`
  or similar): pattern → its `num_steps`; take → `total_len_steps`.
  Don't overload `tp-num-steps` — the step grid still needs the live
  value until it's ported.
- Invalidation: extend `UiInvalidation::PianoRoll` triggers to pool
  writes against the focused pattern and to binding changes.
- New reactive `SEQ.focus-label` for the header/panel: `"Pattern 3 — 4
  clips"` / `"Take 2"` / `"Pattern 5 (scene A)"` (session-derived). The
  clip-use count is a lane scan at publish time, not a new index.

## 4. Slice B — double-click to piano roll

1. **Widget:** `handle_double_click` handles `ItemTitleBar { item }` (and
   `ItemBody`? — no: body is the region surface; title bar only, matching
   "top part of the clip") → emit `(:type :double-click-item :ids (id)
   :time t)`. Item hits other than title bar keep returning `None` so
   body double-click still starts nothing surprising.
2. **Lisp:** `arrangement-track-action` gains a `:double-click-item` arm:
   run the existing `:select` path (bind + region + track select), then
   `seq-open-piano-roll-bottom-for-track i`. Piano-roll targeting comes
   from the binding — the open call stays track-shaped.
3. **Scene lane:** unchanged (background double-click still creates
   scene events). Scene-lane items are not editors' business.
4. The piano roll header shows `SEQ.focus-label` + the source color so
   the pinned state is visible, not inferred.

## 5. Slice C — loop bar retarget + clip window

- In pinned-pattern focus, `:resize-content-length` lowers to a
  pattern-addressed num-steps write (the `capture/restore_pattern_num_steps`
  pair generalizes; effective-pattern targets keep today's
  `SetTrackNumSteps` path so the mirror stays coherent). **Locked: the
  loop bar edits the shared pattern** — every clip referencing it,
  session included. That is the pattern-as-shared-material model; the
  focus label ("— 4 clips") is the required tell. Per-clip length is
  explicitly rejected for now (schema + 3 playback sites + restamp math).
- Take focus: content-length band is read-only (take length is owned by
  recording/splice, resized from the arrangement).
- **Band slide = phase.** Dragging the band body horizontally edits
  `offset_steps` (same lowering as a left-trim restamp at fixed span).
  Rationale: patterns are circular and the loop is the whole pattern, so
  sliding a full-length window by k steps is *audibly identical* to
  shifting phase by k — this is Ableton's grab-the-loop-brace gesture,
  implemented with zero model change. The gesture is *specified* as
  "slide the loop window" (§5.1) so its meaning is unchanged when real
  sub-pattern windows land.
- **Clip window overlay:** when the focus is a pinned clip, draw over the
  header strip: a start marker at `offset_steps` (the source step at the
  clip's left edge) and, if the clip span < one pattern length, the
  played window; if the span > pattern length, a repeat count badge.
  This is most of the value of Ableton's loop-brace visualization and
  needs no model change.

### 5.1 Forward compatibility — sub-pattern loop windows

Loop Position/Length (a loop window smaller than the pattern, slid over
the source) is deferred but **expected eventually** — notably the
"several distinct windows in one pattern's `MAX_STEPS` storage" use.
Three commitments in slices A-D keep the door open at near-zero cost:

1. **Offset is window-relative.** `offset_steps` is defined as the phase
   *within the clip's loop window*; today every window is
   `[0, num_steps)`, so this is observably identical to the current
   definition. When windows land, playback becomes
   `window_start + (offset + delta·steps_per_beat) mod window_len` and
   the existing offset/restamp/split math carries over untouched.
2. **One playback helper.** Slice A funnels the three mod-length sites
   (`advanced_pattern_offset` `arrangement.rs:495`, runtime advance
   `song_playback.rs:283`, `restamped_clip` `arrangement.rs:559-577`)
   through a single `pattern_play_step(offset, delta_steps, window)`
   helper with `window = (0, num_steps)` hardcoded at the call sites.
   Adding real windows is then one function + plumbing, not a
   three-site semantics hunt.
3. **Schema shape reserved.** When it lands, the window is per-clip:
   `ArrClip { loop_window: Option<(start_steps, len_steps)> }`, `None` =
   full pattern — an additive, backward-compatible field. Whether a
   window may address the grey `[num_steps, MAX_STEPS)` storage region
   (the multi-window-per-pattern idea) is decided then; nothing in this
   spec depends on the answer. No `ArrClip` field added now.

Gesture continuity: band-edge drag = resize window (v1: right edge only,
= pattern length), band-body drag = slide window (v1: ≡ phase shift).
Users retrain nothing when windows arrive; the same gestures acquire the
finer meaning.

## 6. Slice D — clip panel

A fixed-width column composed *inside* the `*piano-roll*` buffer, left of
the timeline form (the buffer is currently a single `timeline` widget;
the lower-panel layout node stays a plain single-buffer node). Reuse
`fx-param-row` / `number-picker` patterns
(`effects/param-controls.lisp`, `step-grid.lisp:405-421`).

Fields, v1:

| field | pattern clip | take clip | session (derived focus) |
|---|---|---|---|
| Source | "Pattern N — K clips" | "Take N" | "Pattern N (scene cell)" |
| Start / End (beats) | clip `start_beat`/`end_beat`, editable → `arr_clip_resize` | same (end clamps to playable end) | hidden |
| Start offset (steps) | `offset_steps`, signed display, editable → left-trim-style restamp at fixed span | same (clamps ≥ 0) | hidden |
| Length | pattern `num_steps`, editable (= loop bar) | `total_len_steps`, read-only | `num_steps`, editable |
| Loop | "on" (static) | "off" (static) | "on" |

- **Signed start offset:** displaying `offset_steps` near `num_steps` as
  a negative value (e.g. offset 15/16 shown as −1) matches the Ableton
  pickup-bar mental model. Since our loop region is always the whole
  pattern, Ableton's `start = −1, loop at 0` is *identical* to
  `offset = L−1` — the pickup workflow costs nothing. Display rule:
  offsets in the top half of the pattern render negative, editable
  either way.
- **Loop row is informational** in v1 — it states the pattern/take
  duality rather than toggling it. Converting a clip between
  pattern-source and take-source is `arr_clip_set_source` territory and a
  separate feature.
- **Loop Position/Length (sub-pattern windows) deferred, not rejected** —
  expected down the line; §5.1 reserves the semantics, gesture meanings,
  and schema shape so nothing built here needs unwinding. The panel
  layout leaves room for Position/Length fields next to the Loop row.

## 7. Step grid port (follow-on, in-scope for the model, not slice A-D)

Once focus resolution exists, the step grid in song mode renders and
edits the pinned source via the same read/write surfaces — including
editing a clip that isn't currently sounding. Playhead per §3.3.4. Ship
after the piano roll proves the plumbing; until then the step grid keeps
reading live mirrors (correct in follow mode, which is all session mode
ever is).

## 8. Locked decisions

1. Edit target **is** the sound binding (`song_clip_selection`) — one
   state, five consumers. No parallel edit-target state.
2. Explicit focus always clears on mode/view exit and on source death.
   No sticky pinning into session mode.
3. Loop bar in pinned focus edits the **shared pattern**, clearly
   labeled. No per-clip length.
4. Sub-pattern loop windows deferred but planned-for: offset is
   window-relative, playback mod-math goes through one
   `pattern_play_step` helper, band gestures are specified as
   window-resize / window-slide (§5.1). No `ArrClip` field until the
   feature is actually built.
5. Double-click target region is the clip **title bar** only.
6. Targeted writes are pool-first with the effective-pattern mirror rule
   from `capture_pattern_step_cells`; one writer implementation, not a
   forked copy.
7. Region selection stays a separate concept from focus.

## 9. Open questions

1. Should single-click (which already binds the sound) also retarget an
   *already-open* piano roll, or only double-click? Lean: yes — if the
   piano roll is visible, it follows the binding by definition (§3.2
   invariant); double-click is merely "bind + ensure the panel is open."
2. Take-note edits during song playback of that take: allowed under the
   pool-first path (chunks are pool patterns), or gated behind
   realtime-feedback slice 2 alongside pattern edit-through? Needs a
   look at how chunk expansion snapshots patterns at preflight
   (`song_playback.rs:209-254`) — if rows hold copies, live take edits
   won't sound until re-preflight, which argues for allowing the edit
   (it's still correct, just not yet audible) plus a "changes apply on
   next pass" affordance.
3. `SEQ.focus-label` clip-use count: lane scan per publish is O(clips) —
   fine now; index later if lanes get huge.
4. Does the clip panel also appear for the `*fx*` face of the lower
   panel (source label + start/end only)? Usability says yes eventually;
   out of scope for v1.
