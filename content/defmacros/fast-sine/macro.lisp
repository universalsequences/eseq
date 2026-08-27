; fast-sine — sin(2pi * phase) for phasor-domain phase (0..1 = one cycle).
; Degree-13 root-factored Chebyshev polynomial (max abs error ~5.9e-8),
; rescaled from https://moooo.ooo/chebyshev-sine-approximation/.
; Estrin-scheme evaluation: a Horner chain is a serial dependency chain and
; loses to libm sinf in per-sample feedback loops (measured 36 vs 22
; ns/sample in a feedback-FM patch); Estrin's parallel halves reach parity
; there while staying branchless and SIMD-friendly in vectorized blocks.
; Input is wrapped, so FM-style phase offsets (phase + mod) are safe.
; p(y) = 3.1616y^5 - 14.0497y^4 + 38.4959y^3 - 67.0766y^2 + 64.8358y
;        - 25.1327  =  a + y2*b + y4*c, with a/b/c independent linear terms.
(defmacro fast-sine (phase)
  (def x (wrap (- phase 0.5) -0.5 0.5))
  (def y (* x x))
  (def y2 (* y y))
  (def y4 (* y2 y2))
  (def a (+ (* 64.83582305908203 y) -25.132740020751953))
  (def b (+ (* 38.49587249755859 y) -67.07662200927734))
  (def c (+ (* 3.1616015434265137 y) -14.049662590026855))
  (def poly (+ (+ a (* y2 b)) (* y4 c)))
  (def lead (* x (- 0.25 y)))
  (def out (* lead poly))
  out)
