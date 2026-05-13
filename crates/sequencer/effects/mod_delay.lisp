; DGenLisp effect — processes audio from the track's sampler
; Input on channel 1, output on channel 1

(def input (in 1 @name signal))
(make-history h)
(def fin (read-history h))
(def freq (param freq @min 0.1 @max 500 @default 32))
(def dmax (param delay_max @min 50 @max 5000 @default 100))
(def delaytime (scale (triangle (phasor freq) 0.2) 0 1 50 dmax))
(def x (write-history h (delay (+ input (* (param fbk @min 0.1 @max 0.99 @default .2) fin)) delaytime)))
(out (mix input x .3) 1 @name audio)
(out (mix input x .3) 2 @name audio)
