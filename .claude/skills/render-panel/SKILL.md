---
name: render-panel
description: Render an eseq UI panel (MIDI effect, audio effect, instrument, process panel) to a PNG headlessly with `metal_seq capture`, crop it to the panel, and look at it. Use whenever you write or change a ui.lisp / panel lisp and want to see the result instead of guessing at layout from widget sizes.
---

# Render a panel to PNG and look at it

`metal_seq capture` renders one sequencer buffer with the production Lisp
runtime, UI files and Metal widget renderer, no app window or audio device.
macOS only. Full reference: `docs/metal-seq-ui-capture.md`.

## Recipe (MIDI effect shown; audio fx / instruments are the same shape)

1. Write a capture script in the scratchpad. The `capture-project` form
   builds the project; everything after it is ordinary sequencer Lisp.

   ```lisp
   ;; $SCRATCH/cap-arp.lisp
   (capture-project
     (track :sampler :name "T1" :midi-fx ("arp")))
   ```

   Variants: `:audio-fx ("filter")` for builtin audio fx,
   `(track :instrument "core/drift")` for an instrument's custom UI,
   `:steps (0 4 (8 12))` for pattern content. Custom audio effects on disk
   use their folder name. An optional `(def capture-after-sync () …)` runs
   after SEQ is populated, for selecting a slot, opening a tab, etc.

2. Render wide. The `fx` buffer puts the track's instrument panel first, so
   at 1400px the effect panel is clipped off the right edge; 2400px shows it.

   ```sh
   cargo run -q -p sequencer --bin metal_seq -- capture \
     --script $SCRATCH/cap-arp.lisp --buffer fx --track 0 \
     --width 2400 --height 420 --out $SCRATCH/arp.png
   ```

   Other buffers: `--buffer synth` (instrument panel), `--buffer process`.
   First run pays the build; later runs take a few seconds each.

3. Crop to the panel with `sips` (no extra deps), then Read the PNG.

   ```sh
   sips -c 420 1100 --cropOffset 0 1300 $SCRATCH/arp.png --out $SCRATCH/arp-crop.png
   ```

   `-c HEIGHT WIDTH --cropOffset TOP LEFT`. For the `fx` buffer at 2400 wide,
   the first MIDI/audio effect panel starts around x=1300.

4. Loop: edit ui.lisp → re-run step 2 (custom UIs are re-read from disk on
   every capture) → Read the crop. Check clipping, empty space, label
   truncation and that no `missing: <param>` red label appears.

## Gotchas

- `[button-shader-watch]` lines on stderr are normal noise.
- Keep durable fixtures in `crates/sequencer/ui/capture-fixtures/`; throwaway
  scripts belong in the scratchpad.
- A layout test still has to assert finite nonzero rects; the PNG is for the
  visual review, not a substitute for the test.
- Accent policy (user preference 2026-09-05): one accent color per panel,
  usually `ui-accent-blue`. Don't rainbow the knobs.
