; Research Type-I candidate. Requires feedback-root.lisp.
; Explicit state input/output permits two serial steps in an oversampled voice.
; This is an identified approximation, not the native DFM-1 circuit equation.
; Domain: g > 0, 0 <= resonance <= 1, 0 <= fb-bias <= 1,
; 0 <= low-mix < 1/g, and positive knee thresholds/curves.
(defmacro drift-research-knee (x threshold curve)
  (def magnitude (abs x))
  (def excess (max (- magnitude threshold) 0))
  (* (gswitch (lt x 0) -1 1)
     (+ (min magnitude threshold) (/ excess (+ 1 (* curve excess))))))

(defmacro drift-research-type1-step (x g resonance s1 s2
                                    pre-th pre-curve post-th post-curve
                                    fb-th fb-curve bias fb-bias low-mix)
  (def pre (- (drift-research-knee (+ x bias) pre-th pre-curve)
              (drift-research-knee bias pre-th pre-curve)))
  (def k (/ 1 .423))
  (def c (- 0 low-mix))
  (def offset (* fb-th fb-bias))
  (def scale (+ 1 (* c g)))
  (def a (+ 1 (* g k) (* g g) (* g g k resonance c)))
  (def shift (+ (* c s2) offset))
  (def b (- (+ s1 (* g pre))
             (+ (* g s2) (* g k resonance offset) (* g k resonance c s2))))
  (def root-b (+ (* b scale) (* a shift)))
  (def root-r (* g k resonance scale))
  (def root (drift-feedback-root a root-b root-r fb-th fb-curve))
  ; In the linear region, solve directly for bp to avoid subtracting the
  ; feedback bias from an almost equal root at very low signal levels.
  (def linear-bp (/ (+ s1 (* g (- pre s2)))
                    (+ 1 (* g k (- 1 resonance)) (* g g))))
  (def bp (gswitch (lte (abs root-b) (* (- a root-r) fb-th))
            linear-bp (/ (- root shift) scale)))
  (def lp (+ s2 (* g bp)))
  (tuple (drift-research-knee (* lp 1.541) post-th post-curve)
         (- (* 2 bp) s1) (- (* 2 lp) s2)))

; Explicit state version of the downstream measured high-pass section.
(defmacro drift-research-highpass-step (x g inverse-q s1 s2)
  (def bp (/ (+ s1 (* g (- x s2)))
             (+ 1 (* g inverse-q) (* g g))))
  (def lp (+ s2 (* g bp)))
  (tuple (- x (+ (* inverse-q bp) lp))
         (- (* 2 bp) s1) (- (* 2 lp) s2)))
