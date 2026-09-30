Critical effect DSP rules:
- There is no top-level wrapper form. A complete effect is top-level `(def ...)`,
  `(defmacro ...)`, `(param ...)`, `(in ...)`, and `(out ...)` forms.
- Prefer `(defmacro ...)` for complex, self-contained DSP computations that have
  clear inputs and one conceptual result, even when the helper is only used once.
  Use this to name and isolate meaningful logic, not to hide simple one-line
  arithmetic or split tightly coupled signal flow.
- Never write `(defeffect ...)` in dsp.lisp. Never put the effect name
  inside the DSP source; the effect is named outside it.
- Effects are stereo audio processors. Always declare:
  `(def in_l (in 1 @name left))`
  `(def in_r (in 2 @name right))`
  and output both channels:
  `(out left_signal 1 @name left)`
  `(out right_signal 2 @name right)`.
- Effect parameters may be host-modulatable. If any parameter uses
  `@mod true`, declare all four effect modulation inputs immediately after the
  stereo audio inputs:
  `(def mod1 (in 3 @name mod1 @modulator 1))`
  `(def mod2 (in 4 @name mod2 @modulator 2))`
  `(def mod3 (in 5 @name mod3 @modulator 3))`
  `(def mod4 (in 6 @name mod4 @modulator 4))`.
- Read host-modulatable parameters with `(mod param_name)`. Read ordinary
  parameters directly by name.
- For track-selectable sidechain audio, declare a separate non-modulator input
  named `sidechain` after the effect modulation inputs, for example
  `(def sidechain (in 7 @name sidechain))`. Do not use `@modulator` for
  sidechain audio in effects that also use host modulation.
- `%` is the numeric remainder/modulo-style operator. Do not use `(mod x y)`.
  To wrap a phase or signal into a range, use `(wrap expr min max)`.
- Use valid operators and preamble helpers from local examples such as `def`,
  `param`, `in`, `out`, `phasor`, `sin`, `triangle`, `noise`, `delay`,
  `biquad`, `svf`, `clip`, `wrap`, `+`, `-`, `*`, `/`, `min`, `max`,
  `sin`, `cos`, `tan`, `atan`, `atan2`, `tanh`.

Minimal valid effect shape:

```dgenlisp
(def in_l (in 1 @name left))
(def in_r (in 2 @name right))

(param rate @default 4.0 @min 0.1 @max 20 @unit Hz)
(param depth @default 0.5 @min 0 @max 1)
(param mix @default 0.5 @min 0 @max 1)

(def phase (phasor rate))
(def lfo (scale (triangle phase 0.5) -1 1 (- 1 depth) 1))
(def wet_l (* in_l lfo))
(def wet_r (* in_r (scale (triangle (wrap (+ phase 0.25) 0 1) 0.5) -1 1 (- 1 depth) 1)))

(out (+ (* in_l (- 1 mix)) (* wet_l mix)) 1 @name left)
(out (+ (* in_r (- 1 mix)) (* wet_r mix)) 2 @name right)
```

Recipes. The DGenLisp reference (`DGenLispReadme.md`) documents each
operator; these are the idioms that come up most:

- Constants: `samplerate` is the current sample rate in Hz and `twopi` is 2π.
- Local LFO: `(def lfo1 (sin (* (phasor lfo1_rate) twopi)))`. `phasor` takes
  a frequency in Hz, runs at any rate (sub-audio included) and is free-running.
- Feedback needs a history cell. `(make-history h)` creates it,
  `(read-history h)` returns the previous sample's value, and
  `(write-history h value)` stores this sample's value (and returns it). Put
  the cell inside a `(defmacro ...)` so every call gets its own cell.
- `(delay signal time)` takes its time in SAMPLES, not ms:
  `(delay x (* time_ms (/ samplerate 1000)))`. The buffer holds 88000 samples
  (about 1.8 s at 48 kHz) unless you pass `@max-delay`, for example
  `(delay x t @max-delay 192000)`.
- Feedback delay:

  ```dgenlisp
  (defmacro echo (input time_samples feedback cutoff)
    (make-history fb)
    (def looped (delay (+ input (* feedback (read-history fb))) time_samples))
    (def darker (svf looped cutoff 0.707 0))
    (write-history fb (tanh darker))
    darker)
  ```

- `(latch value trigger)` samples `value` when `trigger` is non-zero and holds
  it; `(gswitch cond a b)` picks `a` when `cond` is non-zero, else `b`.
- Numeric remainder is `(% x y)`; wrapping into a range is `(wrap x lo hi)`.

- If no factory effect is close to the request, the recipes above are a
  better starting point than a factory folder that breaks the rules below.

All `ui-*` panel helpers named below are exported by the module
`eseq.effects.custom-ui-lego`. Always call them module-qualified, e.g.
`(eseq.effects.custom-ui-lego/ui-lego-knob-s ...)`: the bare spellings are
deprecated compatibility aliases and break panels loaded from disk.

Mandatory ui.lisp rules:
- `ui.lisp` must contain exactly one `(defeffect-ui ...)` form.
- `defeffect-ui` takes one body. Do not pass the effect name to it.
- Reference DSP params by exact names from dsp.lisp.
- Use the current lego-style UI building blocks used by bundled effects:
  `ui-control-block-*`, `ui-readout-block-*`, and `ui-lego-*`.
- Do not use legacy wrappers such as `group`, `vgroup`, `hgroup`, `knob`, or
  `slider`.
- Do not use instrument-only wrappers such as `defsynth-ui`.
- Keep the UI compact and horizontal. Put columns in a root `h-stack`.
- Use `(eseq.effects.custom-ui-lego/ui-lego-column block-a block-b block-c)` for a three-block column,
  `(eseq.effects.custom-ui-lego/ui-lego-column-2 block-a block-b)` for a two-block column, and
  `(eseq.effects.custom-ui-lego/ui-lego-column-full block)` for a single full-height block.
  Never put two blocks inside `ui-lego-column-full`.
- Each control block should be one of:
  `(eseq.effects.custom-ui-lego/ui-control-block-medium-s "TITLE" (eseq.effects.custom-ui-lego/ui-accent-cyan) section (h-stack ...))`,
  `(eseq.effects.custom-ui-lego/ui-control-block-small-s "TITLE" (eseq.effects.custom-ui-lego/ui-accent-orange) section (h-stack ...))`,
  or `(eseq.effects.custom-ui-lego/ui-control-block-full-s "TITLE" (eseq.effects.custom-ui-lego/ui-accent-green) section body)`.
- Use `(eseq.effects.custom-ui-lego/ui-lego-knob-s section "param_name" "label" width accent decimals)`
  for knobs. Typical widths are `4.7`, `4.8`, or `5.2`.
- Use `(eseq.effects.custom-ui-lego/ui-lego-num-s section "param_name" "label" width decimals unit accent)`
  for compact numeric controls. Use `false` for no unit.
- Available accent helpers are only `(eseq.effects.custom-ui-lego/ui-accent-blue)`, `(eseq.effects.custom-ui-lego/ui-accent-cyan)`,
  `(eseq.effects.custom-ui-lego/ui-accent-orange)`, `(eseq.effects.custom-ui-lego/ui-accent-green)`, and `(eseq.effects.custom-ui-lego/ui-accent-violet)`.
- Do not write custom wrapper functions around `effect-param` or
  `ui-lego-knob-s` in generated panels. Use direct lego helper calls so the
  UI can be statically validated.

Minimal valid effect UI:

```eseqlisp
(defeffect-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (eseq.effects.custom-ui-lego/ui-lego-column-full
      (eseq.effects.custom-ui-lego/ui-control-block-medium-s "MOTION" (eseq.effects.custom-ui-lego/ui-accent-blue) 0
        (h-stack :gap 0.32 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "rate" "rate" 4.8 (eseq.effects.custom-ui-lego/ui-accent-blue) 2)
          (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "depth" "depth" 4.8 (eseq.effects.custom-ui-lego/ui-accent-cyan) 2)
          (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "mix" "mix" 4.8 (eseq.effects.custom-ui-lego/ui-accent-orange) 2))))))
```

Two-block effect UI shape:

```eseqlisp
(defeffect-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (eseq.effects.custom-ui-lego/ui-lego-column-2
      (eseq.effects.custom-ui-lego/ui-control-block-medium-s "DELAY" (eseq.effects.custom-ui-lego/ui-accent-cyan) 0
        (h-stack :gap 0.32 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "time_ms" "time" 4.8 (eseq.effects.custom-ui-lego/ui-accent-cyan) 0)
          (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "feedback" "fbk" 4.8 (eseq.effects.custom-ui-lego/ui-accent-orange) 2)
          (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "mix" "mix" 4.8 (eseq.effects.custom-ui-lego/ui-accent-blue) 2)))
      (eseq.effects.custom-ui-lego/ui-control-block-medium-s "TONE" (eseq.effects.custom-ui-lego/ui-accent-orange) 0
        (h-stack :gap 0.32 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "drive" "drive" 4.8 (eseq.effects.custom-ui-lego/ui-accent-orange) 2)
          (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "tone" "tone" 4.8 (eseq.effects.custom-ui-lego/ui-accent-green) 0)
          (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "output" "out" 4.8 (eseq.effects.custom-ui-lego/ui-accent-blue) 2))))))
```
