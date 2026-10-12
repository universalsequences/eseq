# Arrangement Rack Lanes

Rev 5 — 2026-10-11. Status: phases 1–4 BUILT (§2–6), region ops over rack lanes BUILT (eseq-qkv0.7). Epic: eseq-qkv0.

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
    pub rack_clip: RackClipId,
    /// Beat at which every member pattern was at step 0 (the phase anchor).
    /// 0.0 = free-running against the global clock, which is what stamping
    /// does today. Capture writes the audible launch's anchor.
    pub anchor_beat: f64,
}
```

A rack-lane clip is the rack-scoped analogue of a scene span: over its span
the rack plays rack clip `rack_clip` in full: every member's pattern, the
clip's `graph_overrides`, and its `scene_slots` (instance documents).

Lanes stay sorted and non-overlapping, validated like track lanes, and share
the track lanes' clip-id allocator. As on a track lane, silence is the
absence of a clip: a rack lane with no clip over a beat leaves its members
silent there (unless their own lanes say otherwise, §2.1).

A rack clip id that no longer exists in the rack's bank is **not** a
validation error (rev 2). Deleting a rack clip does not edit the arrangement,
and its undo step does not cover the arrangement, so the span plays silence
until undo restores the clip. Save drops dangling references
(`arrangement_for_serialization`), because a reload may reissue the id to an
unrelated clip. A lane whose rack has no clip bank (deleted, or legacy) is
ignored by compile.

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
/// Per rack lane: the rack clip governing this row (None = no clip, silent).
pub rack_clips: Vec<(u64, Option<RackClipId>)>,
```

A clip-bearing rack with no lane is absent from `rack_clips` and composes
through the row scene's own pointer, the way a launch does. That also fixes
rack-owned sequencer state for arrangements made before rack lanes existed.

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
- **Rack clip launch**: needs no event kind of its own (rev 2). With D2
  decided as "leave live behavior", a rack clip launch repoints the scene
  and relaunches it, so capture sees a scene launch whose rack pointers,
  read when the launch becomes audible, already name the new clip. That is
  also faithful to what was heard: the relaunch claims every lane live.
  A `CaptureLaunchKind::RackClip` that claims only the rack becomes
  necessary only if `eseq-d398` changes the live launch to stop
  relaunching the scene.
- Consolidation (spec 10.4) treats each rack lane like a track lane: adjacent
  identical states collapse, the punch region `[P, Q)` occludes existing
  rack-lane clips with the same trimming rule (`occlude_rack_span`; no
  restamp, since `anchor_beat` is absolute). A rack the take never claimed
  (no scene launch) keeps its lane untouched. Built in `app/song_capture.rs`
  (`splice_captured_rack_lane`).

**D2 (decided 2026-10-11: leave live behavior):** should a rack clip launch keep mutating the
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
- **Built (rev 4, eseq-qkv0.3):**
  - **Place Scene Patterns:** `stamp_scene_clips` stamps one rack-lane clip per scene span from the
    scene's pointer (`SongCompileContext::song_scene_rack_clips`), anchored
    at 0. It clears the members' own lanes over the span and never stamps
    them, since a member clip would override the rack lane.
  - **Edit ops:** move (the anchor moves with the clip), resize (the anchor
    stays), split, duplicate (the copy continues the phase), change clip,
    delete, and place (one loop, anchored at its start) are `App::arr_rack_clip_*`, lowered from
    `:rack-clip-*` arrangement actions.
  - **Rack row gestures:** the rack row has its own selection and ghost
    state (`arr-rack`, lane key `-2 - gid`), a title-bar drag, edge drags,
    Backspace, and a context menu (Change Clip, Split Here, Duplicate,
    Delete; Insert Clip Here on empty lane).
  - **Song end:** `arr_set_end` counts rack-lane clip ends.
- **Built (rev 5, eseq-qkv0.7): region ops.**
  - **Rows:** a region's ends are rows, and a rack row sits just above its
    rack's first member. `SongRegionSelection` stores this as `rack_a` and
    `rack_b`: the end is the rack row above `track_a`/`track_b`
    (`RegionRow`). The view keys rows as one number: `2i+1` for track `i`,
    `2m` for the rack row above first member `m`. A marquee steps through
    track rows and rack rows alike. `select-region!` takes `:rack-a` and
    `:rack-b`, and `song.region.racks` lists the racks whose rows it covers.
  - **Ops:** copy, paste, move, delete and duplicate carry the rack lane of
    every covered rack row (`RackClipboardSpan`). The anchor is copied
    relative to the region start, so pasted, moved and duplicated music
    keeps its phase. A paste skips a rack whose bank is gone, and rack clips
    deleted since the copy.
  - **Collapsed rack:** its row stands for its hidden members. They are in
    the region exactly when the row is.
  - **Selection:** clicking a rack-lane clip selects it as a one-clip region
    on its rack row. Dragging it inside a larger region moves the region.
    Cmd+A spans every row over all track and rack clips, and Backspace then
    clears both.
  - **Member items:** Backspace on a rack member item, or "Delete Rack Clip"
    in its menu, deletes the rack-lane clip it shows.
  - **Lane state:** each rack lane draws its own ghost and region highlight
    from `arr-rack-lane` (keyed by group id).
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

**Built (rev 2, eseq-qkv0.4):** the host pushes `group.arr-clips` (one dict
per rack-lane clip: span, rack clip name/color, `rings`, `cycle`, `phase`),
re-derived only when the song, scenes, pattern content or groups move
(`sync_rack_lanes`). The timeline widget gained a `:rings` item content
(`TimelineItemContent::Rings`) drawn with filled discs: a guide disc per
ring with the body color inside it, and a tick disc per hit, clockwise from
twelve o'clock. A glyph sits at the item start and at each loop boundary
with room for it. The rack row sits above the rack's first member; its
disclosure is `g.collapsed`, and the arrangement's visible-track order
drops a collapsed rack's members. The rack row is read-only until §5
editing lands. Not built yet: the collapsed-row marker for member overrides
(eseq-qkv0.5).

**Member rows (rev 3):** an expanded member row draws its pattern in each
rack-lane clip's rack clip over that clip's span, phased from the clip's
anchor. The member's own lane clips draw over them, matching the §2.1
precedence. A member item carries a string id, `"rack-<cid>-<track>"`, that
names no stored clip, so it cannot be moved, resized or deleted: the rack
row owns its span. A click binds the member's pattern as the track's edit
focus (`App::select_rack_member_pattern`, a `SongClipSelection` naming the
rack-lane clip). Edits then change the pool pattern, so every span playing
that rack clip changes too. A double-click also opens the piano roll. The
lane's context menu over one offers "Edit Pattern N" and "Make Own Clip",
which copies the item onto the member's lane as a pattern clip at the same
phase: an override. Each `arr-clips` dict carries
`members`, one `(dict :track :pid :num-steps :length :events :phase)` per
member with a pattern. Without them, an expanded rack showed empty member
rows while the members played.

**Live capture (rev 3):** while an arrangement capture runs, the rack row
and its member rows grow under the playhead the way the track lanes'
provisional launches do. `PendingCapture::rack_events` lists, per captured
scene launch (and, for a whole-song capture, the starting state), each rack's
pointer read at launch time. The host pushes them as `group.pending-arr-clips`
(rebuilt with the rest of the pending surface), and each provisional clip
runs to the next one's start or to `song.pending-head`. Like all
provisional content, these items carry no id and cannot be edited.

## 7. Serialization and migration

- `rack_lanes` serializes with the arrangement, skipped when empty. Project
  version 16. Older files load with no rack lanes.
- Load order: the arrangement commits before the groups and rack clip banks
  install, so the loader holds `rack_lanes` back and recommits the
  arrangement with them once `install_rack_clips` has run.
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
