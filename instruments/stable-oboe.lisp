; Stable Waveguide Oboe
; Reed-style excitation with feedback saturation and nasal formant filter

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

(param attack     @default 120  @min 1    @max 1000 @unit ms)
(param release    @default 180  @min 1    @max 2000 @unit ms)
(param vibRate    @default 5.8  @min 0.1  @max 10   @unit Hz  @mod true @mod-mode additive)
(param vibDepth   @default 1.5  @min 0    @max 20   @unit Hz  @mod true @mod-mode additive)
(param pressure   @default 0.7  @min 0.1  @max 1.5  @mod true @mod-mode additive)
(param stiffness  @default 2.0  @min 0.5  @max 5.0)  ; Reed stiffness/resistance
(param reflection @default 0.94 @min 0.5  @max 0.99) ; Bore reflection coefficient
(param brightness @default 0.6  @min 0.0  @max 0.9  @mod true @mod-mode additive)
(param nasal      @default 1200 @min 500  @max 3000 @unit Hz  @mod true @mod-mode additive) ; Oboe formant
(param loop_gain  @default 0.96 @min 0.8  @max 0.99) ; Master feedback stability
(param tuning     @default -2.5 @min -10.0 @max 10.0 @unit samples)
(param gain       @default 0.25 @min 0    @max 1)

; Histories for feedback and filtering
(make-history h_env)
(make-history h_bore)
(make-history h_lp)
(make-history h_dc)

; 1. Smooth Gate Envelope
(def env_prev (read-history h_env))
(def att_c    (exp (/ -1.0 (max 1.0 (* attack 44.1)))))
(def rel_c    (exp (/ -1.0 (max 1.0 (* release 44.1)))))
(def env      (write-history h_env (mix gate env_prev (gswitch (gt gate env_prev) att_c rel_c))))

; 2. Pitch and Vibrato
(def vib_sig  (* (sin (* twopi (phasor vibRate))) vibDepth))
(def pitch_hz (max 20.0 (+ pitch vib_sig)))
(def period   (max 2.0 (- (/ 44100.0 pitch_hz) tuning)))

; 3. Reed Excitation
; A reed is a pressure-controlled valve. Flow = PressureDiff * ReedOpening.
(def bore_prev (read-history h_bore))
(def pres_diff (- (* pressure env) (* bore_prev 0.4)))
; We use tanh to simulate the reed closing under high pressure
(def excitation (tanh (* pres_diff stiffness)))

; 4. Bore Loop with Soft-Clipping
; The feedback is passed through tanh to "compress" the energy and prevent peaking
(def bore_in     (+ excitation (* bore_prev reflection)))
(def bore_del    (delay bore_in period))

; 5. Tone Shaping & Stability
; Brightness Lowpass
(def lp_coeff    (min 0.98 (max 0.1 (- 0.98 (* brightness 0.8)))))
(def bore_lp     (write-history h_lp (mix bore_del (read-history h_lp) lp_coeff)))

; DC Block (crucial for feedback stability)
(def dc_lp       (write-history h_dc (mix bore_lp (read-history h_dc) 0.995)))
(def bore_sig    (- bore_lp dc_lp))

; Final feedback stage with gain-limiting
(def bore_stable (tanh (* bore_sig loop_gain)))
(def _           (write-history h_bore bore_stable))

; 6. Nasal Formant Filter (Bandpass-ish)
; Using a biquad in mode 2 (typically Bandpass) to isolate the Oboe's signature formant
(def nasal_sig   (biquad bore_stable (min nasal 8000) 1.5 1.0 2))

; 7. Output Scaling
(def out_sig     (* nasal_sig (+ 0.3 (* 0.7 velocity)) gain))
(out out_sig 1 @name audio)
