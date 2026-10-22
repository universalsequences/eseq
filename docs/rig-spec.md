# The rig: banks as phrases, parked devices, a channel view

Status: spec rev 2, 2026-10-21. Nothing in this spec is built yet; §2 lists what
already is. Epic: `eseq-ka5t` (`bd list --label rig`).

Rev 2 replaces rev 1's instance-first model. Designing against Autechre's rig
and the gotham-riddim project showed that the unit that changes per scene is a
**channel's device**, not a sequencer instance, and that the main problem is
display, not engine. Rev 1's per-scene instance membership survives, narrowed
to project-owned conductors (§8).

## 1. Goal

One project holds a whole set. A **bank** is a phrase: the user makes a new
bank when they find something worth keeping and keeps working in it as if it
were a fresh project. Work in bank C must not disturb A or B, and the screen
must show what bank C uses, not everything every bank has ever used.

Not a goal: copying Autechre's fixed 16 channels + 4 buses. Parking (§3) is
eseq's own route to the same property: lots of devices loaded, only the
playing ones cost CPU.

Reference project: `.local/projects/gotham-riddim-edit2.json`. Two kits
exported from two projects ("Jungle-ology Kot", 9 pads; "Gotham Piano Kit",
6 pads), each driven by its own rack-owned neural instance, 19 tracks, 40
scenes in banks A and B. Scenes were found by launching each rack's clips and
mixing and matching. The target user story: add bank C, keep the drums, swap
the piano samples for synths, and see only what bank C uses.

## 2. What already exists

- **Slot parking.** `RackSlotSnapshot.enabled` (commit `089e99e88`,
  eseq-bw9v): stored per pattern, a disabled slot gets no triggers, its FX are
  bypassed, and its engine runs zero voices once its release tail ends.
  Custom engines already shrink to 0 voices when idle
  (`shrink_released_voices`).
- **Per-scene slot lists.** A track's Instrument Rack slots live in the track's
  pattern pool entries (`patch.rack_track`), so scenes on different patterns
  can hold different slots. `append_rack_slot_to_current_pattern` and
  `append_rack_slot_for_all_pattern_snapshots`
  (`sequencer_state/rack_editing.rs`) are the two existing add paths.
- **Scene banks** (`scene-banks-spec.md`): view-only spans over the scene list,
  plus `copy-scene-to-new-bank!`.
- **Kits** with rack-owned instances and rack clips
  (`rack-clips-and-break-kits-spec.md`): a drum rack, or "kit" below, is a
  group of real tracks with a pad map, a group bus and attached sequencers. A
  scene points at one clip per kit, or `None` (silent).
- **The autechre package** (`~/.eseq.d/packages/local/autechre/mixer.lisp`):
  one box per track, its devices listed, a group-output picker per box.

Naming, used throughout: **kit** = a drum rack (a group of tracks).
**Slots** = the Instrument Rack on a single track. "Rack" alone is ambiguous in
this codebase today and this spec avoids it.

## 3. Parking

A device that is loaded but not playing in the current scene costs no CPU, and
coming back is a swap, not a rebuild (compiled code, samples and state stay
loaded).

| Thing | Parked when | Status |
|---|---|---|
| Instrument Rack slot | `enabled = false` in the scene's pattern | built |
| A track's own FX chain | the track is silent in the scene (no audible cell) and its tail has decayed | to measure, then build |
| A kit's group bus chain | the kit's clip pointer is `None` and the tail has decayed | to measure, then build |
| A kit's attached sequencers | the kit's clip pointer is `None` | to measure, then build |

Stage P0 is measurement only: in gotham, with all of one kit silent, profile
what its tracks' chains, its group bus and its neural instance cost. Write the
numbers here before building anything. Also check what a bypassed effect costs
(the slot bypass pushes the effect's `enabled` param to 0; whether each effect
skips its DSP when disabled is per-effect).

Tails: a chain parks only after its output stays below a threshold for a
hold time, so a reverb or delay rings out across a scene change. The same rule
covers a slot whose effects are bypassed on the boundary: today they cut. Decide
in P1 whether slot FX bypass should also wait for the tail.

## 4. Swap mode for Instrument Rack slots

Today, swapping a device in one scene means opening the Instrument Rack,
dragging a synth into a new slot, then disabling the old slot. Swap mode turns
that into one gesture.

**Gesture.** On a track whose device is an Instrument Rack (or any track, see
below), dropping an instrument, loading a preset or choosing "Swap device…":

1. adds the new device as a new slot;
2. enables it in **the scope** and disables every other slot in the scope;
3. leaves every pattern outside the scope untouched.

**Scope.** Default: every scene of the viewed bank. Alternative, chosen in the
gesture: this scene only. Bank scope fits the "a bank is a phrase" workflow.

**Shared patterns.** A track's pattern can be referenced by scenes inside and
outside the scope (pool sharing). Swap mode must not change the outside
scenes, so the change forks: each in-scope scene that shares a pattern with an
out-of-scope scene gets its own copy first, then the copy changes. This is the
non-clobbering guarantee; it must be tested on a pattern shared across banks.

**Plain tracks.** A track whose device is a single instrument converts to an
Instrument Rack with that instrument as slot 1 the first time it is swapped,
in every pattern, so other scenes keep sounding the same. After that, the
track's display treats it as one device (§5); the slot machinery stays out of
sight unless opened.

**Undo.** The whole gesture, including forks and the conversion, is one
recorded edit.

**Recall.** "Use this device here too": enable an existing slot in another
scope. In the channel view this is picking from the track's list of slots,
each shown with the banks where it is enabled.

**Kits.** A kit's pad track is an ordinary track, so swap mode applies to its
pads too. Bank C of gotham keeps the piano kit's 6 pads, its clips and its
neural sequencer, and swaps the device on some pads: that is the user story
in §1, needing nothing kit-specific.

**Tuning.** The piano pads' samples are tuned to A, and a synth slot with
`base note -3` added next to them sounds wrong with fit-to-scale on. Swap
mode makes mixing tunings routine, so this needs fixing first: eseq-lv9q.

## 5. Bank-scoped display

**Used in the bank.** A track is used in a bank when any scene in the bank
gives it an audible cell, or, for a kit's pad, the kit's clip pointer is
`Some` in any scene in the bank. Everything else is hidden in that bank's
views: the sequencer track list, the mixer, the channel view, and the route
drop-downs of sequencers. A "show all tracks" toggle overrides it.

This is a view rule, so it belongs in Lisp where the views are, given one host
field: `track.used-in-viewed-bank` (or a list of the bank's used track
indices on `project`). Hidden tracks keep their indices; nothing in the engine
renumbers.

**New bank.** `copy-scene-to-new-bank!` already copies cells, so bank C
starts with exactly the tracks the copied scene uses.

**A track's device line** shows the slot enabled in the current scene
(if several are enabled, the first and a count). Clicking it lists the
track's other slots for swap/recall (§4).

## 6. The channel view

The autechre package grows into the main performance view: one box per
**used** track (§5), each box showing its current device, its FX, its meters
and its mute/solo.

**Kits are boxes too.** A kit collapses to a single box by default, like a
collapsed group in the mixer: the kit's name, a glance of its attached
sequencer, its group bus chain, and a **pad strip**: one thin column per pad
that plays in this bank, each a level meter.

- **Device colour.** Each pad column is tinted by the identity of the device
  enabled on that pad in the current scene (a stable colour per slot). A swap
  changes the colour, so a scene change that swaps the kick is visible from
  the top level without expanding. A pad whose enabled slot is not its first
  slot also carries a small "swapped" mark.
- **Change flash.** On a scene change, pad columns whose device changed flash
  once.
- **Hover** on a column shows the pad name and its device; the column is also
  a drop target (§4 swap) without expanding.
- **Expand in place** (the mixer-group gesture): the kit's pad boxes take its
  place in the grid at a shrunk size (name, device, meter, mute/solo). A kit
  can be pinned expanded, for a performance where its devices should stay in
  view.
- **Zoom** opens the kit full size, its pad boxes and its sequencer's full
  view, like a Max subpatch; Escape returns.

Collapsing trades visibility for calm: Autechre's rig shows every device at
once, this view shows them on demand. The device colour and change flash are
what keep a collapsed kit honest about what it is playing.

**Glances.** A sequencer's small live view is registered in Lisp:
`(def-glance "density" density-glance)` in a tiny `eseq.glance` module, not
a `def-kind` slot. A kit box shows the glances of its attached sequencers;
project-owned sequencers get boxes of their own.

All of §6 is Lisp: `project.instances`, `instance-ref`, the kinds already
published for tracks, groups and devices, and §5's one host field.

## 7. Trigger lines (later)

Autechre's cascade sequencer sends to named trigger lines (`drumtrig 1`,
`drumtrig 2`), and whatever listens plays. eseq sequencers route to track
indices, or pad indices inside a kit. Named trigger lines, with per-scene
listeners on tracks, would let a sequencer keep its routing while the bank
changes what listens. eseq already has the value half (`defchan`, `send`,
`chan-get`). This is a separate spec when §3–§6 are in use; it is what a
"unpack kits onto fixed channels" model would need.

## 8. Project-owned sequencer membership (later)

Rev 1's model, narrowed. A kit's sequencers already follow the kit's clip
pointer. A project-owned sequencer (density across several tracks) runs in
every scene, and density's `sync-density` rewrites track processes for every
density instance in the project. To make bank C not inherit bank A's
conductors:

- per-scene membership `%instance/<id>/%active` in the scene-slot store, with
  `ProjectInstance::default_active` (legacy true, new instances true only in
  the creating bank);
- processes declared with an owner are gated by its membership in the
  scheduler;
- remove-from-bank, recall a scene's settings, an Unused list, explicit purge.

## 9. Order

1. P0: measure parking costs (§3).
2. eseq-lv9q: fit-to-scale with mixed-tuning slots.
3. Swap mode (§4).
4. Bank-scoped display (§5).
5. Parking for track chains, kit buses and kit sequencers (§3), sized by P0.
6. Channel view with kit boxes and glances (§6).
7. Project-owned sequencer membership (§8).
8. Trigger lines (§7), own spec.

## 10. Open questions

- Swap scope default: bank (proposed) or scene?
- Should slot FX bypass wait for the tail like track chains (§3)?
- Swap mode on a kit's pad: does "load kit preset" onto a kit in bank C mean
  swap every pad (a new set of slots per pad) or replace the kit? Proposed:
  swap per pad, matched by pad position.
- Is "used in the bank" per scene too (dim tracks the current scene doesn't
  use), or only per bank?
