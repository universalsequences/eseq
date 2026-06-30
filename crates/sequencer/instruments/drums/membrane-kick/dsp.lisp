; Membrane Kick — port of a dgen physical-modeling kick to dgenlisp.
; Two coupled 4x4 wave-equation membranes (primary + secondary) driven by a
; low (~64 Hz) noise-FM strike, read out through a 3-stage biquad body. Uses
; same-mode conv2d (@padding same) so the Laplacian keeps membrane shape,
; matching the dgen patch's auto-padded conv2d 1:1.
;
; Params are the exact names / ranges / values from the source dgen patch, plus
; one added `impulse_decay` for the strike envelope length. The tensor masks are
; read off that patch's trained tensor editors (circle size ~ value). The output
; clamp + tanh limiter are the only non-param safeguards, since the eyeballed
; masks don't perfectly reproduce the trained stability balance.

; ── Host I/O ────────────────────────────────────────────────────────────────
(def gate (in 1 @name gate))
(def host_pitch (in 2 @name pitch))   ; host pitch lane (unused; freq is a param)
(def velocity (in 3 @name velocity))
(def trigger (in 4 @name trigger))
(def clock (in 5 @name clock))

; ── Params (exact names/ranges/defaults from the dgen patch) ────────────────
(param release @default 0.988 @min 0 @max 1)
(param freq @default 64.154 @min 20 @max 1000)
(param shape2 @default 1.0854 @min 0.2 @max 8)
(param feedback @default 8.9376 @min 0 @max 20)
(param coupling @default 0.5769 @min 0 @max 0.8)
(param shape @default 3.9555 @min 0.01 @max 8)
(param pitch @default 0.0264 @min 0.001 @max 0.25)
(param mixer @default 0.7446 @min 0 @max 1)
(param multi @default 1.0 @min 0 @max 1)
; dgen value was 0.991, but that maps to ~0.0001 (near-lossless) secondary
; damping which drones forever with the approximate masks. 0.5 decays to a
; proper kick (~480ms). Set back to 0.991 if you want the original sustain.
(param release2 @default 0.5 @min 0 @max 1)
(param pitch2 @default 0.0058 @min 0.001 @max 0.07)
; added: strike-impulse envelope decay (separate from the membrane releases)
(param impulse_decay @default 8 @min 0.5 @max 500 @unit ms)
; output body resonators (3 peaking biquads): exposed freq + gain for tuning
(param body1_freq @default 57 @min 20 @max 2000 @unit Hz)
(param body1_gain @default 1.2 @min 0 @max 12)
(param body2_freq @default 111 @min 20 @max 2000 @unit Hz)
(param body2_gain @default 1.5 @min 0 @max 12)
(param body3_freq @default 222 @min 20 @max 2000 @unit Hz)
(param body3_gain @default 6 @min 0 @max 12)
; clean output level (replaces the over-driven makeup; no tanh saturation).
; Low default because the membrane self-oscillates at the faithful param defaults
; with the approximate masks — turn up once the tone is tamed.
(param level @default 0.06 @min 0 @max 1)

; ── Feedback state ─────────────────────────────────────────────────────────
(make-tensor-history h3 @shape [4 4])      ; primary membrane state (t)
(make-tensor-history h2 @shape [4 4])      ; primary membrane state (t-1)
(make-tensor-history sh1 @shape [4 4])     ; secondary membrane state (t)
(make-tensor-history sh2 @shape [4 4])     ; secondary membrane state (t-1)
(make-tensor-history n1prev @shape [4 4])  ; previous primary output (breaks the loop)
(make-history h1)                          ; scalar exciter feedback


; ── Fixed kernels / masks ───────────────────────────────────────────────────
(def laplacian (tensor @shape [3 3] @data [0 1 0  1 -4 1  0 1 0]))
; tensor1 — coupling mask (left grid in the dgen tensor editor)
(def coupling-mask (tensor @shape [4 4] @data [
  0.9  0.9  0.04 0.04
  0.9  0.9  0.1  0.0
  0.5  0.04 0.04 0.0
  0.04 0.0  0.0  0.0]))
; primary strike-injection mask (right grid in the dgen tensor editor)
(def strike-mask (tensor @shape [4 4] @data [
  0.9  0.9  0.9  0.9
  0.04 0.2  0.0  0.1
  0.9  0.04 0.0  0.5
  0.9  0.2  0.0  0.2]))
; secondary strike-injection mask (subpatch literal [1 .2 1 1 0 0 1 ...])
(def strike-mask-s (tensor @shape [4 4] @data [1 0.2 1 1  0 0 1 0  0 0 0 0  0 0 0 0]))
(def zeroes (tensor @shape [4 4] @data [0 0 0 0 0 0 0 0  0 0 0 0  0 0 0 0]))

; ── Strike excitation ───────────────────────────────────────────────────────
; dgen's (in 1) excitation -> a fast velocity-scaled impulse envelope.
(def in1 (* (adsr gate trigger 0.5 impulse_decay 0 4) velocity))
(def scale1 (scale release 0 1 0.03 0.0001))

; Noise-FM exciter with scalar feedback (cos1). phasor resets on trigger (2nd arg).
(def exc-phase (phasor freq trigger))
(def cos1 (cos (+ (+ (* exc-phase twopi)
                     (* (scale (pow in1 shape2) 0 1 0.5 6) (noise)))
                  (* (gswitch trigger 0 (read-history h1)) feedback))))

; ── Secondary membrane (uses previous primary output, so no algebraic loop) ──
(def n1-prev (read-tensor-history n1prev))
(def s-state (read-tensor-history sh1))
(def s-prev (read-tensor-history sh2))
(def s-release (scale release2 0 1 0.03 0.0001))
(def s-stiff (scale pitch2 0 1 0.00001 0.02))
(def s-in1 (* n1-prev 0.5))                                   ; excitation from primary
(def s-coupled (* (read-tensor-history h2) coupling-mask))    ; (* (read-history h2) tensor1)
(def s-inject (* s-in1 strike-mask-s))
(def s-lap (conv2d (+ s-state s-inject) laplacian @padding same))
; restoring force = own Laplacian + coupling, both scaled by stiffness, exactly
; as the dgen subpatch: (* (+ (conv2d ...) (* in2 in3)) stiff).
(def s-next (+ (- (* (- 2 s-release) s-state) (* (- 1 s-release) s-prev))
               (* (+ s-lap (* s-coupled coupling)) s-stiff)))
(def s-nextc (max (min s-next 3) -3))   ; NaN-safety clamp only
(def sec-out0 s-nextc)  ; secondary_membrane1_0  (feeds sum2)
(def sec-out1 s-prev)   ; secondary_membrane1_1  (couples into primary)

; ── Primary membrane ────────────────────────────────────────────────────────
(def p-state (read-tensor-history h3))
(def p-prev (read-tensor-history h2))
(def body (biquad cos1 60 1 4 1))
(def inject (* (* body (pow in1 shape)) strike-mask))
(def p-lap (conv2d (+ p-state inject) laplacian @padding same))
(def couple (* (* sec-out1 coupling-mask) coupling))
(def stiff-gain (scale pitch 0 1 0.00001 0.02))
(def n1 (+ (- (* (- 2 scale1) p-state) (* (- 1 scale1) p-prev))
           (* (+ p-lap couple) stiff-gain)))
(def n1c (max (min n1 3) -3))   ; NaN-safety clamp only

; ── Write feedback (read-before-write order matches the dgen patch) ─────────
(write-tensor-history h2 p-state)   ; h2 <- old h3
(write-tensor-history h3 n1c)       ; h3 <- n1
(write-tensor-history sh2 s-state)
(write-tensor-history sh1 s-nextc)
(write-tensor-history n1prev n1c)
(write-history h1 cos1)   ; native reset: clears FM feedback on each trigger

; ── Output body (dgen chain: (* (mix ...) .2) -> 3 biquads) ─────────────────
(def sum1 (sum n1c))
(def sum2 (sum sec-out0))
(def driven (* (mix (* sum1 sum2) (mix sum1 sum2 mixer) multi) 0.2))
(def b1 (biquad driven body1_freq 8 body1_gain 5))
(def b2 (biquad b1 body2_freq 6 body2_gain 5))
(def b3 (biquad b2 body3_freq 6 body3_gain 5))
(out (* b3 level) 1 @name audio)   ; clean, no saturation
