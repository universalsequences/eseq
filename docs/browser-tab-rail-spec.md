# Spec: Browser tab rail (macOS source-list style) + per-pane search

Companion to `docs/browser-builtin-flattening-spec.md` (independent — can land in
either order, touches different parts of the same file).

## Motivation

The browser's six tabs are currently a v-stack of bordered buttons
(`sbrowser-tab-button`), with the search bar spanning the full browser width
above both the tabs and the content. Two problems:

1. The tabs read as *form buttons*, not navigation. Border chrome + centered
   text + bright primary fill on the selected tab makes the least interesting
   selection on screen the loudest.
2. The search bar visually contains the tab column, implying it filters the
   tabs. It doesn't — it filters the active tab's list.

**Direction:** macOS source list, specifically the **Photos sidebar** variant
(not Ableton, not quite Finder). Reference behavior: idle rows are quiet
monochrome icon + white label with zero chrome; the selected row gets a soft
gray rounded fill and the icon *and* label flip to accent color. The loud
accent-blue selection stays reserved for the content tree (as today), giving a
deliberate loudness hierarchy: quiet nav selection, loud content selection.

## Target layout

```
┌─────────────┬───────────────────────────────┐
│             │ [ 🔍 Search audio effects... ]│
│  ♒ Samples  │ ┌───────────────────────────┐ │
│  🎹 Instrum │ │ + New Effect              │ │
│ ▐🎛 Audio FX│ │ Built-in                  │ │
│  ⌁ MIDI FX  │ │   EQ8                     │ │
│  ◉ Presets  │ │   Filter ...              │ │
│  📁 Projects│ │ Custom                    │ │
│             │ │   ...                     │ │
└─────────────┴───────────────────────────────┘
  rail (fixed)   content column (flex)
```

- The rail runs the **full height** of the browser; the search bar moves
  **inside the content column**, above the active tab's panel.
- The rail gets its own background zone: a fill 3–5% lighter or darker than the
  content pane background, so it reads as a distinct region without a border.

## Rail row spec

Per row: `icon + label`, left-aligned.

| State      | Fill                                   | Icon        | Label       |
|------------|----------------------------------------|-------------|-------------|
| idle       | transparent                            | white/gray  | white       |
| selected   | `rgba(1 1 1 0.07)`, rounded ~8px radius, inset ~4–6px from rail edges | accent blue | accent blue |
| hover      | **none** (macOS source lists don't hover) |          |             |

Metrics (tune by eye, these are the proportions that matter):
- Row height generous relative to text: ~2.0 units tall, font-size ~11–12,
  regular weight (not bold).
- Icon ~14–16px equivalent, ~0.6 unit gap to label, ~0.8 unit left inset.
- ~0.15–0.25 unit vertical gap between rows.
- No borders anywhere. No centered text. No `:variant :primary`.
- The rounded fill inset from the rail edges is the single most
  native-identifying detail — don't run it edge-to-edge.

Icons per tab (line-weight glyphs; reuse/extend the existing icon set — the
old toolbar used `:sampler` / `:waveform`):

| Tab         | Glyph                                            |
|-------------|--------------------------------------------------|
| Samples     | waveform                                         |
| Instruments | piano keys                                       |
| Audio FX    | sliders / knob                                   |
| MIDI FX     | note-with-arrow (DIN plug draws badly at 14px)   |
| Presets     | bookmark or dial                                 |
| Projects    | folder                                           |

If a decent glyph doesn't exist yet for some tab, ship that row icon-less
rather than with a bad glyph — monochrome-idle degrades gracefully.

Labels stay title case, full words ("Instruments", not "INSTR").

## Search bar

### Placement
- Remove `sbrowser-header` from the default (non-editor) widget list in
  `sbrowser-build-widgets` (~line 1128). The search field moves into the
  content column: rendered once, above the active tab panel, inside
  `sbrowser-tabbed-content`'s right-hand box — *not* per-panel copies.
- Keep the per-tab placeholder (`sbrowser-search-placeholder`) exactly as is.
- The Instruments panel's own `sbrowser-create-search-bar` (~line 576) becomes
  redundant — remove it (it shares `sbrowser-filter` with the header today,
  i.e. there are currently two fields bound to the same filter on that tab).
- Editor / preset-save / project-save modes are untouched (they already have
  their own headers).

### ⚠️ Stable-key contract (Rust side)
`src/ui/input.rs` focuses the search field by stable key
`"sbrowser-search-input"` (~lines 182, 212, 819–825: tab-cycling preserves
search focus). The relocated field **must keep** `:key "sbrowser-search-input"`
and there must be exactly **one** widget with that key in the non-editor
layout, or that logic silently breaks. There are tests around this in
`input.rs` (~line 1517) — run them.

### Behavior
1. **Clear filter on tab switch.** In `sbrowser-select-tab` (~line 311), reset
   `sbrowser-filter` to `""` (the existing tag-clearing for non-samples tabs
   stays). Rationale: one shared filter across tabs means a stale invisible
   filter silently empties the next tab's list. Also clear
   `sbrowser-preset-filter` if trivially reachable; otherwise leave it (it's a
   separate field on a separate pane).
2. **Cmd+F** focuses the browser search field when the browser is
   visible/focused. Check `input.rs` first — focus plumbing already exists
   (`focus_widget_by_stable_key`); this may be a small addition or already
   bound. Don't build a new focus system for this.
3. Search semantics inside each pane are unchanged (and the flattening spec
   defines the merged-flat-results behavior).

## Code seams

All in `content/ui/browser.lisp` unless noted:

- `sbrowser-tab-button` (~line 618): the whole restyle happens here — every
  tab renders through this one function. Rewrite as a source-list row
  (icon + label + state colors per the table above).
- `sbrowser-tabs` (~line 628): rail container — full-height, zone background
  fill, adjusted width (can shrink: it no longer aligns under a full-width
  search bar; size to longest label + icon + insets).
- `sbrowser-tabbed-content` (~line 828): insert the search bar at the top of
  the content column (v-stack: search, then active panel).
- `sbrowser-build-widgets` (~line 1113): drop `(sbrowser-header)` from the
  default branch.
- `sbrowser-header` (~line 453): becomes the relocated search widget (or is
  inlined into `sbrowser-tabbed-content`); keep the stable key.
- `sbrowser-select-tab` (~line 311): add filter clearing.
- `sbrowser-create-search-bar` / its use in the instruments panel (~lines
  576, 601): delete.
- `src/ui/input.rs`: verify focus/tab-cycle behavior still passes;
  add Cmd+F binding if not present.

## Tests

- `input.rs` focus tests (search-focus preserved across tab cycling,
  ~line 1517) must still pass with the relocated field.
- Add: switching tabs clears `sbrowser-filter` (e.g. set filter on samples
  tab, `(sbrowser-select-tab "instruments")`, assert tree is unfiltered).
- Layout tests per repo convention: generate any repeated rows with `each`,
  never `map`.

## Non-goals

- No hover states, no tab reordering, no collapsing the rail.
- No "All"/global-search pseudo-tab yet — but this layout is the prerequisite
  for it (search is per-pane, so "search all" can later be just another pane).
- No changes to editor / preset-save / project-save header modes.
- Tree selection color stays the current strong accent — the quiet rail only
  works if the content selection stays loud.
