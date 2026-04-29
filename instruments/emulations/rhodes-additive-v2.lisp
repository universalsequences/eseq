; Rhodes Suitcase Electric Piano (Additive V2)
; Optimized for better high-register response (less "tinny")
; Pure Additive Synthesis - sums sine partials to model tine physics

(def gate     (in 1  @name gate))
(def pitch    (in 2  @name pitch)) ; Fundamental frequency in Hz
(def velocity (in 3  @name velocity))
(def trigger  (in 4  @name trigger))

; Modulator inputs
(def mod1     (in 5  @name mod1 @modulator 1))
(def mod2     (in 6  @name mod2 @modulator 2))
(def mod3     (in 7  @name mod3 @modulator 3))
(def mod4     (in 8  @name mod4 @modulator 4))
(def mod5     (in 9  @name mod5 @modulator 5))
(def mod6     (in 10 @name mod6 @modulator 6))

; --- Sound Shaping Parameters ---
(param decay      @default 1200 @min 100  @max 5000 @unit ms @mod true @mod-mode additive)
(param tine_vol   @default 0.20 @min 0    @max 1    @mod true @mod-mode additive)
(param harmonic_2 @default 0.35 @min 0    @max 1    @mod true @mod-mode additive)
(param harmonic_4 @default 0.15 @min 0    @max 1    @mod true @mod-mode additive)
(param bark_amt   @default 0.50 @min 0    @max 1)
(param detune     @default 0.04 @min 0    @max 0.5  @mod true @mod-mode additive)
(param gain       @default 0.25 @min 0    @max 1    @mod true @mod-mode additive)

; --- Suitcase Stereo Pan (Vibrato) ---
(param vib_speed  @default 4.5  @min 0.1  @max 12   @unit Hz)
(param vib_depth  @default 0.5  @min 0    @max 1    @mod true @mod-mode additive)

; --- Key Scaling Logic ---
; Real Rhodes have shorter sustain and fewer harmonics in high octaves.
; Middle C is ~261Hz. We'll use this as our pivot point.
(def key_factor (pow (/ 261.0 (+ 261.0 pitch)) 0.5)) ; Drops off as pitch increases
(def decay_factor (pow (/ 440.0 (+ 440.0 pitch)) 0.7)) ; Shorter decay for high notes

; --- Envelopes ---
(def scaled_decay (* (mod decay) decay_factor))
(def amp_env (adsr gate trigger 2 scaled_decay 0.0 150))
(def tine_env (adsr gate trigger 1 (* 25 decay_factor) 0.0 1))

; --- Additive Engine ---
; Fundamental (1.0x)
(def fundamental (sin (* twopi (phasor pitch))))

; Harmonic 2 (2.0x) - Scaled slightly by key to avoid tinny highs
(def h2_vol (* (mod harmonic_2) (+ 0.5 (* 0.5 key_factor))))
(def h2 (sin (* twopi (phasor (* pitch (+ 2.0 (* (mod detune) 0.01)))))))

; Harmonic 4 (4.0x) - The "Bark" partial
; Aggressively scaled down in higher registers to avoid a "plasticky" sound
(def h4_key_scaling (pow key_factor 1.5))
(def vel_bark (pow velocity (+ 1.0 (- 1.0 bark_amt))))
(def h4_raw (sin (* twopi (phasor (* pitch (+ 4.0 (* (mod detune) 0.02)))))))
(def h4 (* h4_raw vel_bark (mod harmonic_4) h4_key_scaling))

; Metallic Tine Strike (14.5x)
; Heavily damped for high notes to prevent "whistling"
(def tine_osc (sin (* twopi (phasor (* pitch 14.5)))))
(def tine_sig (* tine_osc tine_env (mod tine_vol) h4_key_scaling velocity))

; --- Mix & Output ---
(def tone_bar (+ fundamental (* h2 h2_vol) h4))
(def dry_sig  (+ (* tone_bar amp_env velocity) tine_sig))

; Saturation helps bind the additive partials together
(def driven (tanh (* dry_sig (+ 1.0 (* velocity bark_amt 2.0)))))

; --- Stereo Pan ---
(def pan_lfo (* 0.5 (+ 1.0 (sin (* twopi (phasor vib_speed))))))
(def pan_depth (mod vib_depth))
(def left_gain  (+ (- 1.0 pan_depth) (* pan_depth (- 1.0 pan_lfo))))
(def right_gain (+ (- 1.0 pan_depth) (* pan_depth pan_lfo)))

(out (* driven left_gain (mod gain))  1 @name left)
(out (* driven right_gain (mod gain)) 2 @name right)
