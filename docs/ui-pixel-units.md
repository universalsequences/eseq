# UI pixel-unit audit

The retained widget tree is laid out in **cells**, while GPU primitives use
framebuffer pixels. Raw visual constants are **design pixels** authored against
the original macOS 2x Retina renderer. Convert those values with
`widget_render::ui_design_px()` before combining them with `cell_w`, `cell_h`,
or framebuffer geometry. Values already derived from framebuffer/font metrics,
and intentional one-device-pixel raster operations, must not be converted.

This audit was performed for `eseq-linux.67` at the Linux workstation's 1.6x
window scale.

## Fixed design-pixel consumers

| Area | Classification and resolution |
| --- | --- |
| `box`, `button`, `dropdown`, `number-picker`, popup menu, modal, response-curve corner radii | Authored design-pixel props/defaults. Converted before normalization. |
| `box :border-width` | Authored design pixels. Border inset/strokes now use converted framebuffer width; inner radius subtraction remains in design space to avoid mixing units. |
| modal `:width-px` / `:height-px` | Authored against the 2x reference. Converted before division by physical cell dimensions. Cell-valued `:width` / `:height` remain unchanged. |
| image `:radius` | Authored design pixels; converted when creating the image primitive. |
| `event-view` cube/grid/point `*-radius-px` and projection inset | Authored design pixels; converted before cable/circle primitive creation and projection. |
| gate LED bezel/lamp insets, macro-owned indicator, EQ8 dots/handles | Authored design-pixel decoration constants; converted. Their centers and outer bounds remain cell/framebuffer-derived. |
| ADSR handles/brackets | Already fixed: the shader receives `ui_px_scale()` and applies it only to authored handle and bracket constants. |
| focus-corner arms/stroke/inset, linegraph stroke, matrix `:stroke-width` | Authored design-pixel visual weights; converted before cell/SDF normalization. |
| timeline piano-key end-cap and label-sound indicator | Authored design pixels; converted at their call sites. Timeline item radii are authored in cells and remain cell-derived. |
| tile borders, radii, split/outer gaps and content insets | Authored design pixels. Layout, hit testing, clipping and both GPU backends now use the same converted values, preventing chrome geometry from disagreeing with usable panel space. |
| WGPU grid font size | Fixed a backend sizing drift: WGPU used 15 pt while Metal used 16 pt. Both now consume one shared 16 pt constant because all authored cell dimensions depend on that grid. |

## Correct as-is

| Area | Why it must not use `ui_design_px()` |
| --- | --- |
| text measurement (`measure_text_px`, `line_height_px`) divided by `cell_w`/`cell_h` | Both numerator and denominator are physical metrics at the same window scale, yielding scale-independent cells. |
| explicit widget `:width`, `:height`, padding, gaps, stack/grid dimensions and aspect conversions | These are cell units, not pixels. This includes dropdown focus/border values that are applied directly as row/column extents. |
| framebuffer-derived radii and geometry | Gate LED outer bounds, `px_w`/`px_h`, waveform samples-per-pixel, NDC conversion, and pixel-aspect calculations already carry compositor scale through physical cell/viewport dimensions. |
| one-device-pixel raster work | Antialias padding, pixel snapping, hairline separators, minimum raster coverage, fwidth shader edges, and sub-pixel visibility thresholds intentionally track device pixels. |
| timeline item corner radius | `:item-corner-radius` is documented and stored in cells, then multiplied by `cell_h`; converting it again would double-scale it. |
| patcher geometry | Patcher constants participate in its own zoom/model-space and hit-test contracts and were audited separately under `eseq-linux.65`; this bead does not apply a second UI-scale transform. |


## Linux reproduction guard

`metal_seq_fx_space_echo_layout_contains_mode_grid_and_knobs` builds real
JetBrains Mono and system-UI font metrics at scale factor 1.6, lays the Space
Echo panel into the 41-by-11-cell boundary that fits the macOS-authored panel,
and verifies the functional knob controls have finite, nonzero geometry. This
keeps the concrete regression Linux-verifiable without launching a window.
