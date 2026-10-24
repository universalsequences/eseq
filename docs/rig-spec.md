# The rig: banks as phrases, parked devices, a channel view

Status: spec rev 3, 2026-10-23. Epic: `eseq-ka5t` (`bd list --label rig`).
§2 lists what is built.

**Built-in UI first.** Everything in this spec ships first in eseq's built-in
UI (sequencer, mixer, fx panel, Instrument Rack and drum rack panels,
browser) and must be fully usable there. The elevated rig view (proto-rig,
§7) is a later experiment that consumes the same host fields and commands,
and may become a product feature down the road; nothing here depends on it.

Rev 3 moves the swap state (which Instrument Rack slots and which kits play)
from patterns to **scenes** (§3). That one change makes swapping per bank
safe without copying patterns, gives kits an on/off per bank, and gives import
a place to land. Rev 3 also adds kit on/off per bank (§6), controlled import
from other projects (§8), and records what the proto-rig prototype settled
for the channel view (§7).

Rev 2 replaced rev 1's instance-first model: the unit that changes per scene
is a **channel's device**, not a sequencer instance. Rev 1's per-scene
instance membership survives, narrowed to project-owned conductors (§10).

## 1. Goal

One project holds a whole set. A **bank** is a phrase: the user makes a new
bank when they find something worth keeping and keeps working in it as if it
were a fresh project. Work in bank C must not disturb A or B, and the screen
must show what bank C uses, not everything every bank has ever used.

The workflow this serves, as the user plays it: jam through scenes to find
what works, then arrange and perform without looking at sequencer detail.

Not a goal: copying Autechre's fixed 16 channels + 4 buses. Parking (§5) is
eseq's own route to the same property: lots of devices loaded, only the
playing ones cost CPU.

Reference project: `.local/projects/gotham-riddim-edit2.json`. Two kits
exported from two projects ("Jungle-ology Kot", 9 pads; "Gotham Piano Kit",
6 pads), each driven by its own rack-owned neural instance, 19 tracks, 40
scenes in banks A and B. The target user story: add bank C, keep the drums,
swap the piano samples for synths, and see only what bank C uses.

## 2. What already exists

- **Slot enable, per pattern.** `RackSlotSnapshot.enabled` (eseq-bw9v): a
  disabled slot gets no triggers, its FX are bypassed, and its engine runs
  zero voices once its release tail ends. It lives in the pattern, which is
  the problem §3 fixes.
- **Add a slot to the current pattern only.** `add-rack-sample-slot` /
  `add-rack-instrument-slot` take `:only-here`, which adds the slot to every
  pattern of the track but enables it only in the current one
  (`park_rack_slot_outside_current_pattern`, test
  `parked_rack_slot_plays_only_in_the_current_pattern`). Uncommitted. §4
  replaces it with the scene-scoped version.
- **Slot add vs replace, today.** Adding a slot (empty row) appends it to
  every pattern of the track, enabled
  (`append_rack_slot_for_all_pattern_snapshots`). Replacing a slot's device
  (filled row or the instrument panel) changes it in the **current pattern
  only** (`replace_layer_rack_slot_source` →
  `replace_rack_slot_source_in_current_pattern`,
  `app/graph/rack_slots.rs`). So one slot row can already hold different
  devices in different patterns (Digi Syn in scene 1's pattern, Digi FM in
  scene 2's), and every scene on the same pattern, in any bank, follows a
  replace.
- **Kits are already on/off per scene.** `Scene.rack_clips` holds one clip
  pointer per kit; a kit with no entry is silent in that scene.
- **Scene banks** (`scene-banks-spec.md`): view-only contiguous spans over
  the scene list, plus `copy-scene-to-new-bank!`. Rev 3 keeps banks
  view-only: "in this bank" is always written onto the bank's scenes.
- **Kit export**: "Export as kit..." in the mixer group menu saves a kit with
  its tracks, clips and sequencers (`rack-clips-and-break-kits-spec.md`).
- **`track.triggered`**: host field lit when a track fires a note this frame,
  for every track, not only kit pads. Uncommitted.
- **proto-rig** (`~/.eseq.d/packages/local/proto-rig/rig.lisp`): the channel
  view prototype, described in §7. It computes "used in the viewed bank" in
  Lisp and warns when a swap would leak into another bank through a shared
  pattern.

Naming, used throughout: **kit** = a drum rack (a group of tracks).
**Slots** = the Instrument Rack on a single track. "Rack" alone is ambiguous in
this codebase today and this spec avoids it.

## 3. Swap state lives on scenes

### 3.1 Why

Slot `enabled` is stored in the track's pattern, and patterns are shared
between scenes, including scenes in different banks. Disabling the piano slot
for bank C therefore also disables it in bank A wherever A plays the same
pattern. Rev 2 answered with forking (copy every shared pattern before the
edit), which multiplies patterns and silently splits note edits that the user
expects to be shared.

Rev 3 separates the two things a pattern carried: **what plays** (notes,
p-locks) stays in the pattern and stays shared; **who plays it** (which slots
are on, which kits are on) moves to the scene. Editing notes in a pattern
used by banks A and C still changes both. Swapping the device in C changes
only C.

A consequence, accepted: two scenes on the same pattern can play it on
different devices. That is the point of the change.

### 3.2 Model

- **Stable slot ids.** `RackSlotSnapshot` gains `id: u64`, unique within the
  track. A slot keeps its id in every pattern of the track; adding a slot
  appends it to every pattern with the same id
  (`append_rack_slot_for_all_pattern_snapshots`). A slot id means one
  device: a slot holds the same instrument in every pattern of the track
  (its params, p-locks and, for a sampler slot, its sample stay per
  pattern). Device identity is the instrument kind: a sampler slot is one
  device whatever sample it holds, because kit pads carry a sample per rack
  clip in the slot snapshot (`rack-clips-and-break-kits-spec.md`) and clip
  launching must keep playing each clip's own sample. Today slots are matched by
  position and a per-pattern replace (§2) can leave different devices in one
  row, so this is new; migration (§3.4) splits such rows.
- **Per-scene slot set.** `Scene` gains
  `slot_enable: Vec<(track_idx, SlotEnable)>`, where `SlotEnable` is the set
  of enabled slot ids. A track with no entry falls back to the pattern's
  `enabled` flags, so legacy projects play unchanged even before migration.
- **Kit on/off** is a separate flag, not the clip pointer. `Scene` gains
  `parked_kits: Vec<group_id>`. A kit plays in a scene when the scene points
  at one of its clips (`rack_clips`) **and** the kit is not in
  `parked_kits`. The clip pointer stays what it is today, the musical choice
  (including "silent in this scene" as no entry); `parked_kits` is the
  resource choice. Parking never touches clip pointers, so turning a kit back
  on restores every scene exactly, intentional silences included.
- **Resolution.** Where the snapshot for a scene is composed, the scene's
  `slot_enable` overrides each slot's `enabled`. Everything downstream (no
  triggers, FX bypass, zero voices) is unchanged.
- **Per-track launches** (launching one track's clip outside a scene change)
  keep the current scene's `slot_enable`. The device set follows the scene,
  not the clip.

### 3.3 New scenes

A scene made by `clone-scene!` or `copy-scene-to-new-bank!` copies
`slot_enable`, `rack_clips` and `parked_kits` from its source, so a new bank starts with the
same devices and diverges from there. A blank scene added with `+` copies
them from the scene before it in the same bank (or, if the bank is empty,
from the last scene of the bank before it), so a new scene in bank C sounds
like bank C.

### 3.4 Migration

On load, for each track with slots:

1. **Split rows by device.** For each slot position, group the patterns by
   the device they hold there (instrument type + engine; sample differences
   are content and never split). The first group keeps the position; each
   other group becomes a new slot appended at the end of the rack (appending
   keeps macro targets, slot-FX records and process targets addressed by
   position valid), fresh id. In practice legacy files cannot express a
   custom-instrument difference within one row (the loader binds every
   pattern to the track's instrument), so legacy migration is ids by
   position plus step 3.
2. **Fill every pattern.** Every pattern gets every slot id. A pattern that
   did not hold a device gets that slot as a copy from a pattern that did,
   disabled.
3. **Write scene state.** Each scene's `slot_enable` is the set of slot ids
   that were enabled and held in its pattern before the split, so it plays
   exactly what it played.

Pattern `enabled` flags are then ignored for scenes with an entry. Project
file version bump; the old fields are still read. Tests: every scene of
gotham resolves the same devices and enabled slots before and after; a
fixture with one row holding two devices across two scenes plays the same
device in each scene after migration.

## 4. Swap: drop an instrument on a channel

The flow, as the user described it:

1. Make a new bank (exists).
2. Drop an instrument on a track.
3. The new device plays in this bank only. Other banks have it as a
   **disabled** slot, so finished banks are never touched.
4. The old device stops playing in this bank.

**One rule: a drop changes only the viewed bank.** Every drop target keeps
the meaning it has today ("add", "use this instead"); what changes is that it
applies to the scenes of the viewed bank and never to other banks. Dropping
an instrument preset counts as dropping its instrument.

**Samples are not devices.** Dropping a sample onto a sampler slot that
already holds a sampler changes that slot's sample in the current pattern,
as today: for a kit pad that is the current rack clip's sample (§3.2). The
bank rule applies when the drop changes the device kind (a sample onto a
synth slot, an instrument onto a sampler slot, or one instrument onto
another).

| Drop target | Today | Rev 3 |
|---|---|---|
| An empty slot row in the Instrument Rack panel | add a slot, enabled everywhere | **add**: add a slot, enabled in the bank only; it exists but is disabled in every other scene |
| A filled slot row in the Instrument Rack panel | replace the device in the current pattern only; every scene on that pattern follows, in any bank | **replace for this bank** (below) |
| The instrument panel of the selected slot | same as a filled slot row | **replace for this bank** (below) |

**Replace for this bank.** What the user sees is a replace in place: the new
device plays here, other banks keep the old one. What is stored depends on
whether another bank uses the old slot:

- **No scene outside the bank enables the old slot**: overwrite it in place
  (in every pattern of the track, since a slot id is one device, §3.2). No
  extra row. Scenes in the bank that enable it all hear the new device;
  today only scenes on the current pattern would.
- **A scene outside the bank enables it**: the old slot cannot be overwritten.
  Insert a new slot **directly after** the old one, enable the new slot in the
  bank and disable the old one in the bank. The old slot keeps playing in
  the other banks. A short notice names them: "Kept Piano for banks A, B".

With "hide disabled slots" on (below), the old row is hidden here and the
rack looks exactly as if the slot had been replaced. With it off, the old row
shows disabled, with the new one directly beneath it, reading as "this
replaced that".

**Rig view (later).** A package view such as proto-rig can add its own drop
targets (a channel box's device pill or name plate) on top of the same
commands; a drop there is a replace for this bank of the track's playing
slot. Not part of the built-in work.

**Replace in all banks.** A slot menu item: overwrite
the slot in every pattern, so every scene that enables it plays the new
device. The only drop-adjacent gesture that touches other banks, and it says
so in its label.

**Layering.** Drop into an empty slot row: the new slot is added and the
slots already playing stay on. There is no modifier-key variant.

**Scope.** The viewed bank. All of the above write `slot_enable` on the
bank's scenes; patterns change only by appending or overwriting a slot (with
the same id in every pattern of the track, §3.2).

Decided 2026-10-23: bank scope, no per-scene variant. The user reports
tripping over today's per-pattern replace regularly (expecting a replace, getting
it in one scene only) and finds device variation within a bank not useful.
Variation inside a bank comes from the patterns, not from the device.

**Plain tracks.** A track whose device is a single instrument converts to an
Instrument Rack with that instrument as slot 1 the first time something is
dropped on it, in every pattern, with slot 1 enabled in every scene. The
display keeps treating a track with one enabled slot as a plain device; the
rack stays out of sight unless opened. This is how the rack becomes the data
model without the user managing racks.

**Hide disabled slots.** The Instrument Rack panel gets a toggle that hides
slots disabled in the current scene, so a track that has collected a dozen
devices over a set shows only what plays here.

**Recall.** "Use this device here too": the track's slot list, each slot
shown with the banks where it is enabled; picking one enables it in the
current scope and disables the others there.

**Undo.** The whole gesture, including the plain-track conversion, is one
recorded edit.

**Kits.** A kit's pad track is an ordinary track, so swap applies to its pads
too: bank C of gotham keeps the piano kit's pads, clips and neural sequencer
and swaps the device on some pads.

**Purge.** A slot enabled in no scene is unused. A purge command lists unused
slots per track and deletes them, as one undo step. Nothing is purged
automatically.

**Tuning.** The piano pads are tuned to A; a synth slot with base note -3 next
to them sounds wrong with fit-to-scale on. Swapping makes mixed tunings
routine, so this is fixed first: eseq-lv9q.

Replaces `:only-here` and `park_rack_slot_outside_current_pattern` (§2),
which scope to a pattern. They are removed once this lands.

## 5. Parking

A device that is loaded but not playing in the current scene costs no CPU, and
coming back is a swap, not a rebuild (compiled code, samples and state stay
loaded).

| Thing | Parked when | Status |
|---|---|---|
| Instrument Rack slot | not enabled in the scene (§3) | built (per pattern); moves to §3 |
| A track's own FX chain | the track is silent in the scene and its tail has decayed | measure, then build |
| A kit's group bus chain | the kit is off in the scene and its tail has decayed | measure, then build |
| A kit's attached sequencers | the kit is off in the scene | measure, then build |

Stage P0 is measurement only: in gotham, with all of one kit silent, profile
what its tracks' chains, its group bus and its neural instance cost. Write the
numbers here before building anything. Also check what a bypassed effect costs
(slot bypass pushes the effect's `enabled` param to 0; whether each effect
skips its DSP when disabled is per effect).

Tails: a chain parks only after its output stays below a threshold for a hold
time, so a reverb or delay rings out across a scene change. Slot FX bypass on
a scene boundary cuts today; decide in P1 whether it waits for the tail too.

### 5.1 P0 measurements (2026-10-23)

**Method.** `bounce::session::park_probe` (ignored release tests, new file
`crates/sequencer/src/bounce/park_probe.rs`) renders a project offline through
the export path: the real graph, the scheduler and every DSP block, with no
device. It edits the project into variants and times each block. The unit is
**ms of render-thread CPU per second of audio**; 10 ms/s is 1% of one core.
The process total is roughly 1.8x that, because the graph's worker thread and
the scheduler add to it. Live, the scheduler runs on its own thread; here it
is inline and timed separately. Each variant ran 3 interleaved rounds and the
table shows the minimum. Rounds agreed within about 5 ms/s when the machine
was quiet. Other agents were building at the time, so treat differences under
about 5 ms/s as noise.

The build was release (optimized + debuginfo) of HEAD `670bd0af9` plus the
probe, on an Apple M1 Max running macOS 26.5.1, at 48 kHz with 512-frame
blocks.

The gotham fixture is `gotham-riddim-edit2.json` with `(import proto-rig.rig)`
removed from the scratch. That package is UI-only and fails in the headless
scheduler. The fixture renders beats 0-32 at 169 bpm (10.4 s timed): scene 40,
kit 1 on clip 62, kit 2 on clip 55. Outputs are in
`.local/benchmarks/park-probe-2026-10-23/`.

How the variants are made:
- **Silent track**: its steps are cleared in every scene pattern. The cell is
  still present but nothing triggers.
- **Kit off**: its rack lane and scene clip pointers are cleared.
- **No seq**: its rack-owned instance is removed from the file.
- **Bypass**: builtin effects are saved with `enabled` = 0.

The synthetic rows use 8 sampler tracks playing a 0.25 s noise burst on every
beat, with one effect per track.

| Thing | Silent / parked candidate | Playing | Method |
|---|---|---|---|
| Empty project (1 track, no FX) | 4.9 | — | synthetic |
| Whole gotham, nothing triggering | 171 | 319 | gotham `all_silent` / `full` |
| Sampler track, no FX (each) | 3.2 | 3.9 | synthetic, (8 tracks − empty) / 7 |
| 4 loose gotham tracks (Heat, 2 Digi Syn, sampler; FX: Compressor, Str8 Delay, Chorus, Filter Table + Space Echo + Slowdown + Compressor) | 39 (≈10 each) | 123 | `loose_silent` / `full` minus `loose_deleted` |
| Kit 1, clip None (9 pads + group bus + neural) | **estimate** 70-80 | 29 above kit-off | `full` − `kit_off`. The silent part is an estimate, see below. |
| Kit's neural instance while the kit is off | ≤ 2.5 per instance (scheduler ~0.4) | — | `kit_off` vs `kit_off_no_seq`: 0.2; `all_silent` vs `all_silent_no_seq`: 4.9 for two |
| Reverb (per instance) | 0 (tail-sleeps) | 1.3 | synthetic |
| Str8 Delay | 0.5 (tail-sleeps) | 2.2 | synthetic |
| Delay | 0.4 | 0.4 | synthetic |
| Compressor | 0.2 | 0.2 | synthetic |
| Multiverb | 3.5 | 3.4 | synthetic |
| Chorus | 3.8 | 4.0 | synthetic |
| Space Echo | 7.5 | 7.9 | synthetic |
| Every builtin FX in silent gotham bypassed (373 records) | saves ~30 of 171 | — | `all_silent_bypass_all`. Kit 1's FX alone saved within noise. |

"Silent" for an effect is its cost over the same tracks without it while
nothing plays. "Playing" is its cost while its track plays.

What the rows say:

- **Silent costs about what playing costs.** Nothing in gotham triggering
  still costs 54% of everything playing (171 of 319 ms/s). An idle sampler
  track costs 80% of a playing one.
- **The neural sequencer is not the cost.** With its kit off it produced
  bit-identical audio with or without the instance, so it already sounds
  nothing. Its scheduler time is under 1 ms/s.
- **Builtin FX that never sleep are the visible per-effect cost.** These are
  Space Echo, Chorus, Multiverb, Slowdown, DJ Mixer, Filterbank, OTT, Tape and
  every DGen effect. Bypass removes the cost for the effects that skip their
  DSP, and Chorus keeps it. Reverb and Str8 Delay already sleep on silence.
- **Most of gotham's silent 171 ms/s is not builtin FX.** Bypassing every
  builtin FX saved about 30. The rest is per-track and per-bus graph work that
  runs every block, about 8 ms/s per gotham track. It comes from:
  - instrument voices,
  - DGen effects (2 ES Compressors on the master, Filter Table,
    ssl-e-channel-study, shimmerpitch),
  - the voice-sum, strip and send nodes, because sampler voices and bypassed
    or DGen effects never declare silence, so nothing downstream can skip.

  This split is from code reading. The probe cannot attribute it per node.
- **The kit-1 parked-candidate number is an estimate.** Deleting a kit through
  `App` and re-capturing gotham does not reload ("Track N has no effective
  pattern", "rack-slot effect-instance count does not match the live chain"),
  so it was not measured directly. The estimate takes the silent whole (171),
  subtracts the loose tracks (39) and the empty project (5), and spreads the
  remaining 127 over 15 kit tracks and the 5 buses: about 8 ms/s per track,
  so 70-80 ms/s for kit 1's 9 pads and its bus.

**Bypass audit** (code reading, `crates/sequencer`). There is no generic
bypass. Slot bypass only pushes `enabled` = 0 (`audio/rack.rs`
`push_rack_slot_effects_gate`), and each kernel decides what that means. A
bypassed effect never declares its output silent, so it also stops silence
propagation (eseq-v6te, `effects/silence.rs`) downstream.

| Effect | `enabled` = 0 | Sleeps on silent input |
|---|---|---|
| Filter, EQ8, Delay, Dimension, Phaser-Flanger, Roar, Multiverb, Compressor, Glue/444 (dynamics), OTT, Limiter, Tape, Filterbank, Space Echo | skips DSP (copies input) | no |
| Reverb | skips DSP | yes, 1 s hold (not while its mods move) |
| Str8 Delay | skips DSP | yes, hold = max delay |
| Chorus | **runs DSP**, wet slews to 0 | no |
| Slowdown | **runs DSP**, wet slews to 0 | no |
| DJ Mixer | **runs DSP**, dry/wet gate | no |
| DGen effects (Convolution Reverb, Filter Table, ES Compressor, user DGen FX) | DGen wrapper copies input and skips the kernel | no; the wrapper never checks or declares silence |

The measured bypass rows match the table: Reverb, Multiverb, Space Echo, Str8
Delay, Delay and Compressor drop to the no-FX cost, and Chorus stays at its
playing cost.

Instruments:
- **Sampler voices** return early when idle or disabled, but they zero their
  outputs without declaring silence.
- **DGen engine voices** skip the kernel and declare silence when idle. A
  disabled but not stale DGen voice zeroes without declaring, which looks
  like a gap.
- **Strips** (gain, sum, `stereo_panner`), `pdc_delay` and `peak_meter`
  propagate declared silence. The chain only goes quiet if every source
  declares.

**Recommendation.** Parking pays, but the order the table above implies is
wrong.

1. **Make idle chains declare silence**, before parking anything. Do these
   first:
   - sampler voices declare when idle,
   - the DGen effect wrapper sleeps on silent input with a tail hold, and
     declares,
   - a bypassed effect copies with silence passthrough,
   - fix the disabled-DGen-voice gap.

   This is small, per kernel, needs no scene logic, and is the only step that
   reaches the per-track floor (about 8 ms/s per gotham track, 3 per bare
   sampler). That floor is most of the 171 ms/s that a silent gotham costs.
2. **Tail-sleep for the non-sleeping time effects** (Space Echo, Multiverb,
   Chorus, Slowdown, DJ Mixer), using the existing `tail_sleep` helper, and
   make Chorus, Slowdown and DJ Mixer skip on bypass. This is the "track's own
   FX chain" row done per effect rather than per scene, and Space Echo alone
   is 7.5 ms/s per idle instance.
3. **Kit group bus parking** comes next, only if 1-2 leave it costing
   anything. Today it is the same non-sleeping effects on a bus.
4. **Kit sequencer parking** is not worth building for CPU: it measured under
   1 ms/s. Build it for the semantics §6 needs (a parked kit's sequencers do
   not run), not for cost.

Rerun to check each step; the probe and its command are in the file header.

#### After stages 1 and 2 (2026-10-24, eseq-ka5t.11)

**What changed.** The graph already had a silence flag on every edge
(`effects/silence.rs`, `ap_set_output_silent` in `graph_engine.c`): a kernel
that declares its outputs silent lets downstream kernels skip. No new
mechanism was needed. The small additions were `ap_input_silent(port)`,
`ap_current_node_noutputs()` and a per-node state-write count
(`ap_current_node_state_writes()`, bumped by every param or bulk state write).

- **Declares silence now:**
  - idle or disabled sampler voices (`sampler_idle_block`),
  - disabled DGen instrument voices (the gap noted above),
  - every builtin bypass. `silence::passthrough` emits a silent input as
    silent and copies anything else.
  - the DGen effect wrapper, on bypass and, for opted-in effects, while
    asleep.
- **Tail-sleep** (`effects::tail_sleep`). An effect sleeps when its input is
  silent and its internal signal has stayed below -120 dB for the hold. While
  asleep it skips its DSP and emits declared silence. The first non-silent
  block wakes it, with its state untouched.

  | Effect | Hold | Notes |
  |---|---|---|
  | DGen effects | max(1 s, memory span in frames) | **Opt-in only**, with a top-level `(effect-tail-sleep)` in the source (stripped before compiling, carried as `eseqEffect.tailSleep` in the manifest, like `effect-latency`). The wrapper judges quiet by its output, which cannot see a wet path hidden at mix 0, self-generated sound or free-running modulation, so only an author can promise there is none. Flagged: Convolution Reverb and Filter Table (both engines); each is a finite FIR behind control-rate smoothers. Not flagged: ES Compressor, whose release (up to 2 s) is hidden gain state the output does not show, and every `content/effects` patch (not audited). A node with no inputs never sleeps, and any param or tensor write restarts the hold. The trailer cells after the redzone replace a dead legacy param mirror in `apply_params` that would have overwritten them. |
  | Multiverb | 1 s | like Reverb: not while host mods move it, not mid mode-switch |
  | Chorus | ring length (~85 ms) | the LFO phase and write head keep advancing while asleep |
  | Slowdown | whole history ring (2^20 frames at 48 kHz, 21.8 s) | |
  | DJ Mixer | whole buffer (10 s) | only while recording; a held loop never counts toward the hold |
  | Space Echo | tape length (2.7 s) | only at age 0. Age > 0 feeds hiss and splice thumps into the loop forever, and that is sound, so an aged machine never sleeps. |
- **Settled bypass skips the DSP.** Chorus once its wet has faded to exactly
  0. Slowdown and DJ Mixer the same way, except that they keep recording
  their history (two stores per frame) so a re-enable plays current audio.
  DJ Mixer's closing gate now snaps to 0 once it goes subnormal; AArch64
  otherwise stalls it near 1e-43. DJ Mixer forgets its last sync cycle on
  bypass and sleep, so a re-enable mid-cycle does not relocate the loop.

**Audio.** The probe A/B (`ESEQ_PARK_DUMP_DIR`, `ESEQ_PARK_FORCE_AWAKE`) was
run from one binary.
- The bypass skips are bit-identical on every gotham variant (same hashes).
- Tail sleep changes only near-silence: `all_silent` max 1.4e-6 (-117 dBFS);
  `full` and `kit_off` are bit-identical to the pre-change build.

**Numbers.** These are render-thread ms per s of audio, release, M1 Max.
- Gotham rows: the minimum of 4 rounds (2 runs of 2, interleaved with a
  pre-change build), re-measured after the review fixes (DGen opt-in).
- Synthetic rows: the minimum of 3 rounds, before the review fixes; none of
  them uses a DGen effect.

Load average was 8-15 from concurrent agents, so both columns read about
10-20% above the P0 table; treat differences under about 10 ms/s as noise.

| Variant | Before | After |
|---|---|---|
| gotham `all_silent` | 206 | **187** |
| gotham `full` | 341 | 336 (noise) |
| gotham `kit_off` | 305 | 313 (noise) |
| gotham `loose_silent` | 270 | 250 |
| sampler track, silent, no FX (each) | 3.7 | 3.3 |
| Chorus silent / bypassed (per instance) | 3.9 / 3.8 | **0.7 / 0.2** |
| Multiverb silent | 4.0 | **0.5** |
| Reverb / Str8 Delay silent | 0.4 / 0.7 | 0.1 / 0.3 |
| Space Echo silent (default age 0.3) | 8.4 | 8.4 (hiss: never sleeps) |
| Slowdown / DJ Mixer silent | 1.5 / 0.9 | unchanged; the 8 s synthetic render is shorter than their 22 s / 10 s hold |
| Slowdown bypassed | 1.5 | 0.6 |

**Why silent gotham only fell about 10%.** `sample` profiles of
`all_silent`, before and after (before the DGen opt-in), show where the rest
goes:
- **Space Echo is 59% of the render.** It sits on a loose track with age 0.3,
  and its Grampian spring alone is 37%. Its hiss is real output
  (`all_silent` peaks at 0.004), so it keeps its own track, the Slowdown and
  Compressor after it, and the master chain (including the largest master
  DGen effect, 14%) awake.
- **The per-voice control nodes are about 28%** and do not depend on
  silence:
  - `gatepitch` (17%) renders its clock ramp every block for every voice,
  - `voice_modulator` (11%).
- The never-sleeping bus effects (Filterbank, OTT, Tape) are about 10%.

**Stage 3 (kit group-bus parking) is not worth building now.** Kit 1 still
costs about 25-35 ms/s (`full` - `kit_off`, the same as before). That cost is
its voices' control nodes, plus the hiss-driven master chain it shares.
Parking its bus chain would save only the bus effects, an estimated few
ms/s that was not measured. The bigger levers are, in order:
1. Decide whether Space Echo's hiss may gate off. That is a sound change and
   needs approval, which is why it was not built.
2. An idle fast path for `gatepitch` and `voice_modulator`.

#### After idle voice control fast path (2026-10-24, eseq-ka5t.16)

**Consumers.** Gatepitch feeds only two kinds of node: its voice modulator
(gate, pitch, velocity, trigger, bar clock, clock increment) and, for a DGen
engine, the synth's named host inputs (any lane, `clock` included). The
sampler gets its notes as events, not from these lanes. The host pushes each
voice's clock phase and increment every block. Idle voice modulators take an
early return (disabled DGen voice, inactive sampler voice with no gate in the
block, every slot Off), but each of those still scanned the gate and trigger
lanes and walked the clock frame by frame to count bar wraps (the multi-bar
LFO count, which must stay on the grid while a voice is silent). An idle DGen
synth reads none of its inputs.

**Invariant.** Every lane is still fully written with the same bits, so any
consumer that reads frames sees what it saw before. Idle modulators keep
their bar count, previous bar phase, previous gate and every slot state
bit-identical; their outputs stay declared silent as before.

**As built.**
- **Gatepitch's clock ramp, exact and vectorized.** Inside one binade a phase
  plus a fixed increment always rounds by the same whole number of ulps, so a
  run is `p0 + k*d` ulps, written as integer offsets of p0's bits.
  Binade crossings, the wrap, zero, ties and odd inputs take the reference
  step. The output is bit-for-bit the per-frame loop, for playing voices too.
- **Declared lane shapes**, a per-pass promise on an edge beside the silence
  flag (`ap_set_output_shape` / `ap_input_shape` in `graph_engine.c`,
  `silence::LaneShape`). `Constant` means every frame equals frame 0.
  `PhaseRamp` means values in [0, 1), at most one wrap, and less than 0.5 of
  rise in the pass. Gatepitch declares the increment constant on every call,
  the clock a ramp when the phase starts in [0, 1) and `inc * frames` < 0.25,
  and gate and trigger constant when the call has no event.
- **Idle modulator, O(1).** The gate check reads frame 0 when both lanes are
  declared constant. `BarTracker::skip_block` counts wraps from the
  endpoints when the clock is a declared ramp: the frame-0 wrap against the
  stored phase, plus one if the last frame is below the first.
- A/B switch: `tail_sleep::FORCE_REFERENCE_VOICE_CONTROL` (bit 4 of
  `ESEQ_PARK_FORCE_AWAKE`) restores the per-frame ramp and drops every shape.

**Tests.** `effects::gatepitch::tests::fast_clock_ramp_matches_reference_bit_for_bit`
covers 20k random and adversarial phase/increment pairs plus 400k frames of
a real bar clock. `instruments::voice_modulator::idle_fast_path_tests` runs a
live graph (gatepitch, a sampler voice modulator and a DGen voice modulator)
through idle, trigger, release and idle. The scenario includes retrigs at frame 0 and
the last frame, a gate in a not-yet-active block, a param-held gate, bar
wraps while idle and a mod reset while idle. Every frame and the modulator
state are bit-identical to the reference.

**Audio.** One binary, `ESEQ_PARK_FORCE_AWAKE=4` against 0: `full`,
`kit_off` and `all_silent` are bit-identical (8 renders per variant, one hash
each).

**Numbers.** Render-thread ms per s of audio, release, M1 Max. Each is the
minimum of 4 rounds (2 runs of 2, reference and fast interleaved). The load
average fell from about 18 to 8 during the runs.
Outputs are in `.local/benchmarks/park-probe-2026-10-24-voice-control/`.

| Variant | Reference | Fast path |
|---|---|---|
| gotham `all_silent` | 181 | **154** (-15%) |
| gotham `kit_off` | 313 | **281** (-10%) |
| gotham `full` | 337 | **315** (-7%) |

Playing variants gain too, because the exact ramp also serves sounding voices.
What is left per idle voice is about a dozen lane-cache checks, the ramp's
vector stores and four shape declarations. The host still pushes two clock
params per voice per block.

## 6. Kits on/off per bank

**Control.** The drum rack component in the fx panel gets a power toggle in
its header, the same control instruments and effects have. It is
bank-scoped: its tooltip reads "Kit on in bank C". The mixer's group strip
for a kit gets the same toggle in its context menu ("Turn kit off in bank
C").

**Semantics** (§3.2):

- **Off**: add the kit to `parked_kits` in every scene of the viewed bank.
- **On**: remove it from `parked_kits` in every scene of the viewed bank.
- Clip pointers are never touched, so off-then-on restores every scene's clip
  and every intentional silence.
- One undo step each. Other banks are untouched.
- **Mixed bank** (some scenes parked, some not, e.g. after copying scenes
  between banks): the toggle shows on with a small dot; clicking it sets the
  whole bank off.

**What follows from it.**

- A parked kit plays nothing: no pad triggers, no clip launch, its rack-owned
  sequencers do not run.
- Its CPU parking (§5) keys off the same flag.
- It counts as unused in the bank for §9, so its pad tracks hide from the
  bank's sequencer list and mixer. The kit itself stays listed in the mixer
  as a dimmed, collapsed group strip, so the toggle can be reached to turn it
  back on (and under "show all tracks"). The way back on must always be
  visible: clicking the dimmed strip selects the kit and shows its drum rack
  in the fx panel with the toggle off; the strip's context menu also has
  "Turn kit on in bank C". A test asserts a parked kit's strip is present in
  the bank's mixer.
- A kit parked in every scene is unused project-wide; it is listed for purge
  like unused slots (§4), never deleted automatically.

**As built (eseq-ka5t.13).** `Scene.parked_kits` (`sequencer/state/kit_parking.rs`),
inherited through `SceneSwapState`, captured by scene-structure undo, cleared
for a deleted kit in `sync_rack_members`, saved as `scene_parked_kits` (v19).
Parking is a gate, not a rewrite: the scene still installs the kit's clip in
the live grid (so its lanes stay editable and save back into the right clip),
and every scheduler snapshot applies the parked set of the scene it plays
(`SequencerSnapshot::park_kits`: the live capture the playing scene's, a
boundary launch its target's, a per-track launch the playing scene's, a song
row its row scene's). A parked kit's member tracks are `scene_silenced` (no
pattern triggers) and its rack-owned graphs and generators skip their ticks
(no fires, no state advance; they resume where they stood). A rack clip launch
on a kit the playing scene parks is refused with "Kit is off in bank C"
rather than repointing a scene that would stay silent. Commands:
`set-kit-parked` (`set-kit-parked!` in `eseq.kinds`); group host fields
`parked`, `parked-banks`, `mixed-banks`, `unused`; `kit-bank-state` reads
`:on` / `:off` / `:mixed`. There is no tooltip primitive, so the drum rack
header prints the hint ("Kit on in bank C") next to the kit's name.

**Rig view (later).** An elevated view can collect parked kits on a shelf
(a strip of chips: name, pad count, banks where it plays; double-click or
drag back in to turn on). Same commands; not part of the built-in work.

## 7. The channel view (prototyped)

proto-rig (`~/.eseq.d/packages/local/proto-rig/rig.lisp`, imported in a
project's scratch in place of `autechre.mixer`) is the prototype; the user
plays sets with it. What it settled:

- **Grid.** One box per track or kit the viewed bank uses, Logic-dense:
  device pills (instrument green, effect blue, bypassed dimmed), fader with a
  level meter on the dB scale, mute/solo, output picker, clip number, name
  plate with mod ports and a trigger dot (`track.triggered`). Buses pack into
  the same grid. Colours come from theme slots only.
- **Kits** are boxes of green pad pills with a hit dot each. Double-click
  opens the kit panel in the lower area: the kit's bus box, then one box per
  pad with its slot strip (click a slot name to open it, power dot to toggle
  it), and a **kit monitor** tile to its right: kit name, sequencer tabs with
  kind icons, the clip, a lattice visualizer, the neural firing history, a
  spectrogram and a note view with a piano.
- **Visualizer** tile beside the grid: the mix as an orbit (each channel a
  bead sized and lit by level, satellites for the notes playing), scopes,
  sequencers not owned by a kit, notes.
- Selection marks only while the *fx* panel shows; no open/zoom buttons
  (double-click instead).

**Built in or a package: undecided, user-led.** The user leans towards
keeping the channel view a package and will lead a design pass before
anything moves into the factory (eseq-ka5t.2, deferred). Until then the
backend work (§3–§6, §8, §9) is designed as host fields and commands any
package can use, and proto-rig adopts them: §9's host field in place of the
Lisp walk, the shelf (§6) under its grid, swap drops (§4) on its device pills
and name plates.

**Glances.** A sequencer's small live view should be registered in Lisp,
`(def-glance "density" density-glance)` in a small `eseq.glance` module, so
the kit monitor and the visualizer show any kind's glance without knowing the
kind. The prototype hard-codes the neural one.

## 8. Peeking into other projects: controlled import

The user pulls parts of other projects into the master project: a kit with
its pads and sequencers, a group with its tracks, single tracks, or a whole
bank.

**Browse.** A **Projects** section in the built-in browser opens another
project file **read-only** and lists it as a tree: banks (with their scenes),
kits, groups and tracks, each with its devices. Nothing in the master
project changes while browsing. Auditioning a picked item before import is
out of scope for the first cut. Pick any mix of kits, groups, tracks, or a bank.

**Import rules** (what "controlled" means):

- **Always new.** Imported tracks and kits are appended with new indices;
  nothing merges into an existing track. Their patterns, clips and sound
  entries are copied into the new tracks' pools.
- **One bank.** Imported things are on only in the target bank: the viewed
  bank or a new one created by the import. Every other scene gets them parked
  (imported kits in `parked_kits`, imported tracks with no cells), so no
  finished bank changes.
- **Importing a bank** brings its scenes as a new bank, with the scenes' cells
  remapped to the new track indices.
- **Sequencers travel with their owner.** A kit's rack-owned instances come
  with it. A project-owned sequencer comes only if picked, and then only
  active in the target bank (§10). Links between picked things are kept and
  remapped; routes to anything not picked are listed in the import summary as
  unconnected, never silently pointed at a master track with the same index.
- **Assets are shared.** Samples and compiled instruments are de-duplicated by
  content, so importing the same kit twice loads it once.
- **Into a slot.** Dropping an imported track's device onto an existing track
  is a swap (§4): it lands as a new slot, enabled in the bank.
- **One undo step** for the whole import.

**Reuse.** Kit export already serializes a kit with its tracks, clips and
sequencers; import of a kit is that format read from a project file instead
of a kit file. Tracks and groups are the same with fewer parts; a bank adds
scenes.

## 9. Bank-scoped display

**Used in the bank.** A track is used in a bank when any scene in the bank
gives it an audible cell, or, for a kit's pad, any scene of the bank points
at one of the kit's clips and does not park it (§6). Everything else is hidden in that bank's views: the sequencer track
list, the mixer, and the route drop-downs of sequencers (and a rig view, if
one is loaded). A
"show all tracks" toggle overrides it.

One host field, `track.used-in-viewed-bank` (and `group.used-in-viewed-bank`
for kits), computed once per change of scenes or viewed bank. The views stay
in Lisp. proto-rig's per-frame walk of every cell and clip (§7) is fine at
gotham's size and is not at hundreds of scenes. Hidden tracks keep their
indices; nothing in the engine renumbers.

**A track's device line** shows the slot enabled in the current scene (if
several are, the first and a count); clicking it opens the slot list for
recall (§4).

**As built (eseq-ka5t.10).** The viewed bank is Lisp state, so the host
publishes per-bank lists instead of one boolean: `track.used-banks` and
`group.used-banks` (`ProjectScenes::bank_usage`, one pass over the scenes
with a per-track bank bitmask, `ui/host_kinds/bank_usage.rs`), recomputed
only when the scenes revision, the track/group/bank instances or the group
topology move. A scene uses a track when it gives it a pattern cell, steps or
none (a graph-driven track plays an empty pattern); a kit's pad when the scene
points at one of the kit's clips and does not park it (a legacy kit: the pad's
cell, unparked); a kit where it plays; a plain group where a member or child
kit is used. Lisp (`eseq.scene-banks`): `used-in-viewed-bank?`,
`track-used-in-bank?`, `group-used-in-bank?`, `kit-bank-view` (`:shown`,
`:off`, `:hidden`) and the `show-all-tracks` defcustom (View > Show All
Tracks, and the bank selector's right-click menu). A track or kit no bank
uses shows in every bank, like an unused clip-grid cell, so a fresh track is
never stranded. Hidden: sequencer rows (and UP/DOWN, shift ranges), mixer
strips and LEFT/RIGHT, the lane track-inlet menu, project-owned graph route
menus (graph demos, neural variable-reset) and jaki's project routes; a
rack-owned sequencer's pad routes stay whole. A menu keeps the track its
control names now. A selected track the bank hides hands the selection to the
nearest shown one. A kit the bank parks keeps its sequencer header and its
dimmed mixer strip (also under show-all). The device line is on Instrument
Rack tracks' sequencer rows (first enabled slot "+N"; a click shows the slot
list in *fx*). The arrangement view still shows every track: it spans banks.

## 10. Project-owned sequencer membership (later)

A kit's sequencers follow the kit's on/off. A project-owned sequencer
(density across several tracks) runs in every scene, and density's
`sync-density` rewrites track processes for every density instance in the
project, so bank C inherits bank A's conductors. To fix it:

- per-scene membership `%instance/<id>/%active` in the scene-slot store, with
  `ProjectInstance::default_active` (legacy true, new instances true only in
  the creating bank);
- processes declared with an owner are gated by its membership in the
  scheduler;
- remove from bank, recall a scene's settings, an Unused list, explicit purge.

This is the same rule as §3 for a different thing: who plays is scene state.

## 11. Trigger lines (later)

Autechre's cascade sequencer sends to named trigger lines (`drumtrig 1`,
`drumtrig 2`), and whatever listens plays. eseq sequencers route to track
indices, or pad indices inside a kit. Named trigger lines, with per-scene
listeners on tracks, would let a sequencer keep its routing while the bank
changes what listens. eseq already has the value half (`defchan`, `send`,
`chan-get`). Separate spec.

## 12. Order

1. Scene-owned swap state: stable slot ids, `Scene.slot_enable`, migration
   (§3).
2. eseq-lv9q: fit-to-scale with mixed-tuning slots.
3. Drop-to-swap scoped to the bank, hide disabled slots, recall, purge (§4).
4. Kit on/off per bank: `parked_kits`, the drum rack toggle (§6).
5. `used-in-viewed-bank` host fields (§9); proto-rig switches to them.
6. P0 measure, then parking for track chains, kit buses and kit sequencers
   (§5).
7. Project browser and controlled import (§8).
8. Glances registry (§7). The channel view's home (factory or package) waits
   on the user's design pass (eseq-ka5t.2).
9. Project-owned sequencer membership (§10).
10. Trigger lines (§11), own spec.

## 13. Open questions

- Should slot FX bypass wait for the tail like track chains (§5)?
- "Load kit preset" onto a kit in bank C: swap per pad matched by pad
  position (proposed), or import the kit as a new kit (§8) and park the old
  one in the bank?
- Is "used in the bank" also shown per scene (dim tracks the current scene
  doesn't use)?
- Does an imported bank keep its own tempo, or adopt the master project's?
