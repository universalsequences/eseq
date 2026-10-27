; spring-connection, centred in time: the force acts on the gap's mean over
; the next sample and the one before last instead of on the next sample's
; alone. spring-connection's backward step takes a factor 1/sqrt(1 + kC) out
; of every mode at the point each sample, and kC grows with pitch squared,
; so a bridge drained a treble string's energy in a fraction of a second
; whatever its stiffness. Centred, a spring k = stiffness / C is conservative;
; keep (just below 1) leaves a little of the backward step's damping so a
; near-rigid spring's own fast oscillation settles:
;   F = k (gap_next + keep gap_old) / (1 + keep),  gap_next = gap - C F
;   F = stiffness (gap + keep gap_old) / ((1 + keep + stiffness) C)
; keep 0 is spring-connection. gap = yA - yB from modal-point (next sample,
; free), gap-old = yA - yB the sample before last; push B by +F, A by -F.
(defmacro spring-centred (gap gap-old C stiffness keep)
  (/ (* stiffness (+ gap (* keep gap-old))) (* (+ 1 keep stiffness) (max C 1e-12))))
