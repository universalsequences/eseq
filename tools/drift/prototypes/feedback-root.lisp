; Research component, not yet connected to production Digi Drift.
; Solve a*u - b - r*S(u) = 0 for a > r >= 0, threshold > 0,
; curve > 0. S is odd, linear through threshold, then approaches
; threshold + 1/curve with slope 1/(1+curve*excess)^2.
; The equation is strictly increasing, so its real root is unique.
(defmacro drift-feedback-root (a b r threshold curve)
  (def slope (- a r))
  (def magnitude (abs b))
  (def excess (max (- magnitude (* slope threshold)) 0))
  (def quadratic (* a curve))
  (def middle (- slope (* curve excess)))
  (def discriminant (sqrt (+ (* middle middle) (* 4 quadratic excess))))
  ; Use the cancellation-free quadratic form on each side of middle=0.
  ; The guarded denominator belongs to the inactive branch when cancellation
  ; rounds it to zero; DGen may evaluate both gswitch inputs.
  (def curved-excess (gswitch (gte middle 0)
    (/ (* 2 excess) (max (+ middle discriminant) 1e-20))
    (/ (- discriminant middle) (* 2 quadratic))))
  (def root-magnitude (gswitch (gt excess 0)
    (+ threshold curved-excess) (/ magnitude slope)))
  (* (gswitch (lt b 0) -1 1) root-magnitude))
