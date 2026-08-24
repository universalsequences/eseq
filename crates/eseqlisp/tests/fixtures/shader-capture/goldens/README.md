# WGSL shader-port golden captures

Each subdirectory is one run of the `eseqlisp_shader_capture` harness
(`crates/eseqlisp/src/shader_capture.rs`, schema v2), named
`<shader-language>-<os>-<arch>`. Every capture holds:

* one `<scene>.png` per ported pipeline, 512×256 `Rgba8Unorm`, rendered
  offscreen with no window or swapchain involved;
* `manifest.json` — schema version, frame size, scene list, the adapter that
  produced the capture, and a sha256 per PNG.

These are reference data, not assertions. The self-consistency judgement that
`eseq-linux.7` requires is asserted directly in
`shader_capture::tests` (color relationships, clip geometry, draw order, mode
selection); these PNGs exist so `eseq-linux.25` can capture the same scene set
through the Metal pipelines on macOS and diff the two.

## Scenes

| Scene | Pipeline it drives | What it is arranged to show |
|---|---|---|
| `text` | `text_vert` + `text_frag` | Eight glyph cells, nearest-sampled, `mix(bg, fg, coverage)` with an alternating per-quad background |
| `proportional-text` | `text_vert` + `prop_text_frag` | The same cells linear-sampled, coverage as alpha, no background rect |
| `image` | `image_vert` + `image_frag` | Unclipped / rounded / circle-clipped / rotated-and-translucent quads |
| `patch-cable` | `patch_cable_vert` + `patch_cable_frag` | Two bezier cables of different radius and color, plus the segmented orthogonal router |
| `widget-surface` | widget preamble + `widget_vert` + `button_surface.wgsl` | Four button surfaces: three corner radii and the splayed tab shape |
| `wavetable` | `wavetable_vert` + `wavetable_frag` | Sine/saw/square/triangle stacked bottom to top, with the morphed selected wave at `wave_pos` 1.6 |
| `waveform` | `waveform_vert` + `waveform_frag` | A decaying burst with a selection, both boundary flags, an active start marker and the playhead |
| `live-spectrogram` | `live_spectrogram_vert` + `live_spectrogram_waterfall_frag` / `live_spectrogram_eq_frag` | Waterfall mode (left) beside EQ-curve mode (right) |
| `widget-<name>` | widget preamble + the named retained-mode widget fragment | Four fixed aspect/color/uniform combinations; one scene for every distinct hand-written widget fragment |

The widget scenes cover all 29 distinct fragment bodies ported by
`eseq-linux.8`; aliases that use the same body are intentionally not duplicated.
All scene inputs are generated procedurally in `shader_capture.rs` — no fonts,
no clock, no sample files — so the capture depends only on the GPU and driver.

## Captures

### `wgsl-linux-x86_64`

* Source: the `eseq-linux.8` widget shader port on branch `eseq-linux`.
* Host: Omarchy (Arch Linux), kernel 6.19.8-arch1-3-surface, x86_64.
* Adapter: `Intel(R) UHD Graphics 620 (KBL GT2)` via Vulkan
  (`mesa 1:26.1.7-1`, `vulkan-intel 1:26.1.7-1`).

## Determinism

Two consecutive runs on this host produced byte-identical PNGs and
`manifest.json`; `tests/shader_capture_cli.rs` asserts that on every run.

Across *different* adapters the PNGs will not be byte-identical — floating
point in the fragment stages is not bit-reproducible between GPUs. Compare with
a tolerance, the way the text goldens in `../../text-capture/goldens` are
compared, rather than by hash.

## Regenerating

```sh
cargo run -p eseqlisp --features wgpu --bin eseqlisp_shader_capture -- \
    --name <language>-<os>-<arch> \
    --output-dir crates/eseqlisp/tests/fixtures/shader-capture/goldens
```
