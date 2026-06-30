; Membrane Kick — port of a dgen physical-modeling kick to dgenlisp.
; Two coupled 4x4 wave-equation membranes (primary + secondary) driven by a
; low (~64 Hz) noise-FM strike, read out through a 3-stage biquad body. Uses
; same-mode conv2d (@padding same) so the Laplacian keeps membrane shape,
; matching the dgen patch's auto-padded conv2d 1:1.
;
; The tensor masks are read off the source dgen patch's trained tensor editors
; (circle size ~ value). Membrane pitch is mapped from the host pitch input by
; the discrete 4x4 Laplacian dispersion relation, and release is mapped to a
; modal T60 damping coefficient so tuning does not change decay while the finite
; difference update stays inside its stability range.

; ── Host I/O ────────────────────────────────────────────────────────────────
(def gate (in 1 @name gate))
(def host_pitch (in 2 @name pitch))
(def velocity (in 3 @name velocity))
(def trigger (in 4 @name trigger))
(def clock (in 5 @name clock))

; ── Params ─────────────────────────────────────────────────────────────────
; release values are modal T60 times in milliseconds: each linear membrane mode
; decays by 60 dB over this time, independent of pitch.
(param release @default 680 @min 20 @max 5000 @unit ms)
(param freq @default 64.154 @min 20 @max 1000)
(param shape2 @default 1.0854 @min 0.2 @max 8)
(param feedback @default 8.9376 @min 0 @max 20)
(param coupling @default 0.5769 @min 0 @max 0.8)
(param shape @default 3.9555 @min 0.01 @max 8)
; Tune offsets the host note pitch. The default maps A4/440 Hz to the original
; kick neighborhood near 64 Hz while still tracking note changes exactly.
(param tune @default -33.3347 @min -60 @max 24 @unit st)
(param mixer @default 0.7446 @min 0 @max 1)
(param multi @default 1.0 @min 0 @max 1)
(param release2 @default 180 @min 20 @max 5000 @unit ms)
(param pitch2_ratio @default 0.484 @min 0.125 @max 4)
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
(param level @default 0.03 @min 0 @max 1)

; ── Feedback state ─────────────────────────────────────────────────────────
(make-tensor-history h3 @shape [4 4])      ; primary membrane state (t)
(make-tensor-history h2 @shape [4 4])      ; primary membrane state (t-1)
(make-tensor-history sh1 @shape [4 4])     ; secondary membrane state (t)
(make-tensor-history sh2 @shape [4 4])     ; secondary membrane state (t-1)
(make-tensor-history n1prev @shape [4 4])  ; previous primary output (breaks the loop)
(make-history h1)                          ; scalar exciter feedback


; ── Fixed kernels / masks ───────────────────────────────────────────────────
(def laplacian (tensor @shape [3 3] @data [0 1 0  1 -4 1  0 1 0]))
; 4x4 square membrane with zero-padded same convolution. For the 5-point
; Laplacian, the positive eigenvalue magnitudes are:
;   mu_pq = 4 - 2*cos(p*pi/(N+1)) - 2*cos(q*pi/(N+1))
; The fundamental uses p=q=1; stability must hold for p=q=N.
(def membrane-cells 4)
(def membrane-cos1 (cos (/ pi (+ membrane-cells 1))))
(def membrane-fundamental-mu (- 4 (* 4 membrane-cos1)))
(def membrane-max-mu (+ 4 (* 4 membrane-cos1)))
; Keep the requested pitch below the grid's dispersion/stability edge for every
; allowed release value. This is a physical model limit, not a tone control.
(def membrane-max-pitch (* samplerate 0.09))
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
(def p-release-sec (* (clip release 20 5000) 0.001))
(def p-damp (- 1 (exp (/ -13.8155106 (* samplerate p-release-sec)))))
(def membrane-pitch-hz
  (clip (* host_pitch (exp (/ (* (log 2) tune) 12))) 1 membrane-max-pitch))
(def p-pitch-sin (sin (* pi (/ membrane-pitch-hz samplerate))))
(def p-stiff-raw (/ (* 4 p-pitch-sin p-pitch-sin) membrane-fundamental-mu))
(def p-stiff-max (* 0.995 (/ (- 4 (* 2 p-damp)) membrane-max-mu)))
(def stiff-gain (min p-stiff-raw p-stiff-max))

; Noise-FM exciter with scalar feedback (cos1). phasor resets on trigger (2nd arg).
(def exc-phase (phasor freq trigger))
(def cos1 (cos (+ (+ (* exc-phase twopi)
                     (* (scale (pow in1 shape2) 0 1 0.5 6) (noise)))
                  (* (gswitch trigger 0 (read-history h1)) feedback))))

; ── Secondary membrane (uses previous primary output, so no algebraic loop) ──
(def n1-prev (read-tensor-history n1prev))
(def s-state (read-tensor-history sh1))
(def s-prev (read-tensor-history sh2))
(def s-release-sec (* (clip release2 20 5000) 0.001))
(def s-damp (- 1 (exp (/ -13.8155106 (* samplerate s-release-sec)))))
(def s-pitch-hz (clip (* membrane-pitch-hz pitch2_ratio) 1 membrane-max-pitch))
(def s-pitch-sin (sin (* pi (/ s-pitch-hz samplerate))))
(def s-stiff-raw (/ (* 4 s-pitch-sin s-pitch-sin) membrane-fundamental-mu))
(def s-stiff-max (* 0.995 (/ (- 4 (* 2 s-damp)) membrane-max-mu)))
(def s-stiff (min s-stiff-raw s-stiff-max))
(def s-in1 (* n1-prev 0.5))                                   ; excitation from primary
(def s-coupled (* (read-tensor-history h2) coupling-mask))    ; (* (read-history h2) tensor1)
(def s-inject (* s-in1 strike-mask-s))
(def s-lap (conv2d (+ s-state s-inject) laplacian @padding same))
; restoring force = own Laplacian + coupling, both scaled by stiffness, exactly
; as the dgen subpatch: (* (+ (conv2d ...) (* in2 in3)) stiff).
(def s-next (+ (- (* (- 2 s-damp) s-state) (* (- 1 s-damp) s-prev))
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
(def n1 (+ (- (* (- 2 p-damp) p-state) (* (- 1 p-damp) p-prev))
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
