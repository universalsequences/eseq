(def in_l (in 1 @name left))
(def in_r (in 2 @name right))

(param curve_type @default 1 @min 1 @max 4)
(param shape @default 0.55 @min 0 @max 1)
(param mix @default 0.65 @min 0 @max 1)

(def drive (+ 1 (* shape 15)))
(def trim (/ 1 (+ 1 (* shape 0.35))))
(def atan_drive (* drive 1.25))
(def fold_drive (+ 1 (* shape 7)))
(def cubic_amount (+ 0.15 (* shape 0.85)))

(defmacro saturate (x)
  (def tanh_curve (/ (tanh (* x drive)) (tanh drive)))
  (def atan_curve (/ (atan (* x atan_drive)) (atan atan_drive)))
  (def hard_curve (clip (* x drive) -1 1))
  (def cubic_in (clip (* x (+ 1 (* shape 3))) -1.5 1.5))
  (def cubic_curve (clip (- (* cubic_in (+ 1 cubic_amount)) (* cubic_amount cubic_in cubic_in cubic_in)) -1 1))
  (def fold_curve (* 0.92 (sin (* x fold_drive))))
  (def selected (selector curve_type tanh_curve atan_curve hard_curve fold_curve))
  (* trim selected))

(def wet_l (saturate in_l))
(def wet_r (saturate in_r))

(out (+ (* in_l (- 1 mix)) (* wet_l mix)) 1 @name left)
(out (+ (* in_r (- 1 mix)) (* wet_r mix)) 2 @name right)