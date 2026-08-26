# Shader-port golden captures

Each subdirectory is one run of a capture harness, named
`<shader-language>-<os>-<arch>`. There are two harnesses, writing the same
files from the same scene data (`crates/eseqlisp/src/capture_scenes.rs`,
schema v3):

* `eseqlisp_shader_capture` (`src/shader_capture.rs`) renders the WGSL
  pipelines through wgpu;
* `eseqlisp_metal_shader_capture` (`src/metal_shader_capture.rs`) renders the
  MSL pipelines the Metal backend compiles, on macOS.

Every capture holds:

* one `<scene>.png` per ported pipeline, 512×256 `Rgba8Unorm`, rendered
  offscreen with no window or swapchain involved;
* `manifest.json` — schema version, frame size, scene list, the adapter that
  produced the capture, and a sha256 per PNG.

These are reference data, not assertions. The self-consistency judgement that
`eseq-linux.7` requires is asserted directly in `shader_capture::tests` (color
relationships, clip geometry, draw order, mode selection); these PNGs exist so
the WGSL port can be diffed against the MSL original. That diff was made and
signed off in `eseq-linux.25` — see `JUDGEMENT.md` for the comparisons, the
thresholds and the verdict, and `compare_captures.py` to regenerate the delta
tables.

## Scenes

| Scene | Pipeline it drives | What it is arranged to show |
|---|---|---|
| `text` | `text_vert` + `text_frag` | Eight glyph cells, nearest-sampled, `mix(bg, fg, coverage)` with an alternating per-quad background |
| `proportional-text` | `text_vert` + `prop_text_frag` | The same cells linear-sampled, coverage as alpha, no background rect |
| `image` | `image_vert` + `image_frag` | Unclipped / rounded / circle-clipped / rotated-and-translucent quads |
| `patch-cable` | `patch_cable_vert` + `patch_cable_frag` | Two bezier cables of different radius and color, plus the segmented orthogonal router |
| `widget-surface` | widget preamble + `widget_vert` + `button_surface.{metal,wgsl}` | Four button surfaces: three corner radii and the splayed tab shape |
| `wavetable` | `wavetable_vert` + `wavetable_frag` | Sine/saw/square/triangle stacked bottom to top, with the morphed selected wave at `wave_pos` 1.6 |
| `waveform` | `waveform_vert` + `waveform_frag` | A decaying burst with a selection, both boundary flags, an active start marker and the playhead |
| `live-spectrogram` | `live_spectrogram_vert` + `live_spectrogram_frag` (MSL, one entry point branching on `mode`) or `live_spectrogram_waterfall_frag` / `live_spectrogram_eq_frag` (WGSL, one pipeline per mode) | Waterfall mode (left) beside EQ-curve mode (right) |
| `widget-<name>` | widget preamble + the named retained-mode widget fragment | Four fixed aspect/color/uniform combinations; one scene for every distinct hand-written widget fragment |

The widget scenes cover all 30 distinct fragment bodies ported by
`eseq-linux.8`, plus the editable button surface that `widget-surface` draws;
aliases that use the same body are intentionally not duplicated, and
`capture_scenes::tests` fails if a fragment exists in either language with no
scene drawing it.
All scene inputs are generated procedurally in `capture_scenes.rs` — no fonts,
no clock, no sample files — so the capture depends only on the shader language,
the GPU and the driver.

`widget-roar-shaper` and `widget-roar-filter` each draw four different modes,
one per instance, rather than four copies of one mode: their fragments select
a mode with `round(value_t)`, and the generic 0.5 is a tie the two shader
languages break in opposite directions (`eseq-linux.76`).

## Captures

### `msl-macos-arm64`

* The Metal reference: the shader sources `ui/metal_backend.rs` compiles,
  re-exported so the capture cannot drift from what the backend runs.
* Host: MacBook Pro, Apple M1 Max, macOS 26.5.1 (25F80), arm64.
* Display: built-in Liquid Retina XDR, 3024×1964, backing scale factor 2.0.
  Nothing in the capture depends on it — the harness renders into a 512×256
  offscreen texture with no window, layer or drawable.
* Adapter: `Apple M1 Max` via Metal.

### `wgsl-macos-arm64`

* The control: the same WGSL as the Linux capture, on the same GPU as the
  Metal capture. `msl-macos-arm64` vs this isolates the shader language;
  this vs `wgsl-linux-x86_64` isolates the GPU.
* Host, display and adapter: same machine as above, reached through wgpu's
  Metal backend.

### `wgsl-linux-x86_64`

* Source: the `eseq-linux.8` widget shader port on branch `eseq-linux`.
* Host: Omarchy (Arch Linux), kernel 6.19.8-arch1-3-surface, x86_64.
* Adapter: `Intel(R) UHD Graphics 620 (KBL GT2)` via Vulkan
  (`mesa 1:26.1.7-1`, `vulkan-intel 1:26.1.7-1`).
* **Still at schema 2**, because it can only be regenerated on the Linux host
  (`eseq-linux.77`). Two consequences, both surfaced by
  `compare_captures.py` rather than left implicit: it has no
  `widget-dropdown-checkmark` scene, and its `widget-roar-shaper` /
  `widget-roar-filter` scenes were drawn from the schema-2 uniforms, so those
  two rows in any comparison against it are meaningless.

## Determinism

Two consecutive runs on one host produce byte-identical PNGs and
`manifest.json`; `tests/shader_capture_cli.rs` and
`tests/metal_shader_capture_cli.rs` assert that on every run.

Across *different* adapters the PNGs will not be byte-identical — floating
point in the fragment stages is not bit-reproducible between GPUs. Compare with
a tolerance, the way the text goldens in `../../text-capture/goldens` are
compared, rather than by hash.

## Comparing

```sh
python3 compare_captures.py               # both default comparisons
python3 compare_captures.py --pairs a:b   # any other pair
python3 compare_captures.py --root DIR    # captures outside this directory
```

Stdlib only, like `../../text-capture/goldens/compare_metrics.py`.

## Regenerating

```sh
# WGSL, any platform with a wgpu adapter
cargo run -p eseqlisp --features wgpu --bin eseqlisp_shader_capture -- \
    --name <language>-<os>-<arch> \
    --output-dir crates/eseqlisp/tests/fixtures/shader-capture/goldens

# MSL, macOS only
cargo run -p eseqlisp --bin eseqlisp_metal_shader_capture -- \
    --name msl-<os>-<arch> \
    --output-dir crates/eseqlisp/tests/fixtures/shader-capture/goldens
```

The Metal harness renders into an `RGBA8Unorm` texture rather than the
swapchain's `BGRA8Unorm`, so its PNG bytes line up with the wgpu capture's
without a channel swizzle in between. Blending, clear value, viewport and
8-bit rounding are unchanged, so no fragment math differs.
