; Improved Waveguide Flute v3
; Added feedback compression/limiting and safer parameter ranges

(def gate     (in 1  @name gate))
(def pitch    (in 2  @name pitch))
(def velocity (in 3  @name velocity))
(def trigger  (in 4  @name trigger))
(def mod1     (in 5  @name mod1 @modulator 1))
(def mod2     (in 6  @name mod2 @modulator 2))
(def mod3     (in 7  @name mod3 @modulator 3))
(def mod4     (in 8  @name mod4 @modulator 4))

(param attack     @default 80   @min 1    @max 1000 @unit ms)
(param release    @default 150  @min 1    @max 2000 @unit ms)
(param vibRate    @default 5.2  @min 0.1  @max 10   @unit Hz  @mod true @mod-mode additive)
(param vibDepth   @default 1.2  @min 0    @max 20   @unit Hz  @mod true @mod-mode additive)
(param jetRatio   @default 0.45 @min 0.1  @max 0.9)
(param pressure   @default 0.8  @min 0.1  @max 1.5  @mod true @mod-mode additive)
(param noise_amt  @default 0.1  @min 0    @max 0.5  @mod true @mod-mode additive)
(param coupling   @default 0.2  @min 0.0  @max 0.5)
(param reflection @default 0.9  @min 0.5  @max 0.98)
(param brightness @default 0.6  @min 0.0  @max 0.9  @mod true @mod-mode additive)
(param loop_gain  @default 0.97 @min 0.8  @max 0.995)
(param tuning     @default -1.2 @min -5.0 @max 5.0  @unit samples)
(param gain       @default 0.3  @min 0    @max 1)

; ── Histories ──
(make-history h_env)
(make-history h_bore)
(make-history h_ap1_in)
(make-history h_ap1_out)
(make-history h_lp)
(make-history h_dc)
(make-history h_noise_lp)

; ── Gate envelope ──
(def env_prev (read-history h_env))
(def att_c    (exp (/ -1.0 (max 1.0 (* attack samplerate 0.001)))))
(def rel_c    (exp (/ -1.0 (max 1.0 (* release samplerate 0.001)))))
(def env      (write-history h_env (mix gate env_prev (gswitch (gt gate env_prev) att_c rel_c))))

; ── Pitch ──
(def vib_sig  (* (sin (* twopi (phasor (mod vibRate)))) (mod vibDepth)))
(def pitch_hz (max 20.0 (+ pitch vib_sig)))
(def period   (max 2.0 (- (/ samplerate pitch_hz) tuning)))

; ── Noise ──
(def raw_noise (noise))
(def b_noise   (write-history h_noise_lp (mix raw_noise (read-history h_noise_lp) 0.8)))

; ── Jet Excitation ──
(def bore_prev (read-history h_bore))
; We scale the coupling by the envelope to ensure feedback dies down when released
(def jet_in    (* env (+ (mod pressure)
                         (* b_noise (mod noise_amt))
                         (* bore_prev coupling))))

(def jet_del   (delay jet_in (* period jetRatio)))

; Jet nonlinearity (soft-clipped)
(def jet_nl    (tanh (* 1.5 jet_del)))

; ── Bore Section ──
(def ap1_in    (+ jet_nl (* bore_prev reflection)))
(def ap1_out   (write-history h_ap1_out (+ (- ap1_in (read-history h_ap1_in)) (* (read-history h_ap1_out) (clip loop_gain 0 0.995)))))
(def _         (write-history h_ap1_in ap1_in))

(def lp_coeff  (- 0.98 (* (mod brightness) 0.9)))
(def bore_sig  (write-history h_lp (mix ap1_out (read-history h_lp) lp_coeff)))

; Bore Delay
(def bore_del   (delay bore_sig (* period (- 1.0 jetRatio))))

; DC block
(def dc_lp      (write-history h_dc (mix bore_del (read-history h_dc) 0.995)))
(def dc_blocked (- bore_del dc_lp))

; Feedback Compression: Soft-clip the signal before it goes back into the loop
; This prevents "peaking" and runaway feedback
(def bore_final (tanh (* 1.1 dc_blocked)))

; Update history
(def _          (write-history h_bore bore_final))

; Output scaling
(def out_sig    (* bore_final (+ 0.3 (* 0.7 velocity)) gain))

(out out_sig 1 @name audio)
