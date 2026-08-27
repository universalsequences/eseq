# MSL-vs-WGSL shader-port judgement (eseq-linux.25)

Sign-off for the WGSL ports of the 8 core pipelines (`eseq-linux.7`) and the
widget fragments (`eseq-linux.8`), judged on macOS against the committed
three-way golden set:

| # | capture | shader language | GPU | what it isolates |
|---|---------|-----------------|-----|------------------|
| 1 | `msl-macos-arm64`   | MSL  | Apple M1 Max | the reference the port was made from |
| 2 | `wgsl-macos-arm64`  | WGSL | Apple M1 Max | 1-vs-2 = the shader language, same GPU |
| 3 | `wgsl-linux-x86_64` | WGSL | Intel UHD 620 | 2-vs-3 = the GPU, same shader |

Regenerate the numbers with `python3 compare_captures.py` (stdlib only).

Capture 2 is the whole point of the design. A `msl-macos` vs `wgsl-linux` diff
would confound the shader port with a different GPU, a different driver and a
different vendor's float behaviour, and could not attribute anything. Running
both languages on the same M1 Max removes the GPU from the comparison
entirely, so 1-vs-2 is the port and nothing else — and 2-vs-3 measures, on the
same shader, how much divergence a GPU change alone causes.

Host for captures 1 and 2: MacBook Pro, Apple M1 Max, macOS 26.5.1 (25F80),
arm64; built-in Liquid Retina XDR, 3024×1964, backing scale factor 2.0. The
scale factor is recorded for the record only: the harnesses render into a
512×256 offscreen texture with no window, layer or drawable, so no display
property can reach the pixels.

## Threshold and rationale

**A scene passes iff its 1-vs-2 delta is either zero, or is confined to
antialiased boundary pixels at a magnitude of 1/255.**

Rationale: these captures are 8-bit quantisations of identical arithmetic. The
two shader compilers may associate or contract float expressions differently,
which can move a value by one unit in the last place — and at a boundary
pixel, one ulp either side of a `smoothstep` edge is one LSB of output. That is
the smallest representable disagreement and cannot accumulate: it does not
shift geometry, change a branch, or alter a colour anyone can see. Anything
larger means the two shaders computed *different things*, which is a port bug
and must be attributed.

Note what this threshold deliberately does **not** allow for. The bead
anticipated a systemic gamma or sRGB-surface-format offset across every
capture. There is none: 37 of 38 scenes are byte-identical, so a constant
colourspace offset can be ruled out by observation rather than argued about.
Both harnesses render into a linear `*8Unorm` texture with no swapchain
involved, which is why.

## Verdict: PASS

**1-vs-2, the shader port — 37 of 38 scenes byte-identical.**

The whole delta table is zeros except one row:

| scene | maxd | mean | diff% | >8% |
|---|---|---|---|---|
| `patch-cable` | 1 | 0.000 | 0.00% (2 px of 131 072) | 0.00% |

The two pixels are (211, 148) and (198, 150), both on the thicker orange
bezier cable, and both differ by one step in the blue channel only —
`[123, 83, 51]` vs `[123, 83, 52]`, `[250, 224, 202]` vs `[250, 224, 203]`.
In bounds by the threshold above, and attributed: `patch_cable_curve_distance`
takes the minimum over 24 straight-line approximations of the cubic bezier,
each one a chain of bezier evaluation, dot products and a `length` — by a wide
margin the longest float expression chain in the set, and the one place where
a compiler's freedom to contract a multiply-add can still be seen after 8-bit
quantisation. The segmented router in the same capture, which is plain
straight-line distance, is byte-identical.

Every remaining scene — all 8 core pipelines and all 31 widget scenes, one
per distinct fragment body,
including the four modes each of `widget-roar-shaper` and
`widget-roar-filter` — is **byte-identical between MSL and WGSL**. Nothing is
filed against a widget or a pipeline, because there is nothing left to file.

**2-vs-3, the GPU — the noise floor, for context.** Not this bead's subject:
both sides run the same WGSL, so nothing here is a port defect. It is
reported because it is what makes the 1-vs-2 result meaningful — the same
comparison across a GPU change is two to three orders of magnitude noisier:

| scene | maxd | mean | diff% |
|---|---|---|---|
| `waveform` | 243 | 0.001 | 0.05% |
| `widget-knob` | 224 | 0.021 | 0.79% |
| `text` | 203 | 0.216 | 16.17% |
| `widget-button` | 92 | 0.005 | 0.60% |
| `widget-patcher-panel` | 89 | 0.005 | 0.33% |

Characterised rather than assumed. For `text`, 20 496 of the 21 199 differing
pixels differ by exactly 1, and the differing region's bounding box is exactly
the glyph band (y 96..151) — i.e. LSB rounding of the same coverage blend. The
576-pixel tail above 32 sits at texel boundaries of the nearest-sampled atlas,
where a half-texel difference in interpolated uv selects the neighbouring
texel. `widget-timeline-cursor-marker` differs on 16.70% of pixels at a maximum
of 1: a gradient, quantised one step apart. Same shape in `widget-knob`: 869
of 1 033 differing pixels differ by 1.

Capture 3 was regenerated at schema 3 under `eseq-linux.77`, so this column
now covers all 38 scenes, the two `widget-roar-*` rows included. Both land at
the quiet end of the noise floor — maxd 1 over 0.15% and 0.02% of pixels — as
expected once the mode selectors are off the tie.

## Filed

Both from the diff review, neither blocking the sign-off:

* `eseq-linux.76` — MSL `round()` breaks a tie away from zero where WGSL
  rounds half-to-even. Found because the schema-2 scenes fed the roar mode
  selectors exactly 0.5, so the two backends drew different modes (maxd 184
  and 186, ~2.5% of pixels). Confirmed by moving the input off the tie, which
  made both scenes byte-identical. No shipped widget passes a tie today; the
  bead is to make the two languages agree by construction rather than by luck.
* `eseq-linux.77` — regenerate `wgsl-linux-x86_64` at schema 3 on the Linux
  host. Done; the 2-vs-3 numbers above are from the regenerated capture.

One gap was found and closed rather than filed: `dropdown-checkmark` had a
fragment in both languages and a capture scene in neither, so
`eseq-linux.8`'s port of it had never been diff-reviewed. It is now
`widget-dropdown-checkmark`, byte-identical, and `capture_scenes::tests`
fails from here on if any fragment is left without a scene.
