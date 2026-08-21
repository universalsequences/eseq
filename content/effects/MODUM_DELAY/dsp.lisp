; DGenLisp effect — processes audio from the track's sampler
; Input on channel 1, output on channel 1

(def input (in 1 @name signal))
(def lfo (cos (* twopi (triangle (phasor (param rate @min 0.1 @max 100 @default 1) ) 0.2 ))))
(def x 
  (biquad 
    input 
    (+ (* (scale lfo -1 1 0 1) 100) (param cutoff @min 50 @max 4000 @default 900)) 
    (param res @min 1 @max 16 @default 4) 
    1 
    1))

(param max1 @min 50 @max 5000 @default 500)
(param max2 @min 50 @max 5000 @default 750)
(param fbk @min 0.1 @max 0.9 @default 0.5) 

(defmacro onepole (sig alpha) 
          (make-history h)
          (write-history h (mix sig (read-history h) alpha))
          )

(defmacro fx (n)
          (make-history h)
          (write-history h (delay (+ x (* (read-history h) fbk ))  (scale lfo -1 1 23 n)) ))

(def l (fx (onepole max1 0.4)))
(def r (fx (onepole max2  0.4)))
(out l 1 @name audio)
(out r 2 @name audio)