# Arrangement Rack Lanes

Rev 1 — 2026-10-10. Status: proposed, unbuilt. Epic: eseq-qkv0.

Answers the open question in `docs/rack-clips-and-break-kits-spec.md` §10
("Do rack clips want their own arrangement lane?"). Rev 1 of that spec said
"project scenes only"; this spec reverses it.

## 1. Motivation

The primary way music gets made in eseq now is: build racks with clip banks,
then press Record in the arrangement view and perform scene changes (and rack
clip launches) live. That performance must land in the arrangement and play
back exactly as heard.

Today it does not, for any project whose racks have a clip bank:

1. **Nothing is drawn for rack members.** A clip-bearing rack's member scene
   cells are moved into its clips and cleared (`rack_clips.rs`, adopt path),
   so `SongCompileContext::song_scene_cell` (`state/arrangement.rs`), which
   reads raw `scene.cells`, answers `None` for every member. Every arrangement
   path asks that question: "Place Scene Patterns" (`stamp_scene_clips`),
   capture commit (`song_capture.rs::captured_lane_resolution`), the legacy
   backdrop migration, row lowering. Rack lanes come out empty, and
   `compile_arrangement` turns empty lanes into explicit silence.
2. **Rack-owned sequencer state is dropped.** Even with member clips drawn,
   `stage_runtime_song_rows` (`sequencer_state/song_playback.rs`) takes
   `graph_overrides` and `scene_slots` from the project scene only, never from
   the rack clip the scene points at. Neural/jaki instances owned by a rack
   play the wrong (or no) state in arranged playback.
3. **Rack clip launches are not captured as themselves.** `launch-rack-clip`
   repoints the current scene's rack-clip pointer and relaunches the scene.
   Capture records a scene relaunch, and at commit every captured use of that
   scene resolves through whatever the pointer is at Stop, not what was heard.

Patching `song_scene_cell` to `composed_scene_cell` fixes (1) only, by
spreading a rack clip into N unrelated member clips. That loses the clip as a
unit, cannot carry (2), and does not address (3). The arrangement needs to
address rack clips directly.

## 2. Model

The stored arrangement (`ProjectArrangement`) gains one lane per
**clip-bearing rack**, beside the scene lane and the track lanes:

```rust
pub struct ProjectArrangement {
    pub scene_lane: Vec<SceneEvent>,
    pub track_lanes: Vec<Vec<ArrClip>>,
    /// Keyed by stable group id. Present only for racks with a clip bank.
    pub rack_lanes: BTreeMap<u64, Vec<RackArrClip>>,
    // ...
}

pub struct RackArrClip {
    pub id: ClipId,                 // same allocator as ArrClip
    pub start_beat: f64,
    pub end_beat: f64,
    /// None = the rack is explicitly silent over this span.
    pub rack_clip: Option<RackClipId>,
    /// Beat at which every member pattern was at step 0 (the phase anchor).
    /// 0.0 = free-running against the global clock, which is what stamping
    /// does today. Capture writes the audible launch's anchor.
    pub anchor_beat: f64,
}
```

A rack-lane clip is the rack-scoped analogue of a scene span: over its span
the rack plays rack clip `rack_clip` in full: every member's pattern, the
clip's `graph_overrides`, and its `scene_slots` (instance documents).

Lanes stay sorted and non-overlapping, validated like track lanes. A rack
clip id that no longer exists in the rack's bank fails validation the same way
a missing pattern id does.

**Legacy racks** (empty clip bank, §4.3 of the rack-clips spec) get no rack
lane. Their members' data still lives in project scenes, so their track lanes
work today and keep working unchanged.

### 2.1 Member lanes under a rack lane

Rack members keep their track lanes. Precedence per member, per beat:

1. A clip on the member's own track lane wins (a deliberate per-member
   override: a take, or a pattern placed by hand).
2. Otherwise the covering rack-lane clip supplies the member's cell from the
   rack clip, phased from `anchor_beat`.
3. Otherwise the member is silent. This is the existing "explicit clips"
   rule (`docs/arrangement-clips-are-explicit`).

This mirrors how a take on one track overrides the scene today, and keeps
member-level recording (armed takes on a pad) working inside a rack.

**Decision for the user (D1):** keep member lanes as overrides (above), or
make rack members lane-less so the rack lane is the only authority. Proposed:
keep them. Takes on pads need them, and they cost nothing when empty.

## 3. Compile

`compile_arrangement` adds every rack-lane clip edge to the boundary set and,
per row, resolves each member track by the §2.1 precedence. A member resolved
from a rack-lane clip emits an ordinary per-track override (the clip's member
cell, phase-stamped from `anchor_beat` with `advanced_pattern_offset`), so
the runtime row model and the scheduler stay rack-unaware for patterns.

Rows also need the rack's sequencer state. `ProjectSongRow` gains

```rust
/// Per clip-bearing rack: the rack clip governing this row (None = silent).
pub rack_clips: Vec<(u64, Option<RackClipId>)>,
```

and `stage_runtime_song_rows` composes `graph_overrides` / `scene_slots` the
way launch does (`ProjectScenes::composed_graph_overrides` /
`composed_scene_slots`), with the row's `rack_clips` in place of the scene's
pointers. Factor the composition so launch and arranged playback share one
function taking an explicit pointer set. That keeps fix (2) honest by
construction.

## 4. Capture

Capture already observes every launch at `App::apply_pattern_launch`.

- **Scene launch at audible beat B**: besides the scene event, write one
  rack-lane clip per clip-bearing rack: `rack_clip` = the scene's pointer
  **resolved at launch time** (not at Stop), `anchor_beat` = the anchor the
  launch actually used (record clock origin today, see
  `captured_lane_resolution`). Member track lanes are NOT written for rack
  members; the rack lane carries them.
- **Rack clip launch**: stop modelling it as "repoint the scene, relaunch the
  scene" for capture purposes. Capture records a `CaptureLaunchKind::RackClip
  { group_id, clip }` event that only opens a new rack-lane clip for that rack.
  The scene pointer side effect stays for live (non-recording) use; see D2.
- Consolidation (spec 10.4) treats each rack lane like a track lane: adjacent
  identical states collapse, the punch region `[P, Q)` occludes existing
  rack-lane clips with the same trimming rule (`occlude_span`).

**Decision for the user (D2):** should a rack clip launch keep mutating the
current scene's pointer at all? It is what makes the launch audible today,
but it silently edits the scene. Proposed for this spec: leave live behavior
alone and capture from the launch event, so recordings are correct either way.
Revisit with `eseq-d398` (rack clip follow-ups).

## 5. Editing

- **Place Scene Patterns** (`arr_scene_patterns_place` → `stamp_scene_clips`)
  stamps rack-lane clips (pointer from the scene, `anchor_beat` 0.0) for
  clip-bearing racks and track clips for everything else.
- **Set/Change Scene** keep editing only the scene lane.
- Rack-lane clips get the region ops track clips have: move, trim, split,
  delete, duplicate, and "change clip" (a menu listing the rack's bank, like
  Change Scene).
- `append_scene_stamped_track_lane` (new track) is unaffected. A rack that
  gains a clip bank (first clip created) gets an empty rack lane, and a rack
  that is deleted drops its lane (recorded, undoable).

## 6. UI

The arrangement timeline shows one **rack row** per clip-bearing rack, in
track order at the position of its first member, with the members collapsed
beneath it (expandable, reusing the sequencer's collapsed rack row idiom).
Rack-lane clips are labelled with the rack clip's name and colored with its
color.

Rack-lane clips must not look like pattern clips. They have no pattern data of
their own (the members do), so drawing a mini step strip would be a lie. Take
Logic's Drum Machine Designer cells as the reference: a solid fill in the clip
color with a distinct **ring summary**, one concentric ring per member and a
tick per hit, read from the rack clip's member cells. It is a glyph of the
clip, not an editable view, and it repeats per loop cycle across a long span.
Keep it cheap: draw it from a cached per-clip summary, not per frame. A member row shows its own override clips; when collapsed, a small
marker on the rack row flags spans where a member override exists.

## 7. Serialization and migration

- `rack_lanes` serializes with the arrangement, skipped when empty. Bump the
  project version. Older files load with no rack lanes.
- Existing arrangements are left as they are. A project that already has
  hand-placed member clips keeps them, and they win per §2.1. Nothing is
  auto-converted. A one-shot "Convert rack member clips to rack lane" command
  can follow if wanted.
- Kits do not carry arrangements; nothing changes there.

## 8. Phasing

1. **Model + compile + playback** (§2, §3): `rack_lanes`, validation, compile,
   `ProjectSongRow::rack_clips`, shared composition in staging. Tests:
   compiled rows carry member overrides and rack graph overrides. A
   rack-owned instance's document and overrides come from the rack clip in
   arranged playback.
2. **Capture** (§4): scene launches and rack clip launches write rack lanes,
   pointer resolved at launch time. Test: record scene A, change rack clip,
   record scene A again, Stop. The two spans play their own rack clips.
3. **Place Scene Patterns + editing** (§5).
4. **UI** (§6).
5. Optional: convert existing member clips to rack-lane clips.

Phases 1–2 are the user-visible fix for performance recording; 3 restores the
context menu.

## 9. Open questions

- D1 and D2 above.
- `anchor_beat` vs. per-member offsets: one anchor assumes all members were
  phase-aligned at launch, which holds for scene and rack clip launches. A
  member pattern launched alone mid-performance is captured as a member
  override clip instead.
- Rack bus chain snapshots (`ProjectRackClip::bus_chain`) are still unapplied;
  when they land they ride the same composition function.
