# Rolling — core track roll + sequencer roll

Status: draft (rev 1)
Provenance: reverse-engineered from `~/code/visual-sampler/sequencer/src/` (the browser
DAW whose roll feel this spec must reproduce), mapped onto eseq's Rust scheduler.
Supersedes `docs/sequencer-roll-spec.md` (the process-layer `latch!`/`emit-latched!`
design) as the plan of record for rolling. See §10 for why, and what survives from it.

---

## 1. What the feature is

Two coupled performance gestures, gated by one **Roll mode** toggle:

1. **Track rolling** — while Roll mode is on, holding a note key on an armed track
   retriggers that track's sound at the current **roll rate** (1/4 … 1/32, straight or
   triplet), locked to the transport grid. Every audible hit is recorded into the
   pattern exactly where it sounded. Rate keys switch the rate live, mid-hold.

2. **Sequencer rolling** — holding the sequence-roll key while Roll mode is on makes
   the *sequencer itself* roll: each participating track loops a window of its own
   pattern, anchored at the position the roll was triggered, window length = one note
   at the roll rate. A beat-repeat at sequence level. The transport free-runs
   underneath; release resumes playback exactly where the beat "really" is.

---

## 2. The feel invariants (from visual-sampler — non-negotiable)

These are the observed rules of the original implementation. Any eseq design decision
that conflicts with one of these is wrong.

- **F1 — No hit on keydown.** A rolled track never triggers immediately; the first
  hit waits for the next roll-grid boundary. There is no "sound now, then align"
  (the original had such a function, `scheduleNext()`, and deliberately never called it).
- **F2 — Rate is read live, not stored.** Track-roll state is *only* the set of held
  (track, note/slice) pairs. The rate is re-read at every scheduling step, which is why
  mid-roll rate switching is instantaneous and free.
- **F3 — Late-bound cancel.** The decision to fire each hit is deferred until ~20 ms
  before it is due; releasing the key inside that window cancels the imminent hit.
  Audio is still scheduled sample-accurately — only the *decision* is late.
- **F4 — Flat dynamics.** Rolled hits use the track's default velocity/params plus the
  held key's transpose. No per-hit velocity.
- **F5 — Record-as-heard.** When recording, one pattern hit is written per audible
  retrigger, at the roll grid line that produced it. The roll grid *is* the quantize.
  A mid-roll rate switch leaves earlier hits untouched and records later hits on the
  new grid — mixed-grid recordings are expected and correct.
- **F6 — Sequence-roll window anchoring.** On trigger, capture the current playback
  position, snap **down** to a multiple of the window length (window = one roll-rate
  note), wrap into the track's own pattern length, then bump the start **up** onto the
  1/16 grid if it landed off it. Rolls never start on an off-1/32.
- **F7 — Transport-locked window phase.** Position inside the window is the *live*
  transport position modulo the window length — not an independent counter. The roll
  breathes with the global grid.
- **F8 — Transport free-runs underneath.** Nothing pauses, seeks, or rewinds. The roll
  only substitutes *which step is read* at scheduling time. On release, playback
  resumes at the true transport position as if the roll never happened. (The original
  author tried rewind-on-release and abandoned it — the comment is still in the code.)
- **F9 — Rate switch re-anchors.** Changing rate during a sequence roll re-anchors the
  window at the **current** live position (snapped per F6), not at the original press.
  This is what makes rapid rate-key runs walk forward through the bar. Re-pressing the
  *same* fast rate (1/32 or any triplet) re-anchors again (stutter gesture); re-pressing
  the same slow rate is a no-op.
- **F10 — Full fidelity.** A sequence roll plays the window's real pattern content —
  chords, p-locks, param locks, everything — because it replays real steps, not
  approximations of them.

Original quirks we intentionally drop: the 1/32 base-grid bucket machinery (eseq's
clock is sample-accurate in beats; triplet rates need no special base grid), and the
duplicate key/MIDI rate maps offset by one.

---

## 3. Vocabulary and state

### Roll rate

Reuse `Timebase` (`sequencer/data.rs:168`) — it already has the exact vocabulary
including triplets, `step_beats()` and labels. The roll rate is one of:

| Rate key | Rate | grid beats |
|---|---|---|
| 1 | 1/4  (`Quarter`)            | 1.0 |
| 2 | 1/4t (`QuarterTriplet`)     | 2/3 |
| 3 | 1/8  (`Eighth`)             | 0.5 |
| 4 | 1/8t (`EighthTriplet`)      | 1/3 |
| 5 | 1/16 (`Sixteenth`)          | 0.25 |
| 6 | 1/16t (`SixteenthTriplet`)  | 1/6 |
| 7 | 1/32 (`ThirtySecond`)       | 0.125 |
| 8 | 1/32t (`ThirtySecondTriplet`) | 1/12 |

(The original keyboard map was 2–9 and its MIDI map was 1–8; we standardize on 1–8.)

### Shared state (control ⇄ scheduler)

New atomics on `TransportState` (`sequencer/state/core.rs:59`), alongside
`record_quantize` / `metronome_enabled`:

```rust
pub roll_mode: AtomicBool,       // the Roll toggle ("'" key / UI button)
pub roll_rate: AtomicU32,        // Timebase discriminant of the current rate
pub sequence_rolling: AtomicBool // momentary: sequence-roll key held
```

Plus one typed command channel drained in the scheduler worker loop (next to
`drain_live_keyboard_inputs`, `worker.rs:~285`):

```rust
pub enum RollCommand {
    NoteOn  { track: usize, transpose: i32 },
    NoteOff { track: usize, transpose: i32 },
    // rate switches & sequence-roll press/release also arrive as commands (not just
    // atomics) so the scheduler can act on them in order and re-anchor exactly once:
    SetRate { rate: Timebase },
    SequenceRoll { on: bool },
    ClearAll, // roll-mode toggled off, transport stop, panic
}
```

Atomics answer "what is the state right now" (UI display, cheap gates); the channel
answers "when did it change" (ordering for re-anchor and cancel semantics).

### Scheduler-side state

On `SchedulerLookaheadState` (`lookahead.rs:8`), next to `accumulator_states` /
`pending_accum_reset` (which already have the reset lifecycle we need):

```rust
pub struct RollState {
    /// Track rolling: held notes per track. Set semantics — duplicate presses of the
    /// same transpose collapse to one roll voice (F2: this is the ONLY held state).
    held: [SmallSet<i32>; MAX_TRACKS],     // transposes; empty = not rolling
    /// Sequence rolling: per-track window anchor, in beats within the track cycle.
    /// None = not sequence-rolling. f64 because triplet windows are fractional.
    window_start: [Option<f64>; MAX_TRACKS],
    /// Bumped on every NoteOff/ClearAll; stamped onto enqueued roll events so a
    /// (optional, Phase 4) audio-side check can drop stale trailing hits.
    generation: u64,
}
```

Note what is *not* stored: no rate (F2 — read `roll_rate` fresh each pass), no
start-time for track rolls, no independent window counter (F7).

---

## 4. Track rolling — mechanics

### 4.1 Input path

Interception happens at the existing live-keyboard seam, `ui/event_loop.rs:700`,
*before* `editor.handle_key` (which drops Release events — `eseqlisp/editor/mod.rs`).
When `roll_mode` is on and the key routes to the live keyboard
(`should_route_to_live_keyboard`, armed track present):

- **Press** → `RollCommand::NoteOn { track, transpose }` for each armed track, instead
  of the normal `KeyboardTrigger` immediate path (F1: no direct audio-callback send).
  `held_notes` bookkeeping stays as-is so Release routing keeps working.
- **Release** → `RollCommand::NoteOff` + normal note-off to the audio path (so a
  sounding rolled voice's envelope releases).
- Number keys 1–8 (no modifiers, roll mode on) → `RollCommand::SetRate` + store the
  atomic. They do *not* reach the editor while roll mode is on.
- Backquote press/release → `RollCommand::SequenceRoll { on }` (momentary, key-repeat
  deduped). See §5.
- `z`/`x` octave shift keeps working (transpose arrives pre-shifted, as today).

When `roll_mode` is off, nothing changes: `handle_recording_key` behaves exactly as it
does now.

### 4.2 Clocking and emission (scheduler)

The roll grid is the **global transport beat grid** at `roll_rate.step_beats()`:
boundaries at `k * grid_beats` in absolute scheduling beats — the same
transport-locked convention as `launch_deadline` (`quantized_launch.rs:603`).
Implementation options: a per-rate `GridBoundaryClock` (`runtime/grid_clock.rs` — used
by generators/graphs, gives identical sample-accurate boundary math), or direct
`next_grid_boundary` calls inside the lookahead pass. Either way:

Each `schedule_playing_lookahead` chunk, for every roll boundary `b` falling inside
`[chunk_start_beats, chunk_end_beats)`, and every track with non-empty `held`:

1. Build a `ResolvedStep` from the track's defaults (velocity = track default per F4),
   one note per held transpose (a chord if several keys are held).
2. Compute `sample_time` for beat `b` and push via `enqueue_step_event`
   (`enqueue.rs:179`) — the normal `ScheduledEventKind::ResolvedTrigger` path with all
   params resolved, exactly like a pattern step. No new event kinds.
3. If recording, emit a record event (§6).

**Cancel semantics (F3):** `RollCommand`s are drained at the top of every worker
iteration (1 ms cadence), *before* the lookahead pass extends the schedule. A hit is
only enqueued once its boundary enters the lookahead horizon
(`scheduler_block_size * 4` samples — a few tens of ms). So a NoteOff cancels every
hit not yet inside the horizon; at most one trailing hit that was already enqueued can
sound. This reproduces the original's `msUntilStep - 20` late-binding with the horizon
as the cancel window. If the horizon proves audibly too long, Phase 4 adds the
`generation` stamp check in the audio callback pop to drop stale roll events exactly.

**Rate switch mid-hold (F2):** `SetRate` just changes which grid the next chunk scans.
Boundaries already enqueued stay; future ones land on the new grid. Nothing to reshape.

**Layering:** track rolling does **not** suppress the track's pattern playback — the
roll layers on top, like live playing (matches visual-sampler; `scheduleStep` ran both
paths). Mono/choke behavior falls out of the track's existing voice policy. (Contrast
with the old process spec's Tempest-style veto — see §10.)

### 4.3 Roll while stopped

v1: like the original, the roll clock only runs with the transport (boundaries are
transport beats). Roll mode with transport stopped behaves as normal live keys.

---

## 5. Sequencer rolling — mechanics

### 5.1 Trigger: capturing the window

On `SequenceRoll { on: true }` (only meaningful while `roll_mode` is on), for **each
track participating** (v1: every track; per-track opt-out is Phase 4):

```
grid      = roll_rate.step_beats()                  // window length in beats (F6)
pos       = track's current position in its cycle   // from SnapshotTrackClockState
            (anchored_local_beats % cycle_beats)
start     = pos - (pos % grid)                      // snap DOWN to window boundary
start     = start % cycle_beats                     // wrap into this track's pattern
if start is not on the 1/16 grid (multiple of 0.25 beats):
    start = ceil(start / 0.25) * 0.25               // bump UP onto the 1/16 grid (F6)
window_start[track] = Some(start)
```

Per-track wrap matters: tracks have independent cycle lengths (`cycle_beats`,
`clock.rs:199`), so each track loops a window of *its own* pattern — exactly like the
original's `% pattern.stepBuckets.length`. Triplet rates give fractional-beat windows
(e.g. 1/16t → 1/6 beat); everything stays f64, no base grid.

### 5.2 Playback: the position remap

The one line that defines the feature, translated from
`stepNumber = patternRollStepNumber + patternStep % steps`:

```
read_pos = (window_start + (live_pos_in_cycle % grid)) % cycle_beats
```

where `live_pos_in_cycle` is the track's true, still-advancing position (F7, F8).

**Seam:** `SnapshotSequencerClock::process_chunk` (`clock.rs:224`). After computing
`pos_in_cycle` for a track (via `anchored_local_beats`), if `window_start[track]` is
set, substitute `read_pos` before `derive_local_step` (`clock.rs:207`). Everything
downstream — boundaries, step_ends, timebase overrides, Sync steps, swing bucketing,
p-lock resolution, the whole `ResolvedStep` build in the lookahead — runs on the
substituted step untouched. **This is why the core approach gets F10 (full fidelity)
for free:** we replay real steps through the real pipeline; there is no event cloning,
no latch store, no scalars-only ceiling.

Details:

- `last_local_step` machinery already handles the remapped step sequence (a jump into
  the window is just a step change; re-entering the window start each cycle re-fires
  step boundaries as expected).
- Playhead publication (`track_playheads` / `track_playhead_phases`, `clock.rs:319`)
  publishes the **read** position while rolling — the UI playhead visibly loops the
  window, matching the original's bracket visualization. The UI additionally gets
  `SEQ.roll-window` reactive state (start, length, per-track) to draw the bracket.
- Engage is seamless by construction: at the trigger instant,
  `read_pos == pos` (start was snapped down from pos, and `pos % grid` is the
  remainder we cut off — modulo the F6 1/16 bump, which can shift the first window
  by ≤ 1/32). No boundary-split needed to engage.
- Release (`SequenceRoll { on: false }`): clear all `window_start`; next sample reads
  the true position (F8). Also seamless — no queue clear, no epoch bump, no resync.
  Commands are applied between chunks; if sub-chunk exactness ever matters, clamp the
  chunk at the command arrival sample using the quantized-launch chunk-split pattern
  (`lookahead.rs:240-274`, `next_session_chunk`) — same mechanism, reused.

### 5.3 Rate switch mid-roll (F9)

On `SetRate` while `sequence_rolling`: recompute §5.1 for every rolling track using
the **current** live position and the new grid — re-anchor at "here", not at the
original press. Additionally, re-pressing the *same* rate key re-anchors when the rate
is 1/32 or any triplet (stutter gesture); same-key re-press at slower straight rates
is a no-op. (This asymmetry is the original's observed behavior — encode it as
`rate.step_beats() <= 0.125 || rate.is_triplet()`.)

Also run the original's per-tick idempotent correction: each chunk, re-snap
`window_start` down to a multiple of the current grid, so window starts can never
drift off-grid regardless of command ordering.

### 5.4 Interaction with track roll

Sequence rolling requires roll mode but is independent of held note keys: with only
backquote held, every track loops its window (the pattern beat-repeats); held note
keys additionally layer track-roll retriggers on top. Releasing backquote ends the
sequence roll; held note rolls continue.

---

## 6. Recording rolled hits (track roll)

Gate: `recording && is_playing()` (and the track armed), same as live-key recording.

The scheduler knows the exact beat `b` of every hit it emits — no wall-clock latency
compensation needed (unlike `record_beats_at_instant`; we're upstream of the render).
Pattern writes stay on the control/UI thread to avoid racing the UI's pattern
ownership:

1. Scheduler, per emitted hit: derive the track-local `(step, phase)` from `b` using
   the boundary geometry it already has (`SnapshotTrackClockState.boundaries`), and
   send `RollHitRecorded { track, step, delay, transpose, velocity }` on a channel
   back to the control thread. `delay` = phase within the step, so roll grids finer
   than the track's timebase (and triplet offsets) land as sub-step delays.
2. UI thread (drained in the reactive tick alongside other scheduler feedback):
   `toggle_step` if inactive + `ChordData::add_note_with_timing(step, transpose,
   duration, delay)` (`data.rs:1223`) + mirror Transpose/Velocity into `step_data` —
   the same write-back as `ui/input.rs:1600-1621` — then
   `publish_scheduler_snapshot()`.
   When song/take recording is active, route through `take_record_note` instead
   (same fork as `ui/input.rs:1575`).

Playback fidelity of the recording is exact: sub-step `delay` replays through the
per-note chord-delay split (`enqueue.rs:54-111`), which is sample-accurate.

Record-as-heard (F5) means **no additional quantize pass** — `record_quantize` is
ignored for rolled hits; the roll grid already quantized them. Mixed-grid recordings
from mid-roll rate switches are represented as steps with differing delays — no
timebase changes are written.

Double-trigger hazard: once a hit is written and the snapshot republished, the
pattern contains the hit *and* the still-held roll keeps emitting it. v1 rule: batch
`RollHitRecorded` events on the control thread and perform the write-back + publish on
NoteOff (write-on-release). The audible roll stays authoritative while held, and the
pattern takes over seamlessly on release — the written steps sit on the same grid the
roll was emitting on.

## 7. Lifecycle

- **Roll mode off** (toggle): `ClearAll` — clear `held`, clear `window_start`, bump
  `generation`. (Original: toggling ROLL always cleared stuck rolls.)
- **Transport stop**: `ClearAll` (original cleared on stop/restart).
- **Scene/pattern switch, quantized launch install, song row change**: `window_start`
  survives *if* the track still has a pattern — windows re-wrap `% cycle_beats` each
  chunk, so a shorter new pattern is handled by the wrap. `held` survives (you can
  hold a roll across a scene launch). Resync branches that clear the event queue
  (`worker.rs:469-530`) don't need special roll handling: roll events are regenerated
  from state next chunk.
- **Serialization**: none. Rolling is purely performance state.
- **Song/arrangement capture** of roll gestures (the original recorded
  `SequenceRollEvent`s with nested rate changes into the mix): out of scope for v1;
  noted as a Phase 5 follow-on riding the takes capture path.

## 8. UI

- Transport bar: Roll toggle + current rate display (reactive on the atomics), styled
  next to record-quantize/metronome (`transport.lisp:~709`).
- Roll-window bracket on track lanes / step rows while sequence-rolling
  (`SEQ.roll-window` reactive: per-track `(start_beats, len_beats)`).
- v1 control is keyboard-first (toggle key TBD — the original used `'`; eseq may want
  a chord or a UI toggle given editor key pressure) + momentary UI buttons for
  touch/mouse parity (the original's `RollButtons`).

## 9. Implementation phases

1. **Track rolling core** — `RollCommand` channel + `TransportState` atomics + worker
   drain; roll-grid boundary scan in the lookahead; `enqueue_step_event` emission;
   event_loop interception (press/release/rate keys). Audible feature, no recording.
2. **Sequencer rolling** — `RollState.window_start`, §5.1 capture + §5.2 clock remap in
   `process_chunk`, release/re-anchor semantics, playhead publication.
3. **Recording** — `RollHitRecorded` feedback channel, write-on-release batching,
   take-recording fork.
4. **Polish** — per-track participation (the original's Roll Effect:
   always-on / always-off / synced + 0.125–8× per-track rate factor via
   `getFactoredResolution`); `generation`-stamped exact cancel; UI bracket; MIDI pad
   mapping with hold-refcount (the original's `rollingCount`).
5. **Capture** — record roll gestures into song/takes as events.

Tests: drive `schedule_playing_lookahead` directly (fixtures in
`scheduler/tests.rs`) — assert boundary-exact emission per rate incl. triplets;
window remap step sequences across cycle wrap and odd pattern lengths; re-anchor on
rate switch; release-resume position; recorded (step, delay) equals emitted beat.

## 10. Relationship to `docs/sequencer-roll-spec.md`

The process-layer spec designed a different flavor: latch the *last fired step's*
full-fidelity event and re-emit it on a process clock (Tempest-style single-step
repeat, pattern vetoed while held). Its core obstacle — lisp step events are
scalars-only, so full fidelity forced new Rust latch/emit primitives — simply
disappears in this design: sequencer rolling replays real pattern steps through the
real pipeline (§5.2), and track rolling emits fresh live-style events. No
`latch!`/`emit-latched!`, no `ProcessRuntime` latch store, no pending-emission round
trip.

What survives from that spec: the lifecycle analysis (clear-on-scene-change,
transport-stop rules), the quantize-to-grid epsilon convention (shared with
`launch_deadline`), and the observation that hold-keys must be intercepted before the
editor drops Release events. The single-step latch-repeat remains a possible future
*mode* (roll rate applied to a latched step rather than a window) — if built, it
should be a core `RollState` variant, not a process primitive.

The process layer still gets the feature for free at the control level: `roll_mode` /
`roll_rate` / `sequence_rolling` are host-command-settable, so lisp processes and
graph outputs can drive rolls (auto-stutter brains) without any new primitives.
