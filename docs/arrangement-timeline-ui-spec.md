# Arrangement Timeline UI Spec

Status: draft / design (rev 2 — verified against widget + piano-roll wiring)
Author: design pass, 2026-07-20
Related: `docs/song-mode-spec.md`, `crates/eseqlisp/src/widget_render/timeline.rs`,
`content/ui/piano-roll.lisp`, `content/ui/sequencer.lisp`

## 1. Summary

Song mode (`docs/song-mode-spec.md`) specifies rows, the derived
`state_at_beat`/lane-projection queries, and a closed set of editing
primitives, all without requiring a graphical editor. This document specifies
how a graphical arrangement/timeline view is built on top of that model, and,
separately, how the existing `timeline` widget generalizes to carry it.

This spec is broader than song mode: it also covers audio-track waveform
display, which `song-mode-spec.md` §3 explicitly excludes. The two documents
compose — song rows and lane projection describe pattern-clip arrangement;
this document describes the widget and layout that can eventually show audio
clips alongside them without a different architecture.

## 2. Goals

- Reuse the existing `timeline` widget (`crates/eseqlisp/src/widget_render/timeline.rs`)
  per track lane rather than building a new mega-widget for the whole
  arrangement.
- Reuse the existing track-header composition (`seqv-track-header` and
  friends in `content/ui/sequencer.lisp`) unchanged.
- Keep every lane instance a pure, stateless, prop-driven view, so many
  instances stay time-synced by construction.
- Support sparse per-track display (a track with nothing playing shows an
  empty lane, not a placeholder block).
- Extend `TimelineItem` minimally so it can preview MIDI-pattern content and
  audio waveforms without coupling the widget to musical or audio-decoding
  domain knowledge.
- Preserve today's piano-roll usage of the widget unchanged.

## 3. Non-goals for V1

- Audio decoding, peak-cache generation, or asset loading (assumed to exist
  or be built separately; this spec only defines what the widget consumes).
- Waveform LOD/mipmap accuracy at extreme zoom.
- Editing gestures beyond what `timeline.rs` already supports (move, resize,
  draw, erase, marquee, scrub) — new arrangement-specific gestures (e.g.
  clip duplicate-drag) are a later pass once the base layout ships.
- Cross-lane marquee selection (§5.3) — marquee works within one lane
  instance; a rubber band spanning multiple track rows needs host-level
  coordination and is deferred.
- Per-track clip editing that is not expressible as a row primitive (§9.2).
- The scene lane's eventual mixer/macro parameter content (`song-mode-spec.md`
  §16); V1 scene lane shows scene markers only.

## 4. Why per-lane composition, not one mega-widget

`content/ui/sequencer.lisp:1730` already does this for the step
grid:

```lisp
(each (seq-visible-track-indices) |i|
  (subtree :key (str "sequencer-track-" (nth SEQ.track-ids i))
    (h-stack :width :fill :gap 0.6 :align :start
      (seqv-track-header i)
      (seqv-track-grid i)
      ...)))
```

`seqv-track-header` (sequencer.lisp:667) is a plain composable function —
color badge, rec-arm dot, mute/solo buttons, name badge, volume meter — with
no dependency on the grid widget beside it. It already lives in its own
`subtree` so chrome changes rerun only the header. The arrangement view
reuses it verbatim.

The alternative, one widget owning every track's clips internally, would
need to reimplement per-track chrome (header, mute/solo, volume) inside a
single Rust widget's prop schema, duplicating what Lisp composition already
does well, and would make per-track reactive scoping (only rerun the row
that changed) harder than the existing `each` + `subtree` pattern already
gives for free. See `[[lisp-ui-each-vs-map]]`: use `each`, not `map`, for
these rows so per-row identity and reactive scoping stay correct.

### 4.1 Composition sketch

```lisp
(v-stack
  (arrangement-scene-lane)               ; single lane: ruler + scene markers
  (scroll-view :vertical? true
    (each (seq-visible-track-indices) |i|
      (subtree :key (str "arr-track-" (nth SEQ.track-ids i))
        (h-stack :align :start
          (seqv-track-header i)          ; unchanged, reused as-is
          (arrangement-track-lane i))))))
```

`arrangement-track-lane` and `arrangement-scene-lane` are both instances of
the `timeline` widget. Vertical scrolling of the track list is the ordinary
buffer/pane viewport, not a widget concern — the same mechanism the step
sequencer already relies on.

### 4.2 One ruler, many headerless lanes

The widget's time ruler and header strip are per-instance props
(`:header-height`, `:time-ruler` — piano-roll passes
`(dict :mode :bars-beats :beats-per-bar 4)`, piano-roll.lisp:240). Stacked
lanes must not each draw their own ruler:

- The scene lane (top, outside the vertical scroll) is the only instance
  with a nonzero `:header-height` and a `:time-ruler`; it doubles as the
  arrangement's bar/beat ruler.
- Every track lane passes `:header-height 0`.
- Every lane passes `:sidebar-width 0`; the per-track sidebar role is played
  by the composed `seqv-track-header` instead. (`:sidebar-style :piano`
  remains a piano-roll-only concern.)

Each track lane is a single-lane instance: `:lanes` of length 1,
`:lane-height` equal to the widget height, `:lane-scroll 0`. Lane scrolling
and `:zoom-lanes` actions are inert/ignored in the arrangement; vertical
navigation belongs to the outer scroll-view.

## 5. Shared time axis: the widget must not own scrolling

Verified against `timeline.rs`: the widget holds no cross-frame scroll state.
`view-start`, `view-duration`, `zoom-min-duration`, `zoom-max-duration`,
`lane-scroll`, `playhead-time`, `cursor-time`, `content-length` are all read
fresh from props every render (`TimelineView::from_props`). The only
`thread_local` in the file is a hover-edge cache used purely for visual
feedback, not position. Every pan/zoom/scroll gesture computes the next
`view-start`/`view-duration`/`lane-scroll` from the incoming props and the
current pointer position and returns it as an action value; nothing is
accumulated internally between renders. `playhead-time` and `cursor-time` are
the widget's only `bindable_props`, so the playhead can sweep during playback
without forcing a tree rebuild of any lane.

This means synchronization is a wiring rule, not a widget change.

### 5.1 Shared reactive values

Every lane instance (scene lane and every track lane) is driven by the same
reactive values:

- `arrangement-view-start` / `arrangement-view-duration` — the visible span.
- `arrangement-zoom-min-duration` / `arrangement-zoom-max-duration` — zoom
  clamps, fed to `:zoom-min-duration`/`:zoom-max-duration`.
- `arrangement-tool` — fed to every lane's `:tool`. The widget's tool set
  (pointer, draw, erase, marquee, pan, scrub) is a per-instance prop, so a
  shared value is what makes the toolbar act on all lanes at once.
- `:content-length` — the song's `end_beat`, read from the `song-end-beat`
  binding (`song-mode-spec.md` §12), fed identically to every instance so
  the end-of-song marker lines up on every row.
- `:playhead-time` — bound to `song-position-beats` (§12; render-rate
  readable per §10.2), the same binding on every lane, so the playhead
  sweeps all rows in lockstep without tree rebuilds.

### 5.2 Shared action routing

Every lane instance's `on-action` routes view actions through the same
setters, mirroring `piano-roll-action` (piano-roll.lisp:188):

- `:scroll-view` → `set-arrangement-view-start` (the event carries either an
  absolute `:view-start` or a `:delta-time`, exactly as piano-roll handles
  at piano-roll.lisp:191-203; `:lane-scroll`/`:delta-lanes` components are
  ignored per §4.2).
- `:zoom-view` → `set-arrangement-zoom` (anchored at the event's cursor
  time; because all lanes render at identical width and share the same
  view props, a zoom anchored in one lane stays visually consistent across
  every other lane the following frame).
- `:set-cursor` → `set-arrangement-cursor-time` (one shared cursor value,
  passed to every lane's `:cursor-time`, so clicking a time in any row moves
  the edit cursor everywhere).

It does not matter which lane the user's pointer is over; the emitted action
always updates the one shared state.

### 5.3 Per-lane state (deliberately not shared)

`:selection` and `:selection-rect` are per-instance props; a marquee lives
inside one widget instance and cannot span rows. V1 keeps selection scoped
to the lane under the pointer (in practice the scene lane, where editing
happens — §9.2). Arrangement-wide selection semantics are a later pass.

## 6. Sparse lanes

A track with nothing playing at a given span must render as empty lane, not
a placeholder block. This is already representable in the data model: scene
cells are `cells: Vec<Option<PatternId>>` (`state.rs:1822`), and the
lane-projection query (`song-mode-spec.md` §5.5) types `LaneClip.pattern` as
`Option<PatternId>`. Rendering sparse is therefore a filter at the point
`LaneClip`s are turned into `items` for the widget: spans where
`pattern.is_none()` simply produce no `TimelineItem`, they are not rendered
as an empty-styled block. No model change is required; this section exists
to record that sparseness was checked against the model, not assumed.

## 7. `TimelineItem` extension

Items arrive as a Lisp list of maps and are parsed by `get_items`
(`timeline.rs:2344`), which today reads `id`, `lane`, `start`, `end`,
`selected`, `label`, `color`. Add two optional keys, both absent by default
so piano-roll's current usage is byte-for-byte unaffected:

```lisp
(dict :id ... :lane 0 :start 16 :end 32 :label "Verse" :color ...
  :kind :midi                          ; new — :midi | :audio | :scene
  :content (dict :dots (list (dict :offset 0.25 :value 0.6) ...)
             :cycle 0.25
             :phase 0.5))             ; new — :dots or :peaks payload;
                                       ; optional :cycle = fraction of the
                                       ; item one repetition covers (>0);
                                       ; optional :phase = source position
                                       ; at the item start (0..1)
```

A pattern clip declares `:cycle = pattern-length / span` and
`:phase = offset-steps / pattern-steps`. `:cycle` may be below 1 (the pattern
repeats) or above 1 (the clip shows only part of one pattern); `:phase`
anchors that source window at the clip's left edge. The widget therefore
keeps notes aligned through either-edge resizes instead of restarting or
stretching the preview. It draws a separator at each visible cycle boundary;
boundaries and dots are skipped below a few px of per-cycle width. The widget
still knows nothing about steps or timebases — the host computes both
ratios. Malformed `:cycle`/`:phase` values degrade to 1/0.

Timeline item labels are styled by the optional widget props
`:item-label-font-size` (points, default `10.5`) and `:item-label-color`
(named or RGBA color, default `:black`). Both accept reactive bindings.
Every label is hard-clipped to the visible item title bar (or item body when
the title bar is disabled), so a long label cannot paint outside a short
clip.

The optional `:background-color` widget prop sets the lane background in
both Metal and terminal rendering and accepts the same named/RGBA colors and
reactive bindings. When absent, the timeline preserves its legacy lane
defaults; the arrangement host passes its theme's `:buffer-bg`.

parsed into:

```rust
struct TimelineItem {
    // ...existing fields unchanged...
    kind: Option<TimelineItemKind>,       // new
    content: Option<TimelineItemContent>, // new
}

enum TimelineItemKind { Midi, Audio, Scene }

enum TimelineItemContent {
    Dots(Vec<TimelineDot>),
    Peaks(Vec<PeakBucket>),
}

struct TimelineDot {
    offset: f64, // 0.0..1.0 within the item's [start, end)
    value: f64,  // 0.0..1.0, vertical placement within the item rect
}

struct PeakBucket {
    min: f32, // -1.0..1.0
    max: f32,
}
```

Malformed or unknown `:kind`/`:content` values parse to `None` (matching
`get_items`' existing lenient `filter_map` style), never to a render error.

`kind` is a rendering hint only (default icon/color convention per clip
type); `content` is what actually gets drawn inside the item rect. They are
intentionally decoupled — a future item kind that is neither MIDI nor audio
(e.g. an automation clip) can still carry `Dots` or a new content variant
without inventing a new `kind`.

### 7.1 MIDI content: flatten in Lisp, draw dumbly in Rust

The widget must not gain any notion of steps, p-locks, or timebase. Lisp
already owns the pattern representation and, for song mode, the projection
that resolves which pattern is effective per span (`song-mode-spec.md` §5.5).
When building `items` for a track lane, Lisp flattens the effective pattern's
events into normalized `(offset, value)` pairs at snapshot time and hands the
widget dumb points to plot.

This directly resolves the p-lock/generative-timebase concern raised in
design discussion: because the preview is a flattened snapshot, not a
live re-derivation, events that land off a clean grid — due to per-note
timebase p-locks today, or an unpredictable polymeter/chord-generating MIDI
effect later — simply appear wherever they fall. The widget was never
coupled to the timing model, so there is nothing to special-case as those
features land. If a pattern's event count or density changes at playback
time (e.g. a generative effect that varies run to run), the preview reflects
whatever snapshot was taken when the item was built; it is not required to
track live variation.

Flattening runs when items are (re)built — a reactive recompute on pattern
or row change, not per frame. Cap dots per item (e.g. 256, densest-first
drop) so a pathological pattern cannot bloat the prop list; at arrangement
zoom the preview is impressionistic anyway.

### 7.2 Audio content: precomputed peaks, not live decode

Waveform peaks are computed once, off the render path, when an audio asset
is loaded or a clip's source region changes — a fixed-size bucket array
(e.g. 256 `PeakBucket`s spanning the clip) cached alongside the asset, not
regenerated per frame or per zoom level. The widget draws vertical bars
scaled to the item's current on-screen width from that fixed bucket count.

V1 accepts that this is not sample-accurate at extreme zoom-in (a real
mipmapped multi-resolution peak cache is a follow-on, tracked as a non-goal
in §3), consistent with the "not too hard" scope the user aimed at: get a
faithful low-zoom overview shipped, defer resolution-adaptive peaks.

### 7.3 Rendering

Both content variants render as additional `MetalQuadPrimitive`s positioned
within the item's rect, alongside the item-body/selection/edge-highlight
quads the GPU path already emits (the `MetalPrimitive::Quad` emission runs
throughout `gpu_render`, roughly timeline.rs:560-880) — no new primitive
types needed, just new geometry derived from `content` instead of solely
from the item's own start/end/color. Dots and peak bars are clipped to the
item rect and skipped entirely below a minimum on-screen item width (a few
pixels), where they would only alias.

## 8. Scene lane

The scene lane is a `timeline` instance with a single lane fed scene-marker
items (`kind: Scene`, no `content` in V1) built from `song-rows` (declared as
a read-only binding in `song-mode-spec.md` §12). It shares the arrangement
time axis exactly as track lanes do (§5), and it is the one instance that
draws the ruler (§4.2). V1 scene-lane items are spans — each row's
`[start_beat, next start_beat)` with the row's scene name as `label` — since
the widget has no marker-only render mode and spans give resize/move edges
for free in Slice C. Mixer/macro visualization on this lane is deferred per
`song-mode-spec.md` §16.

## 9. Editing primitives stay the seam

Timeline gestures must translate into the existing song-mode editing
primitives (`song-mode-spec.md` §5.6) via their host commands, not into a
parallel mutation path. This keeps undo/redo, validation, and the
single-launch-authority rule (song-mode playback/capture disallow primitive
calls) enforced in exactly one place.

### 9.1 Action lifecycle: preview live, commit on finish

The widget's edit gestures emit paired actions (all carry `event.type`, the
key piano-roll matches on in `piano-roll-action`): live absolute updates
during the drag (`:move-items-absolute`, `:resize-item-absolute`,
`:create-item`) and a terminal `:finish-move-items` / `:finish-resize-items`
/ `:finish-create-item` on release. Piano-roll's precedent routes these
"native" actions to one Rust command (`seq-piano-roll-action`,
`ui/natives.rs:2475`) while handling view actions in Lisp; the arrangement
does the same split with its own translator.

The song primitives are atomic, validated, one-undo-entry operations
(§5.6), so the translation rule is:

- **Live actions** update view-layer ghost state only (a preview rect / drop
  indicator). They never call a primitive — calling `song-row-move` per
  drag-frame would spam history and validation.
- **Finish actions** call exactly one primitive (or one compound entry, per
  §5.6's split-here guidance): drag of a scene-lane span → `song-row-move`;
  resize of a span's end edge → the *next* row's `song-row-move` (a span
  ends where the next row starts); draw on empty scene lane →
  `song-row-insert`; erase / marquee-delete → `song-row-remove`.
- A primitive rejection (collision, row zero, normalization guard — §5.6
  enumerates them) discards the ghost and surfaces the reason in the status
  line; the view snaps back. The model never auto-resolves.

### 9.2 Row granularity: where each gesture lives

A song row is a complete state, so row-granular editing lives on the
**scene lane**: a drag there moves a boundary for every track at once,
which is exactly what the model expresses.

Track lanes additionally support **per-track clip editing** over the merged
lane projection (adjacent same-pattern spans render as one clip — the
merge is a view concern; the clip's stable gesture identity is its first
row's id):

- **Select** a clip; selection is exclusive between the scene lane and
  track lanes so Backspace is never ambiguous.
- **Backspace** silences the clip's whole span.
- **End-edge drag** resizes it: shrinking silences the released tail
  (shrinking to the clip start deletes it), growing paints the clip's own
  pattern over what follows.

Every track-clip gesture lowers to exactly ONE `song-track-paint`
(song-mode-spec 5.6): the primitive owns the row surgery (split + restore
row + per-row override rewrite + normalize) as one atomic, one-undo-entry
mutation. Live drags remain ghost-only per §9.1. Silenced regions come
from explicit-empty overrides (`pattern_id: nil`), which never fall back
to the scene cell. Whole-clip *moves* on track lanes are not lowered yet.

### 9.3 Song end is a free gesture

The widget already has a draggable content-length end handle
(`HitRegion::ContentLengthEnd` emitting `:resize-content-length`, which
piano-roll maps to its step count at piano-roll.lisp:210). The arrangement
maps the same action to `song-set-end`, with `:content-length-min` set to
the last row's `start_beat` (the model rejects an end before it anyway) —
song-end editing costs no new widget code.

### 9.4 Snap

Arrangement lanes pass beat-grid snapping through the existing props
(`:snap`, `:resize-snap`, `:snap-mode` etc., as piano-roll does at
piano-roll.lisp:261-267), defaulting to 1-bar snap at the arrangement zoom
range with the same modifier-to-bypass behavior the widget already
implements. Unquantized row positions remain representable (capture writes
them, §5.6 accepts them); snap is a gesture default, not a model constraint.

## 10. Delivery slices

### Slice A: static layout, no editing

- `arrangement-track-lane` / `arrangement-scene-lane` composition per §4,
  including the one-ruler/headerless-lane prop discipline (§4.2).
- Shared view-start/duration/zoom/cursor/tool wiring (§5.1-5.2).
- Playhead bound to `song-position-beats` on every lane.
- Sparse rendering from lane projection (§6), read-only.
- No `TimelineItem` extension yet — plain colored blocks with labels
  (track color; `from_override` spans tinted per §5.5's render hint).

### Slice B: item kind/content

- `kind`/`content` fields on `TimelineItem` (§7), including the Lisp prop
  shape and lenient parsing.
- Lisp-side MIDI pattern flattening into `Dots`, recompute-on-change with
  the per-item dot cap (§7.1).
- Rendering for `Dots`; `Peaks` rendering stubbed/no-op until an audio-track
  asset pipeline exists.

### Slice C: editing

- Scene-lane gestures → song-mode primitives with the live-preview /
  commit-on-finish split (§9.1-9.2).
- `:resize-content-length` → `song-set-end` (§9.3).
- Snap defaults (§9.4).
- Undo/redo coverage inherited from the primitives themselves; rejection
  feedback in the status line.

### Slice D (future, tracks audio-track work): waveform peaks

- Peak-cache generation on asset load.
- `Peaks` rendering wired to real audio clips once audio tracks exist
  (`song-mode-spec.md` §16 lists audio tracks/clips as a future extension).

## 11. Deferred work (agreed, not yet built)

Decisions made during the track-clip editing rounds (2026-07), in rough
priority order. None require stored-model changes; each is its own slice.

1. **Per-clip phase anchoring** — patterns are transport-phase-locked
   (`pos_in_cycle = total_beats % cycle`, scheduler clock), so a clip
   placed at a non-multiple of its pattern length enters mid-pattern.
   Agreed fix: a preflight-DERIVED per-track anchor — phase zero is the
   beat where the track's resolved pattern last changed (walk rows,
   carry the anchor; unchanged pattern inherits it, which preserves the
   verified seamless row-split property). Clock becomes
   `(total_beats - anchor).rem_euclid(cycle)`. Scheduler-path slice with
   the mandated regression suite; care around sync-to-grid steps and the
   accumulator `cycle` counter.
2. **Whole-clip move on track lanes** — `:move-items-absolute` /
   `:finish-move-items` on track lanes are deliberately not lowered
   (§9.2). A moved clip should restart from its beginning, which is
   exactly what phase anchoring provides, so build the two together
   (lowering: silence the old span + paint the new one, or a dedicated
   compound primitive if two undo entries feel wrong).
3. **Editing during song playback** — primitives are locked while
   `SongPlayback`/`ArrangementCapture` are active (single launch
   authority; the scheduler plays prebuilt row snapshots). Rejections
   are now VISIBLE via `SEQ.song-edit-error` + the arrangement banner.
   Making edits audible mid-playback needs prepared-swap machinery:
   diff the committed song against the active `RuntimeSong`, rebuild
   affected row snapshots off the audio thread, hand over atomically
   (see the live-set prepared-swap design). Deliberately deferred until
   the stopped-transport interactions are settled.
4. **Clip copy/paste at the per-track cursor** — the cursor already
   carries (time, track) for this; paste = `song-track-paint` of the
   copied clip's pattern over [cursor, cursor + span).
5. **The rest of the UI follows the arrangement during playback** —
   synth/device panels, the scene launch buttons, mixer faders, and the
   mixer clip-grid selection currently do not update at all as song
   playback moves through rows; they keep showing whatever was live at
   transport start. The control-side mirror already applies each audible
   row (`apply_song_row_control` on `RowApplied` notices: track state
   restore, sampler rebinds, current-scene store), so the gap is the UI
   layer re-reading that state: reactive republish/invalidation on row
   transitions WITHOUT a pattern-epoch bump (the epoch stays untouched
   during playback so in-flight scheduled events are not dropped —
   spec 9's `bump_pattern_epoch: false` mirror contract).
6. **Polish backlog** — label eliding on narrow spans (labels are already
   hard-clipped to their clip bounds); an Ableton-style drop-hover insertion
   preview line while dragging a scene pill; scene-lane vertical scroll
   pass-through to the sibling track scroll container.
