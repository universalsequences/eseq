; Dimension D style stereo chorus
; Stronger, more audible ensemble movement while keeping the polished rack feel

(def in-l (in 1 @name signal-l))
(def in-r (in 2 @name signal-r))
(def mono (* 0.5 (+ in-l in-r)))

(param rate @min 0.05 @max 1.2 @default 0.32)
(param depth @min 1.0 @max 20 @default 8.5)
(param base @min 6 @max 28 @default 12.5)
(param spread @min 0 @max 14 @default 6.0)
(param mix @min 0 @max 1 @default 0.68)
(param tone @min 2000 @max 18000 @default 10500)
(param width @min 0 @max 1 @default 1.0)
(param shimmer @min 0 @max 1 @default 0.28)

(def ph (phasor rate))
(def ph90 (% (+ ph 0.25) 1.0))
(def ph180 (% (+ ph 0.5) 1.0))
(def ph270 (% (+ ph 0.75) 1.0))

(def lfo1 (sin (* twopi ph)))
(def lfo2 (sin (* twopi ph90)))
(def lfo3 (sin (* twopi ph180)))
(def lfo4 (sin (* twopi ph270)))

(def d1 (+ base (* depth lfo1)))
(def d2 (+ (+ base spread) (* depth lfo2)))
(def d3 (+ (+ base (* 0.6 spread)) (* (* depth 1.15) lfo3)))
(def d4 (+ (+ base (* 1.6 spread)) (* (* depth 0.9) lfo4)))

(defmacro mydelay (sig t)
          (make-history h)
          (write-history h (delay (+ mono (* 0.7 (read-history h))) t )))

(def v1 (mydelay mono d1))
(def v2 (mydelay mono d2))
(def v3 (mydelay mono d3))
(def v4 (mydelay mono d4))

(def wet-l-raw (+ (* 0.70 v1) (* 0.52 v2) (* 0.34 v3) (* 0.22 v4)))
(def wet-r-raw (+ (* 0.22 v1) (* 0.34 v2) (* 0.52 v3) (* 0.70 v4)))

(def wet-l-bright (biquad wet-l-raw tone 0.6 1 0))
(def wet-r-bright (biquad wet-r-raw tone 0.6 1 0))

(def wet-l (mix wet-l-raw wet-l-bright shimmer))
(def wet-r (mix wet-r-raw wet-r-bright shimmer))

(out (+ (* (- 1 mix) in-l) (* mix (mix wet-l wet-r (* 0.5 (- 1 width))))) 1 @name out-l)
(out (+ (* (- 1 mix) in-r) (* mix (mix wet-r wet-l (* 0.5 (- 1 width))))) 2 @name out-r)