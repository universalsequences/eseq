# Param Print: recording synth/effect knob gestures as p-locks

Rev 1 — 2026-08-31

Extends live step-param printing (bead eseq-jc9, `crates/sequencer/src/ui/step_print.rs`)
from the *step* buffer's Transpose/Velocity/Duration pickers to **all p-lockable
parameter types**: track instrument (synth) params and effect params, with bus
fx / midi-fx / rack-slot / rack-macro params as follow-up targets.

## 1. Motivation

The *step* buffer already records knob movements: while playing with recording
on, touching Transpose/Velocity/Duration prints the value onto steps as the
playhead passes them. Synth and effect params already have a full p-lock
system — per-step overrides stored in `SlotPLockData` — but the only way to
author them is explicit (select steps, turn knob). Param print makes every
p-lockable knob a live performance recorder: hold it while recording and the
gesture is laid into the passing steps as p-locks.

## 2. Semantics (user-ratified)

1. **Hold-to-print.** Print mode arms when the user grabs a param control while
   `playing && recording` with **no step selection active**; it prints for as
   long as the control is held and disarms on release. This matches the
   intentional feel chosen for eseq-jc9 (not latch-until-stop).
2. **The knob is temporarily a recorder.** While printing, the **base value is
   NOT written** — only p-locks are laid onto passing steps. The step under the
   playhead gets its p-lock immediately (p-lock writes are lock-free and
   audio-thread-visible), so the gesture is audible without the base write. On
   release the base value stays where it was before the touch; the gesture
   lives entirely in the p-locks. Steps the playhead did not pass are
   untouched.
3. **Triggered steps only.** Printing never creates triggers; steps without an
   active trigger are skipped (same as step-param print).
4. **Branch ordering.** Selection active ⇒ today's explicit plock-on-selection
   edit, unchanged. Not recording or not playing ⇒ normal base-value edit,
   unchanged. The neural-override diversion (`record_selected_neural_*_plock`)
   keeps its current precedence over the selection branch; print applies only
   when neither is active.
5. **Undo.** Printed p-locks ride the open `RecordingHistoryTransaction`
   (whole-`ProjectScenes` diff → one "Record take" entry per record pass) via
   `mark_recording_take_changed`, exactly like step-param print. They must NOT
   also enter the coalesced device-plock gesture history
   (`apply_coalesced_device_plock_batch`) — that path is for explicit edits.
6. **Multi-param.** Multiple params held simultaneously all print (the latch is
   a keyed list, as today).

## 3. Implementation seams

### Latch generalization (`step_print.rs`)

`StepPrintState.values: Vec<(StepParam, f32)>` becomes keyed by a target enum:

```rust
enum PrintTarget {
    Step(StepParam),                       // existing behavior
    Instrument { param_idx: usize },
    Effect { slot_idx: usize, param_idx: usize },
    // follow-ups: BusEffect, MidiFx, RackSlot*, RackMacro
}
```

Only the `Step` arm uses `publish_engine_override` / `StepPrintOverride` /
`dirty_unpublished_tracks` — that apparatus exists because `step_data` writes
land behind the playhead. P-lock arms do NOT get to skip the snapshot publish,
though (rev 1 wrongly claimed they could): `SlotPLockData`
(`crates/sequencer/src/effects/mod.rs:8100`) is an `AtomicU32` array, but the
scheduler resolves every device p-lock family — instrument, effect, MIDI-FX,
rack — from `EffectSlotSnapshot` deep copies captured at publish time, so an
unpublished print is inaudible until the next unrelated publish or transport
restart. `tick_step_print` therefore coalesces one copy-on-write
`publish_scheduler_track` per tick whenever any track-scoped p-lock printed
(and one bus-runtime publish for bus targets).

The publish alone is still one loop late for the *live* half of the promise:
the scheduler stamps ALL device params per trigger from the snapshot, so
passing steps actively reset a held knob to its stale value. Instrument and
effect targets therefore also ride `DeviceParamPrintOverride`
(`state/core.rs`, published from `publish_engine_override`), the device analog
of `StepPrintOverride`: the scheduler substitutes latched values at trigger
resolution (`resolve_instrument_params` / `resolve_effect_params`), so the
gesture is heard as it is made. Extended targets (bus/MIDI-FX/rack) do not yet
have live-override coverage — their prints are correct but only audible once
the per-tick publish reaches newly scheduled steps. `override_roll_hit`
applies only to the `Step` arm
(roll + param print composes naturally: rolled hits land as triggers, the
p-lock prints onto them on the same pass).

### Gate location: Rust host commands, not Lisp

Unlike the *step* buffer (one Lisp function), param edits fan out from ~6 Lisp
sites (`param-controls.lisp`, `param-grid.lisp`, `custom-ui-runtime.lisp`,
sampler panel, builtins) but funnel into a few host commands. Gate there:

- `"set-instrument-param"` — `ui/host_commands/instrument_params.rs:39`
- `"set-effect-param"` — `ui/host_commands/effects.rs:680` (note: slot resolved
  via `resolve_effect_slot_target` against a node id)
- plus their `-batch` / `-option` siblings.

These handlers already have resolved track/slot/param, the clamped value, and
`ctx.shared` in scope, and the payload is already the normal-edit payload — so
a raced-off gate falls through to the normal edit for free (same trick as
`print-step-param`, `step_history.rs:445-451`). Gate condition:
`state.is_playing() && shared.recording && !has_selection && !neural_override`.

The release edge comes from the existing drag-gesture lifecycle
(`finish_active_gesture` / the `:on-release` paths) — release must clear the
latch entry for that target.

### Write path

Per tick (`tick_step_print`, called from `reactive_tick.rs:258`), for each
p-lock latch entry, walk `(prev_step, playhead]` wrap-aware as `print_pass`
does, and for triggered steps call `EffectSlotState::set_plock` — NOT raw
`plocks.set` — so the `ParamNodeId` stamp is written and the p-lock
self-invalidates if the device is later swapped. Clamp against the descriptor
first (as `AppCommand::Set*PlockMulti` applies do) and run
`sync_effect_mod_active_plock` / `sync_instrument_mod_active_plock`.

### Invalidation — the perf-critical part

Step-param print pushes targeted `StepInvalidation::Param` and never bumps
`ui_epoch`. P-lock prints need `StepInvalidation::PlockPresence`
(`ui/ui_invalidation.rs:123`) plus the expanded-lane display sync
(`sync_instrument_plock_presence_display_fields`, `reactive_sync.rs:1332`).
HARD CONSTRAINT: do not bump `fx_epoch`/`ui_epoch` per printed step per frame —
the perf comment at `instrument_params.rs:667-676` records a ~30ms `*fx*`
rebuild per bump. Coalesce presence invalidations per tick per track and reuse
the targeted-invalidation discipline from eseq-jc9 / step-buffer-edit-perf.

### Display

While a param prints, its knob/picker should show the held value (it does
naturally — the user is holding it). No display re-binding dance is needed
because the base value is untouched; verify the authoring-display sync
(`sync_*_param_authoring_display`) is NOT run for print writes (it would echo
the base value that isn't changing).

## 4. Slices

1. **eseq-prm.1** — `PrintTarget` generalization + track **instrument** param
   printing (gate in `set-instrument-param` handler, plock write path, release
   edge). Includes the plocks-only/no-base-write semantics.
2. **eseq-prm.2** — track **effect** param printing (`set-effect-param` +
   batch/option variants, `resolve_effect_slot_target` handling).
3. **eseq-prm.3** — invalidation/perf pass: coalesced `PlockPresence`
   invalidation, expanded-lane sync, drag-perf validation against the
   `instrument_params.rs:667` constraint.
4. **eseq-prm.4** — extended targets: bus fx, midi-fx, rack-slot
   instrument/effect, rack macros (each has its own plock command family).

## 5. Non-goals (v1)

- Printing onto steps without triggers (never creates triggers).
- Tensor-param printing (`Set*TensorPlock*`) remains excluded from v1. Tensor
  controls edit one cell while their p-lock stores a whole matrix, so they need
  a cell-addressed print target plus an explicit merge policy for simultaneous
  cell gestures; treating them like scalar targets would lose concurrent edits.
- Any change to explicit selection-plock editing or neural overrides.
- Latch-until-stop mode (explicitly rejected in favor of hold-to-print).
