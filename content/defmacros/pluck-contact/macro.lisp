; Modalys 'pluck connection, solved inside the sample. gap is the free gap
; between plucker and object point (> 0: plucker above), C the sum of both
; ends' compliances, arm a note-on pulse. The plucker catches on a + to -
; crossing, holds through a spring of stiffness k, and releases once |F|
; exceeds threshold. Returns F (push the object by +F, the plucker by -F).
;   F = k (gap - anchor) / (1 + k C)       implicit: exact at any stiffness
; Energy safety: the spring engages at the gap the crossing was detected at
; (zero force; engaging at 0 after a one-sample overshoot adds 1/2 k gap^2 per
; catch and pinned a C8 diamond bar), and it catches once per arm.
(defmacro pluck-contact (gap C arm k threshold)
  (make-history gap-h)
  (make-history held-h)
  (make-history anchor-h)
  (make-history armed-h)
  (def held-prev (read-history held-h))
  (def armed (max arm (read-history armed-h)))
  (def caught (* armed (- 1 held-prev) (gt (read-history gap-h) 0) (lte gap 0)))
  (def anchor (gswitch caught gap (read-history anchor-h)))
  (def holding (max held-prev caught))
  (def F-try (/ (* k (- gap anchor)) (+ 1 (* k C))))
  (def held (* holding (lte (abs F-try) threshold)))
  (def F (* F-try held))
  (write-history anchor-h anchor)
  (write-history armed-h (* armed (- 1 (* holding (- 1 held)))))
  (write-history held-h held)
  (write-history gap-h (- gap (* C F)))
  F)
