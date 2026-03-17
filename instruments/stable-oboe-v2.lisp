; Stable Waveguide Oboe v2
; Reed-style excitation with noise injection and peaking formant filter

(def gate     (in 1  @name gate))
(def pitch    (in 2  @name pitch))
(def velocity (in 3  @name velocity))
(def trigger  (in 4  @name trigger))
(def mod1     (in 5  @name mod1 @modulator 1))
(def mod2     (in 6  @name mod2 @modulator 2))
(def mod3     (in 7  @name mod3 @modulator 3))
(def mod4     (in 8  @name mod4 @modulator 4))
(def mod5     (in 9  @name mod5 @modulator 5))
(def mod6     (in 10 @name mod6 @modulator 6))

(param attack     @default 80   @min 1    @max 1000 @unit ms)
(param release    @default 150  @min 1    @max 2000 @unit ms)
(param vibRate    @default 5.5  @min 0.1  @max 10   @unit Hz  @mod true @mod-mode additive)
(param vibDepth   @default 1.2  @min 0    @max 20   @unit Hz  @mod true @mod-mode additive)
(param pressure   @default 0.8  @min 0.1  @max 1.5  @mod true @mod-mode additive)
(param noise_amt  @default 0.05 @min 0    @max 0.3)
(param stiffness  @default 1.5  @min 0.5  @max 4.0)
(param reflection @default 0.92 @min 0.5  @max 0.98)
(param brightness @default 0.5  @min 0.0  @max 0.9  @mod true @mod-mode additive)
(param nasal_freq @default 1100 @min 500  @max 3000 @unit Hz  @mod true @mod-mode additive)
(param nasal_gain @default 6.0  @min 0    @max 12   @unit dB)
(param loop_gain  @default 0.97 @min 0.8  @max 0.99)
(param tuning     @default -2.0 @min -10.0 @max 10.0 @unit samples)
(param gain       @default 0.3  @min 0    @max 1)

; Histories
(make-history h_env)
(make-history h_bore)
(make-history h_lp)
(make-history h_dc)

; 1. Envelope
(def env_prev (read-history h_env))
(def att_c    (exp (/ -1.0 (max 1.0 (* attack 44.1)))))
(def rel_c    (exp (/ -1.0 (max 1.0 (* release 44.1)))))
(def env      (write-history h_env (mix gate env_prev (gswitch (gt gate env_prev) att_c rel_c))))

; 2. Pitch
(def vib_sig  (* (sin (* twopi (phasor vibRate))) vibDepth))
(def pitch_hz (max 20.0 (+ pitch vib_sig)))
(def period   (max 2.0 (- (/ 44100.0 pitch_hz) tuning)))

; 3. Reed Excitation
; Real reeds close under pressure. 
; Flow is roughly proportional to pressure difference, but throttled by the reed gap.
(def bore_prev (read-history h_bore))
(def p_diff    (- (* pressure env) (* bore_prev 0.3)))
; Reed opening: 1.0 = open, 0.0 = closed.
(def reed_open (max 0.0 (- 1.0 (* p_diff p_diff stiffness))))
(def excitation (* p_diff reed_open))

; Add breath noise to help oscillation start and add texture
(def breath    (* (noise) noise_amt env))
(def total_ex  (+ excitation breath))

; 4. Bore Delay
(def bore_in   (+ total_ex (* bore_prev reflection)))
(def bore_del  (delay bore_in period))

; 5. Stabilization & Filter
; LP to soften the reed buzz
(def lp_coeff  (min 0.98 (max 0.1 (- 0.98 (* brightness 0.8)))))
(def bore_lp   (write-history h_lp (mix bore_del (read-history h_lp) lp_coeff)))

; DC Block
(def dc_lp     (write-history h_dc (mix bore_lp (read-history h_dc) 0.995)))
(def bore_sig  (- bore_lp dc_lp))

; Energy limiting
(def bore_stable (tanh (* bore_sig loop_gain)))
(def _           (write-history h_bore bore_stable))

; 6. Nasal Formant (Peaking filter @ Mode 5)
(def formant   (biquad bore_stable (min nasal_freq 8000) 2.0 nasal_gain 5))

; 7. Final Output
(def out_sig   (* formant (+ 0.3 (* 0.7 velocity)) gain))
(out out_sig 1 @name audio)
