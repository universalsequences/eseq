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
  (def second (svf first cutoff stage-q svf-mode))
  (selector (+ double-stage 1) first second))
