# Spec: Flatten built-ins into the browser tree lists (Instruments + Audio FX)

## Motivation

Built-in instruments currently live in a 2×2 button toolbar above the instrument
tree (Sampler / Mod / Rack / Layer), while built-in audio effects hide inside a
collapsed `Built-in` folder. Both are friction: the toolbar is a second UI idiom
that doesn't participate in search and forces cramped names ("Rack", "Layer"),
and the collapsed folder costs a click every time the Audio FX tab is opened.

**Goal:** one unified, always-visible, searchable list per tab. Built-ins appear
as bare rows at the top of the tree (no collapsible wrapper), followed by the
custom library. Zones are separated by non-interactive section **headers**
(muted labels in the existing "Library" style), never by collapsible folders.

## Target layout

**Instruments tab** (replaces the Sampler/Mod/Rack/Layer toolbar):

```
[ Search instruments... ]
┌─────────────────────────┐
│ Built-in        (header)│
│  🔲 Sampler             │
│  🔲 Modulator           │
│  🔲 Drum Rack           │
│  🔲 Instrument Rack     │
│ Library         (header)│
│  > arcade               │
│    badmallet            │
│  > bass                 │
│  ...                    │
└─────────────────────────┘
```

**Audio FX tab** (replaces the Built-in/Custom collapsible sections):

```
[ + New Effect ]           ← unchanged, stays a button (action, not library item)
┌─────────────────────────┐
│ Built-in        (header)│
│  🔲 EQ8                 │
│  🔲 Filter              │
│  🔲 Space Echo          │
│  ... (~12 builtins)     │
│ Custom          (header)│
│  my-effect              │
│  ...                    │
└─────────────────────────┘
```

MIDI FX tab: no change (already flat).

## Naming

Built-in instrument rows use full names (list rows have width; the toolbar didn't):

| Row label         | Existing action handler            |
|-------------------|------------------------------------|
| Sampler           | `sbrowser-add-sampler-track`       |
| Modulator         | `sbrowser-add-modulator-track`     |
| Drum Rack         | `sbrowser-add-rack-track`          |
| Instrument Rack   | `sbrowser-add-layer-rack-track`    |

⚠️ Confirm the Rack/Layer → Drum Rack/Instrument Rack mapping with the user
before shipping; the old button labels don't disambiguate which is which.

## Behavior

1. **Headers are not folders.** A header row ("Built-in", "Library", "Custom")
   is non-interactive: no disclosure triangle, not selectable, not activatable,
   skipped by keyboard navigation. Styled like the existing `sbrowser-library-label`
   (font-size 10, gray, transparent bg) but rendered *inside* the tree so it
   scrolls with content. The standalone `sbrowser-library-label` widget above
   the instrument tree is removed (the header replaces it).
2. **Empty sections drop their header** (e.g. no custom effects saved → no
   "Custom" header), mirroring the existing `effect_section` behavior.
3. **Search flattens.** When the query is non-empty, omit all headers and show
   one flat merged result list — built-ins and customs filtered by the same
   query, built-ins first. (Custom instrument folders already auto-expand via
   `:expand-all`; keep that.)
4. **Icons.** Built-in rows carry a device glyph to distinguish them from custom
   items (reuse the `:sampler` / `:waveform` icons from the old toolbar buttons).
   If the `tree` widget doesn't support per-item icons yet, add that; if it's
   disproportionate effort, ship without icons — the headers carry the grouping.
5. **Activation** adds a track / effect exactly as the old toolbar buttons and
   the old builtin-effect rows did. Single-click select → status text; activate
   (double-click/enter, whatever `:on-activate` currently means) → add. Same as
   custom items today.
6. **Drag-and-drop:** builtin instrument rows are *not* draggable into folders
   and are not valid drop targets (`sbrowser-drop-instrument-on-folder` should
   ignore them).

## Where the code lives

### Data (Rust)

- `crates/sequencer/src/bin/metal_seq/browser.rs`
  - `build_instrument_tree_value` (~line 570): currently returns only the
    scanned `instruments/` directory tree. Prepend the four builtin leaves
    (subject to the query filter) and the header items.
  - `build_audio_effect_tree` (~line 735): currently wraps builtins/customs in
    collapsible `kind: "section"` nodes via `effect_section`. Replace with flat
    leaves + header items per this spec. Builtin names come from
    `EffectDescriptor::builtin_insert_names()` + `conv_reverb::NAME` (keep that).
- Suggested item shapes (dict fields already used by the tree):
  - header: `{label: "Built-in", kind: "header"}` (no name, no children)
  - builtin instrument: `{label: "Drum Rack", name: "rack", kind: "builtin-instrument"}`
  - builtin effect: unchanged `kind: "builtin-audio-effect"` leaves, now at root.
- Doing this Rust-side (rather than in lisp) keeps search filtering in one place.

### UI (eseqlisp)

- `crates/sequencer/metal-seq-browser.lisp`
  - `sbrowser-create-toolbar` (~line 565): delete (the four buttons move into
    the tree). Keep `sbrowser-add-*-track` functions — they become the
    activation targets.
  - `sbrowser-create-picker` (~line 626): drop the toolbar and
    `sbrowser-library-label` from the stack.
  - `sbrowser-select-create-item` / `sbrowser-focus-create-item` (~line 526):
    handle `kind = "builtin-instrument"` by dispatching on `:name` to the four
    add-track functions; ignore `kind = "header"`.
  - `sbrowser-select-audio-effect` / `sbrowser-activate-audio-effect`
    (~line 387): already handle `builtin-audio-effect` / `custom-audio-effect`
    leaves; just make sure `header` is inert.
  - The Samples tab also has Rack/Layer buttons (~line 672). **Out of scope** —
    leave them; revisit after this lands.

### Tree widget

- If the `tree` widget has no non-interactive header/divider item kind or
  per-item icons, add minimal support in the widget (Rust side). Header rows:
  smaller muted label, extra top padding, no hover highlight, no hit-testing
  for selection.

## Tests

- Update/extend the tree-building tests in
  `crates/sequencer/src/bin/metal_seq/state_values.rs`
  (e.g. `metal_seq_audio_effect_tree_excludes_new_effect_action` ~line 10167):
  - instrument tree starts with the four builtins in the order Sampler,
    Modulator, Drum Rack, Instrument Rack, followed by headers/folders
  - query "samp" returns Sampler + matching customs, and no header items
  - audio effect tree has no `kind = "section"` nodes anymore
  - empty custom list → no "Custom" header
- UI layout tests: per repo convention, generate widget children with `each`,
  not `map` (map passes layout tests but breaks live).

## Non-goals

- No changes to MIDI FX, Samples, Presets, or Projects tabs.
- No renaming of the underlying host commands or track kinds — display labels only.
- No collapsible "Built-in" folders anywhere; that's the pattern being removed.
