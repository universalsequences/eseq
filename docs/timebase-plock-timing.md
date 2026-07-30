# Timing with timebase p-locks: the stateless clock model

How eseq keeps p-locked timebases, sync waits, polymeter drift, and
unquantized scene changes "in time" — and what that machinery demands from
song mode's anchored clips. This is one of the DAW's most distinctive
mechanisms and also one of its easiest to break by accident, because the
correctness lives in an algebraic identity rather than in any one function.

## 1. What makes this hard

A step's duration is normally `track timebase / num_steps`, but any step can
p-lock its own timebase, and any step can carry a `sync` step-param ("1/4",
"1/2 bar", "1 bar", ...) meaning *wait here until the next grid line, then
continue*. Two consequences:

- A pattern's real loop length is **not** `num_steps x step_beats`. It is
  whatever the per-step durations plus the sync waits add up to — often a
  non-bar length, which is exactly what makes the polymeter drift musical.
- "Where would the playhead be if this pattern had been playing the whole
  time?" has no closed-form answer in uniform step units. Answering it
  wrong is instantly audible: the pattern plays rotated, and sync steps
  land off the transport grid.

Session mode never has to answer that question explicitly. Song mode does.
That asymmetry is the whole story.

## 2. The boundary table: a pattern's real geometry

Every clock in the system reduces a pattern to the same precomputed table
(`scheduler/clock.rs::precompute_boundaries`, mirrored in
`sequencer/clock.rs` and `PatternStepGeometry` in `sequencer/data.rs`):

```text
accum = 0
for each step s:
    if sync(s) > 0:        accum = ceil_to_grid(accum, sync(s))   # the wait
    boundaries[s] = accum                                          # step fires here
    accum += timebase(s).step_beats(num_steps)                     # plocked duration
cycle = sync(0) > 0 ? ceil_to_grid(accum, sync(0)) : accum         # padded loop length
```

Everything is in **beats, local to the pattern cycle**:

- `boundaries[s]` is where step `s` triggers within one loop.
- A sync p-lock on a mid-pattern step creates a *fixed gap* baked into the
  cycle shape (the playhead reads as silent/held inside it —
  `derive_local_step` returns `None` there).
- A sync p-lock on **step 0** pads the whole cycle up to that grid, which is
  the idiom for "this pattern re-syncs to the transport every loop".
  Without it the cycle keeps its raw length and deliberately drifts.

Note what sync is **not**: it is not a stateful "hold until the next global
grid line from wherever we happen to be". The wait is snapped inside the
cycle once, and the cycle repeats with that identical shape. Whether the
snapped boundaries coincide with the *global* grid loop after loop depends
on the cycle length being a multiple of the grid — which the step-0 padding
provides when you ask for it.

## 3. Session mode: stateless projection, nothing to get wrong

The playback clocks (`sequencer/clock.rs::process_block` for the live path,
`scheduler/clock.rs::process_chunk` for lookahead scheduling) keep **no
per-track playhead**. Every audio block they:

1. Rebuild each track's boundary table from the *live* pattern state
   (p-locks included).
2. Derive the playhead as pure arithmetic:
   `pos_in_cycle = total_beats % cycle`, binary-searched into the table.

`total_beats` is the one global transport, started at play and never reset
by anything musical. The per-track state (`last_local_step`) exists only to
deduplicate triggers.

This is why unquantized scene changes are always in time in session mode:
launching pattern 2 just swaps which table the next block builds. The
question "where would the playhead be had we been playing this the whole
time?" is answered *implicitly* by the modular projection — the pattern
joins the transport as if it had been running since beat zero, sync waits,
drift and all. There is no launch-time reconstruction step that could be
wrong, because there is no launch-time state at all. Playback is a pure
function of `(transport position, current pattern geometry)`.

## 4. Song mode: the projection gets an origin

Anchored clips (takes spec 7.1) replace free-run with a per-lane origin:

```text
local = total_beats - anchor_beat + offset_beats(offset_steps)
pos_in_cycle = local % cycle
```

`anchor_beat` is where the clip starts on the timeline; `offset_steps` is
the stored fractional step offset; `offset_beats` converts it back to beats
**through the same boundary table** (`SnapshotSequencerClock::offset_beats`).
The defaults (0, 0) reduce to free-run exactly, which is why session mode is
untouched by any of this.

The algebra that makes a recorded arrangement sound like the performance:

> playback ≡ free-run  ⇔  `offset_beats ≡ anchor_beat (mod cycle)`

i.e. the stamped offset must be *the free-run playhead at the clip start,
measured in the pattern's real geometry*. Session mode has no stamp to get
wrong; song mode bakes one reconstruction into project data and replays it
forever. Every historical bug in this area has been a violation of that
identity.

### 4.1 `PatternStepGeometry`: the one stamping ruler

`sequencer/data.rs::PatternStepGeometry` is the boundary table exposed as a
continuous **bijection** between fractional steps `[0, num_steps)` and cycle
beats `[0, cycle)`:

- `steps_at_beats(b)` — invert the table: which fractional step position is
  the free-run playhead at beat `b`?
- `beats_at_steps(o)` — the runtime's `offset_beats` resolution, exactly.
- Positions inside a sync wait (and the end-of-cycle padding) interpolate
  linearly across the whole inter-boundary span on *both* sides, so any
  beat position round-trips — a launch that lands mid-wait is representable.

Every place a pattern-lane offset is stamped or advanced goes through it:
capture (`app/song_capture.rs::captured_lane_resolution`), the arrangement
compiler (`state/arrangement.rs::advanced_pattern_offset`, via the
`song_track_pattern_geometry` context method), preflight row splits
(`sequencer_state/song_playback.rs`), new-lane phase stamps
(`sequencer_state/accessors.rs`), and the app-side
`SongApp::advanced_offset`. Take lanes are the deliberate exception: a
take's step domain is uniform by definition (takes spec 6.1 — MAX_STEPS
chunks under the first chunk's base timebase).

**Invariant:** `PatternStepGeometry` must stay in lockstep with
`precompute_boundaries` in `scheduler/clock.rs` (same EPS, same
`ceil_to_grid`, same cycle padding) and with `offset_beats`' span
interpolation. The scheduler test
`anchored_playback_reproduces_free_run_with_timebase_and_sync_plocks`
enforces the identity end to end: a free-run clock and an anchored clock
with a geometry-stamped offset must produce identical triggers.

### 4.2 The record clock: which zero to measure from

There are two beat domains during arrangement capture, and the stamp must
use the right one:

- The **arrangement timeline** — where clips live.
- The **record clock** — the scheduler transport, zeroed wherever playback
  started. With a committed song, record starts *at the cursor*
  (`app/song_transport.rs::song_transport_play`), so record-clock zero is
  the cursor beat, not timeline zero. `SongCaptureTake.timeline_start_beat`
  is the translation between them.

A manually launched lane audibly free-runs against the **record clock** (the
manual-override latch clears its anchor). So capture stamps
`steps(timeline_beat - timeline_start_beat)` — the phase the performer
*heard* — not `steps(timeline_beat)`. The two differ by
`timeline_start mod cycle`: invisible for 4-beat patterns recorded from a
bar line (the historical case), a full rotation for p-locked cycles. Tests:
`capture_of_plocked_pattern_stamps_real_geometry_phase_end_to_end` and
`capture_from_cursor_stamps_record_clock_phase_for_plocked_pattern` in
`app/song_transport.rs`.

## 5. Editing semantics that follow

- **Sync is clip-local under anchors, by design.** Takes spec 7.4 locks
  "move clip → content moves rigidly", so a hand-placed clip's sync waits
  travel with the clip rather than re-snapping to the transport. For
  *captured* clips this costs nothing: a correct free-run stamp makes the
  clip origin cycle-aligned, so the clip-local grid **is** the transport
  grid.
- **Painted clips** get `offset 0` — step 0 fires at the clip start
  (spec 7.2). That is a different, intentional semantic from capture.
- **Resize-left / split** re-stamp by advancing the offset along the
  timeline through the real geometry (`stamped_clip_override` →
  `advanced_pattern_offset`), so a trimmed clip keeps playing the identical
  slice.
- **Playback start position cancels out.** In the anchored formula both
  `total_beats` and `anchor_beat` shift by the same amount when you play
  from mid-song, so a committed arrangement sounds the same from any start
  point.
- Offset wraps funnel through `pattern_play_step`
  (`state/arrangement.rs`, clip-edit-target spec 5.1) so future sub-pattern
  loop windows have a single seam; the geometry advance happens *before*
  the window wrap.

## 6. Pitfalls (all found the hard way)

1. **Never stamp or advance a pattern offset with a uniform
   steps-per-beat.** Both the ruler and the wrap modulus are wrong on
   p-locked patterns, and the error compounds per loop. If you need
   beat→step, get a `PatternStepGeometry` and use `steps_at_beats` /
   `advance`.
2. **Mind the clock domain.** Any beat fed into a stamp must be in the
   free-run clock the lane was audibly playing against. Timeline beats are
   only safe when record started at timeline zero.
3. **Swing is orthogonal.** Swing delays trigger *sample times* downstream
   of the boundary table; it never changes the geometry, so it plays no
   part in stamping.
4. **Editing p-locks after capture changes the phase.** A stamped offset is
   a step index resolved through the *live* boundaries; retune a pattern's
   timebase/sync p-locks and existing clips of it will re-resolve
   differently. That is inherent to "offsets are step positions", not a
   bug — but it surprises.
5. **Old stamps are data.** Fixing stamping code does not fix offsets
   already stored in a project; those clips must be re-recorded (or
   re-stamped) to sound right.
