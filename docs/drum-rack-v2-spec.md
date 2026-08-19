# Drum Rack v2 — Rack as a Play Surface over Real Tracks

Supersedes the *drum rack* portions of `docs/racks-spec.md`. The Instrument
Rack (`Broadcast` layering container) is out of scope here and keeps its
current slot-based design.

## Why v1 failed

The v1 drum rack is one track whose sequencer is projected into lanes: a hit's
**transpose value is the pad selector** (`audio.rs`
`rack_slot_matches_routing`: `slot.pad_note == transpose.round()`), and
`ByPitch` playback forces synthesis transpose to `0.0`. Structural
consequences:

- A step can't say "snare, pitched −4" — changing transpose re-routes it to a
  different pad.
- Simultaneous hits (kick+snare on one step) share one step's params; you can't
  set their velocities independently.
- `timebase`, `num_steps`, swing, and the accumulator live in `TrackParams` —
  one per track. You cannot give the hi-hat lane 1/64 timebase p-locks or an
  8-step length while the kick runs normally. This is the killer.

The fix is not per-slot copies of sequencer state inside the rack (that would
re-implement tracks: step data, playheads, p-lock lanes, undo slices, song-mode
capture). We already have a fully-featured per-lane sequencer — the track.

## Core model

**A drum rack is a track group with a pad-routing map.** Each pad is backed by
a *real member track* with everything a track has: its own pattern length,
timebase (+ p-locks), swing, accumulator, step transpose (meaning *pitch*
again), midi fx, effect chain, mixer channel, undo, song-mode capture.

The rack itself contributes exactly three things:

1. **Curation** — an ordered set of pads, each mapped `pad_note → member
   track`, plus kit-level identity (name, color, choke groups, pad layout).
2. **Playability** — arming the rack turns the live keyboard into pads:
   incoming note → pad map → trigger (and record into) that member track.
3. **A group fx chain / mix point** — members route into the rack's backing
   bus, whose effect chain is the rack fx chain (matching how v1 racks put fx
   after the slot sum).

This builds directly on the track-groups design
(`crates/sequencer/docs/track-groups-spec.md`): bus-backed group, ordered
member indices, render-order layer, no track-`Vec` reordering. A drum rack is
a track group *plus* note routing and pad metadata.

## Non-goals

- Migration of v1 drum-rack projects. None exist worth preserving.
- Instrument Rack changes (layering stays slot-based for now).
- Nested racks / rack inside a group.
- Per-pad sub-mixes beyond what the member track's own channel + rack bus give.

## Data model

Extend the track-group record rather than inventing a parallel container:

```rust
pub struct ProjectTrackGroup {
    // ... existing fields: id, name, color, collapsed, members, bus_id ...
    #[serde(default)]
    pub rack: Option<ProjectRackConfig>,   // Some(_) => this group is a drum rack
}

pub struct ProjectRackConfig {
    pub pads: Vec<ProjectRackPad>,         // ordered; pad grid position = index
    #[serde(default)]
    pub choke_groups: Vec<Option<u8>>,     // parallel to pads
}

pub struct ProjectRackPad {
    pub pad_note: i32,                     // MIDI note this pad answers to
    pub member: usize,                     // index into group.members
}
```

Invariants (inherit all track-group invariants, plus):

- Every pad's `member` points at a live member of the same group.
- A member track may back at most one pad. Members without a pad are legal
  (an fx-return-ish track inside the kit) but not reachable from the pad
  keyboard.
- `pad_note` values are unique within a rack.
- Deleting a member track removes its pad; deleting the group deletes the rack
  config with it. Track reindex-on-delete updates `members` exactly as the
  track-groups spec requires — pads reference members by position in
  `members`, so they survive reindexing for free.

No new per-track state. Member tracks are ordinary tracks; nothing in
`TrackParams`/`PatternState` changes.

## Arming & live play

Two independent arm targets, exactly as requested:

- **Arm the rack (header row).** The live keyboard becomes pads: incoming note
  `n` → look up pad with `pad_note == n` → trigger that member track at its
  slot's base pitch (transpose 0 + the track's own base note offset). While
  transport records, the hit is written into *that member track's* pattern via
  the existing per-track record path — `record_position_at_beat(track, …)`
  already quantizes against the target track's own timebase, so a hat pad
  recorded at 1/64 timebase lands on its grid while the kick pad lands on its
  grid, in the same performance.
- **Arm a member track directly.** Identical to arming any normal track: the
  keyboard plays it chromatically, transpose is pitch, recording writes
  transposes. This is how you play a melodic line on just the hi-hat.

Arm exclusivity follows whatever the existing single-armed-track rule is; the
rack header is one more armable target. Arming the rack does not arm members
(and vice versa). As shipped: one rack is armed at a time, arming a rack
disarms *its own* member tracks, and arming a member track disarms the rack it
belongs to. Tracks outside the rack keep the existing multi-arm behavior and
play chromatically alongside the pads.

## Trigger routing

- Live pad routing happens where v1's `ByPitch` dispatch ran, but the target is
  now a member *track* trigger, not an intra-track slot — reuse the existing
  cross-track retarget path (`rebind_midi_fx_event_to_track` machinery) rather
  than the slot fan.
- Sequenced playback needs **no rack routing at all**: each member track plays
  its own pattern like any track. This deletes the transpose→slot matching
  from the audio hot path for racks.
- **Choke groups** become a cross-track voice-release pass: when a pad in
  choke group *g* triggers (live or sequenced), release sounding voices of
  other member tracks in *g*. Port `collect_rack_choke_group_voice_releases` /
  `release_rack_choke_group_voices` from per-slot to per-member-track keying.
  This is the one piece of genuinely new audio-thread plumbing.

## Audio / fx routing

Straight from the track-groups audio model:

- Each member track keeps its own effect chain (the per-pad fx chain — better
  than v1, where per-slot fx were bolted onto the slot record).
- Every member's `output` is `Bus(group.bus_id)`. The backing bus's effect
  chain **is the rack's group fx chain**, matching v1's "fx after the slot
  sum" behavior. The header volume fader is the bus fader.
- Per-track sends still work per member; the bus provides the group-level
  send/return position if wanted later.

## Racks inside track groups (rack as a track-like member)

A rack must be groupable exactly as if it were a single track: multi-select a
rack alongside loose tracks, cmd+g, and the rack joins the group as one unit.
Two pieces make this work:

**1. Bus→bus output (new, small primitive).** Today every bus is hard-wired to
the master mix — `ProjectBusChannel` has no output field and bus creation
connects `volume_id → bus_l_id/bus_r_id` unconditionally
(`tui/graph.rs:1157`). Add:

```rust
// ProjectBusChannel
#[serde(default)]
pub output: BusOutput,          // enum BusOutput { Mix, Bus(u64) } — Mix default
```

When a rack joins a parent group, set the rack bus's `output =
Bus(parent.bus_id)` and reconnect its `volume_id` into the parent bus's
`left_id`/`right_id` inputs (same connect/disconnect pattern as track sends at
`graph.rs:1500`). Old projects deserialize as `Mix` and behave unchanged.
Invariant: the bus output graph must stay acyclic — with the nesting rule
below this is structurally guaranteed (rack bus → plain-group bus → mix, max
depth 2), but validate on load anyway.

**2. Group members can be racks.** The plain group's membership generalizes
from track indices to:

```rust
pub enum GroupMember {
    Track(usize),               // stable track index, as today
    Rack(u64),                  // GroupId of a rack group
}
```

Nesting rule (keeps the track-groups "one level deep" spirit): plain groups
may contain tracks and racks; racks contain only member tracks; plain groups
never contain plain groups. A rack belongs to at most one parent group.

Consequences that then fall out:

- **Render order / collapse**: the parent group's block treats the rack as one
  entry — collapsed, it is a single strip (the rack header); expanded, the
  rack's own header+members render nested inside the parent block.
- **Group fx / fader**: parent-group effects and volume apply to the rack
  because the rack bus feeds the parent bus upstream of the parent's chain —
  no special casing.
- **Mute**: muting the parent bus silences the rack via the audio chain.
  **Solo** needs the bus solo pass to understand chained buses: soloing the
  parent must keep its upstream rack bus audible, and soloing the rack must
  keep its downstream parent bus open. Extend the solo resolution to walk
  `BusOutput::Bus` edges (both directions) when computing the audible set.
- **Track deletion reindexing** already updates rack `members`; parent groups
  reference racks by stable `GroupId`, so they are immune to track reindexing.

## UI

The grid view renders the rack as a header row plus full member rows:

```
[arm] [mute] [solo]  Drum Rack        [volume]
  [pad 1: full track row — arm/mute/solo, lanes, expandable step editor]
  [pad 2: full track row — …]
  …
```

- **Header row**: arm (pad-play mode), mute/solo (drive the backing bus),
  name, volume (bus fader). Nothing else. Collapse folds members like the
  mixer group collapse.
- **Member rows are exactly the normal track UI** — same widget path, not a
  miniature. Timebase p-locks, pattern length, accumulator, expand-to-step-
  editor all work because they *are* tracks. The only rack-specific additions
  per row: the pad-note badge and choke-group selector.
- The v1 lane projection UI (`seqv-drum-track-grid`, drum-lane state helpers,
  `DrumLaneHistoryAction`) is deleted once v2 lands.
- Mixer: reuse the track-groups render-order layer; a rack renders as its
  group (header strip + members, collapsible).
- Pad grid (4×4 performance view) can stay as a *view* over the pad map for
  finger drumming / slot browsing; it no longer owns any sequencing.

Follow the `each`-based widget generation convention (never `map`).

## Track budget

- A 16-pad kit consumes 16 of `MAX_TRACKS = 64`. Mitigation: **lazy pads** — a
  pad only claims a track when a sound is dropped on it. Creating a drum rack
  creates the group + bus and zero member tracks. Typical kits (6–10 pads)
  cost 6–10 tracks.
- If this bites in practice, raising `MAX_TRACKS` is a follow-up, not a
  blocker for v1 of this spec.

## What survives from rack v1

- Sampler/custom-instrument slot construction (`build_sampler_voices`,
  `connect_engine_to_track`) — unchanged, members are ordinary tracks.
- Pad-note constants and the drum-rack browser/creation entry points
  (`add_sampler_drum_rack_track` becomes "create rack group", pad-fill helpers
  retarget to "create member track + assign pad").
- Choke-group semantics (re-keyed to member tracks).

Deleted: `ByPitch` slot matching in dispatch, transpose-as-pad-selector, the
drum-lane projection helpers and their history actions, per-slot mix fields
(subsumed by member track channels), `RackSlotParamPlocks` (subsumed by real
per-track p-locks).

## Phasing

1. **Group + pads + audio** — `ProjectRackConfig` on track groups, lazy member
   creation, member output → group bus, header row rendering with full member
   rows. No live play yet; sequencing already works because members are
   tracks.
2. **Pad play + record** — rack arming, note→pad→member trigger, record into
   member patterns, choke groups cross-track.
3. **Rack-in-group** — `BusOutput::Bus` primitive + chained-bus solo pass,
   `GroupMember::Rack`, nested render-order block.
4. **Polish** — pad grid performance view, pad-note badges/choke UI, kit
   save/load as a browser object (a kit = group config + member instrument
   presets).

## Open questions

- ~~Should the rack header's arm and a member arm be mutually exclusive, or
  can the rack stay armed while one member is "focus-armed" for chromatic
  play?~~ Resolved: exclusive, scoped to the rack's own members (see "Arming &
  live play").
- Kit-as-preset: what exactly serializes when saving a drum rack to the
  browser (member instruments + fx + pad map, but presumably not patterns?).
- Does the Instrument Rack eventually migrate onto this model (a group with
  `Broadcast` routing), or stay a slot container? Not needed for v1.
