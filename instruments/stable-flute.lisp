; Improved Waveguide Flute
; Stabilized feedback with DC blocking and soft-clipping (tanh)
; Improved pitch accuracy by compensating for filter phase delay

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
(param vibRate    @default 5.2  @min 0.1  @max 10   @unit Hz  @mod true @mod-mode additive)
(param vibDepth   @default 1.2  @min 0    @max 20   @unit Hz  @mod true @mod-mode additive)
(param jetRatio   @default 0.45 @min 0.1  @max 0.9)
(param pressure   @default 0.75 @min 0.1  @max 1.5  @mod true @mod-mode additive)
(param noise_amt  @default 0.08 @min 0    @max 0.5  @mod true @mod-mode additive)
(param coupling   @default 0.25 @min 0.0  @max 0.7)
(param reflection @default 0.92 @min 0.5  @max 0.99)
(param brightness @default 0.6  @min 0.0  @max 0.9  @mod true @mod-mode additive)
(param loop_gain  @default 0.98 @min 0.8  @max 0.999)
(param tuning     @default -1.2 @min -5.0 @max 5.0  @unit samples) ; offset to align pitch octaves
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
(def att_c    (exp (/ -1.0 (max 1.0 (* attack 44.1)))))
(def rel_c    (exp (/ -1.0 (max 1.0 (* release 44.1)))))
(def env      (write-history h_env (mix gate env_prev (gswitch (gt gate env_prev) att_c rel_c))))

; ── Pitch and timing ──
(def vib_sig  (* (sin (* twopi (phasor (mod vibRate)))) (mod vibDepth)))
(def pitch_hz (max 20.0 (+ pitch vib_sig)))
; Compensate for filter delays by subtracting 'tuning' samples from the total period
(def period   (max 2.0 (- (/ 44100.0 pitch_hz) tuning)))

; ── Noise Source (LP filtered for more 'breath' and less 'hiss') ──
(def raw_noise (noise))
(def b_noise   (write-history h_noise_lp (mix raw_noise (read-history h_noise_lp) 0.8)))

; ── Jet Excitation ──
(def bore_prev (read-history h_bore))
(def jet_in    (* env (+ pressure
                         (* b_noise (mod noise_amt))
                         (* bore_prev coupling))))

; Jet delay
(def jet_del   (delay jet_in (* period jetRatio)))

; Jet nonlinearity: use tanh for smoother saturation than x-x^3
(def jet_nl    (tanh (* 1.5 jet_del)))

; ── Bore / Allpass 1 ──
(def ap1_in    (+ jet_nl (* bore_prev reflection)))
(def ap1_out   (write-history h_ap1_out (+ (- ap1_in (read-history h_ap1_in)) (* (read-history h_ap1_out) loop_gain))))
(def _         (write-history h_ap1_in ap1_in))

; ── Brightness (LP Filter) ──
(def lp_coeff  (- 0.98 (* (mod brightness) 0.9)))
(def bore_sig  (write-history h_lp (mix ap1_out (read-history h_lp) lp_coeff)))

; ── Bore Delay + DC Block ──
(def bore_del   (delay bore_sig (* period (- 1.0 jetRatio))))
; DC block: y = x - x_lp
(def dc_lp      (write-history h_dc (mix bore_del (read-history h_dc) 0.995)))
(def bore_final (- bore_del dc_lp))

; Feedback update
(def _          (write-history h_bore bore_final))

; Velocity scaling
(def out_sig    (* bore_final (+ 0.3 (* 0.7 velocity)) gain))

(out out_sig 1 @name audio)
