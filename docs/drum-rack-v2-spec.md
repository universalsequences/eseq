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
    pub pads: Vec<ProjectRackPad>,         // ordered; grid position is by pad_note, not index
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
- A member track may back at most one pad, and **every member has one**. A
  padless member is not reachable from the pad keyboard, does not appear in the
  pad grid, and has no choke selector — it is invisible, unplayable dead
  weight — so joining a rack always maps a pad: callers that name a `pad_note`
  get that pad, callers that don't (the mixer group-header drop, the track-badge
  drag, move-track-to-group) get the lowest free note. A rack with all 128 notes
  mapped is full and refuses the join. Projects saved before this rule are
  repaired on load: unmapped members claim free notes in member order.
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

As built, the two kinds are stored in two fields rather than one heterogeneous
`Vec` — `members: Vec<usize>` (unchanged, and still what rack pads index into)
plus `rack_members: Vec<u64>` — and `group_members_ordered(group, groups)`
interleaves them into `Vec<GroupMember>` for render order. A rack sits at its
own lowest member track, so it occupies exactly one slot in the parent's block;
racks that have claimed no track yet sort last. Keeping `members` a plain track
list is what lets `ProjectRackPad::member` stay a position in it, and keeps old
files parsing with no custom deserializer.

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
- Pad grid (4×4 performance view) stays a *view* over the pad map for finger
  drumming / slot browsing; it owns no sequencing. It renders in the **`*fx*`
  rack panel** — selecting a rack selects its bus, and that panel answers with
  the kit rather than a bare bus chain. The grid is **note-positional**: a cell
  IS a fixed MIDI note, and it draws whichever pad answers to that note — grid
  position derives from `pad_note`, *never* from the pad's index in the vec, so
  a pad's label and its position can never contradict each other. Cells with no
  pad are empty; the grid is sparse and never compacts.
  - A page shows sixteen consecutive notes with the **lowest bottom-left**,
    ascending left→right then bottom→top. Pages are **octave-aligned** (page
    *k* starts at note `12k`), so the bottom-left cell of every page is a C;
    the price is a four-note overlap between adjacent pages, which is exactly
    what a plain 16-note stride cannot buy. The top page is clamped at *k* = 9
    (notes 108–123) so no cell ever names a note past 127.
  - Paging walks the note range, not the pad list, and its readout names the
    notes on screen. A rack's grid opens on the page holding its lowest pad —
    a kit that lives at C7 must not open onto empty octaves — and an empty rack
    opens at the drum home (note 36). Page state is scoped to the rack, so one
    rack's page never leaks into another's grid.
  - Dropping on an **empty** cell maps the new pad to *exactly* that cell's
    note (lazy pads, "Track budget"); there is no next-free fallback, because
    an occupied cell is never routed down the empty-drop path. Drops that name
    no cell — mixer group header, track badge, move-track-to-group — keep the
    lowest-free-note behavior.

  A cell click hits the pad down the live path (member track at base pitch), so
  choke groups and the member's own fx chain apply exactly as from the
  keyboard, and it also focuses the pad so the panel can open that member's own
  track.
- The pad-note badge is a note name with −/+ nudges, and the choke selector is
  an Off/1..16 dropdown; both address the pad by *(rack group id, pad note)*,
  never by track index, and both are ordinary recorded edits.
- `SAVE KIT` in the `*fx*` rack panel saves the rack as a kit; the browser's
  **Kits** tab lists saved kits and loading one builds a new rack beside the
  existing tracks. The sequencer header carries neither this nor a pad toggle:
  it stays the track-shaped strip described above (collapse, arm, mute/solo,
  name, meter).

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

Deleted (as shipped): the `RackRouting::ByPitch` variant and its slot matching
in dispatch, transpose-as-pad-selector, the v1 drum-rack track and pad-fill
constructors, the sidebar 4x4 pad grid and its pad-bank state, the drum-lane
projection helpers and their history actions, and the `Sound` param mode that
projected transpose as a pad name. The browser's "Add drum rack" now creates a
rack group; a legacy `by_pitch` project still deserializes and loads as a plain
layering rack.

Per-slot mix fields (`gain`/`pan`/`mute`/`solo`) and `RackSlotParamPlocks` were
**kept**: they are also the Instrument Rack's per-layer mixer and per-layer
p-locks, and "Instrument Rack changes" is a non-goal above. Only the drum-lane
UI that surfaced them for drum racks is gone. They can be revisited if and when
the Instrument Rack itself is redesigned.

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

## Kits

A **kit** is a drum rack saved as a browser object (`kits/<name>.kit`). It
carries:

- kit identity — name, color, and the pad map (each pad's note, its choke
  group, and the member track's name);
- one **Sound** per pad, captured exactly as the Sounds browser captures a
  track: the member's instrument (sampler buffer or custom engine), its
  instrument params, and its insert fx chain.

It deliberately does **not** carry patterns, and it does not carry the rack
bus's own fx chain or fader. Loading a kit therefore never overwrites anything
that is already sequenced: it creates a *new* rack group beside the existing
tracks, one empty member track per pad, and re-applies the pad map and choke
groups. Pads whose Sound fails to load (a missing sample, a missing instrument)
are reported by name; the rest of the kit still lands. Every step a load takes
is an ordinary recorded edit, so a kit load undoes like any other.

## Open questions

- ~~Should the rack header's arm and a member arm be mutually exclusive, or
  can the rack stay armed while one member is "focus-armed" for chromatic
  play?~~ Resolved: exclusive, scoped to the rack's own members (see "Arming &
  live play").
- ~~Kit-as-preset: what exactly serializes when saving a drum rack to the
  browser (member instruments + fx + pad map, but presumably not patterns?).~~
  Resolved: pad map + choke groups + kit identity + one Sound per pad; no
  patterns, no rack bus chain. See "Kits".
- Does the Instrument Rack eventually migrate onto this model (a group with
  `Broadcast` routing), or stay a slot container? Not needed for v1.
