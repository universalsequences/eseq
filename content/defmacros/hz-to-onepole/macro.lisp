; hz-to-onepole — convert a cutoff frequency in Hz to a onepole smoothing
; coefficient alpha in 0..1 (impulse-invariant: alpha = 1 - e^(-2pi*hz/sr)).
; Use with the standard onepole update y = y + alpha*(x - y):
;   lowpass  = the smoothed value
;   highpass = x - lowpass
; Per the gen book: phase-modulation feedback wants a onepole lowpass at the
; operator frequency in the feedback path; FM feedback wants a onepole
; highpass. Much cheaper than a biquad in per-sample feedback loops.
(defmacro hz-to-onepole (hz)
  (def alpha (- 1 (exp (/ (* -6.283185307179586 hz) samplerate))))
  alpha)
