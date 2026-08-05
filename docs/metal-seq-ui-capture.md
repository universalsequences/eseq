# Metal sequencer UI capture

`metal_seq capture` renders one sequencer buffer to a PNG without opening the
interactive app or an audio device. It uses the production sequencer state,
Lisp runtime, UI files, text measurement, and Metal widget renderer.

The capture input is a Lisp file with one declarative `capture-project` form.
That form creates the audio-graph/project structure before the rest of the file
is evaluated. All remaining forms are ordinary sequencer Lisp, so process
definitions, `processes`, `load`, and UI state changes use their normal runtime
implementations.

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
(track :sampler)
(track :instrument "saved/instrument-name")
(track :modulator)
(track :drum-rack :samples ("path/to/kick.wav" "path/to/snare.wav"))
(track :layer-rack :samples ("path/to/layer.wav"))
```

Every track accepts an optional display `:name`, initial `:solo` boolean,
`:midi-fx` list, and built-in `:audio-fx` list. `:num-steps` sets the initial
pattern length from 1 through the sequencer's maximum pattern length. A saved
instrument goes through the same compile/load/init path as an instrument added
in the app, so its real custom UI can be captured. Drum and layer racks accept
a `:samples` list through the production rack graph path. Drum-rack samples
populate consecutive pads beginning at the first pad; layer-rack samples are
added as broadcast layers. Sample paths are resolved relative to the capture
script.

`:steps` authors pattern content: a list whose entries are either a step index
or a `(step transpose)` pair, e.g. `:steps (0 4 (8 12) 12)`. The steps are
applied to the live pattern and then persisted into the scene's pattern pool
through the production scene-launch path, so pool-derived read surfaces (such
as the arrangement timeline's `song-lane-events` clip previews) observe them.

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
