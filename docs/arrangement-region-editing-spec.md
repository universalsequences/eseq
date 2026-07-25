# Arrangement Region Editing — Clip Hit Regions, Region Selection, Copy/Paste/Duplicate, Move

Status: mini implementation spec, 2026-07-24
Related: `docs/arrangement-timeline-ui-spec.md` (§9, §11 items 2/4),
`docs/takes-and-additive-arrangement-recording-spec.md` (§7.4),
`crates/eseqlisp/src/widget_render/timeline.rs`,
`crates/sequencer/ui/arrangement.lisp`,
`crates/sequencer/src/ui/arrangement_actions.rs`,
`crates/sequencer/src/app/song_edit.rs`, `crates/sequencer/src/app/take_edit.rs`

## 1. Summary

Four slices, in dependency order:

1. **Ableton clip anatomy** — every clip gets a title bar (move/resize zone,
   hover cursors) and a body (region-selection surface); notes render with
   real durations instead of 3px dots.
2. **Region selection** — click-drag on clip bodies / background selects a
   time × track rectangle, across multiple tracks, quantized to the
   zoom-adaptive grid.
3. **Copy / paste / duplicate** — region → clipboard → paste at the
   per-track cursor; Cmd-D duplicates the region in place, rippling what
   follows right. One undo entry each.
4. **Move** — drag a clip by its title bar; if the clip is inside the
   active region, the whole region moves in unison.

Everything lowers to song-model mutations through new one-commit region
primitives; no parallel mutation path.

## 2. Current facts (verified 2026-07-24)

- Hit regions: `HitRegion::{Header, ContentLengthEnd, Sidebar, Background,
  ItemBody, ItemEdgeEnd}` (`timeline.rs:155-163`); `hit_test`
  (`timeline.rs:1691-1740`). **No start edge, no title-bar zone.** End
  handle is `clamp(width*0.24, 1.25, 4.0)` cells + 0.75 slop.
- Cursors are pull-based: `WidgetDefinition::cursor` (`timeline.rs:499-507`,
  enum `widget_render/mod.rs:405-411`), polled by
  `editor/widget_interaction.rs:745-769`, mapped to winit in
  `ui/metal_backend.rs:3609-3619`. Today only `EwResize` on
  `ItemEdgeEnd`/`ContentLengthEnd`.
- Pointer + `ItemBody` begins a `:move` gesture (`timeline.rs:1749-1769`);
  Pointer + `Background` begins `:marquee` (`timeline.rs:1789-1793`).
  Marquee emits `:marquee-select` per frame (`1988-2012`) and
  `:finish-marquee-select` on release (`2265-2289`); times are raw/unsnapped
  (`1903`, `2246`). `:selection-rect` is a host-fed prop (`2646-2654`,
  rendered `855-903`).
- **Drag capture is per-instance** (`captures_drag`, `timeline.rs:509-511`):
  once a drag starts in one lane instance, other lanes never see events. A
  cross-track marquee must be reconstructed host-side.
- Each arrangement track lane is one single-lane widget instance; the track
  index lives only in the `:on-action` closure
  (`arrangement.lisp:762`, `arrangement-track-action i event`, from
  `(each (seq-visible-track-indices) |i| …)` at `:819`).
- Track-lane `:finish-move-items` is discarded (`arrangement.lisp:660-662`).
  Lowering table: `arrangement_actions.rs:100-254`; `track-paint` at
  `:208-245` carries explicit `:track`.
- `song-track-paint` is **pattern-only** (`song_edit.rs:618` hard-codes
  `take_id: None`). Take-aware row surgery exists privately as
  `paint_take_region` (`take_edit.rs:522-586`).
- No generic multi-command undo transaction. One-entry compound commits are
  done by mutating a cloned model then a single `history.commit` with
  `EditPatch::Song` / `EditPatch::Composite` — template:
  `song_region_to_take` (`take_edit.rs:317-400`).
- Note durations exist in the model (`step_data[step][StepParam::Duration]`,
  `chord_snapshot.durations` — `sequencer/data.rs:1154-1158`) but
  `flatten_pattern_events` (`ui/state_values/song_state.rs:96-136`) publishes
  only `(time transpose velocity)`; `TimelineDot` (`timeline.rs:69-75`) is
  point-only, drawn as a fixed 3px quad (`timeline.rs:1144-1145`).
- Widget already emits `:copy-items {ids}` / `:paste-items {time}` on
  Cmd-C/V (`timeline.rs:2336-2360`); the arrangement ignores them.
  `arrangement-cursor-time` / `arrangement-cursor-track`
  (`arrangement.lisp:10-14`) were built as the paste target.
- Zoom-adaptive snap already exists: `:resize-snap :grid` →
  `TimeViewport::grid_step` ladder (`time_view.rs:189-245`);
  `cursor_snap_time` (`timeline.rs:1599-1602`) uses the same view-derived
  grid.

## 3. Slice 1 — clip anatomy (widget)

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
  edges (start handle mirrors the end handle: `[left - outside_slop,
  left + handle_width]`); middle → `ItemTitleBar`. Rows below the bar →
  `ItemBody`. With `title-bar-height == 0`, behavior is exactly today's
  (no `ItemEdgeStart` — the start handle exists only on the bar, so
  piano-roll never grows one).
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

Ships alone with zero behavior change to editing: title-bar move/resize
still lower to the existing paths (scene `song-row-move`, track resize),
and body-marquee events are ignored by the host until Slice 2.

## 4. Slice 2 — cross-track region selection

### 4.1 State: Rust-owned, like the bound clip

```rust
// app/mod.rs, alongside song_clip_selection
pub struct SongRegionSelection {
    pub track_a: usize,   // inclusive, model track indices
    pub track_b: usize,   // inclusive
    pub start_beat: f64,
    pub end_beat: f64,    // exclusive
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
clears the clip and scene-row selections and releases the sound binding (it
names no single clip, same rule as scene-lane selections, takes spec §16.11).
A **clip selection is itself a one-clip region**: clicking a clip's title bar
selects the clip AND sets the region to that clip's merged span on its track,
keeping the binding (`select_song_clip_span` / `set_song_region_for_clip`).
The span travels from the UI script because a timeline clip is the merged run
of rows sharing a source, which only the lane projection knows. Deleting the
clip, Escape, and a click on empty lane space clear both.

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

## 5. Slice 3 — copy / paste

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
    /// relative to the copied region start. Gaps are implicit (paste
    /// silences them — the clipboard is the whole rectangle). A track that
    /// was silent throughout still travels, with no spans, so paste silences
    /// its destination.
    pub tracks: Vec<(usize, Vec<ClipboardSpan>)>,
}
pub struct ClipboardSpan {
    pub rel_start: f64,
    pub rel_end: f64,
    pub source: LaneSource,      // Pattern | Take (Empty spans are omitted)
    pub offset_steps: f64,       // source offset AT rel_start (advanced if
                                 // the copy boundary cut into a clip)
}
```

Copy reads the committed song's `project_lanes` (`song.rs:437`) clipped to
the region; a clip cut by the region boundary stores its offset advanced to
the cut point (same math as `split_row_state` / `advanced_offset`,
`song_edit.rs:403-470`) so the pasted result plays the identical slice.
Copy is read-only — no history entry.

**Locked: paste is same-track, time-shift only.** Sources are per-track
pool ids, so re-targeting tracks would require cloning pattern data into
another pool. Deferred; the clipboard stores absolute track indices and
paste validates they still exist (skips tracks whose ids no longer resolve).
This covers the actual workflow — grab bars 5–9, paste at bar 33.

**Locked: pattern sources paste as references, take sources paste as
copies.** Pattern clips are already shared views (scene cells reference
pool patterns; multiple rows referencing one pattern is the model's normal
state), so a pasted pattern clip references the same id. A take, though, is
one recorded performance — the planned double-click-to-piano-roll editing
of a take clip must edit only *that* clip, so paste **clones the take**:
mint a new `TakeId`, deep-copy every chunk pattern into the track's pool
(`TrackPatternPool::insert`), name it after the source ("Take 2 copy").
The clipboard still stores the source `TakeId` (validated at paste time,
skipped if since deleted — cheap given no-silent-GC keeps takes alive);
each paste mints a fresh clone. Deleting a pasted region orphans its clone
like any other take (takes spec §6.4).

### 5.2 Primitives (one commit each)

All in `app/song_edit.rs` / a new `app/song_region.rs`, all following the
`song_region_to_take` template (`take_edit.rs:317-400`): clone the
committed song, do every row splice in memory, validate, single
`commit_song_edit`-style entry. All reject while song edits are locked
(`require_song_edit_unlocked`, `song_edit.rs:124`).

First, **generalize the paint helper**: extract the shared row surgery of
`song_track_paint_anchored` (`song_edit.rs:507-651`) and `paint_take_region`
(`take_edit.rs:522-586`) into one internal
`paint_source_region(song, track, start, end, source: LaneSource,
anchor_beat, anchor_offset_steps)` operating on a `&mut ProjectSong`
*without* committing. Both existing entry points become thin wrappers; the
region primitives call it N times on one clone. This also fixes the latent
gap that `song-track-paint` can't paint takes.

- `song_region_paste(dest_beat)` — for every clipboard track: paint
  explicit-empty over `[dest, dest+len)`, then each span's source with its
  stored offset (anchor = its pasted start); take spans first clone the
  take per §5.1 and paint the clone's id. Extend the song end first if
  `dest+len > end_beat` (inside the same commit — the region primitives are
  single-entry by construction, unlike the two-entry `finish-create-item`
  path). Commits `EditPatch::Song` when no takes were cloned, else
  `EditPatch::Composite([SceneStructure, Song])` (ordering per
  `song_region_to_take`, `take_edit.rs:390-398`) — still one undo entry.
  Label "Paste region".
- `song_region_delete()` — explicit-empty paint over the region per track.
  Label "Delete region". (This also gives multi-track Backspace.)
- `song_region_duplicate()` — copy the region and ripple-insert it directly
  after itself; see §5.4. Label "Duplicate region".
- `song_region_move(delta_beats)` — delete source rectangle + repaint at
  the shifted position on one clone (content moves **rigidly**: sources and
  offsets preserved, takes spec §7.4). Move never clones takes — it
  relocates the same clip instances. Label "Move region". Also reused for
  single-clip move (§6).

Host commands `song-region-copy/paste/delete/duplicate/move` registered in
`host_commands/song.rs` (command list `:13-38`); copy/paste additionally
need the clipboard handle, so they follow the piano-roll pattern of being
applied in the UI-side host-command layer (`history_commands.rs:712-760`)
where `LoopCtx` is in scope.

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
- **Backspace**: active region → `song-region-delete`; else existing
  clip/row deletion paths unchanged. Only a MARQUEE region takes the key
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

**Only the SELECTED tracks move.** Two mechanisms, chosen by whether the
region covers every track, with the same audible contract:

| selection | mechanism | why |
|---|---|---|
| every track | shift the song ROWS at/after `insert` right by `len` | rows are the shared time boundaries, so moving them moves every lane at once and the song stays **scene-resolved** — no override churn. The "insert 4 bars into my song" gesture. |
| some tracks | re-paint just those lanes' tails `len` beats later | rows stay put, so untouched lanes keep playing at the beats they always did. |

The partial path reads each rippled lane's spans from `insert` to the song
end **before** any mutation (silence included — a gap that slides right must
leave silence behind it, not the clip it used to sit beside), boundary-cut
offsets advanced per §5.1, then re-stamps each span at `+len` with the offset
it had at its own start. Both paths leave exactly `[insert, insert+len)`
vacated on the rippled lanes, which the duplicate then fills through the same
`paint_clipboard` helper paste uses.

**Locked: partial ripple detaches the rippled lanes from scene resolution.**
Once a lane's content sits `len` beats off the rows' scenes it cannot be
scene-derived any more, so its tail becomes explicit overrides and later
scene-cell edits stop reaching it. This is inherent, not an implementation
shortcut — and it is exactly why the all-tracks case earns its own
row-shifting path rather than being folded into the general one.

Untouched lanes are not left alone by accident: the partial path inserts new
row boundaries under them, which is safe only because every split goes
through `split_row_state` (phase-transparent — same music, more rows). Both
paths grow the song by `len`, so every lane gains that much new time at the
end, governed by whatever it was playing at the boundary; existing material
is untouched.

## 6. Slice 4 — move

- **Track-lane single clip**: `arrangement-track-action` stops discarding
  `:finish-move-items` (`arrangement.lisp:660-662`). Live `:move-items-absolute`
  drives the existing `:move` ghost (`arrangement-ghost`, `:kind :move`,
  drawn via `arrangement-track-ghost-clip`, `arrangement.lisp:429-441`);
  finish lowers to `song_region_move` with region = the merged clip's span
  on that one track and `delta = ghost-start - clip-start`. Vertical
  (`lane`) components are ignored — cross-track moves are invalid for the
  same pool-locality reason as cross-track paste (locked; the widget's
  lane-offset is clamped to 0 by the host).
- **Region move**: if the dragged clip lies inside the active region
  (its track within `[track_a, track_b]` and span intersecting
  `[start, end)`), the gesture moves the **region**: ghost = the region
  rect shifted by the drag delta (rendered through the same per-lane
  `:selection-rect` derivation with the ghost offset applied), finish →
  `song_region_move(delta)`. Otherwise it's a plain single-clip move and
  the region clears.
- **Move snapping**: the title-bar drag inherits the existing
  `:move-snap-mode :alignment-helper` behavior the scene lane already uses
  (`arrangement.lisp` scene props) — add it to the track-lane props.
- **Start-edge resize lowering** (enabled by §3.1): scene lane →
  `song-row-move` of the row itself; track lane → one anchored paint:
  shrink = explicit-empty over `[old_start, new_start)`; grow = the clip's
  source over `[new_start, old_start)` with the clip's existing
  anchor/offset (trim-head semantics, takes spec §7.4; take offsets clamp
  at 0 per §6.3). Both ride `song-track-paint`-style lowering in
  `arrangement_actions.rs` — no new primitive.

## 7. Phasing & tests

1. **Clip anatomy + durations** (§3) — widget-only + read-surface field.
   Tests: `hit_test` unit tests for the three bar zones and both handles
   at `title-bar-height` 0 and >0; parse test for dot `:width`; existing
   piano-roll layout tests must be untouched (title bar off).
2. **Region selection** (§4) — `row-delta` emission test; Lisp
   ordinal↔track mapping test (UI-script pattern); `SEQ.song-region`
   publish/diff in `state_values` tests; exclusivity with clip/row
   selection + binding release.
3. **Copy/paste/delete/duplicate** (§5) — `paint_source_region` extraction
   must keep `song_edit.rs` + `take_edit.rs` suites green; new tests: copy
   clips a boundary-cut clip with advanced offset; paste reproduces
   `project_lanes` of the source region shifted; paste-over extends song end
   in one undo entry; undo restores in one step. Duplicate: all-tracks region
   ripples the rows and chains on repeat; **partial region leaves every
   unselected lane playing the same spans at the same beats**; take sources
   clone.
4. **Move** (§6) — `arrangement_actions.rs` lowering tests (single move →
   one command; region move; start-edge shrink/grow); phase-rigidity test
   (moved clip's audible content identical, offsets preserved).

Each slice is independently shippable; 3 and 4 both depend on 2's region
state, 4's region move depends on 3's `song_region_move`.

## 8. Locked decisions

- Title bar is a widget **prop**; `0` (default) is bit-identical to today —
  piano-roll never sees any of this.
- Clip body (below the bar) is a **selection surface**, not a move surface;
  move/resize live on the bar only. Plain body click = place edit cursor.
- Region state is Rust-owned (`App::song_region_selection`), published as
  `SEQ.song-region`. A MARQUEE region is mutually exclusive with
  clip/scene-row selection and releases the sound binding; a clip selection
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
  `ProjectSong` (`paint_source_region` helper) — never a sequence of
  existing host commands (which would multiply undo entries).
- Paste and move are time-shift only; cross-track re-targeting is deferred
  (per-track pattern pools).
- Duplicate (Cmd-D) is a ripple insert after the region and moves **only the
  selected tracks**; an all-tracks region shifts the song rows (scene
  structure preserved), a partial one re-paints just those lanes' tails
  (which detaches them from scene resolution). §5.4.
- Paste floors its destination to the copied rectangle's grid, capped at one
  bar; duplicate does not snap (it lands exactly at the region end).
- Same-track paste **references** pattern sources (linked, like scene
  cells) but **clones** take sources (new TakeId + deep-copied chunks) so
  future per-clip piano-roll editing of a pasted take never rewrites the
  original. Move never clones.
- Moves are phase-rigid: `start_beat` shifts, `offset_steps` preserved
  (takes spec §7.4).
- The clipboard is the whole rectangle: gaps paste as silence.

## 9. Open questions

Resolved: `Cmd-D` duplicate rode along in Slice 3 — but not as "paste
immediately after", which would have overwritten what follows. It is a
ripple insert, §5.4.

- Title-bar height value: fixed cell constant vs scaling with lane height —
  pick during Slice 1 by eye against the Ableton reference shots.
- Whether the scene lane's marquee-to-all-tracks region should also drive
  scene-row selection simultaneously (Ableton merges these; we currently
  keep them exclusive).
