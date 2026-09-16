# Metal sequencer UI capture

`metal_seq capture` renders one sequencer buffer to a PNG without opening the
interactive app or an audio device. It uses the production sequencer state,
Lisp runtime, UI files, text measurement, and Metal widget renderer.

The capture input is a Lisp file with one declarative `capture-project` form.
That form creates the audio-graph/project structure before the rest of the file
is evaluated. All remaining forms are ordinary sequencer Lisp, so process
definitions, `processes`, `load`, and UI state changes use their normal runtime
implementations.

## Cropping a component after layout

`--key KEY` selects a widget's authored `:key` or an explicit `subtree :key`.
The buffer is laid out and rendered at `--width` / `--height` as usual. Only
then is the selected rectangle read from the rendered texture. Siblings,
parent constraints, wrapping and nested scroll offsets still determine its
shape. This is a screenshot crop: pixels overlapping that rectangle remain.

```sh
cargo run -p sequencer --bin metal_seq -- capture \
  --script crates/sequencer/ui/capture-fixtures/manual-components.lisp \
  --buffer sequencer --width 1600 --height 500 \
  --key track-step-grid-0 --padding 4 --out /tmp/step-grid.png
```

Use the same command with `--list-keys` to list keys after project setup and
layout, without writing an image. Namespaced stable keys are listed too; short
authored keys must match exactly one node. Missing or ambiguous keys, zero-size
nodes and clipped/offscreen components fail explicitly. Enlarge the viewport
or set scroll state in the fixture to show the whole component. Popup/modal
overlay geometry is separate from buffer layout and currently cannot be
selected by key; full-buffer captures still include overlays.

`--padding` adds pixels around the crop, clamped at the image edges; the default
is zero. Fractional layout edges round outward to avoid losing edge pixels.
Both options and `--list-keys` also work in `eseqlisp_capture` for standalone
widgets. Offscreen capture decodes referenced images before drawing its single
frame, so illustrations are present in captures of the manual itself.

`metal_seq capture --hide-status` lays out the isolated buffer without its
mode line. The manual generator uses this for clean figures, including buffers
whose root fills the viewport. `eseqlisp_capture` accepts this flag too.

## Manual illustration assets

Run `./scripts/refresh_manual.py` to regenerate every referenced screenshot and
diagram, update the in-app manual's PNGs, and export HTML to `../eseq-site/manual`.
See [Manual HTML export](manual-web-export.md) for options and adding images.

For screenshot-only work, run `python3 scripts/capture_manual_images.py`. It builds
`metal_seq` and renders the fixtures listed in
`crates/sequencer/ui/capture-fixtures/manual-images.json` to `docs/manual/images/`.
The manifest records each image's project, buffer, key, and surrounding viewport.
Pass image names to regenerate only those figures, for example
`python3 scripts/capture_manual_images.py step-grid piano-roll`.
Pass `--binary /path/to/metal_seq` to use an existing build.
The script stops on a failed capture; inspect regenerated PNGs before accepting
them. All manual chapters use these ordinary relative Markdown images, usable
in the in-app reader and the HTML export. Captures open no interactive window
and do not save projects, import staged samples, or export recordings.

To inspect the result in the real manual:

```sh
cargo run -p sequencer --bin metal_seq -- capture \
  --script crates/sequencer/ui/capture-fixtures/manual-first-session.lisp \
  --buffer manual --width 1200 --height 1100 --out /tmp/manual-first-session.png
```

## Project fixtures

```lisp
(capture-project
  (track :sampler
    :name "Sampler"
    :midi-fx ("arp")
    :audio-fx ("filter"))
  (track :instrument "core/drift"))

(load "@/scripts/processes/process-inlet-patch-demo.lisp")
(process-inlet-demo-attach-track 0)

;; Optional: runs after the project has been synchronized into SEQ.
(def capture-after-sync ()
  (process-panel-select-slot (nth SEQ.process-slots 0)))
```

Supported track forms are:

```lisp
(track :empty)
(track :sampler)
(track :instrument "saved/instrument-name")
(track :modulator)
(track :layer-rack :samples ("path/to/layer.wav"))
```

`capture-project` also accepts `(mod-route SOURCE TRACK INPUT)` entries.
Captures draw patch cables (mixer mod routes, the lane patchbay) with the
same pass as the live renderer, so a wired fixture shows its cables.
All three indices are zero-based; input must be 0–3. The capture installs the
route through the production graph controller after building the tracks, so
mixer cable captures use real routing state. Self-routes and missing endpoints
are rejected.

Use `(rack-slot-macro TRACK MACRO SLOT PARAM MIN MAX)` to map a rack macro to a
layer control in a capture. Indices are zero-based; the layer must be populated
by the track's `:samples` list. `PARAM` is `gain`, `pan`, `base-note`,
`max-polyphony`, `mute`, or `solo`. The range is linear. The
`rack-slot-macro-indicators.lisp` fixture compares mapped and unmapped layers.

Every track accepts an optional display `:name`, initial `:solo` boolean,
`:midi-fx` list, and `:audio-fx` list (builtin or saved effect names). Builtin
names take precedence; other names resolve through the normal saved-effect
library and compile/load/retain path, including local custom UIs. The optional
`spectral-tamer.lisp` and `spectral-tamer-mods.lisp` fixtures require that local
effect to be installed; use widths of 2800 and 3200 respectively to show the
whole sampler/effect strip.

`:num-steps` sets the initial
pattern length from 1 through the sequencer's maximum pattern length. A saved
instrument goes through the same compile/load/init path as an instrument added
in the app, so its real custom UI can be captured. Layer racks accept
a `:samples` list through the production rack graph path, added as broadcast
layers. Sample paths are resolved relative to the capture
script.

`:steps` authors pattern content: a list whose entries are either a step index
or a `(step transpose)` pair, e.g. `:steps (0 4 (8 12) 12)`. The steps are
applied to the live pattern and then persisted into the scene's pattern pool
through the production scene-launch path, so pool-derived read surfaces (such
as the arrangement timeline's `song-lane-events` clip previews) observe them.

`:step-params` authors step values, and `:instrument-locks` authors instrument
p-locks by parameter name in the instrument's stored units:

```lisp
(track :instrument "factory:Synths/Digi Drift"
  :steps (0 4 8 12)
  :step-params ((4 :velocity 0.5) (12 :duration 2))
  :instrument-locks ((8 "lp_freq" 650)))
```

Step parameter names use lowercase labels with hyphens, such as `:velocity`,
`:duration`, `:transpose`, and `:rate`. Unknown names, nonfinite numbers and
values outside the parameter range fail explicitly. These declarations use
the production app edit commands and are saved into the same pattern pool as
`:steps`. Expanded-lane projections are synchronized before the capture frame.
Use these declarations for musical state; the UI's `seq-set-step-param`
queues a live event-loop command, which the headless fixture does not dispatch.

From the repository root:

```sh
cargo run -p sequencer --bin metal_seq -- capture \
  --script crates/sequencer/ui/capture-fixtures/process-panel.lisp \
  --buffer fx \
  --track 0 \
  --width 2000 \
  --height 420 \
  --out /tmp/metal-seq-process-panel.png
```

`--buffer fx` and `--buffer '*fx*'` are equivalent. The selected buffer is
isolated from the app's tiled layout before rendering, which makes dimensions
stable and keeps the image focused on the panel under development. The command
is macOS-only because it uses the Metal capture backend.

The optional `capture-after-sync` function is useful for selecting a row,
opening an instrument tab, or otherwise establishing UI state that depends on
the populated `SEQ` namespace. It runs once after project/process state has
been synchronized and before the frame is rendered.

Keep durable visual fixtures in `crates/sequencer/ui/capture-fixtures/`. Layout
tests should still assert finite, nonzero widget rectangles; PNG capture adds the
visual review needed for spacing, typography, hierarchy, and clipping.

## Bus and group routing

`(group 0 1)` inside `capture-project` groups the named zero-based track indices
through the normal group creation path. Members must be distinct existing tracks.
The capture command also accepts the normal `add-bus` and `set-bus-output` host
commands. See `crates/sequencer/ui/capture-fixtures/bus-routing.lisp` for a mixer
capture with a group routed through several buses.

Layer racks can declare `:instruments ("factory:Synths/Digi Drift")` to load saved instruments into slots through the normal host path. When combined with `:samples`, instrument slots follow the sample slots. See `crates/sequencer/ui/capture-fixtures/rack-slot-presets.lisp`.

## Tiled capture and scroll replay

`--all-panels` preserves the fixture's panel layout and focuses the existing
tile named by `--buffer`. It fails if that buffer is not visible, so selecting
FX cannot replace the transport or another panel. This path uses the production
tiled renderer. Without `--all-panels`, a scroll replay isolates the selected
buffer first.

```sh
cargo run --release -p sequencer --bin metal_seq -- capture \
  --script crates/sequencer/ui/capture-fixtures/renderer-scroll.lisp \
  --buffer fx --all-panels --width 2400 --height 1400 \
  --scroll-frames 240 --scroll-x 220 \
  --out /tmp/renderer-scroll.png
```

Scroll distances are in layout cells; `--scroll-y` selects vertical movement.
The replay moves from zero to the requested distance and back, subject to the
buffer's actual scroll limits. It warms the same positions once, then records
the second pass. The JSON beside the PNG includes the active buffer and actual
scroll offsets, CPU frame-building/submission times, scene preparation, GPU
time, visible primitives, rebuilt/reused/culled nodes, reindexed nodes, refreshed
bounds, compiled cache hits and misses, and new static GPU geometry allocations. The PNG contains the final
frame. Use `--scroll-frames 2` to capture the maximum scroll position.

CPU timing excludes waiting for GPU completion. This is a headless project
replay without an audio device, playback scheduling, event delivery, or display
presentation latency; it does not measure whole-app input latency. Use a release
build and compare saved baseline/candidate binaries with identical fixtures and
viewport sizes. Report live playback and event-to-present measurements separately.
