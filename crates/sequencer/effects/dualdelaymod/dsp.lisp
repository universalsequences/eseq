; DGenLisp effect — processes audio from the track's sampler
; Input on channel 1, output on channel 1

(def input (in 1 @name signal))
(def lfo (cos (* twopi (triangle (phasor (param lforate @min 0.1 @max 32 @default 1))))))
(def x 
  (biquad 
    input 
    (+ (* (scale lfo -1 1 0 1) 1000) (param cutoff @min 50 @max 4000 @default 900)) 
    (param res @min 1 @max 16 @default 4) 
    1 
    1))

(param fbk @min 0.1 @max 0.99 @default 0.98)
(defmacro mdelay (ma)
(make-history h)
(write-history h (delay (+ x (* fbk (read-history h))) 
  (scale lfo -1 1 23 ma))))

(def z1 (mdelay (param m1 @min 50 @max 5000 @default 374))) 
(def z2 (mdelay (param m2 @min 50 @max 5000 @default 2753)))
(def z (mix z1 z2 0.5))
(out z 1 @name audio)
