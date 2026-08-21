(def in_l (in 1 @name left))
(def in_r (in 2 @name right))

(param rate @min 0.1 @max 20 @default 5)
(param depth @min 0 @max 1 @default 0.8)
(param spread @min 0 @max 1 @default 0.5)

(def p (phasor rate))

; Left LFO
(def lfo_l (scale (triangle p 0.5) -1 1 (- 1 depth) 1))

; Right LFO with phase offset (spread)
(def lfo_r (scale (triangle (wrap (+ p spread) 0 1) 0.5) -1 1 (- 1 depth) 1))

(out (* in_l lfo_l) 1 @name left)
(out (* in_r lfo_r) 2 @name right)