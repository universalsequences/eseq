; A lumped two-mass (Modalys bi-two-mass): a tip hung off a driven hand on a
; damped spring. The tip is a two-pole with unit DC gain toward the hand:
;   tip = a tip[t-1] + b tip[t-2] + follow hand
; Returns (a, b, follow, C): C = follow / stiff is the tip's compliance, how
; far a unit force moves it this sample (for modal-point-style solves).
(defmacro two-mass-coefs (hz damp stiff)
  (def zeta (min damp 0.9))
  (def wt (* twopi (/ hz samplerate)))
  (def rt (exp (* -1 zeta wt)))
  (def at (* 2 rt (cos (* wt (sqrt (- 1 (* zeta zeta)))))))
  (def bt (* -1 rt rt))
  (def follow (- 1 (+ at bt)))
  (tuple at bt follow (/ follow stiff)))
