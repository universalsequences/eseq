(use-defmacro heat-soft-clip)

; Off, Sym 1/2/3, Asym 1/2/3. Signal units put the drive knee at 1.
; Asymmetric modes retain the broad negative tail while lowering the positive
; ceiling. Amplifier gain, pan and the final output limiter belong downstream.
(defmacro heat-drive (input mode)
  (def choice (clip (round mode) 0 6))
  (def gain (selector (+ choice 1) 1 1 1.41421356237 2 1 1.41421356237 2))
  (def positive_limit (selector (+ choice 1) 8 8 4 2 4 2 1.3))
  (def negative_limit (selector (+ choice 1) 8 8 4 2 8 8 8))
  (def ceiling (selector (+ (lt input 0) 1) positive_limit negative_limit))
  (def driven (heat-soft-clip (* input gain) 1 ceiling))
  (selector (+ (gt choice 0) 1) input driven))
