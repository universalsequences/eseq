# Arrangement Region Editing — Clip Hit Regions, Region Selection, Copy/Paste/Duplicate, Move

Status: rev 3, 2026-07-27 — **all four slices shipped.** Slice 4 (move)
landed on `arrangement-timeline`: `song_region_move` in `song_region.rs`, the
`region-move` lowering case, the `:track-move` / `:region-move` ghosts in
`arrangement.lisp`, and `:move-snap-mode :alignment-helper` on the track
lanes. Rev 2 (2026-07-26) restated the spec over the lane model; rev 1
(2026-07-24) was written against the ROW
model; the arrangement has since been rewritten onto lanes and clips
(`docs/arrangement-lane-model-spec.md`, all 6 phases landed), and slices 1-3
shipped in both worlds — first as row surgery, then ported. This revision
restates §2, §5, §6, §7 and §8 in lane-model terms so the remaining slice can
be built from this document without cross-reading the row-model history.

Related: `docs/arrangement-lane-model-spec.md` (§6 model, §8 primitives,
§12 read surfaces — **authoritative** where this spec and it disagree),
`docs/arrangement-timeline-ui-spec.md` (§9, §11 items 2/4),
`docs/takes-and-additive-arrangement-recording-spec.md` (§7.4),
`crates/eseqlisp/src/widget_render/timeline.rs`,
`content/ui/arrangement.lisp`,
`crates/sequencer/src/ui/arrangement_actions.rs`,
`crates/sequencer/src/app/arr_edit.rs`,
`crates/sequencer/src/app/song_region.rs`,
`crates/sequencer/src/sequencer/arrangement.rs`,
`crates/sequencer/src/app/take_edit.rs`

## 1. Summary

Four slices, in dependency order:

1. **Ableton clip anatomy** — every clip gets a title bar (move/resize zone,
   hover cursors) and a body (region-selection surface); notes render with
   real durations instead of 3px dots. **(shipped)**
2. **Region selection** — click-drag on clip bodies / background selects a
   time × track rectangle, across multiple tracks, quantized to the
   zoom-adaptive grid. **(shipped)**
3. **Copy / paste / duplicate** — region → clipboard → paste at the
   per-track cursor; Cmd-D duplicates the region in place, rippling what
   follows right. One undo entry each. **(shipped, then ported to clips)**
4. **Move** — drag a clip by its title bar; if the clip is inside the
   active region, the whole region moves in unison. **(shipped)**

Everything lowers to arrangement mutations through the lane-model primitives
(`arr_edit.rs`, lane spec §8) or one-commit region primitives
(`song_region.rs`); no parallel mutation path.

## 2. Current facts (re-verified against the lane model, 2026-07-26)

**Widget (shipped in slice 1, unchanged by the lane rewrite — the widget
never knew about rows or clips, only items):**

- Hit regions now include `ItemTitleBar` and `ItemEdgeStart` alongside
  `ItemBody`/`ItemEdgeEnd`, gated on the `:title-bar-height` prop (`0` =
  piano-roll behavior). Containment wins over edge slop, so abutting clips
  hit-test deterministically (§3.1, commit `bbd2576b`).
- `WidgetCursor::Move` exists and is returned for `ItemTitleBar`.
- Pointer + `ItemTitleBar` begins the `:move` gesture; pointer + `ItemBody`
  (title bar active) or `Background` begins `:marquee`. Marquee emits
  `:marquee-select` per frame and `:finish-marquee-select` on release, both
  carrying the unclamped `row-delta` (§4.2); `:marquee-snap :grid` quantizes
  the emitted times to the zoom-adaptive ladder.
- **Drag capture is per-instance** (`captures_drag`): once a drag starts in
  one lane instance, other lanes never see events. A cross-track marquee is
  reconstructed host-side, and the same will hold for a cross-lane move ghost.
- `:selection-rect` + `:selection-rect-style :region` render the region
  highlight per lane; the move gesture still emits `:move-items-absolute`
  live and `:finish-move-items` on release.
- Notes carry real durations end to end: `flatten_pattern_events` publishes
  `(time transpose velocity duration)`, `arrangement-windowed-dots` emits
  `:width`, `TimelineDot` draws a bar.
- Each arrangement track lane is one single-lane widget instance; the track
  index lives only in the `:on-action` closure (`arrangement-track-action i
  event`, from `(each (seq-visible-track-indices) |i| …)`).

**Model (post-lane-rewrite — this is what rev 1 got wrong):**

- There is no row surgery any more. `ProjectSong` rows are a *compiled*
  playback artifact; the stored, edited object is `ProjectArrangement`
  (`sequencer/arrangement.rs`): `scene_lane: Vec<SceneEvent>` +
  `track_lanes: Vec<Vec<ArrClip>>`, each `ArrClip` a first-class object with
  a `ClipId`, a half-open span, a mandatory source, and `offset_steps`.
- `song_edit.rs`'s `song-track-paint` / `song_track_paint_anchored` /
  `paint_take_region` and the `paint_source_region` helper rev 1 planned to
  extract **are gone**. Their replacements are the lane primitives in
  `arr_edit.rs` (lane spec §8): `arr_clip_create/delete/clear_span/move/
  resize/split/set_source`, `arr_scene_event_insert/set/move/remove`,
  `arr_set_end/loop`, `arr_replace/clear`. Every one commits a single
  `EditPatch::Arrangement(ArrangementStructurePatch)` (whole-object memento)
  and ends with validate → recompile → `set_committed_song`.
- The shared span surgery lives in `sequencer/arrangement.rs` as free
  functions the primitives and the region ops both call:
  `occlude_span(arrangement, scenes, track, start, end)` (remove/trim/split
  whatever a write lands on), `insert_clip_sorted`, and
  `restamped_clip(scenes, track, clip, beat)` (re-anchor a clip to a cut,
  re-stamping `offset_steps` by the split rule; `None` when nothing is left
  to play, e.g. past a take's end).
- **Silence is the absence of a clip** (lane spec §6.2). There is no
  explicit-empty override and no scene backdrop: clearing a span stores
  nothing. Every clipboard/region op below is stated in those terms.
- Region state and the clipboard live in `app/song_region.rs`, already
  clip-native: `SongRegionSelection { track_a, track_b, start_beat, end_beat,
  scene_lane }`, `ArrangementClipboard`, and
  `song_region_copy/paste/duplicate/delete`. `song_region_move` **does not
  exist** — it is the one primitive slice 4 still needs.
- Gestures address real model objects (lane spec §12): a scene-lane item's
  id IS its scene event's start beat; a track-lane item's id is its
  `clip-id`. `arrangement_actions.rs` lowers `clip-resize` →
  `arrangement-clip-resize` and **already lowers `clip-move` →
  `arrangement-clip-move`** (naming track + clip-id + start-beat). What is
  missing is the Lisp side: `arrangement-track-action`'s
  `:finish-move-items` arm still just clears the ghost
  (`arrangement.lisp:891-893`).
- Start-edge resize **shipped with the lane model**: both edges lower to the
  one `arr_clip_resize` primitive (`arrangement-track-resize-start-finish` /
  `-end-finish`), which re-stamps `offset_steps` on a left trim and clamps
  takes on the right. Rev 1's §6 bullet planning it is obsolete.
- No generic multi-command undo transaction. One-entry compound commits are
  done by mutating a cloned model then a single `history.commit` —
  `EditPatch::Arrangement`, or `EditPatch::Composite([SceneStructure,
  Arrangement])` when takes were cloned (`commit_region_edit`,
  `song_region.rs:662-723`).
- Zoom-adaptive snap: `:resize-snap :grid` / `:marquee-snap :grid` →
  `TimeViewport::grid_step` ladder (`time_view.rs`); track lanes carry the
  same `:time-ruler` as the scene lane so they pick the bars ladder.

## 3. Slice 1 — clip anatomy (widget) — SHIPPED

Widget-only; the lane rewrite did not touch it. Kept as the record of what
the props mean.

### 3.1 Title bar prop

New optional prop `:title-bar-height` (cells, default `0`). `0` reproduces
today's behavior byte-for-byte — piano-roll is unaffected, same
compatibility pattern as `:kind`/`:content`.

With `title-bar-height > 0`, an item's rect splits at
`item.top + title_bar_height`:

| zone | hit region | pointer gesture | cursor |
|---|---|---|---|
| bar, start-edge handle | `ItemEdgeStart { item }` (new) | `:resize-start` gesture (new) | `EwResize` |
| bar, end-edge handle | `ItemEdgeEnd { item }` | `:resize-end` (existing) | `EwResize` |
| bar, middle | `ItemTitleBar { item }` (new) | `:move` (existing gesture, moved here) | `Move` (new variant) |
| body below bar | `ItemBody { item }` | `:marquee` (region select, §4) | `Default` |

Changes:

- `HitRegion` gains `ItemTitleBar` and `ItemEdgeStart` (`timeline.rs:155`).
  `hit_test` (`:1714-1732`): when the item has a title bar and the pointer
  row is within it, apply the existing proportional handle math to both
  edges (start handle mirrors the end handle); middle → `ItemTitleBar`.
  Rows below the bar → `ItemBody`. With `title-bar-height == 0`, behavior is
  exactly today's
  (no `ItemEdgeStart` — the start handle exists only on the bar, so
  piano-roll never grows one).
- Handles are narrow (`(width * 0.24).clamp(0.5, 1.25)` cells) and
  **containment wins**: an item that actually contains the pointer decides
  the region; the `outside_slop` zone outside an edge is only a fallback,
  used when no item contains the pointer at all. Back-to-back clips share
  one boundary, and without this rule their two slop zones overlap and draw
  order picks the winner — approaching a clip's end edge flips to resizing
  the *next* clip's start. Left of a shared boundary is the left clip's end
  handle, right of it the right clip's start handle.
- `begin_gesture` (`:1742-1818`): `ItemTitleBar` → the existing `:move`
  gesture; `ItemBody` (title bar active) → `:marquee`; `ItemEdgeStart` →
  new `:resize-start` gesture `{id, ids, anchor-start, anchor-end,
  raw-time-offset, alignment-helper-snapped}`.
- Drag/up handlers: `:resize-start` emits `:resize-item-absolute` with
  `edge :start` and `time` = new start (clamped to `[collection min, item.end
  - min-duration]`), plus `:finish-resize-items` unchanged. Snap via
  `effective_resize_snap` exactly as the end edge.
- `WidgetCursor` gains `Move` (`widget_render/mod.rs:405-411`), mapped to
  `CursorIcon::Move` in `metal_backend.rs:3609-3619`. `cursor()`
  (`timeline.rs:499-507`) returns it for `ItemTitleBar`, `EwResize` for both
  edges.
- Rendering (`build_metal_primitives`, item loop `:905-964`): draw the bar
  as a solid band of the item color at full saturation with a 1px hairline
  under it; the body below renders at the existing (slightly dimmed) body
  fill. Labels move into the bar. Selected border/edge-hover highlights
  unchanged.

### 3.2 Real note durations

- **Surface**: `flatten_pattern_events` (`song_state.rs:96-136`) appends a
  4th element: `(time transpose velocity duration)`, duration in the same
  step units as `time`, from `StepParam::Duration` (conversion precedent:
  `piano_roll.rs:174-176`) and `chord_snapshot.durations` per voice. Take
  aggregation (`song_state.rs:180-230`) passes it through unchanged.
- **Lisp**: `arrangement-windowed-dots` (`arrangement.lisp:338-361`) emits
  `:width` per dot — duration normalized to the item's span, clamped so a
  note never paints past the item end. Cap and thinning unchanged.
- **Widget**: `TimelineDot` (`timeline.rs:69-75`) gains optional
  `width: f64` (default 0). In `push_item_content_primitives`
  (`:1092-1181`): width 0 → today's 3px dot; else a bar `max(width_px, 3px)`
  wide × ~3px tall at the dot's y, clipped to the item rect. Lenient parse
  in `parse_item_content` (`:2602-2637`).

Shipped alone with zero behavior change to editing: title-bar move/resize
still lowered to whatever paths existed then, and body-marquee events were
ignored by the host until Slice 2. (Those paths are now
`arrangement-scene-move` and `arrangement-clip-resize`.)

## 4. Slice 2 — cross-track region selection — SHIPPED

### 4.1 State: Rust-owned, like the bound clip

```rust
// app/song_region.rs, alongside song_clip_selection
pub struct SongRegionSelection {
    pub track_a: usize,   // inclusive, model track indices
    pub track_b: usize,   // inclusive
    pub start_beat: f64,
    pub end_beat: f64,    // exclusive
    /// Set only when the marquee was swept in the SCENE lane. The rectangle
    /// is identical either way (a scene sweep spans every track); this bit is
    /// what tells the region ops to carry the scene EVENTS too (lane spec §8).
    /// Added by the lane rewrite — a track-lane marquee that happens to cover
    /// every track still never touches the scene lane.
    pub scene_lane: bool,
}
pub song_region_selection: Option<SongRegionSelection>,
```

Set/cleared by natives `seq-song-set-region {track-a track-b start end}` /
`seq-song-clear-region` (precedent: `seq-song-select-clip`,
`natives.rs:1127-1169`); published as `SEQ.song-region`
(`sync_song_state`, `song_state.rs:513-685`). Rust ownership is what lets
the keyboard seam (§5.3) and the primitives read it, and makes it survive
buffer reloads like the bound clip does.

Mutual exclusivity, **as amended during Slice 2**: a free *marquee* region
clears the clip and scene-event selections and releases the sound binding (it
names no single clip, same rule as scene-lane selections, takes spec §16.11).
A **clip selection is itself a one-clip region**: clicking a clip's title bar
selects the clip AND sets the region to that clip's span on its track,
keeping the binding (`select_song_clip_span` / `set_song_region_for_clip`).
The span still travels from the UI script, though under the lane model it is
just the stored clip's own `[start_beat, end_beat)` — rev 1 needed the script
because a timeline clip was then a merged run of rows sharing a source, which
only the lane projection knew. Deleting the clip, Escape, and a click on
empty lane space clear both.

### 4.2 Capturing the drag across lanes

The drag stays inside the originating lane instance (capture,
`timeline.rs:509-511`), so the widget reports vertical travel and the host
converts it to a track span:

- Marquee payloads (`:marquee-select` / `:finish-marquee-select`,
  `timeline.rs:1998-2011`, `2275-2288`) gain `row-delta`: pointer row minus
  gesture-start row, in cells, signed, **unclamped** (the pointer may be far
  above/below the instance rect).
- `arrangement-track-action i` maps it:
  `visible-ordinal-delta = round(row-delta / arrangement-track-row-pitch)`
  where the pitch is the fixed track-row height + v-stack gap
  (`arrangement.lisp:33-40` constants — assert the row pitch constant next
  to the lane-height constants so they can't drift apart). The originating
  track's *visible ordinal* (position of `i` in
  `(seq-visible-track-indices)`) plus the delta, clamped to the visible
  range, maps back through `seq-visible-track-indices` to the second model
  track index. Collapsed tracks are simply not present, so the region is
  always over visible tracks.
- Region also starts from lane `Background` (empty lane space) — same
  `:marquee` gesture, already emitted today.
- Scene-lane marquee (already wired to `arrangement-selection-rect`,
  `arrangement.lisp:514-518`) becomes "select this time span across **all**
  tracks": same region state with `track_a=0, track_b=last`. One extra arm
  in `arrangement-scene-action`.

### 4.3 Grid quantization

New widget prop `:marquee-snap :grid`: on emit, snap `min(time-a,time-b)`
down and `max` up to `alignment_helper_grid_step` (`timeline.rs:1447-1451`
— the zoom-adaptive ladder), so a drag always selects whole grid cells at
the current zoom, which is what makes "grab exactly 4 bars" trivial.
Arrangement lanes pass it; piano-roll doesn't.

### 4.4 Live preview and rendering

- During the drag, the Lisp handler stores a transient
  `arrangement-region-ghost {track-a track-b start end}` (defstate, like
  `arrangement-ghost`). On `:finish-marquee-select` it calls
  `seq-song-set-region` and clears the ghost. The widget's degenerate
  zero-movement release (`:clear-selection`, `timeline.rs:2268-2272`)
  lowers to `seq-song-clear-region` + set `arrangement-cursor-time/-track`
  — a plain click on a clip body places the edit cursor, Ableton-style.
- Rendering reuses the existing per-instance `:selection-rect` prop
  (`timeline.rs:855-903`, currently unused on track lanes): each track lane
  passes a full-height rect over `[start, end)` iff its track is inside the
  (ghost-else-committed) region. No new widget rendering path.

## 5. Slice 3 — copy / paste — SHIPPED (restated over clips)

### 5.1 Clipboard

`Arc<Mutex<Option<ArrangementClipboard>>>` in `LoopCtx`
(precedent: piano-roll clipboard, `piano_roll.rs:71-82`,
`loop_ctx.rs:135`):

```rust
pub struct ArrangementClipboard {
    pub len_beats: f64,
    /// Grid the copied rectangle sat on; paste floors its destination to it.
    /// The coarsest rung of [4, 2, 1, 1/2, 1/4, 1/8] beats that divides both
    /// the region start and its length — capped at one BAR so a 4-bar copy
    /// snaps to the bar rather than jumping the destination four bars back.
    /// 0.0 = paste exactly where told.
    pub snap_beats: f64,
    /// Absolute model track index → spans, rel_start/rel_end in beats
    /// relative to the copied region start. One entry per CLIP the region
    /// intersects; a lane gap contributes nothing, because a gap is silence
    /// and paste must reproduce it. A track with no clips at all still
    /// travels, with no spans, so paste clears its destination.
    pub tracks: Vec<(usize, Vec<ClipboardSpan>)>,
    /// Scene lane, carried only for a scene-lane region (lane spec §8):
    /// `(rel_beat, scene)` per event inside the span, led by an entry at
    /// rel 0 restating the scene governing the span's start.
    pub scene_lane: bool,
    pub scene_events: Vec<(f64, usize)>,
}
pub struct ClipboardSpan {
    pub rel_start: f64,
    pub rel_end: f64,
    pub source: LaneSource,      // Pattern | Take (a stored clip always has one)
    pub offset_steps: f64,       // source offset AT rel_start (advanced if
                                 // the copy boundary cut into a clip)
}
```

Copy reads the committed **arrangement's** `track_lanes` clipped to the
region (`arrangement_clip_spans_in`); a clip the boundary cuts into is
re-anchored through `restamped_clip`, the compiler's own split rule, so the
fragment plays the identical slice wherever it lands. A cut that leaves
nothing playable (a take past its end) contributes no span. Copy is
read-only — no history entry.

**Locked: paste is same-track, time-shift only.** Sources are per-track
pool ids, so re-targeting tracks would require cloning pattern data into
another pool. Deferred; the clipboard stores absolute track indices and
paste validates they still exist (skips tracks whose ids no longer resolve).
This covers the actual workflow — grab bars 5–9, paste at bar 33.

**Locked: pattern sources paste as references, take sources paste as
copies.** Pattern clips are already shared views (scene cells reference
pool patterns; many clips referencing one pattern is the model's normal
state), so a pasted pattern clip references the same id. A take, though, is
one recorded performance — the planned double-click-to-piano-roll editing
of a take clip must edit only *that* clip, so paste **clones the take**:
mint a new `TakeId` over freshly registered chunk patterns on the same track
(`clone_take_for_paste` → `register_track_take`), named after the source
("Take 2 copy"). Clones are minted once per `(track, take)`, so a take split
across several clips lands as ONE take.
The clipboard still stores the source `TakeId` (validated at paste time,
skipped if since deleted — cheap given no-silent-GC keeps takes alive);
each paste mints a fresh clone. Deleting a pasted region orphans its clone
like any other take (takes spec §6.4).

### 5.2 Primitives (one commit each)

All in `app/song_region.rs`: clone the committed **arrangement**, do every
clip edit in memory, install through `set_committed_arrangement` (which
validates and recompiles), commit one entry via `commit_region_edit` —
`EditPatch::Arrangement`, or `EditPatch::Composite([SceneStructure,
Arrangement])` when takes were cloned (scenes first, ordering per
`song_region_to_take`). Any failure rolls the take clones back and leaves the
committed arrangement untouched. All reject while song edits are locked
(`require_song_edit_unlocked`).

Rev 1 planned a `paint_source_region` helper extracted from the row painters.
The lane rewrite deleted all of them; the region ops now compose the same two
free functions the clip primitives use — `occlude_span` to clear/trim a
destination span and `insert_clip_sorted` to drop a clip in, with
`restamped_clip` for any cut. **A region op is clip surgery on a lane, not a
row splice, and it never normalizes anything afterwards.**

- `song_region_paste(clipboard, dest_beat)` — per clipboard track:
  `occlude_span` over `[dest, dest+len)` (paste is the op that *does*
  truncate, lane spec §8), then insert each stored span as a clip with a
  fresh `ClipId`, its stored offset, anchored at its pasted start. Take spans
  paste their clone (§5.1); a since-deleted take source is skipped, not an
  error. `end_beat` is raised to `dest+len` first, inside the same commit.
  A scene-lane clipboard also stamps its events (§5.3 below). Label
  "Paste region".
- `song_region_delete()` — `occlude_span` over the region per track: the
  clips go away and the span is **silent**, nothing revealed underneath. A
  scene-lane region additionally removes the scene changes inside it and
  restores the scene governing the region's end. Label "Delete region".
  (This is also multi-track Backspace.)
- `song_region_duplicate()` — copy the region and ripple-insert it directly
  after itself; see §5.4. Label "Duplicate region".
- `song_region_move(delta_beats)` — **not yet built; slice 4 (§6).** Lift the
  region's clips off their lanes, `occlude_span` the vacated rectangle and
  the destination rectangle, re-insert the lifted clips shifted by `delta`
  on one clone. Content moves **rigidly**: sources and `offset_steps`
  preserved (takes spec §7.4), so only `start_beat`/`end_beat` change. Move
  never clones takes — it relocates the same clip instances. Label
  "Move region".

Scene-lane handling is shared by paste and delete: `clear_scene_lane_span`
(never removing the mandatory event at 0.0) returns the scene that governed
the span's end, and `restore_scene_tail` re-inserts it if the edit changed
it — so a scene-lane region op is local to its rectangle and nothing after it
moves. A move of a scene-lane region must run the same pair on both
rectangles.

Host commands `song-region-copy/paste/delete/duplicate` are registered as
natives (`ui/natives.rs`) and applied in the UI-side host-command layer where
`LoopCtx` (and so the clipboard handle) is in scope; `song-region-move` joins
them.

### 5.3 Keyboard seam

Handled in `ui/input.rs` when the Arr view is active (precedent: step
clipboard copy/paste, `input.rs:1262-1310`), reading Rust-side state
directly:

- **Cmd-C**: active region → region copy; else selected clip
  (`song_clip_selection`) → copy its merged span as a 1-track region.
- **Cmd-V**: paste at `(arrangement-cursor-time)` — mirror the cursor to
  Rust alongside the region native (`seq-song-set-region` carries it, or a
  tiny `seq-song-set-arr-cursor`), floored to the clipboard's snap.
- **Cmd-D**: active region → `song-region-duplicate` (§5.4). Consumed only
  when a region exists, so it never shadows the mixer's Cmd-D.
- **Backspace**: active region → `song-region-delete`; else the existing
  clip-delete path (`arr_clip_delete`) unchanged. Only a MARQUEE region takes the key
  (region set, `song_clip_selection` empty); a clip click's one-clip region
  keeps falling through to the existing clip-delete path.

Widget-emitted `:copy-items`/`:paste-items` (`timeline.rs:2336-2360`) are
routed to the same commands when they arrive with lane focus, so both entry
points converge.

### 5.4 Duplicate — Cmd-D (ripple insert)

`song_region_duplicate()` copies the region and inserts the copy directly
after itself at `insert = region.end_beat`, pushing what follows right by
`len = region length`. One undo entry; the region selection then becomes
`[insert, insert+len)` so repeated Cmd-D chains down the timeline. Take
sources clone exactly as paste's do (§5.1) — a duplicated take is its own
performance.

**Only the SELECTED tracks ripple.** One mechanism now — `ripple_lane_right`
per lane — with the scope chosen by the region:

| selection | scope | why |
|---|---|---|
| every track, or a SCENE-LANE sweep | every lane's clips **and** the scene events at/after `insert` slide right | the whole timeline opens up: the "insert 4 bars into my song" gesture. A scene sweep is by definition whole-timeline — it carries the scene lane, so the ripple must move it. |
| some tracks | just those lanes' clips slide | the scene lane and every other lane stay exactly where they were, still playing at the beats they always did. |

`ripple_lane_right` splits a clip straddling `insert` first (via
`restamped_clip`, so the moving half carries the phase it had at the
boundary and a tail with nothing left to play is simply dropped), then adds
`len` to every clip at or after the boundary. Both scopes leave exactly
`[insert, insert+len)` vacated on the rippled lanes, which the duplicate then
fills through the same `paste_clipboard` helper paste uses, and both grow
`end_beat` by `len`.

The row model's caveat here is **gone**: there is nothing to detach from.
Clips are the stored truth and resolve on their own (lane spec §6.2), so a
partial ripple no longer converts anything into overrides, and no lane is
silently cut off from later scene-cell edits. The only asymmetry left is
whether the scene lane rides along.

## 6. Slice 4 — move — SHIPPED

Two gestures share one title-bar drag: moving **one clip** and moving the
**selected region**. Both are rigid — `offset_steps` never changes, so the
moved music sounds identical, just later or earlier (takes spec §7.4).

### 6.1 Track-lane single clip

Shipped as written below. Everything below the UI script already existed: `arrangement_actions.rs`
lowers a `clip-move` action to the `arrangement-clip-move` host command, and
`arr_clip_move(clip_id, new_start)` is a validated one-undo-entry primitive
that lifts the clip, `occlude_span`s the destination (so it truncates
whatever it lands on, like every other clip write), re-inserts it and raises
`end_beat` if it now runs past. **Only the Lisp arm was missing, and it now exists
(`arrangement-track-move-ghost` / `arrangement-track-move-finish`):**

- `arrangement-track-action` stops discarding `:finish-move-items`
  (`arrangement.lisp:891-893`). Live `:move-items-absolute` sets an
  `arrangement-ghost` of a new kind `:track-move` carrying
  `{:track i :clip-id (get event :anchor-id) :start (get event :start)}`,
  previewed by `arrangement-track-ghost-clip` alongside the existing
  `:track-resize` case (which already rewrites a clip's rendered span from
  the ghost).
- On `:finish-move-items`, guard the ghost the way the resize arm does
  (`(= (arrangement-ghost-kind) :track-move)` **and** same `:track`), then
  `arrangement-clip-edit i clip (dict :type :clip-move :start ghost-start)`.
  A start equal to the clip's own start commits nothing; a negative start
  clamps to 0. Clear the ghost on every path — a stale ghost is the failure
  mode the current arm was written to avoid.
- Vertical (`lane`) components are ignored: cross-track moves are invalid for
  the same per-track-pool reason as cross-track paste (locked). The host
  clamps the widget's lane offset to 0.
- The moved clip stays selected, and its one-clip region (§4.1) is re-set to
  the new span so a following Cmd-C/Backspace addresses where it now is.

### 6.2 Region move

If the dragged clip lies inside the active region **and that region reaches
beyond it** — another track, or more time (`arrangement-clip-drags-region?`) —
the gesture moves the **region**. The "reaches beyond" half is load-bearing:
selecting a clip makes its own span a one-clip region (§4.1) and the widget
selects before it drags, so "the region covers this clip" is true of *every*
single-clip drag; testing only that turns every move into a region move,
previewed as a bare rectangle instead of the clip itself. A rectangle that is
exactly the dragged clip IS the clip, and moves as one. Otherwise it is a
plain single-clip move and the region gives way, exactly as a clip `:select`
does.

For a real region grab the ghost is the region rectangle shifted by the drag
delta, rendered through the same per-lane `:selection-rect` derivation
(`arrangement-region-for-track`) with the ghost offset applied, and **every
clip the rectangle covers, in every lane it spans, previews the same slide**
(`arrangement-region-ghost-covers?` / `arrangement-region-ghost-clip`): the
rect alone shows where the music will land but not what lands there. The
release lowers to `song-region-move` with
`delta = ghost-start - region-start`.

`song_region_move(delta_beats)` is the one new primitive (§5.2), and it
lifts the rectangle by calling `song_region_copy` — so a move follows copy's
cut-and-restamp rule by construction, and `paste_clipboard` stamps the
destination with an IDENTITY take map (a move clones nothing). Over one
cloned arrangement, per track in the region:

1. Collect the region's clips, cut at the region edges via `restamped_clip`
   (a partially covered clip moves only the part inside the rectangle — the
   region is a rectangle over time, not a set of whole clips, and this is the
   same rule copy already follows).
2. `occlude_span` the **source** rectangle, so the vacated span goes silent
   rather than leaving the trimmed remainders of what moved.
3. `occlude_span` the **destination** rectangle `[start+delta, end+delta)`,
   then `insert_clip_sorted` each collected clip shifted by `delta` with a
   fresh `ClipId` and its offset untouched.
4. Raise `end_beat` if the destination runs past it; reject a delta that
   would push the rectangle below beat 0 (clamp at the UI, error in the
   primitive — never silently truncate the leading clips).
5. When the region carries the scene lane, move its events the same way:
   `clear_scene_lane_span` + `restore_scene_tail` on both rectangles, never
   moving the mandatory event at 0.0.

Order matters: source clearing must precede destination clearing, or an
overlapping move (delta smaller than the region length) erases what it just
placed. One `commit_region_edit` entry, `EditPatch::Arrangement` — move never
clones takes, so it is never composite.

After the commit the region selection follows the move (like duplicate's
does), so repeated drags chain.

### 6.3 Move snapping

The title-bar drag takes the `:move-snap-mode :alignment-helper` behavior the
scene lane already uses — add it to the track-lane props next to the existing
`:resize-snap-mode :alignment-helper`, so a clip snaps to the same zoom-
adaptive ladder its edges already resize to and to neighbouring clip edges.

### 6.4 Already done (was in rev 1's slice 4)

Start-edge resize lowering landed with the lane model: both edges lower to
`arr_clip_resize`, which re-stamps `offset_steps` by the split rule on a left
trim (and runs it backwards when growing left) and clamps takes on the right.
The scene lane's start-edge drag lowers to `arrangement-scene-move`, since a
scene span's left edge *is* its event. Nothing further is owed here.

## 7. Phasing & tests

1. **Clip anatomy + durations** (§3) — SHIPPED. Widget-only + read-surface field.
   Tests: `hit_test` unit tests for the three bar zones and both handles
   at `title-bar-height` 0 and >0; parse test for dot `:width`; existing
   piano-roll layout tests must be untouched (title bar off).
2. **Region selection** (§4) — SHIPPED. `row-delta` emission test; Lisp
   ordinal↔track mapping test (UI-script pattern); `SEQ.song-region`
   publish/diff in `state_values` tests; exclusivity with clip/scene-event
   selection + binding release.
3. **Copy/paste/delete/duplicate** (§5) — SHIPPED. Tests live in
   `song_region.rs`: copy of a boundary-cut clip carries the advanced offset;
   paste reproduces the source rectangle's clips shifted; paste-over extends
   `end_beat` in one undo entry; undo restores in one step. Duplicate:
   all-tracks region ripples the scene lane and chains on repeat; **partial
   region leaves every unselected lane playing the same clips at the same
   beats**; take sources clone.
4. **Move** (§6) — SHIPPED. Tests as listed:
   - `arrangement_actions.rs`: `clip-move` → exactly one
     `arrangement-clip-move` command; `region-move` → `song-region-move`
     carrying the delta (`region_move_lowers_to_one_song_region_move`).
   - `arr_edit.rs`: `arr_clip_move` truncation + `end_beat` growth (exists).
   - `song_region.rs`: `song_region_move` tests — partially covered clip
     moves only its covered part; the vacated rectangle is silent (no
     leftover trimmed fragments); an overlapping move (delta < region length)
     keeps everything it placed; scene-lane region moves its events and
     restores the tail scene; negative delta rejected; one undo entry that
     restores in one step.
   - Phase rigidity: the moved clips' `offset_steps` are byte-identical
     before and after, at both the clip and region level.
   - UI-script test (`ui/tests.rs` pattern): a `:move-items-absolute` then
     `:finish-move-items` on a track lane emits one `clip-move` action with
     the ghost's start, and leaves `arrangement-ghost` nil on both the commit
     and the guard-failure path.

Each slice is independently shippable; 3 and 4 both depend on 2's region
state, 4's region move depends on the `song_region_move` primitive (§5.2),
which is the only model-side code slice 4 added. The clip-move path also
re-anchors the one-clip region through `App::refresh_song_region_for_clip`,
called from the `arrangement-clip-move` host command.

## 8. Locked decisions

- Title bar is a widget **prop**; `0` (default) is bit-identical to today —
  piano-roll never sees any of this.
- Clip body (below the bar) is a **region surface**, not a move surface;
  move/resize live on the bar only. In FX mode a plain body click places the
  edit cursor. While the explicit arrangement piano roll is open, that same
  press retargets the clip editor before retaining the body-marquee drag
  behavior; body double-click still performs no mode transition.
- Region state is Rust-owned (`App::song_region_selection`), published as
  `SEQ.song-region`. A MARQUEE region is mutually exclusive with
  clip/scene-event selection and releases the sound binding; a clip selection
  is a one-clip region and keeps its binding (§4.1 as amended).
- The clip BODY is not a selection surface for the clip: pressing it clears
  the selection, parks the edit cursor and starts a region. Only the title
  bar selects the clip.
- Region highlight is `:selection-rect-style :region`: an OPAQUE fill in the
  selected-body colour over the lane background AND over each covered clip's
  body (title bars keep the clip colour), with the grid redrawn on top. It
  lights the lane rather than washing over it, so a gap and a clip body read
  as one band. `:marquee` (default) is unchanged for the piano roll and the
  scene lane.
- Every arrangement lane carries the same `:time-ruler` and `:grid-density`
  even though only the scene lane draws ruler chrome (gated on
  `:header-height`): the ruler is the time BASE that picks the grid ladder
  both the drawn lines and `:grid` snapping quantize to. Without it a lane
  falls onto the seconds ladder and desyncs from the bars above.
- Marquee times snap to the zoom-adaptive grid ladder (min down, max up).
- Region operations are **new single-commit primitives** over one cloned
  `ProjectArrangement` (composing `occlude_span` / `insert_clip_sorted` /
  `restamped_clip`) — never a sequence of existing host commands (which would
  multiply undo entries).
- Paste and move are time-shift only; cross-track re-targeting is deferred
  (per-track pattern pools).
- Duplicate (Cmd-D) is a ripple insert after the region and ripples **only the
  selected tracks**; an all-tracks region (or a scene-lane sweep) also slides
  the scene events, a partial one leaves the scene lane and every unselected
  lane untouched. §5.4.
- A region op is a rectangle over time, not a set of whole clips: a clip the
  rectangle only partially covers is cut at the edge and re-anchored through
  `restamped_clip`. Copy, duplicate's ripple and move all share this rule.
- Clearing a span stores nothing — silence is the absence of a clip (lane
  spec §6.2). Rev 1's explicit-empty overrides no longer exist.
- Paste floors its destination to the copied rectangle's grid, capped at one
  bar; duplicate does not snap (it lands exactly at the region end).
- Same-track paste **references** pattern sources (linked, like scene
  cells) but **clones** take sources (new TakeId + deep-copied chunks) so
  future per-clip piano-roll editing of a pasted take never rewrites the
  original. Move never clones.
- Moves are phase-rigid: `start_beat` shifts, `offset_steps` preserved
  (takes spec §7.4).
- The clipboard is the whole rectangle: gaps paste as silence.
- A region move clears the SOURCE rectangle before writing the destination,
  so a delta smaller than the region does not erase what it just placed; and
  it never clones takes, so it is always a plain `EditPatch::Arrangement`.

## 9. Open questions

Resolved: `Cmd-D` duplicate rode along in Slice 3 — but not as "paste
immediately after", which would have overwritten what follows. It is a
ripple insert, §5.4.

Resolved: title-bar height is a fixed cell constant,
`arrangement-clip-title-bar-height 0.9` (`arrangement.lisp:54`), not
lane-height-scaled.

- **How a multi-clip rectangle gets grabbed.** A title-bar press narrows the
  region to the clicked clip (`select_song_clip_span`, §4.1 as amended), so by
  the time the drag frames arrive the rectangle is usually the one clip and
  §6.2's region branch does not fire. `song_region_move` is fully built and
  tested; what is missing is a gesture that reliably reaches it. Options: let
  a press whose clip lies INSIDE a larger region bind the sound without
  collapsing the rectangle (Ableton: clicking inside a selection keeps it),
  or put the region grab on a modifier. Until then a region move is reachable
  only when the press does not narrow (e.g. a programmatic region).
- Whether the scene lane's marquee-to-all-tracks region should also drive
  scene-event selection simultaneously (Ableton merges these; we currently
  keep them exclusive).
- Whether a region move should be allowed to cross the song end. **Shipped
  following paste**: a move past the end extends `end_beat` in the same entry
  (`move_past_the_song_end_extends_it_in_the_same_entry`). Ableton's Cut
  Time/Paste Time semantics may still argue for clamping instead.
