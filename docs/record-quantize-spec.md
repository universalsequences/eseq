# Record Quantize (Unquantized Recording) + Metronome Spec

## Goal

Live keyboard recording currently hard-snaps every note to the integer step the
playhead was on at key press. Add a **record quantize** dropdown in the transport,
next to the scene launch quantize dropdown, with an **off** setting that captures
true sub-step timing, plus coarser grids (1/16 = today's behavior, 1/8, 1/4, ...).

Alongside it, add a **metronome toggle** in the same transport strip — without a
click, unquantized recording is much harder to play accurately.

## Why this is cheap: the engine already plays sub-step timing

- Per-note delays exist end-to-end: `ChordData.delays` (`sequencer/data.rs:1038`),
  written via `add_note_with_timing(step, transpose, duration, delay)` where
  `delay` is a 0..1 fraction of the step.
- The scheduler honors them: `explicit_note_delay_beats` (`scheduler.rs:~1453`)
  offsets each chord note by `delay * step_beats`. Steps without chords use
  `StepParam::Delay` the same way.
- The Delay lane is already a visible/editable step param (`StepParam::VISIBLE`),
  so recorded micro-timing is inspectable and hand-tweakable after the fact.

So "unquantized" = record the note at the step it landed in **plus** the
fractional phase within that step as the note's delay. No scheduler changes.

## What's missing

1. **A per-track sub-step phase to capture.** The Metal record path
   (`ui/input.rs handle_recording_key`, ~line 1384) reads only
   `transport.track_playheads[ct]` (integer step). The only phase atomic is
   `transport.playhead_phase`, which is the global 16th-note phase
   (`scheduler.rs:358`) — wrong for tracks with timebase overrides.
2. **The setting itself** (reactive value + host command + shared atomic).
3. **The dropdown** in `ui/transport.lisp`.
4. **Quantize logic at record time** (snap-to-grid for the non-off settings,
   round-to-nearest instead of today's floor).

## Plan

### Phase 1 — per-track playhead phase (engine)

In `TransportState` add `track_playhead_phases: Vec<AtomicU32>` (f32 bits,
0..1 fraction of the current *track* step), sized like `track_playheads`.

Write it in `Scheduler::update_playheads`-equivalent loop in `scheduler.rs`
(~line 320–350) where per-track `boundaries`/`total_beats` are already in hand:
`phase = (cycle_beats - tc.boundaries[step]) / step_beats(step)`, clamped 0..1.
Store every block, not just on step change (the phase moves within a step).

### Phase 1b — true press time (de-blocking + latency compensation)

Reading the phase atomic at press is not "true time": the atomics are written
once per audio callback, so a press reads a value up to one block stale
(~10 ms at 512 frames / 48 kHz ≈ 8% of a 16th at 120 BPM), and they carry the
*render* clock, which runs ahead of what the user hears by the output buffer
latency. Nothing today correlates the audio clock with wall clock. (The TUI's
`record_quantize_thresh` is a hand-tuned fudge for exactly this bias.)

- **Anchor pair**: the audio callback publishes
  `(total_beats_at_block_start, Instant)` each block via a seqlock-style
  triple atomic (write counter + two payload words) — no locking on the RT
  thread.
- **Extrapolation at press**:
  `beats_now = anchor_beats + elapsed_since_anchor * bpm / 60 - latency_beats`.
  Wall-clock interpolation removes the block quantization; `latency_beats`
  removes the render-ahead bias.
- **Per-track resolve**: true phase for track `t` = published
  `(step, phase)` advanced by `(beats_now - anchor_beats) / step_beats(t)`,
  carrying overflow into following steps modulo `num_steps` — correct under
  timebase overrides.
- **Record latency offset**: a tunable setting, default = output buffer
  duration (cpal won't reliably report device latency). Same idea as
  Ableton's driver error compensation; also absorbs OS/terminal input
  latency, which the user tunes by ear. Timestamp at **press** (the note-on
  they hear), never at release.
- Caveat: crossterm key events add a few ms of jitter we can only
  bias-compensate, not remove. Acceptable; the systematic 10–25 ms bias is
  the real target.

### Phase 2 — the setting

- New enum `RecordQuantize { Off, Sixteenth, Eighth, Quarter, Half, Bar }` —
  keep it separate from `LaunchQuantize` because `Off` means the opposite
  ("record exact timing" vs "launch immediately"). Give it
  `from_transport_label` / `transport_label` / `grid_beats` like
  `LaunchQuantize` (`quantized_launch.rs`). A shared home: put it in
  `quantized_launch.rs` or a small `record_quantize.rs`.
- Shared runtime state: `transport.record_quantize: AtomicU8` (default
  `Sixteenth`, i.e. today's behavior) so the input thread reads it locklessly.
  (`record_quantize_thresh` already lives there — precedent.)
- Reactive seed `("record-quantize", "1/16")` in `state_values.rs` next to
  `scene-launch-quantize` (~line 24697).
- Host command `set-record-quantize` in `ui/main.rs`, cloned from the
  `set-scene-launch-quantize` handler (~line 7292): validate label, store the
  atomic, set `SEQ.record-quantize`, run reactive cycle.

### Phase 3 — record-path capture (`ui/input.rs handle_recording_key`)

- `HeldKeyboardNote`: replace `step_at_press: usize` with per-armed-track
  capture, e.g. `positions: Vec<(track, step, phase)>` resolved at press via
  the Phase 1b extrapolation for every armed track
  (today it reads only `current_track`, which is already subtly wrong when
  armed tracks have different lengths/timebases — fix it while here).
- On release, per armed track, resolve `(step, phase)` by the setting:
  - **off**: keep `step`, write note with `delay = phase` via
    `chord_data[track].add_note_with_timing(step, transpose, duration, phase)`
    (or `add_note` + `set_delay` on the new note index). Still mirror
    transpose/velocity/duration into `step_data` as today; do **not** write
    `StepParam::Delay` (it's step-wide — per-note chord delay is the right
    channel, and the scheduler prefers chord delays when a chord exists).
  - **1/16**: round to nearest step — `step + (phase >= 0.5)` — instead of
    today's floor (players press early/late symmetrically). Delay = 0.
  - **coarser grids**: convert grid beats to track steps via the track's
    timebase (`step_beats(num_steps)`), round `step + phase` to the nearest
    grid multiple, wrap modulo `num_steps`. Delay = 0.
- Duration capture stays wall-clock-based as today.

### Phase 4 — transport UI (`ui/transport.lisp`)

- `record-quantize-options '("off" "1/16" "1/8" "1/4" "1/2" "1 bar")`,
  `seq-set-record-quantize` → `host-command "set-record-quantize"`.
- Second dropdown next to `transport-scene-launch-quantize` inside the LED
  panel h-stack (~line 622), same styling, `:debug-name
  "transport-record-quantize"`. The LED box is `:width 49` — widen it (or trim
  the clock spacer) to fit a second ~7.2-wide dropdown; verify with the layout
  test. Consider tiny "REC" / "SCN" labels above the two dropdowns since two
  identical quantize dropdowns side by side are ambiguous.

### Phase 5 — metronome (engine + UI)

No metronome exists today; it's a small audio-callback addition.

- **State**: `transport.metronome_enabled: AtomicBool` (default off) in
  `SequencerState`; optional `metronome_gain` later. Reactive seed
  `("metronome", false)` in `state_values.rs`; host command
  `"toggle-metronome"` in `ui/main.rs` flips the atomic and mirrors it into
  `SEQ.metronome`.
- **Click synthesis**: in `audio_callback` (`audio.rs:~6670`), **after**
  `data.master_recorder.capture(output)` so WAV exports stay click-free, and
  only while playing + enabled. The callback already knows
  `block_start_sample`/`block_end_sample` and bpm: quarter-note boundaries are
  `n * samples_per_quarter`; for each boundary inside the block, start a tick.
  Tick = short exponentially-decaying sine burst (~5 ms), ~1.5 kHz on beat,
  ~2 kHz accented on the bar downbeat (every 4 quarters). Keep a tiny
  oscillator/envelope state struct in `AudioCallbackData` so ticks span block
  boundaries; mix into both channels at modest fixed gain (~-12 dB).
  Note: per-track WAV/resample capture paths are upstream of this point too —
  verify nothing else taps `output` after the mix-in besides the peak meters
  (metering the click is acceptable, or compute peaks before the click).
- **UI**: a toggle in the transport near the two quantize dropdowns, styled
  like the existing "WAV" label-button in `transport.lisp` (~line 600):
  `(label "CLK")` (or a metronome glyph) with `:color (if SEQ.metronome
  :white :gray)`, `:on-click` → `(host-command "toggle-metronome")`.

### Phase 6 — tests

- `state_values.rs`: clone
  `metal_seq_transport_scene_quantize_dropdown_is_visible_and_routes_launch_mode`
  (~line 30239) for the new dropdown/command/reactive round-trip; assert the
  scene dropdown still fits (both inside the transport rect).
- `ui/input.rs live_keyboard_tests`: unit tests for the resolve logic —
  off preserves phase as chord delay; 1/16 rounds to nearest; 1/4 snaps across
  steps; wrap at pattern end.
- Scheduler already has chord-delay playback tests; no changes expected there.
- Metronome: layout test that the CLK toggle exists and routes
  `toggle-metronome`; an audio-side unit test that a rendered block with the
  metronome enabled contains nonzero samples at the beat boundary while the
  master recorder's captured buffer does not.

## Open questions / decisions taken

- **Persistence**: `scene-launch-quantize` is session-only today; keep
  record-quantize session-only for parity (persisting both is a separate,
  easy follow-up in project save/load).
- **TUI parity**: the TUI has its own capture path with
  `record_quantize_thresh` (`tui/input.rs:~520`). Out of scope for v1; the
  new atomic makes wiring it later trivial.
- **Default**: `1/16` (today's behavior) so existing muscle memory is
  unchanged; "off" is the new opt-in.
