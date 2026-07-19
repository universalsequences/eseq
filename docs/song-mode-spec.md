# Song Mode and Performance Capture Spec

Status: draft / design (revised 2026-07-18: timeline-readiness pass)
Author: design pass, 2026-07-10
Related: `crates/sequencer/src/sequencer/state.rs`,
`crates/sequencer/src/scheduler.rs`,
`crates/sequencer/ui/transport.lisp`,
`crates/sequencer/docs/undo-redo-system-spec.md`

## 1. Summary

Add a hardware-style **song mode** that plays a linear sequence of scene and
per-track pattern assignments without requiring an arrangement timeline UI.

Song mode has two complementary workflows:

1. A song may be authored declaratively as a sequence of rows.
2. A song may be captured by performing ordinary scene and track-pattern
   launches while the transport is recording.

This is pattern arrangement, not audio arrangement. It deliberately excludes
audio tracks, waveform clips, parameter automation, and graphical timeline
editing.

Although V1 ships no timeline UI, the model is specified so a future
arrangement view is a pure view-plus-gesture layer: rows carry stable
identity (5.2), the state at any beat is a defined query (5.5), every
timeline gesture reduces to a small set of validated editing primitives
(5.6), and each primitive is one undoable project mutation. A timeline
editor added later must not require a data-model migration.

The transport gains a user-facing **Use Arrangement** control:

| Use Arrangement | Record | Play behavior |
|---|---:|---|
| Off | Off | Play the current session state, as today |
| Off | On | Use the existing pattern/note recording behavior |
| On | Off | Play the stored song |
| On | On | Capture a performance of scene and track-pattern launches into a new song take |

Song playback and song capture are separate runtime modes. During song
playback, the song is the launch authority. During song capture, the performer
is the launch authority and the existing committed song is not played.

## 2. Goals

- Arrange existing scenes and per-track patterns into a deterministic linear
  song.
- Preserve the current scene-plus-track-override model instead of introducing
  another kind of pattern container.
- Permit row boundaries at arbitrary musical beat positions.
- Treat unquantized performance as a first-class workflow.
- When launch quantization is enabled, reproduce the actual quantized launch
  boundary exactly.
- Record scene and track-pattern changes from every launch source: UI, key
  binding, MIDI controller, Lisp, or future control surfaces.
- Make capture non-destructive until the take is explicitly committed.
- Keep row transitions sample-aligned and deterministic.
- Serialize songs as part of the project.
- Provide a foundation that a future graphical arrangement view can edit
  without changing the playback model.

## 3. Non-goals for V1

- Audio tracks or audio clips.
- A graphical timeline.
- Parameter, mixer, mute, solo, device, or automation recording.
- Recording edits made to pattern contents during capture.
- Punch-in, range replacement, overdubbing onto an existing song, or comping
  multiple song takes.
- Tempo or time-signature automation.
- Session-launch overrides during ordinary song playback.
- Post-capture timing correction or destructive quantization.
- Arrangement playback from an arbitrary playhead position. V1 playback and
  capture begin at song beat zero.

The data model must not preclude seeking, punch-in, or a graphical timeline,
but those behaviors are not required for the first implementation.

## 4. Terminology

**Session state**
: The currently active scene plus any per-track pattern overrides.

**Launch intent**
: A request to launch a scene or track pattern. The intent may become audible
  immediately or at a later boundary due to launch quantization.

**Audible launch**
: The scheduler-authoritative moment at which a launch takes effect.

**Song row**
: A complete session state beginning at a specified song beat.

**Committed song**
: The song stored in the project and used for normal song playback.

**Capture take**
: A temporary sequence of captured session states. It does not replace the
  committed song until Stop commits it.

## 5. Song data model

The persisted representation is based on absolute row start positions. A row
does not store a duration; its duration is the distance to the next row. The
song stores an explicit end position for the final row.

Illustrative Rust types:

```rust
#[derive(Clone, Serialize, Deserialize)]
pub struct ProjectSong {
    pub rows: Vec<ProjectSongRow>,
    pub end_beat: f64,
    #[serde(default)]
    pub loop_enabled: bool,
    /// Monotonic allocator for SongRowId; never reused within a project.
    pub next_row_id: u64,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SongRowId(pub u64);

#[derive(Clone, Serialize, Deserialize)]
pub struct ProjectSongRow {
    pub id: SongRowId,
    pub start_beat: f64,
    pub scene: usize,
    #[serde(default)]
    pub overrides: Vec<ProjectSongTrackOverride>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ProjectSongTrackOverride {
    pub track: usize,
    pub pattern_id: u64,
}
```

`ProjectFile` gains an optional/defaulted song field so projects without a song
continue to load:

```rust
#[serde(default)]
pub song: Option<ProjectSong>
```

### 5.1 Complete-state invariant

Every row describes a complete session launch state:

- `scene` is the row's base scene.
- `overrides` is the complete set of per-track overrides for that row.
- An override absent from the row is inactive, even if it was active in the
  preceding row.

Rows are not imperative launch-event logs. This prevents state leakage and
makes playback, seeking, serialization, and future editing deterministic.

Example:

```text
row 0: scene 1, track 2 -> pattern 3
row 1: scene 2, no overrides
```

At row 1, track 2 follows scene 2. Pattern 3 does not remain active.

### 5.2 Stable row identity

Rows are identified by `SongRowId`, following the `PatternId(u64)` convention
in `state.rs`. Row order and playback semantics still come from `start_beat`;
the id exists so anything that must survive an edit can refer to a row without
depending on its position:

- Timeline selection, drag, and hover state refer to ids, not indices.
- Undo mementos target ids, satisfying undo invariant H5 (history targets
  stable identities) in `undo-redo-system-spec.md`.
- Observability bindings that report "current row" report the id alongside the
  ordinal so a future editor can highlight the playing row after edits
  reorder it.

Rules:

- Ids are allocated from `next_row_id` and never reused within a project.
- Every mutation that preserves a row's musical meaning preserves its id
  (moving a boundary, editing its state, normalization keeping the earlier of
  two identical adjacent rows).
- Replacing the song wholesale (`def-song`, committing a capture take)
  allocates fresh ids; there is no id correspondence across wholesale
  replacement.

### 5.3 Validation and canonical form

A valid committed song must satisfy all of the following:

- It contains at least one row.
- Its first row starts at beat `0.0`.
- Every beat is finite and non-negative.
- Rows are strictly ordered by `start_beat`.
- `end_beat` is finite and greater than the last row's `start_beat`.
- Every scene exists.
- Every override track exists.
- Every referenced pattern exists in that track's pattern pool.
- A row contains at most one override per track.
- Overrides are stored in ascending track order.
- Adjacent rows may not contain identical resolved launch states; normalization
  removes the redundant later row (the earlier row's id survives).
- Row ids are unique and every id is less than `next_row_id`.

The project loader must reject malformed song data with an actionable error;
it must not silently clamp, reorder, or drop invalid references.

### 5.4 Topology edits

V1 follows the repository's existing index-based scene and track identity.
Topology operations must preserve song validity atomically:

- Deleting a track removes that track's overrides from every row and decrements
  higher track indices.
- Inserting or moving tracks remaps override indices in the same transaction as
  the project topology change.
- Deleting a scene referenced by the song is rejected until the song rows are
  reassigned or the song is cleared.
- Deleting a pattern referenced by the song is rejected with the referencing
  row positions listed in the error.
- Editing a scene cell or referenced pattern changes what future playback of
  that reference produces; song rows do not clone scene or pattern contents.

### 5.5 Derived queries and timeline projection

Two read-only queries are part of the model contract. They are pure functions
of a valid song plus the project it references, and V1 must implement them
even though V1 has no editor, because playback, capture normalization, and
tests already need them and a timeline view is built entirely on top of them.

**State at beat.** `state_at_beat(song, b)` returns the row governing beat
`b`: the row with the greatest `start_beat <= b`, or none when `b >= end_beat`
(loop-normalized first when `loop_enabled`). Because rows are complete states
(5.1), this single query is sufficient for seeking, scrub preview, and
starting playback anywhere; no event replay from beat zero is ever required.

**Track-lane projection.** `project_lanes(song, project)` derives, for each
track, an ordered list of clip spans:

```rust
struct LaneClip {
    row_id: SongRowId,        // row whose state this span comes from
    start_beat: f64,
    end_beat: f64,            // next row's start, or song end
    pattern: Option<PatternId>, // resolved: override, else scene cell, else None
    from_override: bool,      // render hint: override vs scene-provided
}
```

Adjacent spans on one lane that resolve to the same pattern via the same
mechanism may be merged for display, but the merge is a view concern; the
stored model remains rows. The projection is never stored and never edited
directly — a timeline gesture on a lane clip is translated by the view into
row-model primitives (5.6). This keeps the row model the single source of
truth and means the timeline UI carries no persistence of its own beyond
view state (zoom, scroll, selection).

The row-strip presentation (one column of rows, hardware style) and the
track-lane presentation are two projections of the same data; neither is
privileged.

### 5.6 Editing primitives

All song edits — Lisp today, timeline gestures later — reduce to this closed
set of primitives. Each primitive validates against section 5.3, applies
atomically to the committed song in one project mutation, and forms exactly
one undo entry (undo invariant H10: one user gesture is one history entry).
A failed primitive changes nothing and reports why.

```text
song-row-insert(start_beat, scene, overrides) -> SongRowId
song-row-remove(row_id)
song-row-move(row_id, new_start_beat)
song-row-set-state(row_id, scene, overrides)
song-set-end(end_beat)
song-set-loop(bool)
song-replace(rows, end_beat)          ; wholesale: def-song, capture commit
song-clear
```

Semantics chosen for timeline friendliness:

- `song-row-insert` at beat `b` splits the governing span: the new row starts
  at `b` and the previous row's audible span simply ends there. Inserting a
  row whose state equals the governing row's state is rejected (it would be
  normalized away), except that an editor may pre-seed the state and then
  call `song-row-set-state`; hosts that want "split here" as a gesture should
  insert a copy of `state_at_beat(b)` and immediately edit it as one compound
  undo entry.
- `song-row-move` may reorder rows; ordering is re-derived from `start_beat`.
  Moving row zero away from `0.0`, or moving a row to collide exactly with
  another row's `start_beat`, is rejected rather than auto-resolved — the
  view layer decides how to snap or nudge, the model does not guess.
- `song-row-remove` extends the previous row's span; removing the last
  remaining row is `song-clear`.
- Every primitive re-runs normalization; if normalization would delete a row
  the user just explicitly created or edited, the primitive is rejected
  instead, so editor gestures never silently vanish.

Primitives operate on the committed song only. They are rejected during
`SongPlayback` and `ArrangementCapture` in V1 (single launch authority,
section 13); live-editing the playing song is a future extension and must be
built by making these same primitives boundary-safe, not by adding a second
edit path.

## 6. Declarative authoring

The Lisp-facing form should express complete rows using absolute musical
positions:

```lisp
(def-song "bossa-1"
  (at 0
    :scene 0)

  (at 32
    :scene 1
    :patterns ((1 3)))

  (at 47.5
    :scene 2
    :patterns ((1 5)
               (3 2)))

  :end 64)
```

Numeric positions are quarter-note beats, matching the scheduler's musical
beat domain. Convenience expressions such as `(bars 8)` may be added, but the
underlying stored value remains an absolute beat.

The host must validate the complete definition before replacing the committed
song. A failed definition leaves the previous song unchanged. `def-song`
lowers to the `song-replace` primitive (5.6): fresh row ids, one undo entry.

Duration-oriented syntax may be added as sugar later, but absolute `at`
positions are canonical because they represent unquantized capture directly
and map naturally to a future timeline.

## 7. Transport behavior

### 7.1 User-facing state

The transport exposes:

- `Use Arrangement`: persistent project/session preference selecting session
  playback or song behavior.
- `Record`: the existing record control, interpreted according to the table in
  section 1.
- `Play` and `Stop`: existing transport controls.
- A visible mode/status indicator: `SESSION`, `SONG`, or `ARR REC`.
- The current song row while song playback is active.

`Use Arrangement` is not itself Play and does not alter audio immediately.
Changing it while stopped selects what the next Play operation will do.

For V1, changing `Use Arrangement` while playing is rejected with a status
message. This avoids switching launch authority mid-block without a defined
handoff policy.

### 7.2 Session playback

With `Use Arrangement` off, Play retains current behavior. Song state does not
participate in playback.

### 7.3 Song playback

With `Use Arrangement` on and Record off:

1. Play validates and preflights the committed song.
2. The transport begins at song beat zero.
3. Row zero is applied as one atomic launch state.
4. Later rows become audible at their exact stored beat positions.
5. At `end_beat`, playback stops, or returns to beat zero when `loop_enabled`
   is true.

Manual scene or pattern launches are rejected during V1 song playback with a
message directing the user to stop, disable arrangement playback, or enter
arrangement recording. There is only one launch authority at a time.

### 7.4 Song capture

With `Use Arrangement` on and Record armed, Play starts arrangement capture:

1. The existing committed song remains untouched.
2. The transport starts at song beat zero.
3. The current resolved session state is captured as the row at beat zero.
4. Existing scene and track-pattern launch controls remain active.
5. Each audible launch boundary captures the complete resulting session state.
6. Repeated capture of an identical state produces no row.
7. Stop records `end_beat`, normalizes and validates the take, then atomically
   replaces the committed song.
8. Cancel discards the take and preserves the committed song.

Capture does not play the previous committed song underneath the performance.
The musician's session launches are authoritative.

If capture fails validation or resource preparation, it remains an uncommitted
take and the previous song remains intact. An error must explain why it could
not be committed.

### 7.5 Record-control precedence

There are three distinct recording concepts in the application:

- Pattern/note recording.
- Arrangement capture.
- Master WAV recording.

The existing WAV control remains independent. `Use Arrangement + Record`
selects arrangement capture instead of pattern/note recording. The transport
must display `ARR REC`; merely illuminating the generic record icon is not
sufficient feedback.

Master WAV recording may run concurrently with either song playback or song
capture, because it records rendered output and has a separate control.

## 8. Timing and quantization

### 8.1 Authoritative rule

The song recorder stores **when a launch becomes audible**, not when its input
gesture occurs.

```text
input gesture -> launch intent -> scheduler boundary -> audible launch
                                                   \-> captured song row
```

The capture path must observe the central launch application, not UI events.
This makes UI, Lisp, MIDI, and keyboard launches behave identically and keeps
recorded playback aligned with the performance that was heard.

### 8.2 Unquantized launches

When launch quantization is off, a launch becomes audible at the scheduler's
earliest safe sample boundary. Its sample-derived musical beat is stored in the
row without snapping.

The implementation must not round an unquantized row to a beat, step, or bar.
Serialization precision must be sufficient to reproduce the boundary to within
one output sample at the capture sample rate when tempo is unchanged.

### 8.3 Quantized launches

When launch quantization is active, the launch intent is scheduled for the
selected musical grid. Capture stores that scheduled audible beat.

Suggested launch grids:

- Off
- 1/16
- 1/8
- 1/4
- 1/2
- 1 beat
- 1 bar

Song capture has no independent destructive quantizer in V1. To record a
quantized performance, the user enables launch quantization. A later editing
feature may offer non-destructive or explicit post-capture quantization, but it
must not silently alter a captured take.

### 8.4 Tempo

Rows are stored in musical beats and follow the project tempo during playback.
V1 has one project tempo and does not record tempo changes. Unquantized capture
therefore preserves musical position rather than fixed wall-clock time if the
project tempo is changed later.

## 9. Atomic row application

A row transition must not be implemented as a visible sequence of operations:

```text
launch scene -> publish -> launch override A -> publish -> launch override B
```

That would expose intermediate states and could emit triggers from the wrong
pattern set. Introduce one state operation conceptually equivalent to:

```rust
apply_song_row(scene, complete_overrides, effective_beat)
```

It must:

1. Resolve the scene and complete override set.
2. Resolve every effective per-track pattern.
3. Prepare one coherent scheduler snapshot.
4. Make the snapshot effective at the requested scheduler boundary.
5. Update the UI-visible current scene, overrides, and row consistently.
6. Publish one pattern/topology epoch change for the transition.

The audio callback must not acquire scene/project mutexes, clone pattern data,
load assets, allocate row structures, or rebuild instruments.

## 10. Runtime architecture

### 10.1 Preflight

Before song playback starts, build an immutable runtime song:

```rust
struct RuntimeSong {
    rows: Vec<RuntimeSongRow>,
    end_beat: f64,
    loop_enabled: bool,
}

struct RuntimeSongRow {
    id: SongRowId,
    start_beat: f64,
    scene: usize,
    overrides: Vec<(usize, PatternId)>,
    resolved_pattern_ids: Vec<Option<PatternId>>,
    scheduler_snapshot: Arc<SequencerSnapshot>,
}
```

Preflight must:

- Validate all references.
- Resolve the complete state of every row.
- Ensure required sampler assets and compatible instrument resources are
  loaded before Play succeeds.
- Materialize scheduler data outside the audio callback.
- Fail before transport start if a row cannot be prepared.

No disk access or instrument compilation may occur at a row boundary.

Preflight is start-position independent: it prepares every row, and the
resulting `RuntimeSong` is valid for playback beginning at any beat. The
internal start API is `start_song_playback(runtime_song, start_beat)`, with
the initial row found via `state_at_beat` (5.5). V1 transport always passes
`0.0`, but the parameter exists from the first implementation so that seeking
and a timeline playhead-drop later touch only the transport/UI layer, not the
scheduler.

### 10.2 Scheduler ownership

The scheduler owns:

- Current song beat.
- Current row index.
- Detection of row boundaries within lookahead windows.
- The exact rendered-sample position at which a row becomes effective.
- Emission of an `AudibleLaunchApplied` record for capture.

The UI/render loop may display song position but must not poll the clock and
initiate transitions. Polling would make row timing frame-rate dependent.

Song position must nevertheless be cheaply readable at render rate: the
scheduler publishes enough (last rendered sample position plus tempo mapping)
for the UI to derive a smooth fractional `song-position-beats` every frame.
This is what a timeline playhead consumes; it must not require locking
scheduler state.

Row boundaries that fall inside a scheduler block must divide scheduling at
the boundary: events before it use the preceding row, and events at or after it
use the new row. A large callback or lookahead window must not move the
transition to the start or end of the block.

### 10.3 Capture staging

Arrangement capture accumulates lightweight records outside the audio
callback:

```rust
struct CapturedSongState {
    start_beat: f64,
    scene: usize,
    overrides: Vec<(usize, PatternId)>,
}
```

The real-time path publishes a bounded event containing the effective beat and
resolved launch identity. A control-side capture component consolidates events,
copies the complete state, and constructs the staging take.

If the bounded event channel overflows, capture must enter a failed state and
must not commit an incomplete song.

### 10.4 Capture normalization

On Stop:

1. Sort events by authoritative audible beat.
2. Consolidate all changes sharing the same effective boundary into one state.
3. Ensure the first state starts at `0.0`.
4. Remove adjacent identical states.
5. Sort overrides by track and reject duplicates.
6. Set `end_beat` to the authoritative Stop boundary.
7. Validate the canonical song.
8. Replace the committed song via the `song-replace` primitive: one project
   mutation, one undo entry, fresh row ids.

An audible scene launch clears all track overrides before subsequent launches
at that boundary are consolidated. Therefore, if a scene and several track
patterns are launched for the same scheduled beat, the resulting row contains
the new scene plus those final overrides regardless of input-event ordering.

## 11. UI requirements

V1 does not have an arrangement editor. It requires only transport and status
UI:

- A `SESSION / SONG` or equivalent **Use Arrangement** control.
- A clearly visible `ARR REC` state during capture.
- Current row and total row count during song playback.
- Capture Cancel and Stop/commit behavior.
- Actionable empty-song and invalid-song messages.

The control must have finite, nonzero measured geometry inside the visible
transport panel. Add a layout regression test using reactive values for its
active state, following the repository's UI/layout testing requirements.

## 12. Commands and observability

The final naming may follow existing conventions, but V1 needs operations
equivalent to:

```text
seq-use-arrangement(bool)
seq-song-play
seq-song-capture-arm(bool)
seq-song-capture-cancel
seq-song-status
```

plus host commands wrapping every editing primitive from 5.6
(`song-row-insert`, `song-row-remove`, `song-row-move`, `song-row-set-state`,
`song-set-end`, `song-set-loop`, `song-replace`, `song-clear`). Exposing the
primitives as commands from V1 — even with no editor — means the timeline UI
later binds gestures to already-tested, already-undoable commands, and the
primitives get exercised by Lisp and tests in the meantime.

Normal Play/Record controls should call these through the transport state
machine rather than requiring users to invoke separate playback commands.

Expose at least the following state to Lisp/UI bindings:

```text
song-exists
use-arrangement
song-mode                 ; session | song-playback | arrangement-capture
song-current-row          ; ordinal, for hardware-style display
song-current-row-id       ; stable id, for editor highlight (5.2)
song-row-count
song-position-beats       ; smooth, render-rate readable (10.2)
song-end-beat
song-loop-enabled
song-capture-failed
song-capture-error
song-rows                 ; read-only row list (id, start, scene, overrides)
```

`song-rows` plus the lane projection query (5.5) is the complete read surface
a timeline view needs; the view must not reach into project internals.

## 13. State-machine constraints

Only one of these launch-authority modes may be active:

```text
Stopped
SessionPlayback
SongPlayback
ArrangementCapture
```

Required transitions:

```text
Stopped + Play, arrangement off              -> SessionPlayback
Stopped + Play, arrangement on, record off   -> SongPlayback
Stopped + Play, arrangement on, record on    -> ArrangementCapture
SessionPlayback + Stop                       -> Stopped
SongPlayback + Stop/end                      -> Stopped
ArrangementCapture + Stop                    -> validate and commit -> Stopped
ArrangementCapture + Cancel                  -> discard take -> Stopped
```

Invalid transitions must return a clear error and leave the prior state
unchanged. In particular:

- Arrangement capture cannot begin while transport is already playing.
- Arrangement mode cannot be toggled during playback in V1.
- Song playback cannot start without a valid committed song.
- A failed capture cannot be committed by Stop.

## 14. Testing requirements

### 14.1 Data and serialization

- Song round-trips through project serialization.
- Projects without `song` deserialize with no song.
- Invalid beats, ordering, end positions, references, and duplicate overrides
  are rejected.
- Track insertion/deletion/move remaps every override correctly.
- Referenced scene and pattern deletion is rejected.

### 14.2 Row resolution

- A row resolves scene cells plus its complete overrides.
- Overrides do not leak from the previous row.
- A scene launch clears previous overrides.
- Multiple launches at one boundary consolidate into one final row.
- Adjacent identical states normalize to one row.

### 14.3 Deterministic scheduler tests

Extend the extracted scheduler lookahead harness. Tests must prove:

- A boundary inside a processing block routes pre-boundary triggers through the
  old row and post-boundary triggers through the new row.
- An unquantized boundary retains its within-block sample offset.
- Quantized launches are captured at the audible scheduled beat, not the input
  request beat.
- Scene plus track override becomes one atomic scheduler state.
- Song looping reapplies row zero without a stale override or duplicate edge
  trigger.
- Large lookahead windows do not apply rows early.
- Graph/Lisp sequencer and MIDI FX routing continue to use the effective
  pattern and target-track parameters after a row transition.

When implementing scheduler routing, run the repository-mandated deterministic
route regression:

```sh
cargo test -p sequencer scheduler::tests::scheduler_lookahead_routes_lisp_graph_seed_and_propagation_through_midi_fx -- --nocapture
```

### 14.4 Capture tests

- Capture always creates a beat-zero row from the initial session state.
- Immediate launches retain their scheduler-derived beat.
- Quantized launches store their audible boundary.
- Stop commits a valid staging take atomically.
- Cancel and capture failure preserve the previous committed song.
- Capture event overflow prevents commit.
- UI, Lisp, and MIDI launch paths reach the same central capture seam.

### 14.5 UI tests

- Use Arrangement, Song status, and ARR REC indicators have finite, nonzero
  visible rects.
- The arrangement toggle is tested with a `ReactiveRef`, not only a literal.
- The transport mode table in section 1 is covered at the command/state-machine
  level.

### 14.6 Derived queries and editing primitives

- `state_at_beat` returns the correct row at boundaries: exactly on a row
  start, between rows, at `end_beat`, and loop-normalized.
- Lane projection covers the full song per track with no gaps or overlaps,
  resolves override-vs-scene per span, and round-trips: applying
  `song-row-set-state`/`song-row-insert` derived from a projected span edit
  reproduces the intended projection.
- Each editing primitive: success case, validation rejection leaves the song
  unchanged, id stability per 5.2, one undo entry, and undo/redo restores the
  exact prior song including `next_row_id`.
- `song-row-move` reordering rows keeps ids attached to the right states.
- Primitive rejection during `SongPlayback`/`ArrangementCapture`.

## 15. Delivery slices

### Slice A: model and declarative playback

- Project song serialization and validation, including row ids.
- Editing primitives (5.6) as undoable host commands, exercised via Lisp.
- Derived queries: `state_at_beat` and lane projection (5.5).
- Declarative `def-song` authoring.
- Runtime preflight (start-position independent).
- Atomic scheduler row transitions.
- End and loop behavior.
- No capture yet.

### Slice B: transport integration

- Use Arrangement control.
- Context-sensitive Play and Record behavior.
- Song status/current-row bindings.
- Transport layout and state-machine tests.

### Slice C: performance capture

- Central audible-launch observation.
- Non-destructive staging take.
- Unquantized and quantized boundary capture.
- Stop/commit and Cancel/discard.
- Capture normalization and failure handling.

These are implementation slices, not independently shippable partial
semantics. Song mode should be presented as complete only after all three are
integrated and tested.

## 16. Future extensions

The following should build on the same rows rather than replacing them:

- **Graphical arrangement editing.** With 5.2/5.5/5.6 in place this is a view
  layer: render `song-rows` through the lane projection, draw the playhead
  from `song-position-beats`, and translate drag/split/delete/paint gestures
  into the existing primitive commands. It should require no new persistence,
  no new mutation paths, and no scheduler changes.
- **Seeking and starting playback from an arbitrary row or beat.** Already
  plumbed as `start_song_playback(_, start_beat)` (10.1); exposing it is a
  transport/UI change only.
- **Loop region.** Add optional `loop_start_beat`/`loop_end_beat` to
  `ProjectSong` (defaulting to whole-song, preserving `loop_enabled`
  semantics for old projects) rather than inventing a separate loop object.
- Punch-in and range replacement (a capture take spliced via `song-replace`
  generalized to a range).
- Per-track session overrides with a "Back to Arrangement" operation.
- Explicit post-capture quantization.
- Row naming, color, and hardware-style repeat counts (new optional fields on
  `ProjectSongRow`, keyed by the stable id).
- Live-editing the committed song during song playback (making the 5.6
  primitives boundary-safe).
- Tempo and time-signature events.
- Parameter automation lanes.
- MIDI-note clips independent of the step-pattern representation.
- Audio tracks and audio clips.

The central invariant remains: a song row is a complete, deterministic launch
state at an absolute musical position, and every stored transition reflects the
boundary at which the musician actually heard the change.
