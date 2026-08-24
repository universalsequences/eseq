# Fontdue-vs-CoreText delta judgement (eseq-linux.20)

Sign-off for the CoreText → fontdb/fontdue glyph-atlas swap (`57b76e2a`),
judged on Linux against the committed three-way golden set:

| # | capture | source commit | what it isolates |
|---|---------|---------------|------------------|
| 1 | `coretext-macos-arm64` | `10c0e912` (= `57b76e2a^`) | pre-swap baseline |
| 2 | `fontdue-macos-arm64`  | `7664460f` | 1-vs-2 = rasterizer swap, same host |
| 3 | `fontdue-linux-x86_64` | `3fd9cf96` | 2-vs-3 = platform, same rasterizer |

Regenerate the numbers in this document with
`python3 compare_metrics.py` (full per-size/per-scale delta tables; stdlib
only). PNG diff stats below came from ImageMagick `compare -metric AE/RMSE`
on the mono region (`960x250+0+110`) and proportional region (`960x110+0+0`)
of `text.png`.

## Threshold ("bounded") and rationale

**A metric delta is in bounds iff |Δ| < 0.5 px at scale 1.0, per field
(`cell_w`, `cell_h`, `ascent`, `descent`, `leading`), against capture 1.**

Rationale: these numbers feed integer-quantised consumers, so half a pixel is
the smallest change that can have any discrete effect:

* `cell_w`/`cell_h` drive the UI grid (`WidgetViewport.cell_w/cell_h`, the
  `text_cell_w/h` layout in `metal_backend.rs`). A drift that still rounds to
  the same integer cell cannot reflow anything; a whole-pixel drift reflows
  the entire grid. 0.5 px is the rounding boundary.
* `ascent`/`descent`/`leading` position glyphs vertically, and
  `copy_glyph_into_line` rounds `dst_y` to whole rows, so a sub-half-pixel
  baseline drift cannot move any glyph by a row.

Sub-half-pixel drift is therefore cosmetic by construction; anything at or
above it is treated as a reflow and must be separately attributed and either
accepted deliberately or filed.

**For appearance (PNG):** no glyph displacement is permitted — the diff
against capture 1 must be edge-outline only (antialiasing weight), never
ghosting/doubling. Coverage-weight differences are accepted as inherent to
unhinted linear-coverage rendering.

## Findings

### 1. The rasterizer swap itself: metric delta is zero

Monospace is the only clean rasterizer comparison (both macOS captures
resolve the same `JetBrainsMono-Regular` file; the proportional captures
resolve different faces, see finding 3). Over every captured scale, fontdue
reproduces CoreText's `cell_w`, `cell_h`, `ascent`, `descent`, `leading` and
all 95 printable-ASCII advances **exactly, to the 4-decimal JSON precision**
(worst raw diff 0.0002 px, f32 print rounding). Example, 16 pt @ 2.0×:

```
coretext-macos-arm64  cell_w=20.0 cell_h=43.0 ascent=32.6401 descent=9.6001 leading=0.0
fontdue-macos-arm64   cell_w=20.0 cell_h=43.0 ascent=32.6400 descent=9.6000 leading=0.0
```

The mono grid — the thing the whole UI lays out on — did not move at all.
**PASS, zero margin consumed.**

### 2. The rasterizer swap, appearance: thinner antialiasing, no displacement

The mono-region pixel diff between captures 1 and 2 is a hollow outline of
each glyph: same shapes, same positions, different edge coverage (2,667 of
240,000 pixels differ; max channel Δ 0.77; RMSE 6.8%). This is the predicted
consequence of fontdue emitting linear, unhinted coverage where CoreText
applies font smoothing and gamma — text reads slightly thinner. With metrics
bit-identical and no displacement, this is cosmetic. **ACCEPTED**; optional
stem-darkening/gamma filed as backlog `eseq-linux.43` should the weight ever
become a real complaint.

### 3. Every proportional delta is font *selection*, not the rasterizer

The proportional columns exceed the threshold in both comparisons — and in
both, the resolved face changed, so the rasterizer is not the cause:

* **1-vs-2** (`.SFNS-Regular` → `Helvetica`, same host): `cell_h` −1…−6 px,
  `ascent` up to −6.3 px, `m` advance within ±0.55 px. fontdb does not
  enumerate the hidden macOS system face, so `load_system_ui_font` falls
  through to Helvetica. Filed as **`eseq-linux.42`** (P2).
* **2-vs-3** (`Helvetica` → `NimbusSansNarrow-Regular`): `m` advance/`cell_w`
  −0.98…−4.8 px (condensed face), `leading` +1.3…+6.4 px, `cell_h` +1…+7 px.
  This is exactly the already-filed **`eseq-linux.19`** (P2): the Linux
  fallback query resolves a condensed sans. Charged to font selection, not
  fontdue.

Supporting evidence that the metric pipeline is face-faithful: whenever the
font file is held constant (finding 1), the deltas vanish; and finding 4
shows the rasterizer is deterministic across platforms. Nothing here
suggests tuning fontdue — the fix in both cases is picking the right face.

### 4. The platform contributes nothing once the face matches

Between the two fontdue captures, the mono region of `text.png` is
**pixel-identical** (AE = 0) and all mono metrics are byte-equal in the
JSON — even though Linux resolves the Nerd-Font build
(`JetBrainsMonoNF-Regular` vs `JetBrainsMono-Regular`), whose ASCII outlines
and metrics are unpatched. Noted for awareness; no action needed. The entire
2-vs-3 image diff is confined to the two proportional lines (finding 3).

## Verdict

**Signed off.** The rasterizer swap is metrically exact where comparable and
visually an antialiasing-weight change only. All out-of-bounds deltas are
attributed to font selection and tracked: `eseq-linux.19` (Linux condensed
face, pre-existing), `eseq-linux.42` (macOS Helvetica-for-.SFNS, filed by
this judgement), plus cosmetic backlog `eseq-linux.43`. Re-judge against the
same thresholds after either font-selection fix by re-running the harness
and `compare_metrics.py`.
