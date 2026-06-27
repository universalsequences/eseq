# Inspect Mode

Inspect mode is a browser-devtools-style workflow for moving from a rendered UI
widget back to the Lisp source that produced it.

## Workflow

- Press `Ctrl+Shift+I` to enter inspect mode.
- Hover UI widgets to see inspect information in that tile's status line.
- The hovered widget is highlighted with a translucent rectangle overlay.
- Click the highlighted widget to open its source.
- Press `Esc` to leave inspect mode without opening source.

Inspect mode is global: after it is enabled, hover and click work across all
visible tiles. The status line and overlay are tile-local, so only the tile that
contains the hovered widget shows inspect details.

## Source Opening

Clicking a widget opens the source file associated with the widget metadata and
moves the cursor to the best available definition location.

The source pane behavior is intentionally stable:

- The source buffer opens to the right of the whole tile layout.
- Source buffers open in text-only mode.
- If the inspect source pane already exists, later inspect clicks reuse that
  pane instead of creating more code splits.
- Selecting another widget from the same file reuses the pane and moves the
  cursor.
- Selecting a widget from a different file loads that file into the same inspect
  source pane.

## What Gets Highlighted

The overlay uses the same layout node selected by inspect hit-testing. That means
the highlighted rectangle should match the widget whose status text and source
click behavior are active.

For nested UI, inspect mode prefers the deepest useful source-identifiable node,
not necessarily the deepest visual child. A node is considered useful when it has
metadata such as a debug name, stable key, explicit key, or source symbol.

## Source Metadata

Inspect mode depends on metadata stamped into widget layout nodes during Lisp
compilation and evaluation:

- source module path
- source buffer id when available
- source symbol/function origin when available
- widget identity hints such as `debug-name`, `key`, and stable keys

When exact metadata is available, inspect mode can jump to the widget form or
the producing function. When metadata is incomplete, it falls back to opening the
source file and logs the reason with an `[inspect]` prefix.

## Debug Output

Inspect source opening prints diagnostic lines like:

```text
[inspect] click widget=box stable_key=Some("seqv-step-cell-1-0") ...
[inspect] opening source module path /path/to/file.lisp
[inspect] resolved exact widget form at 42:7
[inspect] opened buffer=file.lisp path=/path/to/file.lisp cursor=42:7
```

Useful failure cases include:

- source metadata is missing
- a source file opens but no matching widget form is found
- inspect falls back to the top of the source buffer

## Current Limits

- This is not a full parser-backed source map yet.
- The styles pane equivalent does not exist; widgets do not have CSS-like style
  rules to inspect.
- Exact source jumps depend on the quality of widget metadata and Lisp source
  matching.
- TUI rendering approximates the translucent overlay with cell colors because
  terminal cells do not support alpha blending.

## Implementation Notes

The feature is split across three layers:

- Editor inspect state tracks the active mode, hovered tile, hovered widget,
  status text, and hovered layout rect.
- Tiled frame construction exposes per-tile inspect status and an
  `InspectOverlay`.
- Backends render the overlay as a transient visual layer. Metal draws a
  translucent fill plus border clipped to the tile content area; TUI draws a
  cell-based approximation.

Inspect overlays and status text are transient frame state and must not be
stored in inactive tile render caches.
