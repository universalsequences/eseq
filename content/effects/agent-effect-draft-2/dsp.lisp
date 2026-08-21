(def in_l (in 1 @name left))
(def in_r (in 2 @name right))

(param time @default 16000 @min 200 @max 88200 @unit samples)
(param spread @default 0.08 @min -0.3 @max 0.3)
(param feedback @default 0.42 @min 0 @max 0.92)
(param drive @default 1.8 @min 0.5 @max 8)
(param tone @default 4200 @min 500 @max 12000 @unit Hz)
(param wow_rate @default 0.35 @min 0.05 @max 5 @unit Hz)
(param wow_depth @default 0.008 @min 0 @max 0.04)
(param mix @default 0.35 @min 0 @max 1)
(param output @default 0.9 @min 0 @max 1.5)

(def phase (phasor wow_rate))
(def wow (sin (* twopi phase)))
(def flutter (triangle (wrap (* phase 7.31) 0 1) 0.5))
(def wobble (+ wow (* 0.35 flutter)))
(def mod_samples (* time wow_depth wobble))

(def base_l (clip (* time (- 1 spread)) 10 88200))
(def base_r (clip (* time (+ 1 spread)) 10 88200))
(def delay_l (clip (+ base_l mod_samples) 10 88200))
(def delay_r (clip (- base_r mod_samples) 10 88200))

(def in_sat_l (tanh (* in_l drive)))
(def in_sat_r (tanh (* in_r drive)))

(defmacro tape_line (input delay_time)
  (make-history fb)
  (def fb_read (read-history fb))
  (def driven (tanh (* (+ input (* fb_read feedback)) drive)))
  (def dark (svf driven tone 0.7 0))
  (def delayed (delay dark delay_time))
  (write-history fb delayed))

(def wet_l (tape_line in_sat_l delay_l))
(def wet_r (tape_line in_sat_r delay_r))

(def out_l (* output (+ (* in_l (- 1 mix)) (* wet_l mix))))
(def out_r (* output (+ (* in_r (- 1 mix)) (* wet_r mix))))

(out out_l 1 @name left)
(out out_r 2 @name right)
