; Three parallel formant resonators. Centers are physical Hz, and the signed
; weights multiply the canonical (un-normalized) SVF bandpass outputs.
; Formant 6 uses Q=20. Formant 12 uses Q=45 and two-thirds the weights.
; Keeping the vowel trajectory outside the resonators lets modulation move
; their coefficients without replacing or resetting filter histories.
(defmacro heat-formant-bank (input f1 f2 f3 weight1 weight2 weight3 steep)
  (def high-q (gt steep 0.5))
  (def q (selector (+ high-q 1) 20 45))
  (def gain (selector (+ high-q 1) 1 0.6666666667))
  (def band1 (svf input f1 q 1))
  (def band2 (svf input f2 q 1))
  (def band3 (svf input f3 q 1))
  (* gain (+ (* weight1 band1) (* weight2 band2) (* weight3 band3))))
