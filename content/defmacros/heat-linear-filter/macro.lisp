; Heat's linear filter family, identified from isolated Analog captures.
; Mode: LP12, LP24, BP6, BP12, Notch2, Notch4, HP12, HP24.
; cutoff is the physical corner in Hz; q is the dimensionless resonance,
; not a normalized knob value. LP24/HP24 split Q across their two stages;
; BP12/Notch4 retain Q in each stage. No gain compensation or saturation.
; The shared svf is a trapezoidal-integrator state-variable filter.
(defmacro heat-linear-filter (input cutoff q mode)
  (def kind (clip (round mode) 0 7))
  (def double-stage (eq (% kind 2) 1))
  (def split-q (* double-stage (+ (lt kind 2) (gte kind 6))))
  (def stage-q (selector (+ (gt split-q 0.5) 1) (clip q 0.1 100) (sqrt (clip q 0.1 100))))
  (def family (floor (/ kind 2)))
  ; svf's HP and notch enum order differs from Heat's menu order.
  (def svf-mode (selector (+ family 1) 0 1 3 2))
  (def first (svf input cutoff stage-q svf-mode))
  ; block-gate freezes rather than resets the integrators of a stage it skips,
  ; so switching a single-stage mode to its two-stage partner (LP12 to LP24,
  ; BP6 to BP12, Notch2 to Notch4, HP12 to HP24) resumes whatever state the
  ; second stage was frozen with, which at high Q is an audible thump. Ramp the
  ; stage in over 10 ms instead of switching onto it: the ramp scales both the
  ; stage's input and its share of the output, so the stage is fed from silence
  ; and re-warms while it is still inaudible, and its frozen state is faded in
  ; rather than stepped in. Measured on a held 220 Hz note at Q 40 / 300 Hz,
  ; the peak after an LP12 to LP24 switch falls from 1.44x the settled level to
  ; 1.01x, which is what the same switch measures with no frozen state at all.
  ; The ramp is deliberately asymmetric: it returns to exact zero in the same
  ; sample the mode leaves a two-stage family, so leaving the mode is still the
  ; original hard switch with no dropout, and the gate predicate stays the
  ; frame-invariant mode test that lets an unused stage be skipped outright.
  (make-history stage_ready_hist)
  (make-history stage_fade_hist)
  (def fade-step (/ 1 (* 0.01 samplerate)))
  ; The ramp is initialized to the mode it starts in, so a patch that is
  ; already two-stage is unchanged from the first sample; only a change of
  ; mode ramps.
  (def fade (gswitch (eq (read-history stage_ready_hist) 0) double-stage
    (gswitch double-stage (min 1 (+ (read-history stage_fade_hist) fade-step)) 0)))
  (write-history stage_ready_hist 1)
  (write-history stage_fade_hist fade)
  (def second (block-gate double-stage (svf (* first fade) cutoff stage-q svf-mode)))
  (+ (* first (- 1 fade)) (* second fade)))
