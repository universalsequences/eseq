# ADSR Editor Polish Spec

Status: implemented
Author: design pass, 2026-07-14
Related: `crates/eseqlisp/src/widget_render/adsr_editor.rs` (widget + Metal
shader), `crates/sequencer/metal-seq-fx/custom-ui-lego.lisp` (adsr lego
layouts)

## 1. Goal

Bring the `adsr-editor` widget up to Ableton-grade look and feel. Today it
reads as an engineering plot: thick saturated royal-blue polyline, heavy
gridlines, full border, dead side-panel layout, zero motion. Reference
targets are Ableton's Drift/Operator/Analog envelope displays: thin
desaturated-cyan exponential curves, corner-bracket framing, curve gets the
full panel width with values in a row beneath, and interaction feedback
(hover/drag states).

Two deliverables:

1. **Rendering** — restyle the Metal shader (curves, stroke, palette,
   framing, under-fill).
2. **Feel** — hover/drag feedback.

Plus a layout pass in the lego wrappers so the curve is the hero.

## 2. Locked decisions

1. **Linear attack, exponential fall curves.** Attack renders as a straight
   line. Decay and release segments render as exponential sags
   (`y = end + (start - end) * exp(-k*t)`, k ≈ 5). This is a *display* stylization first; the
   DSP `adsr` macro is unchanged in v1. If DSP slope lands later, the same k
   feeds both.
2. **Stroke is specified in pixels, not fwidth-of-uv.** Target ~2 px. The
   current `pixelDist * 1.35` inflates with widget aspect (why the line
   looks ~4 px chunky on wide panels).
3. **Palette: desaturated bright cyan curve (~#71BFCF), warm orange handles
   (keep), near-black plot.** High lightness / low chroma line. Curve color
   stays overridable via `curve-color` so per-env accents remain possible.
4. **Corner brackets replace grid + border.** No mid/top gridlines, no full
   border. Four curve-colored 16 px L-brackets frame the plot (Ableton's signature). A
   baseline hint at y=0 may stay at ≤0.08 alpha.
5. **Under-curve gradient fill.** Curve color at ~10% alpha under the line,
   fading to 0 at the baseline.
6. **Curve gets full panel width; A/D/S/R values in one row underneath.**
   `ui-detail-adsr-s` drops its 8.2-unit side block; badge/title becomes a
   small overlay or header row. Applies to all adsr lego variants. Wavetable
   uses a dedicated 1.75x-wide envelope slice with Amp/Filter tabs and divider
   rules below the tabs and graph.
7. **Handles: all five draggable or four drawn.** The (0,0) start handle is
   currently rendered but dead (`handle_idx > 0`); either give it a purpose
   (initial level) or remove it. v1: remove it.
8. **Release starts at a fixed timeline boundary.** Keep `release_start`
   pinned at 0.68 regardless of attack and decay. Attack and decay move only
   their handles inside the pre-release region, matching Ableton's stable
   envelope timeline.
9. **Wavetable uses one tabbed oscillator surface.** Osc 1 and Osc 2 share a
   single panel and use the same underline-tab language as the envelope tabs.
   Oscillator visibility is scope-local and independent of Amp/Filter envelope
   selection. The viewer is explicitly 3.5 layout rows tall so the new tab row
   and divider fit without overflowing or compressing controls implicitly.
10. **The attack origin sits inside the left brackets.** Envelope time zero is
    inset from the bracket corner and aligned with the inner endpoint of both
    left bracket arms. A zero-millisecond attack is a true vertical segment at
    that inset origin; zero must not receive a positive log-mapping floor. The
    envelope's 0–1 amplitude range is inset vertically as well, so peak and
    baseline handles align with the inner endpoints of the bracket arms rather
    than their outer corners.
11. **Oscillator enablement uses a toggle.** Osc 2's binary on/off parameter is
    shown as a native toggle rather than an option dropdown. Toggle `value` is
    a supported reactive prop and accepts the numeric 0/1 bindings used by
    instrument parameters.
12. **Wavetable filter uses the shared response curve.** The filter column is
    a full-height surface with the production `response-curve-editor` above
    large cutoff and resonance knobs. Mode, key tracking, envelope amount,
    base note, and volume remain compact alongside the knobs. Velocity
    sensitivity is removed from both the panel and the instrument signal path.

## 3. Phases

### Phase 1 — Shader restyle (an afternoon, biggest visual win)

All in `ADSR_EDITOR_SHADER` + the widget's default colors:

- Corner brackets; delete gridlines (`gridMid`/`gridOne`) and the full
  border pass. Keep faint y=0 baseline.
- Pixel-space stroke ~2 px (mirror the `handleHalfPx` approach).
- New defaults: curve `#71BFCF`-ish, 12 px handles
  (half-size ~6 px, ~1.5 px stroke).
- Under-curve gradient fill.
- Fixed release start (decision 8) — keep the 0.68 constant mirrored in
  `adsr_x_positions` (Rust) and the shader so hit-testing remains aligned.

### Phase 2 — Exponential curves + hover feedback

- Exp segments: subdivide decay/release into ~8 short
  segments inside the SDF loop, or distance-to-curve per-x. Update the
  under-fill test to the curve function.
- Hover/drag state: track hovered handle in the widget (mouse-move), pass
  the index via a free uniform (`uniform_b.y`). Shader renders that handle
  filled/enlarged and brightens the curve slightly. Drag renders the same
  as hover.
- Drag readout: while dragging, highlight the corresponding number in the
  row below (Ableton underlines the active value). Needs the lego layer to
  know the active handle — simplest: `on-change` payload gains an
  `:active` key naming the dragged param.

### Phase 3 — Lego layout pass

- `ui-detail-adsr-s`: full-width editor, values row beneath, badge as
  header/overlay (decision 6).
- Audit `ui-adsr` / `ui-lego-adsr-s` / `ui-adsr-c` heights so the editor
  takes the dead vertical space visible in analog-bread's mods column.
- Sanity-check all four core instruments (analog-bread, drift, operator,
  wavetable) + any fx panels using the adsr legos.
- Wavetable: collapse the two always-visible oscillator columns into one
  tabbed column, render only the selected oscillator subtree, and keep its
  selection independent from the envelope section state.

## 4. Out of scope (v1)

- DSP curve slopes / Time-vs-Slope editing (Ableton's mid-segment slope
  diamonds). Natural v2 once display curves exist.
- Per-segment loop modes, hold stages, breakpoint envelopes.
- Live playback cursor / any animated motion (dot traveling the curve,
  glow pulses). Explicitly cut — static rendering + interaction states
  only.
- Theming the rest of the instrument panels (multi-color knob accents vs
  Ableton's two-accent discipline) — separate design pass.

## 5. Gotchas / notes

- Geometry is **dual-maintained** between `adsr_x_positions` +
  `plot_point` (Rust, hit-testing/drag) and the shader's `adsr_point` +
  `adsr_toPlot`. Every geometry change lands in both or handles drift from
  the drawn curve. Same lesson as the wavetable widget's warp/fold math.
- `nearest_handle` uses a 1.0-cell hit radius; if handles shrink, keep the
  hit radius generous — visual size and hit size are independent.
- TUI fallback (`tui_render`) just prints values; unaffected, leave it.
- Layout tests won't catch visual regressions here — verify in the live
  app (`each`-vs-`map` lesson generalizes: trust the running UI).
