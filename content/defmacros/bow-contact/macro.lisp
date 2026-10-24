; Modalys 'bow connection: friction between a moving bow and an object point,
; solved inside the sample. y / C come from modal-point (the point's free
; position and compliance); y-prev is the point's position LAST sample after
; every force (write sum(next phi) to a history after modal-advance: if the
; bow assumed only its own force moved the point, another connection's push
; would read as string velocity and the friction would pump energy in). vb is
; the bow's velocity (per sample), v0 the slip
; velocity scale, pressure the bow force relative to the point's give. The
; friction curve is rosin-like: a steep stick region around zero relative
; velocity, static friction 1 falling hyperbolically to a kinetic 0.35:
;   mu(x) = tanh(x / 0.05) (0.35 + 0.65 / (1 + |x|)),  x = (vb - v) / v0
; (a curve that falls to zero at large slip, like the soft exp(-x^2) one,
; left most of the bowing range aperiodic).
; With F = (pressure v0 / C) mu(x) the point's velocity v = vfree + C F turns
; the implicit force into one scalar equation in the relative velocity x:
;   h(x) = x + pressure mu(x) - xfree = 0,  xfree = (vb - vfree) / v0
; Every root lies in [xfree - pressure, xfree + pressure] (|mu| <= 1) and
; there is always one. Above a small pressure there can be three (stick,
; slip, and an unstable middle one): the root kept is the one whose bracket is nearest last
; sample's x, which is the stick / slip hysteresis behind Helmholtz motion.
; The range is scanned in 64 brackets (tensors), then the chosen bracket is
; refined by one regula falsi step. Returns (F x): push the object by +F,
; and store x for the next sample's x-prev. The macro keeps no state, so a
; patch can solve it inside Gauss-Seidel sweeps with the connections that
; share its object (solved once before them, a stiff bridge's push on the
; bow point read as string velocity and pumped energy in after release).
(defmacro bow-contact (y y-prev x-prev C vb v0 pressure)
  (def vfree (- y y-prev))
  (def xfree (/ (- vb vfree) v0))
  (def step (/ (* 2 pressure) 64))
  (def x-lo (+ (- xfree pressure) (* (iota 64) step)))
  (def x-hi (+ x-lo step))
  (def h-lo (- (+ x-lo (* pressure (* (tanh (* 20 x-lo)) (+ 0.35 (/ 0.65 (+ 1 (abs x-lo))))))) xfree))
  (def h-hi (- (+ x-hi (* pressure (* (tanh (* 20 x-hi)) (+ 0.35 (/ 0.65 (+ 1 (abs x-hi))))))) xfree))
  (def crosses (lte (* h-lo h-hi) 0))
  ;; nearest crossing bracket to last sample's x, as a soft-min: weights
  ;; exp(-(dist - lb) / step) with lb a lower bound on the nearest distance,
  ;; so the exponent stays in float range; brackets a few steps farther than
  ;; the nearest get negligible weight
  (def dist (abs (- (+ x-lo (* 0.5 step)) x-prev)))
  (def lb (max 0 (- (abs (- xfree x-prev)) pressure step)))
  (def pick (* crosses (exp (* -1 (min (/ (- dist lb) step) 80)))))
  (def picks (max (sum pick) 1e-30))
  (def lo (/ (sum (* pick x-lo)) picks))
  (def hl (/ (sum (* pick h-lo)) picks))
  (def hh (/ (sum (* pick h-hi)) picks))
  (def x (- lo (/ (* hl step) (gswitch (gt (abs (- hh hl)) 1e-12) (- hh hl) 1e-12))))
  (def F (/ (* pressure v0 (* (tanh (* 20 x)) (+ 0.35 (/ 0.65 (+ 1 (abs x)))))) (max C 1e-12)))
  (tuple F x))
