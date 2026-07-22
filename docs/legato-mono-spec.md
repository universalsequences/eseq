# Mono Legato (Single-Trigger) Spec

## Goal

Add classic mono-synth **legato / single-trigger** behavior as a track option next
to poly / voices: when the track is mono and notes overlap, the new note changes
pitch (and velocity) **without retriggering envelopes**; releasing the newest key
falls back to the previously held note, still without retriggering; releasing the
last key gates off. Today's behavior (every note-on retriggers) stays the default
as **retrig** / multi-trigger mode.

Terminology: this is "single-trigger" mode on vintage monos. Fingered
portamento/glide is the *pitch slew* companion feature — explicitly out of scope
here, but the option enum leaves room for it.

## Why this is nearly plumbing-complete already

- **Pitch/velocity update mid-gate already works.** GatePitch's pitch (out1) and
  velocity (out2) are continuous signals that any `GBE_NOTE_ON` rewrites
  (`effects/gatepitch.rs:117-118`), and voice DSP reads them live each sample. A
  note-on into a sounding voice already bends its pitch — the only things
  forcing a retrigger are (a) the mono allocator's explicit gate-off before the
  next note-on, and (b) GatePitch's unconditional trigger pulse.
- **The `adsr` macro needs no change.** Its retrigger condition is
  `(max gate_rising trigger_sig)` (`lisp_host.rs:2788-2790`). Keep gate high and
  suppress the trigger pulse and every stock envelope sails through untouched.
  Instruments that excite one-shots off `trigger` (drums) correctly don't
  re-excite — that's exactly what legato means.
- **Track options have a well-worn end-to-end groove** (fts scale is the
  template): `TrackParams` atomic → `ProjectTrackParams` → `AppCommand` →
  history witness → TUI row → Metal panel dropdown + reactive.

## What's missing

1. **The setting**: a mono trigger-mode enum on `TrackParams`, serialized,
   undoable, in both UIs.
2. **A legato note-on in the event protocol**: `GBE_NOTE_ON` must be able to say
   "don't pulse trigger, don't require a preceding gate-off".
3. **Allocator policy**: the three mono gate-off-before-note-on sites must skip
   the gate-off and flag the note-on as legato when the voice is still *gated*
   (not merely still ringing in release).
4. **A held-note stack for live input**: `active_keyboard_notes` is a flat
   transpose-keyed set (`audio.rs:423`) with no ordering — "release newest →
   return to previous held note" needs per-track last-note-priority state.

## Locked decisions

- **Option**: `trig` dropdown, values `retrig` (default) | `legato`. Stored as
  `MonoTrigger { Retrig = 0, Legato = 1 }` in an `AtomicU32` — room for a future
  `legato+glide`. The option is only *consulted* when the track plays mono
  (poly OFF, or voices = 1); it's freely editable regardless, like swing on an
  empty track.
- **Protocol**: extend `GBE_NOTE_ON` with `aux[2] = legato` (0/1, `aux_count`
  3) rather than a new event kind — every consumer that only matches on kind
  keeps working. GatePitch is the one consumer that reads it.
- **Fallback is GatePitch's job**: a legato-flagged NOTE_ON arriving while
  GatePitch's own gate is low (previous note already released — allocator raced
  or miscounted) is treated as a normal trigger. The flag means "legato *if
  possible*", so a stale flag can never produce a silent stuck note.
- **Legato transitions only while the gate is open.** A voice in envelope
  release (gate already off) gets a normal retrigger — matching real monos.
- **Same-pitch overlap is still legato** (no retrigger), like hardware.
- **Scope v1**: Custom/dgenlisp tracks only. Sampler tracks ignore the option
  (restarting sample playback ≠ envelope legato; needs its own design). Rack
  tracks defer to a later phase (per-slot polyphony makes "mono" per-slot, see
  the rack caveat in `track-panels.lisp:~448`). Modulator: N/A.
- **Velocity in legato** updates continuously (out2 rewrites), so amp may step
  on each legato note — Ableton "last" style. Acceptable; envelopes don't
  restart, which is the point.

## Plan

### Phase 1 — GatePitch legato event (`effects/gatepitch.rs`)

- Widen the timeline stride from `(frame, kind, pitch, velocity)` to include
  `legato` (`TIMELINE_LEGATO`); bump `TIMELINE_STRIDE` and the state-size math.
  `gatepitch_schedule_event` (`:51-82`) copies `aux[2]` (default 0 when
  `aux_count < 3`).
- In `gatepitch_process` (`:116-123`):

  ```rust
  if kind == GBE_NOTE_ON {
      pitch = ...; velocity = ...;
      let legato = legato_flag > 0.5 && gate > 0.5;  // fallback: gate low ⇒ full trigger
      gate = 1.0;
      if !legato { trigger = 1.0; }
  }
  ```

- Unit test at this layer: schedule NOTE_ON, then legato NOTE_ON mid-gate →
  assert out3 (trigger) pulses exactly once, out1 (pitch) steps, out0 (gate)
  never dips; then legato NOTE_ON after GATE_OFF → asserts a full trigger.

### Phase 2 — a truthful "gate open" bit per voice

The allocator must distinguish *gated* from *ringing*. `stole_active_voice`
(`audio.rs:606-627`) conflates them, and pending-gate-off state lives in the
countdown queue. Add `gate_open: bool` to the custom-pool voice slot:

- Set `true` at every note-on that opens a gate; `false` in
  `dispatch_gate_off_event` (`audio.rs:1666`) and in the live release path
  (`release_active_keyboard_voice`, `:1832`).
- `cancel_gate_off_for_lid` (`:301`) already runs before mono note-ons; the
  legato condition is simply `mode == Legato && slot.gate_open`.

### Phase 3 — sequenced mono path (`audio.rs`)

At the two custom sequenced note-on sites — chord (`:5632-5636`) and single
(`:5838-5842`) — when the track resolves mono and `MonoTrigger::Legato` and the
reused voice's `gate_open`:

- **Skip** `send_custom_note_off`.
- Send the NOTE_ON with `aux[2] = 1.0` via `send_custom_trigger`
  (`:1540` — add a legato arg or a `_legato` variant).
- Still `cancel_gate_off_for_lid` for the old note and
  `schedule_gate_off_event` for the new note's duration (`:5694-5705`,
  `:5899-5910`) — the *new* note owns the gate now.

Sequenced semantics fall out naturally: overlap happens exactly when a step's
gate length reaches past the next note-on (duration/`chord_delays` already
express this), so tied/overlapping steps become legato phrases and gapped steps
retrigger. `free_patch` voices keep today's retrig behavior (they're one-shot by
construction, `:5632`).

### Phase 4 — live input: held-note stack (`audio.rs` keyboard drain, `:6315+`)

New per-track state next to `active_keyboard_notes` (`:423`):

```rust
mono_held: [ArrayVec<HeldMonoNote, N>; MAX_TRACKS]  // (source_transpose, velocity), press order
```

Maintained only while the track plays mono (clear it when poly toggles on, on
all-notes-off/panic, and on transport stop):

- **Note-on, stack empty** → push; normal trigger (today's path, `:6453-6457`).
- **Note-on, stack non-empty** → push; in `Legato` mode skip the gate-off and
  send legato NOTE_ON; in `Retrig` mode today's gate-off + trigger. Either way
  the *sounding* `ActiveKeyboardNote` must be **re-keyed** to the new transpose
  (its `source_transpose` is the lookup key for `take_active_keyboard_note`,
  `:1818`) — update in place rather than store/take pairs.
- **Note-off of the top (sounding) note** → pop; if the stack is non-empty,
  send a legato NOTE_ON back to the new top (last-note priority) at its stored
  velocity and re-key the active entry; if empty, today's release
  (`:6333-6338`).
- **Note-off of a buried note** → just remove it from the stack; it isn't
  sounding, nothing is sent.

This also fixes a latent mono note-off correctness gap: today releasing an
*older* held key transpose-matches its stale `active_keyboard_notes` entry and
can gate off the voice that a newer key is using. In `Retrig` mode the stack is
still consulted for *which* entry a note-off may release (only the top), even
though note-ons keep retriggering.

### Phase 5 — the setting, end-to-end (clone the fts groove)

- `TrackParams.mono_trigger: AtomicU32` + `get/set` accessors
  (`sequencer/data.rs:697+`, next to `polyphonic`/`max_polyphony`).
- `ProjectTrackParams.mono_trigger` with serde default 0 (`project.rs:733+`),
  applied on load in `sequencer/state.rs:4096-4106`, bridged in
  `sequencer/snapshot.rs:124-134`, and added to `TrackParamsSnapshot`
  (`data.rs:968`) so undo captures it.
- `AppCommand::SetTrackMonoTrigger { track, mode }`: apply arm in
  `tui/command.rs` (next to `SetTrackFtsScale`, `:3046`),
  `HistoryPolicy::Record` (`:1001-1016`), and the
  `capture_track_params_witness` match arm (`tui/edit.rs:2537-2545`).
- **TUI**: new `TP_MONO_TRIG` row const (`tui/mod.rs:148-163`), row + dropdown
  handling in `tui/params.rs` (pattern: `TP_FTS` at `:213/:290/:868`), label
  `trig`, values `retrig`/`legato`.
- **Metal UI**: dropdown in the primary params row of
  `ui/effects/track-panels.lisp` (after `scale`, `:470` area), label `trig`,
  `:on-change (seq-set-track-param :mono-trigger v)`-style via a small native
  or the `slice3_numeric_history_command` op route: register the op string in
  `ui/main.rs:1606`'s match (like `"fts"`), seed + sync reactive
  `SEQ.tp-mono-trigger` in `state_values.rs` (seed `~:25327`, per-track sync
  `~:10853`, and the track-0 initial block `~:1274` in `natives.rs`). Hide or
  disable the control for sampler and rack tracks (`SEQ.tp-is-rack` precedent
  in the poly button).

### Phase 6 (later) — rack tracks, glide

- Racks: mono is per-slot (`RackSlotSnapshot::max_polyphony`); legato would be a
  per-slot flag threaded through the rack note-on path (`audio.rs:6348`).
- `legato+glide` enum value: fingered portamento as a pitch slew inside
  GatePitch (slew out1 toward target when the transition was legato) — zero
  instrument changes, one new time param. Separate spec when wanted.

## Edge cases

- **Pattern stop / panic / poly toggle**: clear `mono_held`, force gate-offs as
  today. A stale stack must never survive a transport stop.
- **Voices > 1 with poly OFF**: mono paths key off the same condition they do
  today (`is_polyphonic()` / voice-0 reuse); legato applies exactly when the
  reuse path runs.
- **Legato NOTE_ON racing a just-fired gate-off in the same block**: event
  ordering within a slice is by frame; the GatePitch gate-low fallback (Phase 1)
  makes the worst case a retrigger, never silence.
- **Scene/pattern switch mid-phrase**: sequenced legato state is per-voice
  `gate_open`, which the existing teardown paths already clear via note-offs;
  no extra state to migrate.

## Testing

(Scoped tests only, per repo convention — no package-wide runs.)

- GatePitch unit test (Phase 1) — trigger-pulse counting as above.
- Allocator test: sequenced overlapping steps in mono legato → exactly one
  trigger event and no intervening `GBE_GATE_OFF` on the shared logical id;
  gapped steps → retrigger.
- Held-stack test: on A, on B, off B → sounding pitch returns to A with no
  trigger; off A → gate off. Plus the buried-note-off case (off A while B
  sounds → nothing sent).
- Track-param roundtrip: serialize/load + undo/redo of `mono_trigger`
  (mirror an existing `TrackParamsPatch` test).
- Audible check via the audition harness (`tools/audition/audition.py`) with a
  slow-attack pad: overlapping phrase should swell once, not per-note.
