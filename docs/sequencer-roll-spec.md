# Sequencer Roll (Tempest-style beat repeat)

Status: draft
Depends on: process layer (def-process, project layer, `:every` brains) — landed

## What this is

Press-and-hold performance roll, modeled on the DSI Tempest's roll button:

- While **roll** is held, the pattern's own trigs are suppressed and the step
  that was playing when roll was pressed repeats at a selectable rate.
- The rate (**timebase**) is live-jammable while the roll is held
  (1–8 → musical subdivisions).
- Repeats are **full-fidelity clones** of the latched step event: chord,
  chord durations, step p-locks, instrument/effect param locks, MIDI-FX
  params — not a note/velocity approximation.
- Scope is the whole kit: every track that has fired latches its own last
  event and rolls it (per-track latch, one shared control).

v1 control surface is a momentary UI button plus a timebase control. A
global keyboard binding (backquote hold + 1–8) is explicitly a later phase.

## Why the process layer alone can't do it today

Three verified constraints shape the design:

1. **The lisp-visible event is scalars-only.** `process_step_event_value`
   (`scheduler.rs:3758`) builds the event map a `:run` body sees: track,
   step, beat, duration, velocity, speed, transpose, pan, chop, aux-a/b.
   Chord, p-locks, param locks, and MIDI-FX params are not in it, so a lisp
   `:state` latch can never be full-fidelity.

2. **Full fidelity lives Rust-side, and the delivery plumbing exists.**
   `materialize_process_ratchet` (`scheduler.rs:3503`) clones the base
   `ResolvedStep`, re-materializes the complete step event with
   `step_event_with_process_overlay` (`scheduler.rs:3457`), and schedules it
   via `ProcessRuntime::schedule_step_event_at` (`process.rs:2223`) as a
   `ProcessScheduledStepEvent { event: StepEvent, midi_fx_params }`
   (`process.rs:1319`). Due events drain through `take_due_events`
   (`process.rs:2236`) into `enqueue_due_process_emissions`
   (`scheduler.rs:2329`, Step branch at `scheduler.rs:2366`). So "schedule a
   complete step event at an arbitrary future beat" is already a proven
   path — ratchets use it every tick. What's missing is a way to *hold onto*
   such an event across ticks and re-schedule it later.

3. **Self-clocked (`:every`) processes can't issue step commands.** The
   standalone brain path (`process_block` invocation loop,
   `scheduler.rs:~6919`) applies run results via
   `ProcessRuntime::apply_run_result`; `veto!`/`ratchet!` are step-scoped
   commands applied only in `apply_step_process_commands` during a track
   fire. A roll clock therefore cannot ratchet, and lisp `emit`
   (`build_process_emit_event`, `lisp_host.rs:8402`) builds from
   `default_resolved()` + keywords — the low-fidelity path.

Everything else the roll needs already exists:

- **Project layer**: `(processes :project …)` slots run ahead of every
  track's own chain, including tracks added later, with runtime state and
  RNG keyed per `(instance, track)` (`process.rs:417–419`, `:3081`). One
  shared instance = one roll inlet + one timebase inlet, per-track behavior.
- **Free-running clock**: `:every (timebase-beats (in :timebase))` is a
  `ProcessTimeExpr` evaluated against inlets (`process.rs:96`); inlet writes
  re-resolve the interval (`process.rs:1786–1806`), so the rate jams live.
- **Veto**: per-fire, doesn't halt the chain, exactly the suppression
  semantics we need while the roll is held.
- **UI → inlets**: instance calls like `(roll-h :roll 1)` from widget
  callbacks are the established pattern (process-ui-control-demo).

## Design

Two cooperating processes plus two small Rust primitives.

```
                    ┌──────────────────────────────────────────┐
 track fires ─────▶ │ roll-gate  (project layer, per-track)    │
                    │   not rolling → (latch!)                 │
                    │   rolling     → (veto!)                  │
                    └──────────────┬───────────────────────────┘
                                   │ latch! materializes the full step event
                                   ▼
                    ┌──────────────────────────────────────────┐
                    │ ProcessRuntime track-event latch store   │
                    │   HashMap<track, LatchedTrackEvent>      │
                    └──────────────┬───────────────────────────┘
                                   │ emit-latched! clones + schedules
                                   ▼
                    ┌──────────────────────────────────────────┐
 :every timebase ─▶ │ roll-clock (standalone brain)            │
                    │   rolling → (emit-latched! :track t      │
                    │               :quantize interval)        │
                    └──────────────────────────────────────────┘
```

### New Rust primitive 1: `(latch!)` — capture the full event

- New run command `ProcessRunCommand::LatchEvent`, legal only in step-fire
  scope (same rule as `veto!`).
- Handled in `apply_step_process_commands` (`scheduler.rs:3580`), which
  already has `snapshot`, `track`, `step`, `resolved`, and the target
  overlay in hand: materialize with `step_event_with_process_overlay` —
  identical to the ratchet path — and store:

  ```rust
  struct LatchedTrackEvent {
      event: ProcessScheduledStepEvent, // full StepEvent + midi_fx_params
      step_beats: f32,                  // for duration-relative shaping later
      latched_at_beat: f64,
      pattern_epoch: u64,               // invalidation
  }
  ```

  keyed by **track only** in a new `ProcessRuntime` field
  (`track_event_latches: HashMap<usize, LatchedTrackEvent>`). Runtime-global
  (not per-instance) keying is deliberate: the roll clock is a different
  instance than the project-layer gate, and "last thing this track played"
  is a track-level fact. A future second consumer (e.g. a stutter effect)
  reads the same store.

- Latching happens on **every un-rolled fire**, so the store always holds
  the most recent fired event per track. Pressing roll between fires
  retroactively grabs the step you were just on — which is the real Tempest
  feel, since a finger never lands exactly on a trig. Freezing during the
  roll is expressed in lisp (the gate stops calling `latch!` while `roll`
  is held), not hardcoded in Rust.

### New Rust primitive 2: `(emit-latched! …)` — clone and schedule

- New run-scope native, legal in **any** process scope including
  self-clocked brains:

  ```lisp
  (emit-latched! :track n
                 :quantize beats      ; snap to next multiple of this grid
                 :vel-scale s         ; optional, default 1
                 :transpose semis)    ; optional, default 0
  ```

- The native can't touch the latch store directly (natives run in the
  `ScratchControlRuntime`; the store lives in the scheduler-side
  `ProcessRuntime`), so it pushes a command carried by
  `ProcessRunResult.commands`:

  ```rust
  ProcessRunCommand::EmitLatched {
      track: usize,
      quantize_beats: Option<f64>,
      vel_scale: f32,
      transpose: f32,
  }
  ```

- `ProcessRuntime::apply_run_result` — which owns the store — resolves it:
  - No latch for `track`, or stale `pattern_epoch` → **silent no-op**
    (consistent with the unbound-port philosophy: the process still runs,
    other tracks still roll).
  - Otherwise clone `event`, apply `vel_scale`/`transpose` to the resolved
    values, compute the target beat, and `schedule_step_event_at`.

- **Quantized target beat**: `ceil(invocation_beat / q) * q` (with an
  epsilon so a tick landing exactly on the grid schedules *that* grid
  point, not the next). Quantizing in `emit-latched!` rather than trusting
  the clock's phase makes the roll transport-locked regardless of when the
  brain was started or the timebase changed.

- **Idempotent scheduling**: before pushing, replace any pending
  `EmitLatched`-originated event with the same `(source runtime_id, track,
  target beat)`. This makes chunk replays and clock/quantize races safe —
  at most one repeat per track per grid point.

### Lisp layer (ships as a demo script + preset chain)

```lisp
;; Gate: one shared instance on the project layer; state per (instance, track).
(def-process roll-gate
  :doc "While roll is held, veto pattern trigs; otherwise latch every fire."
  :in ((roll :bool :default false))
  :run (if (in :roll)
         (veto!)
         (latch!)))

;; Clock: one standalone brain; interval re-reads the timebase inlet each tick.
(def-process roll-clock
  :doc "While roll is held, re-emit each track's latched event on the grid."
  :in ((roll :bool :default false)
       (timebase :int 1 8 :default 4)
       (num-tracks :int 1 16 :default 16))
  :every (timebase-beats (in :timebase))
  :run (if (in :roll)
         (map (lambda (t)
                (emit-latched! :track t
                               :quantize (timebase-beats (in :timebase))))
              (range 0 (in :num-tracks)))
         nil))

(def roll-gate-h  (roll-gate))
(def roll-clock-h (roll-clock))
(processes :project roll-gate-h)   ; composes ahead of per-track chains
(start roll-clock-h)

;; UI: momentary press/release writes both instances' roll inlet.
(def roll-press ()   (do (roll-gate-h :roll true)  (roll-clock-h :roll true)))
(def roll-release () (do (roll-gate-h :roll false) (roll-clock-h :roll false)))
(def roll-timebase (n) (roll-clock-h :timebase n))
```

Notes:

- `timebase-beats` mapping (already used by the conductor demo) gives the
  1–8 → subdivision table; confirm the mapping covers 1/4 → 1/32 with a
  triplet or two in the middle, Tempest-style. If the current table doesn't,
  extend it rather than inventing a second mapping.
- Emitting only rolls tracks that actually have a latch (silent no-op
  otherwise), so "roll the whole kit" and "roll the one playing track" are
  the same code; a `tracks` list inlet can narrow scope later.
- v1 UI: the mixer/perform panel gets a momentary button. If the generic
  `button` widget only supports `:on-click`, reuse the press/release wiring
  the `macro-momentary` widget already has (`state_values.rs`,
  scene-macro momentary) rather than adding a new widget kind.

### Lifecycle rules

- **Release**: the clock stops emitting on its next tick (it checks
  `(in :roll)`), and at most one already-scheduled repeat (≤ one grid
  interval away) still fires. That trailing repeat is quantized, so it
  sounds intentional; no flush command in v1.
- **Scene / pattern change**: pending process events are already cleared on
  scene change; clear `track_event_latches` at the same seam and stamp
  `pattern_epoch` so a stale latch can never replay an event materialized
  against a previous pattern's instrument/effect slots.
- **Transport stop**: clear pending scheduled repeats (existing behavior for
  pending process events); latches may persist — harmless, they're
  overwritten on the next fire, and epoch checks guard staleness.
- **Timebase change mid-roll**: takes effect on the clock's next tick (the
  `:every` re-resolve seam, `process.rs:1786–1806`) and re-quantizes from
  the new grid. No attempt to reshape already-scheduled repeats (there is at
  most one in flight).
- **Veto ordering**: `roll-gate` sits in the project layer, which runs
  *before* per-track chains — so per-track processes still run (state keeps
  advancing, per the veto-doesn't-halt rule) but the base event dies before
  any of them can ratchet it. This is the desired "pattern is suppressed,
  roll owns the output" behavior.

## Implementation plan

### Phase 1 — latch store + `latch!` (Rust)

1. `process.rs`: add `LatchedTrackEvent`, `track_event_latches` on
   `ProcessRuntime`, `ProcessRunCommand::LatchEvent`, and clear-on-scene /
   epoch plumbing next to where pending events are cleared.
2. `lisp_host.rs`: register `latch!` native (step-fire scope check, mirrors
   `veto!` registration at `lisp_host.rs:5061`).
3. `scheduler.rs`: handle `LatchEvent` in `apply_step_process_commands` via
   `step_event_with_process_overlay`.
4. Tests (scheduler.rs test module, reuse the sparse-process fixture
   pattern around `run_sparse_process_accumulator_fixture`):
   - fire a p-locked/chorded step through a latching project-layer process;
     assert the stored `StepEvent` carries chord + param locks.
   - latch is per-track and overwritten by the newest fire.
   - scene change clears the store.

### Phase 2 — `emit-latched!` (Rust)

1. `process.rs`: add `ProcessRunCommand::EmitLatched { … }`; resolve it in
   `apply_run_result` (clone, shape, quantize, dedupe, schedule).
2. `lisp_host.rs`: register the `emit-latched!` native (any run scope);
   keyword parsing mirrors `emit`.
3. Tests:
   - a `:every` brain + latched track produces scheduled step events on
     exact grid beats; drained events reach the queue with chord/p-locks
     intact (assert against `enqueue_due_process_emissions` output).
   - no latch → no event, no error.
   - replayed chunk / double invocation → exactly one event per grid point.
   - stale `pattern_epoch` → no event.

### Phase 3 — lisp demo + UI (no new Rust)

1. `content/scripts/processes/process-roll-demo.lisp`: the
   `roll-gate` / `roll-clock` pair above + a script tab with a momentary
   roll button and a 1–8 timebase strip (eight small buttons or a
   number-picker), wired per the process-ui-control-demo pattern.
2. Verify by ear + `SEQ.track-events` event-view: hold roll over a sparse
   pattern (empty steps mid-roll is the key case), jam the timebase, release
   on/off the grid.
3. Confirm `timebase-beats` table; extend if needed.

### Phase 4 — polish

- `tracks` scope inlet on both processes (roll only selected tracks).
- Optional `:vel-scale` ramp / accent shaping inlets on the clock (roll
  crescendos — this is where the Tempest pressure-roll feel comes from
  later; the seam is already in `EmitLatched`).
- Decide whether release should flush the in-flight repeat (add a
  `clear-latched-pending!` command only if the trailing repeat bothers us
  in practice).

### Phase 5 — keyboard (explicitly deferred)

Backquote hold + 1–8 as a global performance key. Requirements gathered so
far: the metal backend forwards winit key **Release** events
(`metal_backend.rs:9923–9924`) but the editor key path drops non-Press
events (`editor/mod.rs:4905`), so the hook must intercept before that, with
context gating like `global_sequencer_navigation_available`
(`ui/input.rs`) since backquote is an ordinary typed character. Press →
`(roll-press)`, release → `(roll-release)`, digits while held →
`(roll-timebase n)`. Out of scope until the UI-button version proves the
feel.

## Non-goals (v1)

- Rolling audio (this is an event/sequencer roll, not a beat-repeat DSP
  effect).
- Per-track independent timebases (one shared rate, like the Tempest).
- Reshaping already-scheduled repeats on timebase change (max one in
  flight; not audible).
- Note-repeat from pads (rolling a *held pad* rather than the latched
  sequence step) — related feature, same latch store won't help; different
  spec.

## Open questions

1. Exact `timebase-beats` table for slots 1–8 (straight only, or include
   1/8T + 1/16T like Tempest's 8 positions?). Decide in Phase 3 by ear.
2. Should a roll started while the transport is stopped do anything (Tempest
   rolls free-run)? v1: no — the clock only ticks with the transport.
3. Does `emit-latched!` need `:gate`/probability for stutter-flavored rolls,
   or does that stay in downstream per-track processes? Lean: downstream.
