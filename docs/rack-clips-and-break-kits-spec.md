# Rack Clips and Break Kits

**Status:** rev 4. All four phases built: §7.1 kit bus chain, §5 rack-owned
sequencers, §2–4 + §6 rack clips, §7 break kits.
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

## 3. Data model — BUILT (eseq-172r.3)

**What was built differs from the sketch below in two deliberate ways.**

1. **Runtime storage is pointers, not serialized pattern data.** A clip's member
   slice is `Vec<Option<PatternId>>` into the member track's own
   `TrackPatternPool` — exactly the shape of a scene cell. The bank lives in
   `ProjectScenes` (`sequencer/state/rack_clips.rs`: `RackClip`,
   `RackClipBank`, `ProjectScenes::rack_banks`), and `Scene` gains
   `rack_clips: Vec<(u64, RackClipId)>`. That is what makes the composition step
   one lookup and the edit redirect free: every read and write funnel already
   goes through "which pattern id does track m play", so redirecting that one
   answer redirects all of them.
2. **The wire form is one full-width `ProjectPattern` per clip**, in which only
   the rack's member tracks are meaningful, plus `members: Vec<bool>` for the
   positional presence — the same trick take chunks use, so clip content reuses
   the scene pattern conversion and load-time sample resolution unchanged.
   `Vec<SerializedTrackPatternData>` (one full-width pattern per member) would
   have multiplied the file by the member count for no gain.

Serialized as `ProjectRackConfig::{clips, next_clip_id}` (`ProjectRackClip`) and
`ProjectFile::scene_rack_clips` (per scene, `(group id, clip id)` — the file-level
home of `ProjectScene::rack_clips`, which keeps `ProjectPattern` untouched and
mirrors how `scene_cell_presence` is carried). `PROJECT_FILE_VERSION` is now 13.
Every field is `#[serde(default)]`, so a pre-v13 project loads with an empty bank
on every rack — a legacy rack (§4.3) — and behaves exactly as before.

`ProjectRackClip::bus_chain` exists on the wire (`Option<ProjectKitBusChain>`)
and round-trips, but nothing applies it yet; the §10 question about params vs
structure is still open, so launch composition deliberately does not touch the
rack bus chain.

One piece of state the sketch did not anticipate: `ProjectScenes::live_rack_clips`
records, per rack, the clip pointer the LIVE grid was installed from. Every
recorded edit in this codebase saves the live grid into the current scene before
capturing history, so re-pointing a scene without relaunching would let that
save-back clone the stale live lanes over the newly pointed clip. A member lane
whose rack has been re-pointed since the grid was installed is therefore treated
as stale and skipped, the same way `stale_mask` protects song-latched lanes.

The original sketch follows.

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
    pub sequencer_id: u64,      // namespaced: hash of "name@rack:<gid>"
    pub sequencer_name: String, // def-sequencer name as authored
    /// The Lisp form the host evaluates under the rack owner to bring the
    /// instance back on project open: `(load "content/scripts/…")` for a
    /// project script, `(import author.package)` for a package module, or
    /// the script text itself. Empty = unknown origin, does not come back.
    /// (Built as a plain form string in phase 2 rather than the enum first
    /// drafted here; the form already distinguishes the three cases.)
    pub source: String,
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

## 4. Launch composition — BUILT (eseq-172r.3)

Built as described, through one pair of helpers on `ProjectScenes`:
`composed_scene_cell(scene, track)` (the §4.1 step-2 lookup) and
`composed_graph_overrides(scene)`. `scene_snapshot`, `launch_scene`,
`launch_scene_tracks`, `effective_pattern_id`, `effective_sound_refs` and
`save_scene_snapshot_masked` all go through them, which is the whole of §4.1 and
§4.2.

One correction to §4.1: the snapshot carries a clip's overrides **unresolved**
(routes stay member indices, `owner_rack` set), exactly as scene overrides do.
Since phase 2 the scheduler resolves member → track in `reconcile_graph_runtimes`
via `resolve_rack_member_routes` and `snapshot.rack_memberships`, so resolving
them again here would double-map.

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
patch — the recorded GROUP-structure patch
(`apply_recorded_bus_group_structure_mutation`), whose `BusGroupStructureState`
captures the whole `ProjectScenes`, so the bank, the pointers and the override
rewrites undo together. Implicit migration by kit export is not built (phase 4);
"Convert to clips" in the rack menu and "Save clip as…" are.

### 4.4 Quantized launch — BUILT

The `launch-rack-clip` host command sets the pointer in the current scene (one
recorded edit) and then delegates to the ordinary `switch-pattern` handler for
the current scene index with the transport's scene-launch quantize. No new
boundary code, and every scene-switch sync runs unchanged.

Known limitation: the pointer moves immediately while the audible relaunch waits
for the boundary, so with quantize on the *edit target* and the clip run's lit
cell change ahead of the sound. Making the pointer itself boundary-scheduled
would mean new boundary code, which this phase deliberately avoids.

## 5. Rack-owned sequencers

### 5.1 Ownership (built, eseq-172r.2)

The rack's context menu (mixer strip) lists every project-owned graph
sequencer as "Move "name" into rack" and every rack-owned one as "Detach
"name"". Moving is allowed only when the manifest's default route and every
route/seed track in every scene are members of that rack; otherwise the error
names the offending nodes and nothing changes. Moving rewrites the overrides
to member indices, sets `owner_rack`, re-keys them to the namespaced id,
republishes the manifest under the rack and records a `ProjectRackSequencer`
whose source is the same `(import demos.x)` the scratch uses for a package
script (a plain file records `(load "<path>")`). Detaching reverses it
(member routes expand to tracks, id back to the project one, rack instance
unpublished). All three are recorded group-structure edits, and the
structure state captures the scene bank, so the override rewrites undo with
them.

**Ownership is a property of the module.** The app publishes a map
module name → owning rack (`set_rack_owner_modules`, rebuilt from every
rack's recorded imports on each group-topology change), and the UI
`def-sequencer` native asks the VM which module is evaluating
(`ctx.current_module()`) and looks it up. So the scratch stays the one place
scripts are imported: on project open its own `(import demos.x)` publishes
the instance as rack-owned, with no second replay and no scratch edits.
Attaching and detaching re-evaluate the `(import …)` in a fresh eval pass
(imports are load-once *per pass*, so this re-runs the module) and the map
decides who owns the result, which flips the script's tab and route dropdown
immediately. `replay_rack_sequencer_sources` only evaluates recorded sources
the scratch does not import (plain `(load …)` files). An explicit
`with_graph_owner_rack` scope still wins over the map, which is what tests
and `attach-rack-sequencer` use.

Phase 3 moves the rack-owned overrides from project scenes into rack clips;
in phase 2 they still live in the scenes, only the route space changed.

### 5.2 Namespacing (built, eseq-172r.2)

Two imported kits may both carry `neural-variable-reset-demo`. A rack-owned
instance's id is `stable_sequencer_id("name@rack:<gid>")`
(`graph_instance_id`), so each rack gets its own; the manifest carries
`owner_rack`. Override matching goes through one rule,
`GraphManifest::matches_overrides`: exact id, or name *within the same
owner*. Ids are masked to 53 bits so they survive the trip through Lisp
numbers; projects saved with wider ids still match by name.

The handle is the number `def-sequencer` returns (both the UI and lisp_host
natives), and every `graph-*` native accepts it. A bare name still resolves
when exactly one instance carries it, or, while the host is evaluating a
rack-attached script (`with_graph_owner_rack`), to that rack's own copy;
otherwise it is an error naming the candidate ids. Scripts meant to run under
a rack write `(def gvr-name (def-sequencer …))`, as the demo now does.
Duplicating one script across two racks also needs per-instance buffer and
widget keys, which is the script's business (§10).

### 5.3 Script UI (built)

`seq-register-script-step-sequencer-tab` registers one tab per instance; the
demo labels its tab with the rack name when `(graph-owner handle)` is set.
Two natives feed rack-aware scripts: `(graph-owner handle)` → owning group id
or nil, and `(graph-route-tracks handle)` → the member track index behind
each route option (nil when project-owned). Route option *n* is always route
value *n* with "Off" last, and `bind-graph … :route options` indexes by that
value rather than by label, so the demo builds pad labels and colours from
the member tracks and the rest of its route code is unchanged. The UI also
publishes `SEQ.graph-sequencers` (`{id name owner-rack}` per instance) for
UI that lists instances, such as the rack menu.

### 5.4 Route semantics (built)

Rack-owned overrides are skipped by `remap_graph_overrides_after_track_delete`.
The scheduler is the one place member routes become tracks:
`reconcile_graph_runtimes` builds the config with overrides as before and then
`resolve_rack_member_routes` maps routes and seed masks through the
`rack_memberships` the snapshot carries (mirrored into `SequencerState` by
`App::publish_rack_choke_runtime`, which every group-topology edit already
calls). The manifest's own default `:route n` is a member index too. A member
leaving the rack (detach, move, ungroup, track delete) runs
`remap_after_rack_member_removed` over every scene inside the same recorded
edit: nodes routed to it go to `None`, later members shift down.

## 6. UI — BUILT (eseq-172r.3)

Built as described, with two exceptions called out in §6.1. The bank reaches the
UI as one reactive field, `SEQ.rack-clips`
(`{group-id, active, clips: [{id name}]}` per rack, published by
`build_rack_clips_value` from `sync_pattern_state` and `sync_rack_pad_map`), and
the Lisp side reads it through `eseq.drum-rack-v2/{clip-bank, clips, active-clip,
has-clips?, launch-clip, save-clip-as, delete-clip, rename-clip,
convert-to-clips}`. A rack absent from that field is legacy, which is how the UI
decides between "Convert to clips" and the per-clip actions.

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

**Not built:** drag reorder of clip cells (the run renders from a reactive field
with no drop target; `ProjectScenes::reorder_rack_clip` exists for when it is
wired). The activity strip is built and reuses the per-track
`rack-pad-trigger-<track>` bindings the pad map already reads, so it needed no
new host feed.

### 6.2 Mixer

The collapsed mixer strip (`track-collapsed-strip` in `content/ui/mixer.lisp`)
gets the same clip run vertically where a plain track shows its pattern grid.
The expanded rack keeps member strips.

### 6.3 Rack header actions

Rack header `…` menu gains: Attach sequencer…, Detach sequencer, Convert to
clips (legacy racks only), Export as kit…, Save clip as…, Delete clip. All
built. "Delete clip <name>" is listed once per clip
rather than acting on a selection.

### 6.4 Scene list

The scene list is unchanged. A scene's row could later show a tiny per-rack
clip glyph; not in scope.

## 7. Break kits (kit v2) — BUILT (eseq-172r.4)

### 7.1 What a kit carries

```rust
pub struct ProjectKitPreset {
    // ... existing: version, metadata, color, pads, bus_chain ...
    #[serde(default = "default_kit_version")]
    pub kit_version: u32,                          // 1 = pre-break, 2 = break kit
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sequencers: Vec<ProjectRackSequencer>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub clips: Vec<ProjectRackClip>,               // PAD-positional
}
```

Four decisions differ from the sketch this section used to hold.

1. **The version bump is `kit_version`, not `version`.** A kit's `version` is
   the PROJECT file generation, and it has to stay that, because a kit clip's
   content is a `ProjectPattern` and parses by that generation. `kit_version`
   counts the kit's own payload instead, defaults to 1, and leaves
   `PROJECT_FILE_VERSION` at 13 — no project file changes meaning for a kit
   feature.
2. **No `embedded_sources`.** `ProjectRackSequencer::source` is already a Lisp
   form that distinguishes all three cases: `(import module)` for a package
   script, `(load "path")` for a plain file, and the script text itself when
   neither applies. A second, name-keyed side table would have been a
   duplicate. The export warns instead: a source that is empty, or a `(load …)`
   whose file no longer exists, is reported by sequencer name at save time.
3. **Everything positional in a kit is in PAD space**, not member space: a
   clip's `members` flags, the meaningful lanes of its `pattern`, and its
   graph-override routes and seed sets. A kit has no project to index into and
   rebuilds its members *from* pads, so pads are the only stable coordinate.
   Export maps member position → pad, import maps pad → the member track it
   just built (`app/break_kits.rs`).
4. **A kit clip's `pattern` is a compact `ProjectPattern` of exactly pad-count
   width.** Import lifts it back to full project width
   (`expand_kit_clip_pattern`) before handing it to the ordinary
   `project_pattern_into_snapshot_with_policy`, so kit clips get the same
   sample resolution, effect-slot rebinding and fallback accounting a project
   load gets, with no second conversion path.

The bus chain is carried unconditionally (phase 1). This is the one place
where "preset replaces device but keeps your effects" is the wrong instinct:
a kit's group processing is part of the kit.

### 7.2 Export

"Export as kit…" in the rack's mixer menu opens the existing kit save panel
(`content/ui/browser.lisp`), now carrying a scene checklist. The default
selection is every scene the rack actually plays, read from the per-scene
pointers `SEQ.rack-clips` publishes (`scene-clips`); a LEGACY rack has no
bank to read, so its checklist defaults to every scene and the export drops
the ones it finds empty. Unticking everything saves the old kind of kit.

Each chosen scene becomes clip 1..n, in scene order, named after the scene and
carrying that scene's rack-owned overrides. A legacy rack is converted to clips
first (`App::convert_rack_to_clips_recorded`, its own undo entry) so the export
reads one representation; a chosen scene the rack is silent in contributes no
clip, which is the rule conversion itself uses. The rack's sequencers travel as
recorded — no re-classification at export time, because the recording already
chose `(import …)` over `(load …)` when the script sat under a module root.

### 7.3 Import

Loading a break kit does what a kit load does today (new rack group beside
existing tracks, one member per pad, Sounds re-applied, failures reported by
name) and additionally restores the bus chain, registers the rack-owned
sequencers and fills the clip bank. Every existing project scene gets `None`
for the new rack, so it is silent until the user launches a clip. The whole
load is one undo entry (`load_kit_as_rack` now squashes; it used to leave one
entry per pad).

The import has two halves, because the App cannot evaluate Lisp:

- **App half** (`app/break_kits.rs`, inside the squashed load): re-derive each
  sequencer id for the NEW rack — a rack-owned instance id is
  `graph_instance_id(name, Some(gid))`, so the recorded id belongs to the
  exporting rack and is meaningless here — record the entries with
  `attach_rack_sequencer_recorded` (which republishes the module owner map),
  then install the clip bank, rewriting each clip override's `sequencer_id`
  through that old→new map and its `owner_rack` to the new group.
- **Host half** (`ui/host_commands/drum_rack_v2.rs`): evaluate each recorded
  source under the new rack (`evaluate_rack_sequencer_source`), reporting
  failures by module. A missing package is reported and the rest lands; the
  recorded entry survives, so a later re-import brings the instance back.

A kit without clips is still a valid kit; nothing about existing `.kit` files
changes meaning.

**Auditioning a break kit onto an existing rack** (`load_kit_onto_rack`, the
browser's activate-with-a-rack-selected path) replaces that rack's sequencers
and its whole clip bank, in the same single undo entry, because a kit's
processing and its clips are part of the kit — the same argument §7.1 makes for
the bus chain. The scene pointers are cleared with the old bank, so the
auditioned rack is silent until a clip is launched, exactly like a fresh
import. A kit with no clips of its own leaves the bank alone, which is what
keeps every pre-feature `.kit` file behaving as before.

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
2. **Rack-owned sequencers.** BUILT (eseq-172r.2). Ownership, member-relative
   routes, instance handles, rack-aware route dropdown. Overrides still live
   in project scenes for this phase (racks are still legacy); only the route
   space changed.
3. **Rack clips.** BUILT (eseq-172r.3). Data model, launch composition, edit
   redirect, legacy migration, collapsed-row clip launcher in sequencer and
   mixer. Rack-owned overrides moved into clips.
4. **Break kit export/import.** BUILT (eseq-172r.4). Scene picker, sequencer
   bundling, clip bank in the kit, import with silent pointers, audition
   replaces the bank.

## 10. Open questions

- Should a clip carry the rack bus chain *params* (snapshot) or only the
  kit-level chain *structure*? Rev 1 says params optional per clip, structure
  kit-level. Still open. Phase 3 reserved the wire field
  (`ProjectRackClip::bus_chain`) and applies nothing, so the answer costs no
  format change either way. Revisit once a real break kit exists.
- Do rack clips want their own arrangement lane, or does the arrangement keep
  addressing project scenes only? Rev 1: project scenes only.
- Should the clip pointer itself be boundary-scheduled on a quantized launch,
  so the edit target and the lit cell move with the sound rather than ahead of
  it? Phase 3 says no (it would mean new boundary code); revisit if the lead
  reads wrong in practice.
- Instance handles vs. name-keyed `graph-*` natives: dynamic default keeps
  old scripts working, but `reactive-set "GRAPH" (graph-key ...)` field names
  must also be instance-unique. Likely `graph-key` returns an id-prefixed
  key; confirm nothing parses those keys.
