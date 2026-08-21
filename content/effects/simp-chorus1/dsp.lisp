(def in_l (in 1 @name left))
(def in_r (in 2 @name right))

(param rate @default 0.38 @min 0.05 @max 3.0 @unit Hz)
(param depth @default 210 @min 0 @max 650)
(param base @default 520 @min 80 @max 1800)
(param spread @default 180 @min 0 @max 700)
(param feedback @default 0.18 @min 0 @max 0.72)
(param mix @default 0.48 @min 0 @max 1)
(param width @default 0.85 @min 0 @max 1)
(param tone @default 10500 @min 1800 @max 18000 @unit Hz)

(def mono (* 0.5 (+ in_l in_r)))
(def phase (phasor rate))
(def phase_b (wrap (+ phase 0.23) 0 1))
(def phase_c (wrap (+ phase 0.51) 0 1))
(def phase_d (wrap (+ phase 0.77) 0 1))

(def lfo_a (sin (* twopi phase)))
(def lfo_b (sin (* twopi phase_b)))
(def lfo_c (sin (* twopi phase_c)))
(def lfo_d (sin (* twopi phase_d)))

(def d_a (max 1 (+ base (* depth lfo_a))))
(def d_b (max 1 (+ (+ base spread) (* (* depth 0.83) lfo_b))))
(def d_c (max 1 (+ (+ base (* 0.55 spread)) (* (* depth 1.12) lfo_c))))
(def d_d (max 1 (+ (+ base (* 1.35 spread)) (* (* depth 0.71) lfo_d))))

(defmacro chorus-delay (sig t fb)
  (make-history hist)
  (write-history hist (delay (tanh (+ sig (* fb (read-history hist)))) t)))

(def voice_a (chorus-delay in_l d_a feedback))
(def voice_b (chorus-delay in_r d_b feedback))
(def voice_c (chorus-delay mono d_c (* feedback 0.82)))
(def voice_d (chorus-delay mono d_d (* feedback 0.68)))

(def wet_l_raw (+ (* 0.48 voice_a) (* 0.32 voice_c) (* 0.20 voice_d)))
(def wet_r_raw (+ (* 0.48 voice_b) (* 0.20 voice_c) (* 0.32 voice_d)))

(def wet_l_tone (biquad wet_l_raw tone 0.7 1 0))
(def wet_r_tone (biquad wet_r_raw tone 0.7 1 0))

(def wet_l_wide (+ (* wet_l_tone (+ 1 width)) (* wet_r_tone (- 0 width))))
(def wet_r_wide (+ (* wet_r_tone (+ 1 width)) (* wet_l_tone (- 0 width))))

(def wet_gain (- 1 (* feedback 0.28)))
(def out_l (+ (* in_l (- 1 mix)) (* wet_l_wide mix wet_gain)))
(def out_r (+ (* in_r (- 1 mix)) (* wet_r_wide mix wet_gain)))

(out out_l 1 @name left)
(out out_r 2 @name right)
