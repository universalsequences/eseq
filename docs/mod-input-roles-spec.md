# Mod input roles: sequencing a track from its ext mod inputs

Status: rev 1, 2026-10-13. Epic: see `bd show` for the `mod-input-roles` epic.

## 1. What it is

A track's four ext mod inputs (the ports mixer mod cables land on) are
today always **modulators**: the signal reaches every voice modulator's
`INPUT_EXT_BASE..+4` and is available as a modulation source.

Each input gets a **role**, chosen in the *track* buffer under a "mods"
toggle that reveals a four-row table (`ext1..ext4` → dropdown):

| Role | Meaning |
|---|---|
| `modulator` (default) | Current behaviour. Nothing else happens. |
| `trigger` | A rising edge fires a note on this track. |
| `velocity` | Sampled-and-held at each trigger: the note's velocity. |
| `duration` | Sampled-and-held at each trigger: the note's length. |
| `note` | Sampled-and-held at each trigger: the note's transpose. |

A non-modulator input **still feeds the voice modulators**. Its role adds
behaviour and takes nothing away, so one cable can both trigger and
modulate.

Plus one per-track mode next to the table:

| `mod-steps` | Meaning |
|---|---|
| `play` (default) | The track's own steps sound as usual. |
| `latch` | The track's steps do not sound. They only update the latch (§3), so the step sequence becomes a pitch/p-lock memory that the cable plays. |

`mod-steps` only matters while at least one input has the `trigger` role.

## 2. Timing: one engine block of latency

The callback decides a block's note events before the graph renders that
block (`audio/callback.rs`: `dispatch_block_events_until` → `render_chunk`).
The ext signal for block N exists only after block N renders, so a trigger
detected in block N fires in block N+1 **at the same frame offset**.

* Latency is a constant `ENGINE_BLOCK_FRAMES` (512) samples: 10.7 ms at
  48 kHz, 11.6 ms at 44.1 kHz.
* Timing between cable-triggered notes is sample-exact (no jitter); only
  their offset against step-sequenced notes moves by one block.
* Removing the delay would mean allocating voices inside the graph, which
  means rebuilding `fire_resolved` as a node. Rejected.

## 3. What a cable trigger plays: the latch

Each track keeps a **latch**: the `Arc<ScheduledEvent>` of the most recent
scheduler-based trigger (`ResolvedTrigger` or `NetworkTrigger`) dispatched
on that track. A cable trigger replays the latched payload (resolved step,
chord, instrument/sampler/effect params, rack macros/slot params, voice
policy), with `velocity` / `duration` / `note` overriding the
corresponding `ResolvedStep` fields when those roles are assigned.

* Holding the `Arc` costs a refcount bump and no copy. The audio thread
  already drops consumed scheduled events, so replacing a latch is no worse
  than what happens today.
* Cable triggers never update the latch.
* With no latch yet, the trigger plays the track's defaults:
  `generator::default_resolved()`, no chord, no step p-locks, and sampler
  params / voice policy built from the audio thread's
  `scheduler_snapshot` the same way the scheduler builds them.
* The latch is cleared when the transport stops and when the pattern
  epoch changes (scene/pattern switch).
* Only tracks with a `trigger` input keep a latch.

## 4. Value mappings

The mod input clip node already clamps each input to 0..1.

| Role | Mapping |
|---|---|
| `velocity` | `v` (0..1) directly. |
| `duration` | `2^(8v − 4)` steps: 1/16 step at 0, 1 step at 0.5, 16 steps at 1. |
| `note` | `round(48v − 24)` semitones of transpose (0.5 = root, ±2 octaves). Replaces the latched step's transpose. |

## 5. Edge detection

Schmitt trigger per input: a rising edge fires when the signal reaches 0.5
or above after having been below 0.25. That way a smoothed or audio-rate
gate retriggers cleanly and noise near the threshold does not chatter.
Detection state (armed/disarmed) persists across blocks. Several
`trigger` inputs on one track each fire independently.

Cable triggers fire only while the transport is playing (the same as step
triggers). While stopped, the detector keeps tracking edges but emits nothing.

## 6. Engine plumbing

* **Publishing the signal.** The four `mod_in_clip` nodes per track
  (`instruments/track_modulator.rs`) are watched, so they are never
  orphaned. Each gets one extra state cell, `MOD_IN_CLIP_STATE_PUBLISH`:
  `0` = never, `lane + 1` = its `instruments/mod_trigger_lanes` lane. It is
  set from the node's initial state when the track shell is created and
  re-pushed after a track delete shifts indices. Bus clips stay 0. Each
  block the callback arms only tracks with a `trigger` input, and only
  armed clips copy their block into the lane.
* **Reading it.** After `render_chunk`, the callback scans armed tracks'
  lanes, runs edge detection, samples value-role lanes at the edge frame,
  and stores the pending triggers in fixed per-callback storage on
  `AudioCallbackData` (no allocation).
* **Firing.** Next block, before countdown/scheduled-event collection,
  pending triggers become `BlockEventKind::ModTrigger` block events at their
  frame offsets. They are dispatched after scheduled triggers on the same
  frame (priority 3) so a coinciding step updates the latch first, then
  go through `dispatch_scheduled_step` / `dispatch_scheduled_network_step`
  → `fire_resolved`. Voice allocation, mute groups, racks, gate-offs and
  retrig all behave as for any sequenced note.

## 7. Data model and persistence

`ModInputRoles` (four `ModInputRole`s + `ModSteps`) packs into one
`TrackParams::mod_roles: AtomicU32`, so the audio thread reads it with one
load. It travels like `global_transpose`: the track-params snapshot (so it is
per pattern/scene), `SeqParams` in sound entities, the scheduler snapshot,
the project file (`mod_roles`, omitted while default) and the undo witness.
One command, `SetTrackModRoles { track, edit: ModRolesEdit }`, applies a
single role or the steps mode to the roles current when it lands. Lisp:
`(seq-set-track-param :mod-role-N role)` (label or index) and
`(seq-set-track-param :mod-steps "play"|"latch")`; read back as
`track.mod-roles` (list of labels) and `track.mod-steps`.

## 8. Slices

1. Data model + persistence + commands + Lisp setters.
2. Engine: clip publish, detection, latch, `ModTrigger` dispatch, latch-only
   steps. Tests drive a real graph.
3. Track buffer UI: a "mods" ON/OFF button on the settings strip's first row
   opens `mod-roles-panel` (in place of the strip, like the scale editor):
   header with the steps dropdown, then a 2x2 grid of role dropdowns.
4. Later: a `gate` role (note-on at the rising edge, note-off at the falling edge), and
   step-param roles (retrig, probability, p-lock params).
