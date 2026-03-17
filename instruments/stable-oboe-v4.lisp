; Stable Waveguide Oboe v4
; Fixed silence issue by increasing internal feedback gain
; Improved pitch tracking and "bite"

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
(param release    @default 150  @min 1    @max 2000 @unit ms)
(param vibRate    @default 5.5  @min 0.1  @max 10   @unit Hz  @mod true @mod-mode additive)
(param vibDepth   @default 1.2  @min 0    @max 20   @unit Hz  @mod true @mod-mode additive)
(param pressure   @default 0.9  @min 0.1  @max 2.0  @mod true @mod-mode additive)
(param stiffness  @default 1.5  @min 0.5  @max 5.0)  
(param reflection @default 0.98 @min 0.8  @max 0.999) 
(param brightness @default 0.5  @min 0.0  @max 0.9  @mod true @mod-mode additive)
(param nasal      @default 1100 @min 500  @max 3000 @unit Hz  @mod true @mod-mode additive) 
(param tuning     @default 1.4  @min -5.0 @max 5.0  @unit samples)
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

; 2. Pitch / Timing
(def vib_sig  (* (sin (* twopi (phasor (max 0.1 vibRate)))) vibDepth))
(def pitch_hz (max 20.0 (+ pitch vib_sig)))
(def period   (max 2.0 (- (/ 44100.0 pitch_hz) tuning)))

; 3. Reed Model
(def bore_prev (read-history h_bore))
; Adding a tiny bit of noise helps the reed "catch" and start oscillating
(def reed_noise (* (noise) 0.01))
(def pres_diff  (- (+ (* (mod pressure) env) reed_noise) (* bore_prev 0.5)))
(def excitation (tanh (* pres_diff stiffness)))

; 4. The Loop
; We combine excitation and feedback, then delay
(def loop_sig    (+ excitation (* bore_prev reflection)))
(def bore_del    (delay loop_sig period))

; 5. Stability Filters
(def lp_coeff    (min 0.99 (max 0.05 (- 0.98 (* (mod brightness) 0.7)))))
(def bore_lp     (write-history h_lp (mix bore_del (read-history h_lp) lp_coeff)))

; DC Block to prevent offset buildup
(def dc_lp       (write-history h_dc (mix bore_lp (read-history h_dc) 0.995)))
(def bore_sig    (- bore_lp dc_lp))

; Drive the loop hard enough to sustain, but tanh clips it to 1.0
(def bore_out    (tanh (* bore_sig 1.1))) 
(def _           (write-history h_bore bore_out))

; 6. Nasal Formant (Peaking Filter)
; Mode 6 is often a peaking/bell filter which preserves the signal better than BP
(def nasal_sig   (biquad bore_out (min (mod nasal) 10000) 1.5 2.0 6))

; 7. Output
(def out_sig     (* nasal_sig (+ 0.2 (* 0.8 velocity)) gain))
(out out_sig 1 @name audio)