# Control palette roles

Sequencer themes live in `content/ui/themes/`. Control colors are independent
of the global accent: changing the accent must not recolor every enabled
button or replace an instrument's existing material treatment.

- `control-on-bg` / `control-on-fg`: lit state of mute/audio-passing, gate,
  warp, Poly, and other boolean parameter buttons.
- `scene-active-base` / `scene-active-fg`: selected scene pill material and
  label. The `*-base` slots are inputs to the existing lighting shader, not
  final pixel colors. Queued, pushed, hovered, and add/delete scene controls
  have their own material slots.
- `effect-mode-on-bg` / `delay-reverb-mode-on-bg`: delay selectors and sync
  controls; their labels use `control-on-fg`.
- `device-enabled` / `device-disabled`: instrument/effect enable indicators.
- `save-icon-fg`, `preset-save-*`, `icon-*`, `clock-fg`, and the transport
  material slots control icons without borrowing the primary-button palette.
- `number-slider-fill`, `sequencer-volume-handle`, `mixer-volume-handle`,
  `text-input-bg`, and `search-icon`: shared control details.
- `scene-clip-*`, `clip-label-fg`, `arrangement-*`, `timeline-*`, and
  `piano-*`: arrangement and piano-roll surfaces and markers.

The original material colors are explicit in the existing themes, including
macOS Dark's purplish-blue scene pills and yellow-orange on-state controls.
Phosphor supplies its own values. Theme switching must not leave these roles
inherited from whichever theme was active before it.

`track-tint` is display-only: its alpha blends authored track **and group**
colors toward the theme RGB. Alpha zero preserves authored colors; alpha one
replaces them. Mute/take dimming is applied afterward. Saved project colors
are never rewritten.

For headless visual checks, use
`crates/sequencer/ui/capture-fixtures/theme-controls.lisp` with `metal_seq
capture`. It covers sampler/Space Echo on track 0, sampler/Str8 Delay on track
1, and Digi Hat's instrument sliders on track 2. It also supplies a song for
arrangement/piano-roll captures. See [the capture guide](metal-seq-ui-capture.md).
