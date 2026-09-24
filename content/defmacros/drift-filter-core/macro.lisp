; Solve a*u - b - r*S(u) = 0 for a > r >= 0, threshold > 0,
; curve > 0. S is odd, linear through threshold, then approaches
; threshold + 1/curve with slope 1/(1+curve*excess)^2.
; The equation is strictly increasing, so its real root is unique.
(defmacro drift-feedback-root (a b r threshold curve)
  (def slope (- a r))
  (def magnitude (abs b))
  (def excess (max (- magnitude (* slope threshold)) 0))
  (def quadratic (* a curve))
  (def middle (- slope (* curve excess)))
  (def discriminant (sqrt (+ (* middle middle) (* 4 quadratic excess))))
  ; Use the cancellation-free quadratic form on each side of middle=0.
  ; The guarded denominator belongs to the inactive branch when cancellation
  ; rounds it to zero; DGen may evaluate both gswitch inputs.
  (def curved-excess (gswitch (gte middle 0)
    (/ (* 2 excess) (max (+ middle discriminant) 0.00000000000000000001))
    (/ (- discriminant middle) (* 2 quadratic))))
  (def root-magnitude (gswitch (gt excess 0)
    (+ threshold curved-excess) (/ magnitude slope)))
  (* (gswitch (lt b 0) -1 1) root-magnitude))

(defmacro drift-knee (x threshold curve)
  (def magnitude (abs x))
  (def excess (max (- magnitude threshold) 0))
  (* (gswitch (lt x 0) -1 1)
     (+ (min magnitude threshold) (/ excess (+ 1 (* curve excess))))))

(defmacro drift-type1-step (x g resonance s1 s2
                                    pre-th pre-curve post-th post-curve
                                    fb-th fb-curve bias fb-bias low-mix)
  (def pre (- (drift-knee (+ x bias) pre-th pre-curve)
              (drift-knee bias pre-th pre-curve)))
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
  (tuple (drift-knee (* lp 1.541) post-th post-curve)
         (- (* 2 bp) s1) (- (* 2 lp) s2)))

; Measured 12 dB response with asymmetric input saturation and nonlinear
; feedback. Q=.423/(1-resonance) in the small-signal limit. The low-pass
; contribution inside the feedback shaper changes how loud notes damp resonance.
(defmacro drift-type1 (x cutoff resonance)
  (make-history band_state)
  (make-history low_state)
  (def g (tan (/ (* pi cutoff) samplerate)))
  (def (y next_band next_low)
    (drift-type1-step x g resonance
      (read-history band_state) (read-history low_state)
      .4597608481 51.28837662 .2855820088 .4684337539
      .2377988585 .4106747694 .1626344034 .6038363968 .3779898126))
  (write-history band_state next_band)
  (write-history low_state next_low)
  y)

; Trapezoidal Sallen-Key section with an implicitly solved feedback limiter.
; For this resonance law k<2.03, so (1+g)^2-g*k is positive for every g>0.
; No iterative solve or feedback-state clamp is needed.
(defmacro drift-sallen-key (x g k threshold curve bias)
  (make-history state1)
  (make-history state2)
  (def s1 (read-history state1))
  (def s2 (read-history state2))
  (def a (* (+ 1 g) (+ 1 g)))
  (def r (* g k))
  (def b (+ (* (+ 1 g) s2) (* g s1) (* g g x)))
  (def offset (* threshold bias))
  (def root-b (+ b (* (- a r) offset)))
  (def u (drift-feedback-root a root-b r threshold curve))
  (def lp (gswitch (lte (abs root-b) (* (- a r) threshold))
            (/ b (- a r)) (- u offset)))
  (def feedback (- (drift-knee (+ lp offset) threshold curve) offset))
  (def v1 (/ (+ s1 (* g x) (* k feedback)) (+ 1 g)))
  (write-history state1 (- (* 2 (- v1 (* k feedback))) s1))
  (write-history state2 (- (* 2 lp) s2))
  lp)

; The low-frequency Type-II reference differs from Type I by four gentle
; DC-blocking poles. Keep these separate from the adjustable high-pass.
(defmacro drift-dc-block (x)
  (make-history state)
  (def s (read-history state))
  (def g (tan (/ (* pi 1.6) samplerate)))
  (def v (* (- x s) (/ g (+ 1 g))))
  (def low (+ v s))
  (write-history state (+ low v))
  (- x low))

; Four-pole approximation of the measured Type-II response: two Sallen-Key
; sections, with the high-Q section first and a distinct soft feedback limiter
; in each. This captures MS2-style resonance, not the full native OTA circuit.
(defmacro drift-type2 (x cutoff resonance)
  (def r2 (* resonance resonance))
  (def k1 (/ (+ (* 4.1256140047 resonance) (* 1.8359078001 r2))
             (+ 1 (* 3.2117741295 resonance))))
  (def k2 (/ (- (* 8.0522398087 resonance) (* .06219915383 r2))
             (+ 1 (* 2.9532898642 resonance))))
  (def g1 (tan (/ (* pi cutoff 1.0219682037) samplerate)))
  (def g2 (tan (/ (* pi cutoff 1.0226839218) samplerate)))
  (def pre (- (drift-knee (+ x .1550136067) .4581724596 199.9745374)
              .1550136067))
  (def first (drift-sallen-key pre g2 k2 .2707240824 5.680632221 .07640951001))
  (def second (drift-sallen-key first g1 k1 .1027683762 8.333819518 .07640951001))
  (def shaped (drift-knee (* second 1.2966450151) 1.082975062 36.82033997))
  (drift-dc-block (drift-dc-block (drift-dc-block (drift-dc-block shaped)))))


; Shared calibrated low-pass and high-pass path. Control mapping belongs to callers.
(defmacro drift-filter-core (x cutoff resonance hp-cutoff filter-type)
  (def type1 (drift-type1 x cutoff resonance))
  (def type2 (drift-type2 x cutoff resonance))
  (svf (gswitch (gte filter-type 0.5) type2 type1) hp-cutoff 1.469 2))

; Smooth topology blend for instruments with continuous live type changes.
; Both nonlinear filters retain independent, continuously advanced state.
(defmacro drift-filter-morph (x cutoff resonance hp-cutoff type-mix)
  (def type1 (drift-type1 x cutoff resonance))
  (def type2 (drift-type2 x cutoff resonance))
  (svf (mix type1 type2 type-mix) hp-cutoff 1.469 2))

; The unselected topology sleeps once the blend reaches an endpoint. During
; a crossfade both filters run. Callers provide the smoothed blend; snap its
; last -80 dB of contribution so an exponential smoother can finish the fade.
; Unlike drift-filter-morph, inactive filter state is frozen between calls.
(defmacro drift-filter-morph-gated (x cutoff resonance hp-cutoff type-mix)
  (def blend (gswitch (lte type-mix .0001) 0
               (gswitch (gte type-mix .9999) 1 type-mix)))
  (def type1 (block-gate (lt blend 1) (drift-type1 x cutoff resonance)))
  (def type2 (block-gate (gt blend 0) (drift-type2 x cutoff resonance)))
  (svf (mix type1 type2 blend) hp-cutoff 1.469 2))
