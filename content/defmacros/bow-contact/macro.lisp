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
; mu falls no faster than 0.428 per unit x, so h rises everywhere and its root
; is unique below pressure 2.33 (the knob's range ends at 2): the stick / slip
; hysteresis of Helmholtz motion comes from the string, not from choosing
; between roots. mu is odd, so the root has xfree's sign and the macro solves
; for a = |xfree| on [0, a] (h(0) = -a, h(a) >= 0). It starts at the larger of
; the stick line's root (tanh(20x) <= 20x, a lower bound) and the slip
; curve's (tanh = 1, a quadratic; near the knob's top pressure it can land
; just past the root), then takes three Newton steps, each kept inside the
; shrinking bracket (a step leaving it bisects instead). That is more
; accurate than a 64-bracket scan plus a regula falsi step (force error under
; 0.5% of pressure against 13%) at a twentieth of its cost, which matters: a
; patch solves its bow inside every Gauss-Seidel sweep. x-prev is unused, kept
; so patches written against the scanning version still compile.
; Returns (F x): push the object by +F. The macro keeps no state, so a patch can solve it inside
; Gauss-Seidel sweeps with the connections that share its object (solved once
; before them, a stiff bridge's push on the bow point read as string velocity
; and pumped energy in after release).
; The friction curve mu at x >= 0.
(defmacro bow-mu (x)
  (* (tanh (* 20 x)) (+ 0.35 (/ 0.65 (+ 1 x)))))

; One Newton step on h(x) = x + p mu(x) - a from x inside [lo, hi], h(lo) <= 0
; <= h(hi): the step, or the midpoint of the narrowed bracket when the step
; leaves it. Returns (x lo hi) after it.
(defmacro bow-newton-step (x lo hi a p)
  (def t (tanh (* 20 x)))
  (def r (/ 1 (+ 1 x)))
  (def g (+ 0.35 (* 0.65 r)))
  (def h (- (+ x (* p t g)) a))
  (def d (max (+ 1 (* p (- (* 20 (- 1 (* t t)) g) (* 0.65 t r r)))) 0.0001))
  (def below (lt h 0))
  (def lo-next (gswitch below x lo))
  (def hi-next (gswitch below hi x))
  (def n (- x (/ h d)))
  (tuple (gswitch (* (gte n lo-next) (lte n hi-next)) n (* 0.5 (+ lo-next hi-next))) lo-next hi-next))

(defmacro bow-contact (y y-prev x-prev C vb v0 pressure)
  (def vfree (- y y-prev))
  (def xfree (/ (- vb vfree) v0))
  (def a (abs xfree))
  (def p pressure)
  (def slip-b (+ (- 1 a) (* 0.35 p)))
  (def slip-disc (- (* slip-b slip-b) (* 4 (- p a))))
  (def x-slip (gswitch (gte slip-disc 0) (* 0.5 (- (sqrt (max slip-disc 0)) slip-b)) 0))
  (def x0 (max (max (/ a (+ 1 (* 20 p))) x-slip) 0))
  (def (x1 lo1 hi1) (bow-newton-step x0 0 a a p))
  (def (x2 lo2 hi2) (bow-newton-step x1 lo1 hi1 a p))
  (def (x3 lo3 hi3) (bow-newton-step x2 lo2 hi2 a p))
  (def s (gswitch (lt xfree 0) -1 1))
  (def F (/ (* pressure v0 s (bow-mu x3)) (max C 1e-12)))
  (tuple F (* s x3)))
