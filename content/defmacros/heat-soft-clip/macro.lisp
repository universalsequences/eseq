; Symmetric soft knee with unit slope below knee and a finite asymptote.
; The rational tail matches the isolated Analog drive measurements. Explicit
; multiplication keeps the cubic cheap and avoids a general power operation.
(defmacro heat-soft-clip (input knee ceiling)
  (def threshold (max 0 knee))
  (def span (max 0.000001 (- ceiling threshold)))
  (def magnitude (abs input))
  (def excess (max 0 (- magnitude threshold)))
  (def reciprocal (/ 1 (+ 1 (/ excess (* 3 span)))))
  (def tail (* span (- 1 (* reciprocal reciprocal reciprocal))))
  (* (selector (+ (lt input 0) 1) 1 -1)
    (+ (min magnitude threshold) tail)))
