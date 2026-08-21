; Lush Lexicon-style Reverb
; Mono-in, Stereo-out

(def in-l (in 1 @name signal))

; Params
(param mix @min 0 @max 1 @default 0.3)
(param size @min 0.5 @max 2.0 @default 1.0)
(param decay @min 0.1 @max 0.95 @default 0.7)
(param damping @min 500 @max 10000 @default 3000)
(param pre @min 0 @max 2000 @default 500)

; Pre-delay
(def pre-sig (delay in-l pre))

; Allpass macro: (allpass signal delay-samples gain)
(defmacro allpass (sig d g)
  (make-history h)
  (def d-sig (delay (read-history h) d))
  (def node (+ sig (* g d-sig)))
  (write-history h node)
  (- d-sig (* g node)))

; Input Diffusion
(def diff1 (allpass pre-sig (* 142 size) 0.7))
(def diff2 (allpass diff1 (* 107 size) 0.7))
(def diff3 (allpass diff2 (* 379 size) 0.6))
(def diff4 (allpass diff3 (* 277 size) 0.6))

; Main Tank Loop
(make-history tank-l-h)
(make-history tank-r-h)

; Left branch of tank
(def tank-l-in (+ diff4 (* (read-history tank-r-h) decay)))
(def ap-l1 (allpass tank-l-in (* 671 size) 0.5))
(def d-l1 (delay ap-l1 (* 1800 size)))
(def lp-l (biquad d-l1 damping 0.707 1 1)) ; mode 1 = lowpass
(write-history tank-l-h lp-l)

; Right branch of tank
(def tank-r-in (+ diff4 (* (read-history tank-l-h) decay)))
(def ap-r1 (allpass tank-r-in (* 947 size) 0.5))
(def d-r1 (delay ap-r1 (* 2300 size)))
(def lp-r (biquad d-r1 damping 0.707 1 1))
(write-history tank-r-h lp-r)

; Output mix
(def out-l (+ (* in-l (- 1 mix)) (* (read-history tank-l-h) mix)))
(def out-r (+ (* in-l (- 1 mix)) (* (read-history tank-r-h) mix)))

(out out-l 1 @name left)
(out out-r 2 @name right)
