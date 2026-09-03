# Step Retrig (Machinedrum RTRG / RTIM) Spec

rev 1, 2026-09-02

## Goal

Give every track two always-present, per-step, p-lockable, live-printable
parameters that reproduce the Elektron Machinedrum's **RTRG** (retrig count) and
**RTIM** (retrig time), so a single step can carry a roll, a flam, a buzz, or an
audio-rate pitched burst, with no process attached and no per-instrument work.

Reference behaviour (SPS-1UW MKII manual, E12 / ROM / RAM machines):

- **RTRG** 0..127: "Sets the number of times a sample will retrig. If set to
  127 the sample will retrig infinitely." 0 = single hit. Counts repeats, not
  total hits.
- **RTIM** 0..127: "Defines the time between two retrigs. The time is relative
  to the tempo. If the value is set to zero the RTRG parameter will not affect
  anything." Community mapping of the slow end: 127 = 4/beat (16ths), 109 =
  6/beat, 96 = 8/beat (32nds), 77 = 12/beat. Below that it runs into audio rate
  and "the retrigs will create a pitch".
- The burst is owned by the voice, not the step: with RTRG high it keeps rolling
  past the step until the next trig on that track cuts it.
- Each retrig restarts the sound from the start point with the current envelope.
  Velocity is constant across the burst; dynamics come from decay, slides or LFO
  on volume.
- Both knobs are ordinary machine params: p-locked per step, slid with parameter
  slides, modulated by the track LFO.

## Decisions

1. **Two new `StepParam`s, appended at the end of the enum**: `Retrig` (index
   10) and `RetrigRate` (index 11). Appending is mandatory: discriminants are
   the storage index and the JSON position (`data.rs:581`, `project.rs:3020`).
2. **The burst is scheduled host-side on the audio thread**, reusing the existing
   `Chop` countdown machinery (`audio/events.rs` `schedule_chop_events` ->
   `CountdownEventKind::Chop` -> `dispatch_chop_event` in `audio/fire.rs`). It is
   already sample-accurate (repeats inside a block are pushed as block events at
   exact frame offsets; the remainder rides a countdown), already covers sampler,
   modulator and rack paths, and already cancels on the next fire of the same
   track (`cancel_chops_for_track`), which is exactly the MD "until next trig"
   rule. It is **not** implemented inside `gatepitch` or the voice modulator: the
   modulator only observes gate/trigger, and the sampler node does not read
   gatepitch's trigger stream at all (`node_build.rs:470`: gatepitch feeds the
   modulator only).
3. **No LFO or modulator routing to these params for now.** If it is ever added
   it is a control-rate scheduler-side ramp, not a graph input (a modulator ->
   gatepitch edge would be a cycle).
4. **`Chop` is retired from scheduling**, not from storage. It is hidden
   (`StepParam::VISIBLE` never listed it), sampler-only, 1..8 gate subdivisions,
   and would fight retrig for the same countdown slot. Index 7 stays in the enum
   so old files load; `fire_resolved` stops consulting it. If a project has a
   non-default chop value, migrate on load: `Retrig = chop - 1`,
   `RetrigRate = step subdivision` (one-time, in `project.rs`).

## Parameter definitions

### `Retrig` (label "Retrig", short "rtrg")

| field | value |
| --- | --- |
| type | integer count, stored as f32 |
| range | 0..127 |
| default | 0 |
| increment | 1 |
| display | integer; 127 renders as `inf` |
| meaning | number of **repeats** after the initial hit; 127 = repeat until the next trig on this track |

### `RetrigRate` (label "Rate", short "rtim")

The MD knob is an opaque 0..127 that gets faster as it goes down. We store a
musically meaningful, continuous value instead:

| field | value |
| --- | --- |
| type | retrigs per beat, f32, continuous |
| range | 1.0 .. 1024.0 |
| default | 4.0 (16ths) |
| slider | log scale (bespoke curve like `Duration`, see `data.rs:703`) |
| increment | 1.0 up to 32, then geometric (x1.0595, one semitone) above 32 so the number picker sweeps pitch in semitones |
| display | below 32: `4/b`, `6/b`, `8/b`, with musical suffix at exact detents (`16th`, `16T`, `32nd`, `32T`, `64th`); at or above 32: the resulting Hz at current BPM, e.g. `220Hz` |
| meaning | interval between retrigs = `60 / (bpm * rate)` seconds, recomputed at the moment each repeat is scheduled |

Why per-beat and continuous: tempo relativity is what keeps rolls in time and the
continuous range is what makes "sweep into pitch" one gesture. At 120 BPM, rate
220 = 440 Hz. The pitched zone is the whole point of the feature, so the range
deliberately goes well past rhythmic use.

`Retrig = 0` or `RetrigRate` at minimum with no repeats: nothing scheduled,
identical to today.

### Gate per hit

Each retrig hit gets gate length `min(interval, remaining step duration)`, so
hits butt together like the MD (whose retrigs restart the amp envelope). The
initial hit keeps the full step `duration` unless a repeat is due before it
ends, in which case its gate is shortened to the interval. Custom (dgen) voices
are retriggered on the **same** logical voice via `send_custom_trigger`, which
gatepitch turns into a fresh trigger pulse with the gate held; envelopes keyed
on `(max gate_rising trigger)` restart, one-shots re-excite. Sampler voices go
through the existing chop dispatch (restart from start point, existing retrigger
crossfade keeps it click-free).

### Infinite (127)

`repeats = u32::MAX`. Ends on the next fire of the track (existing cancel),
on stop, on scene/pattern switch (the pattern-epoch check already drops stale
countdowns), or on the track's gate-off if the track is a held-gate (keys /
piano roll) source: a released key must stop the roll.

## Scheduling details (`audio/fire.rs`)

Replace the two `chop` call sites (`fire.rs:324` modulator, `fire.rs:840`
sampler) with one retrig block, executed for every instrument type including
custom:

```
let repeats = if resolved.retrig >= 127 { u32::MAX } else { resolved.retrig as u32 };
let interval = samples_per_beat / resolved.retrig_rate;   // f64 samples
if repeats > 0 {
    schedule_retrig_events(data, track_idx, frame_offset, interval, interval,
                           repeats, step, gate_for_hit, logical_voice);
} else {
    cancel_retrigs_for_track(...);
}
```

`CountdownEventKind::Chop` / `ChopEvent` are renamed `Retrig` / `RetrigEvent`
and gain the custom-voice identity (engine id + logical id) so
`dispatch_retrig_event` can re-fire a dgen voice. The remove-the-`!is_custom`
gate is the one behavioural change to the existing path.

Budget: at rate 1024 and 120 BPM the interval is ~23 samples, ~22 block events
per 512-frame block per rolling track. `SCHEDULED_BLOCK_SCRATCH_CAPACITY`
(`audio/mod.rs:112`) is 4096 plus headroom, so 16 tracks rolling at the top of
the range still fit; document the cap in a test. The countdown loop in
`schedule_countdown_or_block_event` already walks repeats per block.

Slides / live edits: the interval is read from the **resolved step** when the
burst is scheduled. Because a countdown carries `period_samples`, a live change
to `RetrigRate` on a rolling infinite step will not take effect until the next
trig. Acceptable for rev 1; the step-print pre-echo (below) covers the
performance case because it re-fires.

## Storage and serialization

- `NUM_PARAMS` 10 -> 12. **Before bumping**, change `step_values_from_vec`
  (`project.rs:3020`) to compare against literal `9` and `8` instead of
  `NUM_PARAMS - 1` / `NUM_PARAMS - 2`, otherwise every current 10-value file
  falls into the Pan-shift branch and is silently corrupted. Today's 10-value
  rows then fall through to the generic prefix copy, which is correct.
- `PROJECT_FILE_VERSION` 9 -> 10 as an explicit marker.
- `ResolvedStep` gains `retrig` and `retrig_rate`; every struct literal (the
  compiler lists them; ~20 in production, ~20 in tests) gets defaults 0.0 / 4.0.
- `set_resolved_step_param` gets real arms (processes may write both, so a
  process can do the MD "slide RTIM into the downbeat" trick).

## UI

### Step lane panel (`content/ui/sequencer.lisp:1549`)

Tabs become `vel dur tpose pan sync delay rtrg rate`. Both are lane sliders
like pan; `rate` uses the log slider curve. Mode ids 7 and 8; bump
`PROCESS_LANE_MODE_OFFSET` (`ui/state_values/process_and_macros.rs:3`) and
`seqv-process-lane-mode-offset` (`content/ui/seqv-track-params.lisp:73`) to 9
together. All three Rust mode<->param maps (`ui/input.rs:759`,
`ui/state_values/expanded_step.rs:133`, `ui/reactive_sync.rs:376`) and every
`(if (= mode N) ...)` ladder in `seqv-track-params.lisp` get the two modes.
Reactive lists `retrigs` / `track-retrigs` / `retrig-rates` /
`track-retrig-rates` are registered and published alongside `pans`.

### Step inspector (`content/ui/effects/track-panels.lisp:405`)

`Transpose  Velocity  Duration  Retrig  Rate` number pickers. Retrig shows
`inf` at 127. Rate picker steps in the semitone increments above 32 so
dragging it is a pitch sweep.

### Live printing (`ui/step_print.rs`, `docs/param-print-spec.md`)

Both params become printable: add the keywords to `print-step-param` /
`print-step-param-release` (`ui/host_commands/step_history.rs:461,498`), add
readout fields in `fx_step_param_value_field` and `seq-core-state.lisp:100`,
extend the hardcoded velocity/duration/transpose triple in
`restore_cursor_display_fields` and in `publish_engine_override`
(`step_print.rs:148`). The engine pre-echo matters here: while play+record and
the user is dragging Rate, the override must reach `lookahead.rs` so the roll
is heard on the passing step before the loop comes round, and the printed value
lands on the step (existing `print_pass` write path is generic).

## Out of scope (rev 1)

- LFO / modulator routing to retrig params.
- Per-retrig velocity or pitch shaping (the `repeater` process keeps its
  `decay` shape for that; it stays as the "shaped burst" tool).
- Parameter slides (no slide feature exists yet; when it does these two params
  join it like any other).
- Retiring the `Chop` enum slot.

## Test plan

- `data.rs`: round-trip a 10-value and a 9-value and an 8-value row through
  `step_values_from_vec`; assert Pan/Chop/Sync/Delay land where they did before
  the bump.
- `audio/events.rs`: a step with `Retrig = 3`, rate 8/beat at 120 BPM produces
  hits at 0, 3750, 7500, 11250 samples at 48k; `Retrig = 127` keeps producing
  until a second fire on the track cancels it; a custom-engine track fires
  `send_custom_trigger` on the same logical id for every repeat.
- `audio/events.rs`: 16 tracks at rate 1024 do not exceed block scratch
  capacity in one 512-frame block.
- Step print: dragging the Rate picker while play+record prints the value onto
  the passing step and the engine override fires the roll before the loop
  wraps (pattern from `docs/param-print-spec.md` tests).
- Headless project probe: load a v9 project, save, reload, assert step data
  identical.
