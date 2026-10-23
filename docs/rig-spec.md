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
  (its params and p-locks stay per pattern). Today slots are matched by
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
   the device they hold there (instrument type + name, or sample for a
   sampler slot). The first group keeps the position; each other group
   becomes a new slot inserted directly after it, with a fresh id. Example:
   row 1 holding Digi Syn in pattern 1 and Digi FM in pattern 2 becomes row
   1 = Digi Syn, row 2 = Digi FM.
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
  back on (and under "show all tracks").
- A kit parked in every scene is unused project-wide; it is listed for purge
  like unused slots (§4), never deleted automatically.

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
