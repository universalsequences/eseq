; Membrane Kick — port of a dgen physical-modeling kick to dgenlisp.
; Two coupled 4x4 wave-equation membranes (primary + secondary) driven by a
; low (~64 Hz) noise-FM strike, read out through a 3-stage biquad body. Uses
; same-mode conv2d (@padding same) so the Laplacian keeps membrane shape,
; matching the dgen patch's auto-padded conv2d 1:1.
;
; The coupling/strike tensor masks below are read off the dgen patch's trained
; tensor editors (circle size ~ value); param defaults are the user's settings.
; Those masks are eyeballed approximations, so the two coupled membranes sit near
; the stability edge. A small DC leak, a percussive VCA envelope, and a tanh
; output bound keep it a reliable kick; exact mask values would let you drop
; those safeguards and recover the original's natural decay.

; ── Host I/O ────────────────────────────────────────────────────────────────
(def gate (in 1 @name gate))
(def pitch (in 2 @name pitch))
(def velocity (in 3 @name velocity))
(def trigger (in 4 @name trigger))
(def clock (in 5 @name clock))

; ── Params (defaults from the source dgen patch) ────────────────────────────
(param release @default 0.988 @min 0 @max 1)
(param exc_freq @default 64.0 @min 20 @max 1000 @unit Hz)
(param exc_shape @default 1.0854 @min 0.2 @max 8)        ; was shape2
(param feedback @default 8.9376 @min 0 @max 20)
(param coupling @default 0.5769 @min 0 @max 0.8)
(param strike_shape @default 3.9555 @min 0.01 @max 8)    ; was shape
(param stiffness @default 0.0264 @min 0.001 @max 0.05)   ; was pitch
(param release2 @default 0.991 @min 0 @max 1)
(param stiffness2 @default 0.0058 @min 0.001 @max 0.07)  ; was pitch2
(param mixer @default 0.7446 @min 0 @max 1)
(param multi @default 1.0 @min 0 @max 1)
(param gain @default 0.8 @min 0 @max 1)
(param amp_decay @default 220 @min 20 @max 2000 @unit ms)

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

; ── Strike excitation ───────────────────────────────────────────────────────
; dgen's (in 1) excitation -> a fast velocity-scaled impulse envelope.
(def in1 (* (adsr gate trigger 0.5 6 0 4) velocity))
; Percussive VCA. Gate gives a clean full-level attack; sustain 0 with both
; decay and release = amp_decay means the kick rings out over ~amp_decay whether
; the host holds the note long or releases it immediately.
(def amp-env (adsr gate trigger 0.5 amp_decay 0 amp_decay))
; Resting damping raised from the patch's 0.03 so the approximate masks don't
; pump the membrane; strike damping (in1 high) stays low for a bright attack.
(def scale1 (scale (* release in1) 0 1 0.10 0.0008))

; Noise-FM exciter with scalar feedback (cos1).
(def exc-phase (phasor exc_freq trigger))
(def cos1 (cos (+ (+ (* exc-phase twopi)
                     (* (scale (pow in1 exc_shape) 0 1 0.5 6) (noise)))
                  (* (read-history h1) feedback))))

; ── Secondary membrane (uses previous primary output, so no algebraic loop) ──
(def n1-prev (read-tensor-history n1prev))
(def s-state (read-tensor-history sh1))
(def s-prev (read-tensor-history sh2))
; release2 mapped to ~0.0001 in the patch (near-lossless); floor it so the
; secondary always sheds energy with the approximate masks.
(def s-release (scale release2 0 1 0.16 0.05))
(def s-stiff (scale stiffness2 0 1 0.00001 0.02))
(def s-in1 (* n1-prev 0.5))                                   ; excitation from primary
(def s-coupled (* (read-tensor-history h2) coupling-mask))    ; (* (read-history h2) tensor1)
(def s-inject (* s-in1 strike-mask-s))
(def s-lap (conv2d (+ s-state s-inject) laplacian @padding same))
; restoring force = own Laplacian + coupling, both scaled by stiffness (matches
; the dgen subpatch: (* (+ (conv2d ...) (* in2 in3)) stiff)). The Laplacian must
; be ADDED, not multiplied — multiplying drops the restoring force when the
; primary is quiet and the membrane drifts into the rails.
(def s-next (+ (- (* (- 2 s-release) s-state) (* (- 1 s-release) s-prev))
               (* (+ s-lap (* s-coupled coupling)) s-stiff)))
; The leapfrog damping has unity gain at DC, so a slow ratchet builds an offset
; that drifts into the rails; a small per-sample leak bleeds it. Clamp = NaN net.
(def leak 0.999)
(def s-nextc (* (max (min s-next 3) -3) leak))
(def sec-out0 s-nextc)  ; secondary_membrane1_0  (feeds sum2)
(def sec-out1 s-prev)   ; secondary_membrane1_1  (couples into primary)

; ── Primary membrane ────────────────────────────────────────────────────────
(def p-state (read-tensor-history h3))
(def p-prev (read-tensor-history h2))
(def body (biquad cos1 60 1 4 1))
(def inject (* (* body (pow in1 strike_shape)) strike-mask))
(def p-lap (conv2d (+ p-state inject) laplacian @padding same))
(def couple (* (* sec-out1 coupling-mask) coupling))
(def stiff-gain (scale stiffness 0 1 0.00001 0.02))
(def n1 (+ (- (* (- 2 scale1) p-state) (* (- 1 scale1) p-prev))
           (* (+ p-lap couple) stiff-gain)))
(def n1c (* (max (min n1 3) -3) leak))

; ── Write feedback (read-before-write order matches the dgen patch) ─────────
(write-tensor-history h2 p-state)   ; h2 <- old h3
(write-tensor-history h3 n1c)       ; h3 <- n1
(write-tensor-history sh2 s-state)
(write-tensor-history sh1 s-nextc)
(write-tensor-history n1prev n1c)
(write-history h1 cos1)

; ── Output body ─────────────────────────────────────────────────────────────
(def sum1 (sum n1c))
(def sum2 (sum sec-out0))
; 0.2 was the dgen patch's body drive; makeup brings the small membrane sums up.
(def driven (* (mix (* sum1 sum2) (mix sum1 sum2 mixer) multi) 2.4))
(def b1 (biquad driven 57 8 1.2 5))
(def b2 (biquad b1 111 6 1.5 5))
(def b3 (biquad b2 222 6 6 5))
; tanh hard-bounds the body FIRST (so a railed membrane can't exceed unity),
; THEN the VCA env shapes the percussive decay and gates the tail to silence.
(out (* (tanh (* b3 gain 26)) amp-env 0.9) 1 @name audio)
