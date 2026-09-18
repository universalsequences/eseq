# Rack Clips and Break Kits

**Status:** rev 1, design. Nothing built.
**Epic:** `bd show eseq-172r` (children .1 bus chain, .2 rack-owned sequencers,
.3 rack clips, .4 break kits).
**Depends on:** `docs/drum-rack-v2-spec.md` (rack = group with `rack: Some(_)`),
`docs/track-groups-spec.md`, `docs/neural-groups-spec.md`,
`docs/package-system-spec.md` (package tier for bundled sequencer scripts).

## 1. Motivation

Two real projects drove this:

- **garageddd** has a good bank of scenes and wants a "B" part, a jungle
  breakdown. At ~12 tracks the sequencer and mixer already feel claustrophobic,
  so adding six more tracks for the breakdown is not attractive.
- **jungle-ology** has exactly the breakdown material: a set of drum tracks
  driven by a custom graph sequencer
  (`content/scripts/sequencers/graph-neural-variable-reset-demo.lisp`) with
  group-wide processing on the rack bus.

What we want is to treat "a rack of sounds + the sequencing that makes it a
break" as one loadable object, the way a drum-break sample is one object.
Loading it should not spill six step rows and six mixer strips into the host
project, and it should still be trivially launchable from any host scene.

The naive alternative, "import project appends tracks", was rejected. It
makes the density problem worse, it needs track-index remapping for every
graph route (`route: Option<usize>` in `ProjectGraphRouteOverride` is a raw
track index today), and it leaves an unanswerable question: after importing
scenes 1..4 of the source into a host with scenes 1..7, which host scene owns
which set of graph-sequencer parameters?

## 2. Core model

A rack gains its own scene axis. Think Max `pattr` hierarchy: the project
scene is the top preset object, and each rack is a nested preset object the
project scene points into.

```
project scene 7
  ├─ plain tracks: pattern index per track (as today)
  ├─ rack "break" (group id 12): clip 3
  │     ├─ member 0 → pattern, member 1 → pattern, ...
  │     ├─ rack-owned sequencer overrides (routes, params, weights, config)
  │     └─ (optional) rack bus chain param snapshot
  └─ rack "perc" (group id 15): none  → members silent
```

Definitions:

- **Rack clip.** The rack-scoped slice of what a project scene holds today:
  one `TrackPatternData` per member (positional over `group.members`), the
  `ProjectGraphOverrides` of every sequencer the rack owns, and optionally a
  param snapshot of the rack bus fx chain. Clips are ordered in a per-rack
  bank and addressed by index in the UI, by stable id in storage.
- **Clip pointer.** Per project scene, per rack: `Option<RackClipId>`. `None`
  means the rack's members are silent in that scene. Silence is explicit, the
  same rule the arrangement uses for empty spans
  (`docs/arrangement-clips-are-explicit`), never "keep playing whatever the
  previous scene had".
- **Rack-owned sequencer.** A graph-mode `def-sequencer` instance whose owner
  is a rack rather than the project. Its node routes are **member indices**
  into the rack, not track indices. Its overrides live in rack clips, not in
  the project scene.

The scheduler and audio graph consume the same flat `PatternSnapshot` they do
today. The indirection resolves when a snapshot is built: launching project
scene 7 composes the plain-track slice from the scene with each rack's clip
slice, and rack-owned sequencer routes resolve member index → track index at
that moment. No audio-thread or scheduler change is required for the core
model.

## 3. Data model

All in `crates/sequencer/src/project.rs` unless noted. Serialization version
bumps once for the whole feature; every new field is `#[serde(default)]` so
pre-feature projects load unchanged (racks with an empty clip bank behave
exactly as today, see §4.3).

```rust
pub type RackClipId = u64;

pub struct ProjectRackConfig {
    // ... existing: pads, choke_groups ...
    #[serde(default)]
    pub clips: Vec<ProjectRackClip>,          // ordered bank
    #[serde(default)]
    pub next_clip_id: u64,
    #[serde(default)]
    pub sequencers: Vec<ProjectRackSequencer>, // rack-owned instances
}

pub struct ProjectRackClip {
    pub id: RackClipId,
    pub name: String,
    #[serde(default)]
    pub color: Option<[f32; 3]>,
    /// Positional over `group.members`. Length == members.len() is an invariant
    /// maintained by the same reconcile funnels that keep the per-track lane
    /// roster in step (see `TrackPatternData` funnels, eseq-53y7).
    pub members: Vec<SerializedTrackPatternData>,
    /// One entry per rack-owned sequencer, keyed by `sequencer_id`.
    #[serde(default)]
    pub graph_overrides: Vec<ProjectGraphOverrides>,
    /// Rack bus insert-chain param snapshot. None = leave the chain as is.
    #[serde(default)]
    pub bus_chain: Option<ProjectFxChainParamSnapshot>,
}

pub struct ProjectRackSequencer {
    pub sequencer_id: u64,      // same id space as project-owned sequencers
    pub sequencer_name: String, // def-sequencer name
    /// Where the def-sequencer source comes from. Package ids are preferred
    /// for anything that is meant to travel inside a kit.
    pub source: RackSequencerSource,
}

pub enum RackSequencerSource {
    Package { id: String },       // pkg:author.name/<path>
    ProjectScript { path: String },
    Inline { source: String },    // last resort, kit-embedded
}

pub struct ProjectScene {
    // ... existing ...
    /// Per rack, by group id. Missing entry == None == silent.
    #[serde(default)]
    pub rack_clips: Vec<(u64 /* group id */, RackClipId)>,
}
```

`ProjectGraphOverrides` gains one field:

```rust
pub struct ProjectGraphOverrides {
    // ... existing ...
    /// None = project-owned (routes are track indices, stored in the scene).
    /// Some(gid) = owned by rack `gid` (routes are member indices, stored in
    /// that rack's clips).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_rack: Option<u64>,
}
```

Invariants:

- A rack clip's `members` is exactly as long as `group.members`. Joining a
  member appends a default entry to every clip; leaving removes it from every
  clip. This piggybacks on the existing rack join/leave funnels.
- A rack-owned sequencer's route override, when `Some(i)`, satisfies
  `i < group.members.len()`. Routes outside the rack are not representable;
  that is the point.
- A project scene's `rack_clips` entry references a clip that exists in that
  rack's bank. Deleting a clip clears every pointer to it (those scenes fall
  back to `None`, silent).
- Deleting a rack deletes its clips, its sequencer instances and every pointer
  to them. Track reindex-on-delete touches nothing here because everything is
  by group id and member position.

## 4. Launch composition

### 4.1 Snapshot build

`SequencerState::scene_snapshot` (`sequencer/state/scenes.rs`) and
`launch_scene` / `launch_scene_tracks` compose in this order:

1. Plain tracks and the scene's own `graph_overrides` (project-owned
   sequencers only) exactly as today.
2. For each rack group, look up the scene's clip pointer:
   - `Some(clip)`: for each member `m` at position `p`, install
     `clip.members[p]` as track `m`'s pattern data. Append the clip's
     `graph_overrides` to the snapshot with every route resolved
     `member p → group.members[p]`. Apply `bus_chain` if present.
   - `None`: install an empty pattern for every member and register no
     overrides for the rack's sequencers, so they do not fire.
3. Publish.

Resolution of member routes happens here and only here. The runtime
(`runtime/graph.rs`) keeps seeing track indices; it never learns about racks.

### 4.2 Editing while a clip is active

Every edit that today lands in "the current scene's slice for track m" is
redirected when `m` is a rack member: it lands in the rack's active clip for
the current scene. The existing `TrackPatternData` write funnels get one
lookup: `track → (rack gid, clip id) | plain`. Step edits, p-locks, lane
roster changes, rack pad edits and graph-override edits from the script UI all
go through those funnels, so they redirect for free once the funnel does.

If the current scene points at `None` for the rack and the user edits a
member, the edit creates a new clip, points the scene at it, and proceeds.
Editing silence should never be a dead end.

### 4.3 Migration of existing projects

A rack whose `clips` is empty is a **legacy rack**: its members' data stays
in the project scenes as today and the composition step is skipped. The first
time a legacy rack is given a clip (explicitly via "convert to clips" in the
rack header, or implicitly by a kit export), migration runs once: for every
project scene, the rack's member slices and the overrides of any sequencer
being attached are moved into a new clip named after the scene, and the scene
gets a pointer to it. Scenes whose member slices are all empty share one
`None` pointer rather than producing empty clips. Migration is one undoable
`SceneStructurePatch`.

### 4.4 Quantized launch

Clip launches from the rack row (§6) are scene edits (`set rack pointer in
current scene`) followed by a relaunch of the current scene, so they ride the
existing quantized-launch path (`quantized_launch.rs`) and the rack
scene-swap logic (`docs/rack-scene-swap-spec.md`). No new boundary code.

## 5. Rack-owned sequencers

### 5.1 Ownership

A sequencer is attached to a rack from the rack header ("Attach sequencer…",
choosing a package or script) or converted from a project-owned instance
("Move into rack"). Conversion is allowed only when every route of the
instance already targets a member of that rack; otherwise the action explains
which nodes route outside and refuses. No guessing.

Attachment allocates a fresh `sequencer_id`, writes a `ProjectRackSequencer`,
and moves the instance's overrides into the rack's clips (migrating per §4.3
if the rack is legacy). Detaching reverses it with member routes expanded to
track indices.

### 5.2 Namespacing

Two imported kits may both carry `neural-variable-reset-demo`. Instances are
keyed by `sequencer_id`, not name, everywhere it matters already
(`ProjectGraphOverrides.sequencer_id`). What is name-keyed today is the Lisp
surface: `bind-graph`, `graph-node`, `graph-param`, `graph-config` and
`graph-key` take `gvr-name`. Add an instance handle: the script's
`def-sequencer` evaluates to an instance object when loaded under a rack
owner, and the `graph-*` natives accept either a name (project-owned, as
today) or a handle. The loader binds `*graph-instance*` for rack-loaded
scripts so unmodified demo scripts keep working via a dynamic default.

### 5.3 Script UI

`seq-register-script-step-sequencer-tab` registers one tab per instance. The
tab label is the rack name when the sequencer is rack-owned. The route
dropdown of a rack-owned instance lists the rack's pads (pad name, colored by
member track) plus "Off". `gvr-route-options` in the demo hardcodes
"Track 1".."Track 16"; the rack-aware version should read the option list from
a native (`graph-route-options handle`) so scripts do not have to know which
mode they are in.

### 5.4 Route semantics

Member-relative routes make `track_delete_remap.rs` a no-op for rack-owned
instances. A member leaving the rack is the only structural event: nodes
routed to it are set to `:off`, reported in the status line, and the change
is part of the same undo entry as the leave.

## 6. UI

### 6.1 Collapsed rack row in the sequencer

Today a collapsed rack (`eseq.drum-rack-v2/collapsed?` in
`content/ui/sequencer.lisp`, `group-block`) renders only the header row.
Collapsed becomes useful: the row shows the rack's clip bank as a horizontal
run of clip cells, one per clip, plus a `+` cell. The cell for the clip the
current scene points at is lit; `None` shows no lit cell. Clicking a cell
sets the pointer in the current scene (quantized like a scene launch);
Shift-click renames; drag reorders. The right-hand third of the row shows a
compact activity strip for the rack (per-member trigger dots reusing the
trigger-matrix data the demo script already renders).

Opening the rack still reveals member rows for editing. Nothing about the
expanded view changes except the header now also shows the clip run.

### 6.2 Mixer

The collapsed mixer strip (`track-collapsed-strip` in `content/ui/mixer.lisp`)
gets the same clip run vertically where a plain track shows its pattern grid.
The expanded rack keeps member strips.

### 6.3 Rack header actions

Rack header `…` menu gains: Attach sequencer…, Detach sequencer, Convert to
clips (legacy racks only), Export as kit…, Save clip as…, Delete clip.

### 6.4 Scene list

The scene list is unchanged. A scene's row could later show a tiny per-rack
clip glyph; not in scope.

## 7. Break kits (kit v2)

### 7.1 What a kit carries

`ProjectKitPreset` (`project.rs`) version bumps and gains:

```rust
pub struct ProjectKitPreset {
    // ... existing: version, metadata, color, pads ...
    #[serde(default)]
    pub bus_chain: Option<ProjectFxChainPreset>,   // the rack bus insert chain
    #[serde(default)]
    pub sequencers: Vec<ProjectRackSequencer>,     // package ids preferred
    #[serde(default)]
    pub embedded_sources: Vec<(String, String)>,   // name → source, for Inline
    #[serde(default)]
    pub clips: Vec<ProjectRackClip>,               // member-positional over pads
}
```

The bus chain is carried unconditionally from now on. This is the one place
where "preset replaces device but keeps your effects" is the wrong instinct:
a kit's group processing is part of the kit.

### 7.2 Export

"Export as kit…" opens the existing kit save modal extended with a scene
picker: a checklist of project scenes (default: all scenes where the rack's
pointer is not `None`, or for legacy racks, all scenes where any member has
steps). Each chosen scene becomes clip 1..n in the kit, named after the
scene. Rack-owned sequencers are bundled by package id when the source is a
package, by path when it is a project script that lives under `content/`, and
inlined otherwise, with a warning that inline sources do not update.

### 7.3 Import

Loading a break kit does what a kit load does today (new rack group beside
existing tracks, one member per pad, Sounds re-applied) and additionally:
restores the bus chain, registers the rack-owned sequencers, and fills the
clip bank. Every existing project scene gets `None` for the new rack, so it is
silent until the user launches a clip. The whole load is one undo entry, as
today.

A kit without clips is still a valid kit; nothing about existing `.kit` files
changes meaning.

### 7.4 Packages

A kit that references package sequencers should be exportable as a package
itself (`eseqpack` already handles instruments/effects/presets). Add a `kits/`
entry kind. Out of scope for the first cut but the id scheme should not
preclude it.

## 8. Implementation seams

- `project.rs`: `ProjectRackConfig`, `ProjectScene`, `ProjectKitPreset`,
  `ProjectGraphOverrides.owner_rack`, version bump, load repair for
  invariants in §3.
- `sequencer/state/scenes.rs`: `scene_snapshot`, `launch_scene`,
  `launch_scene_tracks` gain the composition step (§4.1).
- `sequencer/state/pattern_snapshot.rs`: `graph_overrides` route resolution
  helper; the `remap_graph_overrides_after_track_delete` call skips
  rack-owned entries.
- `TrackPatternData` write funnels (the per-track lane roster funnels from
  eseq-53y7 are the model): add the `track → clip` redirect (§4.2).
- `runtime/graph.rs`: no runtime change; `ProjectGraphNodeIntrinsicOverride
  .route` stays `Option<usize>`.
- `lisp_host`: `graph-*` natives accept instance handles; `graph-route-options`
  native; loader binds the dynamic default instance for rack-loaded scripts.
- `content/ui/sequencer.lisp` `group-block`, `content/ui/mixer.lisp`
  `track-collapsed-strip`: clip run widgets.
- `app/effects.rs` kit save/load/audition paths (`a_drum_rack_round_trips_
  through_a_saved_kit` is the existing round-trip test to extend).
- `package_export.rs` / `package_install.rs`: kit entry kind (§7.4, later).

## 9. Phasing

Each phase is independently shippable and independently valuable.

1. **Kit bus chain.** Kits carry and restore the rack bus insert chain.
2. **Rack-owned sequencers.** Ownership, member-relative routes, instance
   handles, rack-aware route dropdown. Overrides still live in project scenes
   for this phase (racks are still legacy); only the route space changes.
   jungle-ology benefits immediately.
3. **Rack clips.** Data model, launch composition, edit redirect, legacy
   migration, collapsed-row clip launcher in sequencer and mixer. Rack-owned
   overrides move into clips here.
4. **Break kit export/import.** Scene picker, sequencer bundling, clip bank
   in the kit, import with silent pointers.

## 10. Open questions

- Should a clip carry the rack bus chain *params* (snapshot) or only the
  kit-level chain *structure*? Rev 1 says params optional per clip, structure
  kit-level. Revisit once a real break kit exists.
- Do rack clips want their own arrangement lane, or does the arrangement keep
  addressing project scenes only? Rev 1: project scenes only.
- Instance handles vs. name-keyed `graph-*` natives: dynamic default keeps
  old scripts working, but `reactive-set "GRAPH" (graph-key ...)` field names
  must also be instance-unique. Likely `graph-key` returns an id-prefixed
  key; confirm nothing parses those keys.
