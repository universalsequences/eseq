# Digi Drift source preview

`content/instruments/Synths/Digi Drift/ui.lisp` binds the `drift-waveform`
widget directly to parameter float slots and the five additive host-modulation
offsets used by its knobs. It fills the space beside the noise controls.
Filter routing, cutoff, resonance, drive, output volume and `ui_epoch` are not
preview inputs.

The Rust evaluator in `crates/eseqlisp/src/widget_render/drift_waveform.rs`
is dual-maintained with the instrument's `morph-osc`, `basic-osc`,
`osc-frequencies` and `source-mixer` macros. It includes both wave selectors,
shape, octave ratios, osc2 semitone detune, on/off gates, dB gains and the
noise mute threshold. DGen's basic `triangle` is **unipolar**; osc1's authored
asymmetric triangle is bipolar. Preserve this distinction when updating it.

This is a phase-aligned cycle diagram, not captured audio. It plots one
base-pitch cycle at 512 intervals, with polyBLEP evaluated at that diagram
sampling rate. Octave and detune can make the two sources non-periodic over
that window. A fixed-seed white-noise realization avoids meaningless animation.
The plot preserves amplitude on a +/-2 axis, expanding headroom when necessary
rather than clipping the sum. It does not simulate free-running voice phase,
analog drift, envelopes or the internal modulation matrix: the current knob
telemetry publishes host modulation, not those internal voice signals.

A bounded, UI-thread LRU retains 128 effective input snapshots and their sample
buffers. Identical patches share samples; deleted widget IDs do not accumulate
state. The common anti-aliased stroke mesh renders the result on Metal and
wgpu without a new shader or audio-thread work. `evaluation_count()` exposes
cache misses for probes; changing colors or geometry does not evaluate sound.

## Checks

```sh
cargo nextest run -p eseqlisp --lib -E 'test(widget_render::drift_waveform::tests::)'
cargo nextest run -p sequencer --bin metal_seq \
  -E 'test(=state_values::tests::drift_waveform_tests::digidrift_preview_layout_live_bindings_and_idle_probe)' \
  --no-capture
cargo run -p sequencer --bin metal_seq -- capture \
  --script crates/sequencer/ui/capture-fixtures/digidrift-preview.lisp \
  --buffer fx --track 0 --width 1800 --height 420 \
  --out /tmp/digidrift-preview.png
```

The production-Lisp test checks visible nonzero geometry, actual `ReactiveRef`
props, all five live modulation offsets, and zero additional oscillator
evaluations across 120 draws with changing filter/epoch values. It reports draw
time diagnostically, without a machine-dependent timing ceiling. Capture is
macOS-only; inspect the PNG as well as running the geometry test.
