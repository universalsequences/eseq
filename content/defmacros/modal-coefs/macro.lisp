; Two-pole resonator coefficients for every mode: s[t] = a s[t-1] + b s[t-2]
; rings at freqs (Hz) and decays at sigma (1/s). Works on tensors or scalars.
(defmacro modal-coefs (freqs sigma)
  (def w (min (* twopi (/ freqs samplerate)) 2.8))
  (def r (exp (/ (* -1 sigma) samplerate)))
  (tuple (* 2 r (cos w)) (* -1 r r)))
