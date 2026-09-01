; mobius-warp — Möbius (rational bilinear) phase distortion for phasor-domain
; phase (0..1 = one cycle), the core Wavetable synth's "warp" stage applied
; BEFORE a table read:  p' = k*p / (1 + (k-1)*p)  with k = 1 + 6*amount.
; Endpoints stay pinned (0->0, 1->1) so the remap is click-free; in between
; the read head sprints through the early cycle and crawls through the rest,
; squeezing the waveform asymmetrically for a PWM/sync-like brightness.
; amount 0 is the identity (k = 1). Typical use:
;   (sample table (mobius-warp (phasor freq) warp) wave)
(defmacro mobius-warp (phase amount)
  (def k (+ 1 (* 6 (clip amount 0 1))))
  (def out (/ (* k phase) (+ 1 (* (- k 1) phase))))
  out)
