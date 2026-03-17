; Stable Waveguide Flute
; Stabilized with internal saturation (tanh) in the feedback loop and DC blocking.
; This prevents the feedback from blowing up even with high gain/pressure settings.

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

(param attack     @default 60   @min 1    @max 1000 @unit ms)
(param release    @default 120  @min 1    @max 2000 @unit ms)
(param vibRate    @default 5.2  @min 0.1  @max 10   @unit Hz  @mod true @mod-mode additive)
(param vibDepth   @default 1.2  @min 0    @max 20   @unit Hz  @mod true @mod-mode additive)
(param jetRatio   @default 0.4  @min 0.1  @max 0.9)
(param pressure   @default 0.8  @min 0.1  @max 2.0  @mod true @mod-mode additive)
(param noise_amt  @default 0.1  @min 0    @max 0.5  @mod true @mod-mode additive)
(param coupling   @default 0.3  @min 0.0  @max 0.8)
(param reflection @default 0.9  @min 0.5  @max 0.98)
(param brightness @default 0.6  @min 0.0  @max 0.9  @mod true @mod-mode additive)
(param saturation @default 1.5  @min 0.5  @max 5.0) ; Controls how hard the feedback is "compressed"
(param tuning     @default -1.5 @min -5.0 @max 5.0  @unit samples)
(param gain       @default 0.25 @min 0    @max 1)

; ── Histories ──
(make-history h_env)
(make-history h_bore)
(make-history h_ap_in)
(make-history h_ap_out)
(make-history h_lp)
(make-history h_dc)
(make-history h_noise_lp)

; ── Gate envelope (one-pole smoother) ──
(def env_prev (read-history h_env))
(def att_c    (exp (/ -1.0 (max 1.0 (* attack 44.1)))))
(def rel_c    (exp (/ -1.0 (max 1.0 (* release 44.1)))))
(def env      (write-history h_env (mix gate env_prev (gswitch (gt gate env_prev) att_c rel_c))))

; ── Pitch with Vibrato ──
; Note: We use the base parameter names for now to ensure compatibility if 'mod' operator is strict
(def vib_sig  (* (sin (* twopi (phasor vibRate))) vibDepth))
(def pitch_hz (max 20.0 (+ pitch vib_sig)))
(def period   (max 2.0 (- (/ 44100.0 pitch_hz) tuning)))

; ── Noise Generator (Filtered for breathiness) ──
(def b_noise (write-history h_noise_lp (mix (noise) (read-history h_noise_lp) 0.85)))

; ── Jet Section ──
(def bore_prev (read-history h_bore))
(def jet_in    (* env (+ pressure 
                         (* b_noise noise_amt) 
                         (* bore_prev coupling))))

; Jet delay & cubic nonlinearity
(def jet_del (delay jet_in (* period jetRatio)))
(def jet_nl  (- jet_del (* jet_del jet_del jet_del)))

; ── Reflection & Allpass Loop ──
(def ap_in   (+ (clip jet_nl -1 1) (* bore_prev reflection)))
; Simple allpass: y[n] = (x[n] - y[n-1]) * g + x[n-1] ... wait, the reference used:
; (ap1_in - h3) + h2*loop_gain
(def ap_out  (+ (- ap_in (read-history h_ap_in)) (* (read-history h_ap_out) 0.98)))
(def _       (write-history h_ap_in ap_in))
(def _       (write-history h_ap_out ap_out))

; ── Brightness (Low-pass) ──
(def lp_c    (- 0.98 (* brightness 0.9)))
(def lp_out  (write-history h_lp (mix ap_out (read-history h_lp) lp_c)))

; ── Bore Delay & DC Block ──
(def bore_del (delay lp_out (* period (- 1.0 jetRatio))))
(def dc_lp    (write-history h_dc (mix bore_del (read-history h_dc) 0.995)))
(def bore_sig (- bore_del dc_lp))

; ── Feedback Compression (The Stabilizer) ──
; Tanh keeps the internal signal from exceeding 1.0, effectively compressing the feedback loop.
(def bore_final (tanh (* bore_sig saturation)))
(def _          (write-history h_bore bore_final))

; ── Output Scaling ──
(def out_sig (* bore_final (+ 0.3 (* 0.7 velocity)) gain))

(out out_sig 1 @name audio)
