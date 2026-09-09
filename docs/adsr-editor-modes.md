# Envelope editor modes

`adsr-editor` uses the same drag capture, live preview and final `on-change`
commit lifecycle for four envelope shapes:

- Default: Attack, Decay, Sustain, Release.
- `:mode :decay`: bipolar Initial level and decay Time to zero.
- `:mode :ade`: Delay, Attack, Decay, End level, with optional gated decay and
  hold at note-off. The End level persists; this mode has no release-to-zero stage.

```lisp
(adsr-editor :mode :ade :width 32 :height 2.5
  :delay 20 :attack 10 :decay 400 :end 0.35
  :delay-max 5000 :attack-max 5000 :decay-max 12000
  :gated false :hold-on-release true
  :on-change (lambda (env) ...))
```

Delay, Attack, Decay and End are draggable. The Decay handle edits time
horizontally and End vertically; the final End handle edits only level.
Times use the editor's logarithmic spacing. `delay`, `attack`, `decay`, `end`,
`gated`, and `hold-on-release` accept reactive bindings. Flags accept booleans
or numeric 0/1 values. Mode selection remains an authored keyword.

The preview puts an illustrative note-off at the dotted vertical line.
Triggered mode decays before that line; gated mode waits at the peak and decays
after it. With gated hold enabled, the solid curve stays at the peak and the
unused decay is dimmed, while its handles remain editable. This is a contour
preview, not a live envelope monitor: an actual early note-off can hold a value
during Delay or Attack instead. The widget does not change DSP behavior.

ADE callbacks contain `:delay`, `:attack`, `:decay`, and `:end`, plus `:active`
(the dragged stage keyword, or false on mouse release). The final callback
commits the last dragged values even if host/reactive props have not caught up.
Gated/hold/reset controls remain ordinary parameter controls, not graph handles.

Factory UIs can call `custom-ui-set-envelope-in-scope` with `(stage parameter)`
pairs to apply an envelope atomically through the usual parameter/p-lock path.
`custom-ui-set-adsr-in-scope` is a wrapper for its original four-stage mapping.
Digi FM's A/B pages use the ADE mode; its Amp and Filter pages use default ADSR.

## Timed hold and exponential durations

`:mode :ahd` exposes `:attack`, `:hold`, and `:decay` durations (milliseconds),
with their corresponding `-max` limits. All three durations accept reactive
bindings. Set `:hold 0 :hold-editable false` for an Attack–Decay contour.
The callback contains those three stage values and the usual `:active` field.

For AHD and decay modes, `:decay-db 60` makes duration the time to fall 60 dB
(T60). The curve continues toward zero after that handle instead of falsely
reaching zero at the duration. Omitting it retains the normalized exponential.
`:initial-editable false` in decay mode hides the Initial handle; Melt uses
this for its release-only editor with initial level one. These shape and
editability options are static authoring props; timed values remain reactive.

Melt uses native ADSR for operators, AHD for amplitude, AD (zero hold) for its
filter, and decay for release. The amplitude release preview is separate
because note-off may occur during any AHD stage, releasing its current level.
