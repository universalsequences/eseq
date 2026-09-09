; Research only: Gaussian PAF, not a Yamaha FS1R model.
; Width is a dimensionless index, not calibrated bandwidth in Hz.
; Static controls only: discontinuous updates need a defined transition policy.
(def pitch (in 1 @name pitch))
(param center @default 1100 @min 100 @max 18000)
(param width_index @default 2 @min 0 @max 30)
(param pm_depth @default 0 @min 0 @max 8)
(param pm_ratio @default 2 @min 0.25 @max 16)
(def ph (phasor pitch))
(def mp (phasor (* pitch pm_ratio)))
(def pm (* pm_depth (sin (* twopi mp))))
(def ratio (/ center (max pitch 1)))
(def k (floor ratio))
(def q (- ratio k))
(def pulse (exp (* -1 width_index width_index (pow (sin (* pi ph)) 2))))
; Same radian PM on both carrier cosines; the window stays on the base phase.
; This is a candidate synthesis law, not identified FS1R behavior.
(def carrier (+ (* (- 1 q) (cos (+ (* twopi k ph) pm)))
                (* q (cos (+ (* twopi (+ k 1) ph) pm)))))
(out (* pulse carrier) 1 @name audio)
(out ph 2 @name phase)
(out mp 3 @name mod_phase)
