; Aphex-style 808 Tom Synth
; Features a sine-based core with extreme FM, ring modulation, and "laser" pitch envelopes.

(def gate (in 1 @name gate))
(def pitch (in 2 @name pitch))
(def velocity (in 3 @name velocity))
(def trigger (in 4 @name trigger))
(def mod1 (in 5 @name mod1 @modulator 1))
(def mod2 (in 6 @name mod2 @modulator 2))
(def mod3 (in 7 @name mod3 @modulator 3))
(def mod4 (in 8 @name mod4 @modulator 4))
(def mod5 (in 9 @name mod5 @modulator 5))
(def mod6 (in 10 @name mod6 @modulator 6))

; --- Parameters ---
(param tune           @default 80   @min 40    @max 400   @unit Hz @mod true @mod-mode additive)
(param decay_ms       @default 300  @min 20    @max 2000  @unit ms @mod true @mod-mode additive)
(param pitch_env_amt  @default 150  @min 0     @max 2000  @unit Hz @mod true @mod-mode additive)
(param pitch_env_dec  @default 40   @min 5     @max 500   @unit ms @mod true @mod-mode additive)
(param impact_lvl     @default 0.2  @min 0     @max 1     @mod true @mod-mode additive)
(param fm_amt         @default 0.0  @min 0     @max 1000  @unit Hz @mod true @mod-mode additive)
(param fm_ratio       @default 3.5  @min 0.5   @max 20    @mod true @mod-mode additive)
(param drive          @default 1.2  @min 1.0   @max 20    @mod true @mod-mode additive)
(param ring_mod_freq  @default 0    @min 0     @max 5000  @unit Hz @mod true @mod-mode additive)
(param crush          @default 0    @min 0     @max 1     @mod true @mod-mode additive)
(param gain           @default 0.6  @min 0     @max 1)

; --- Envelopes ---
; Fast exponential decay for the 808 "thump"
(def p_env (adsr trigger trigger 1 (mod pitch_env_dec) 0 5))
; Main amplitude envelope
(def a_env (adsr gate trigger 2 (mod decay_ms) 0 10))
; Impact envelope for the initial "stick" sound
(def i_env (adsr trigger trigger 0 10 0 5))

; --- Oscillators ---
; Base frequency calculated from MIDI pitch (assumed Hz) or Tune param
(def base_freq (+ (mod tune) (* pitch 0.1))) ; blending midi pitch slightly
(def sweep_freq (+ base_freq (* p_env (mod pitch_env_amt))))

; FM Modulator
(def fm_mod (* (sin (* (phasor (* sweep_freq (mod fm_ratio))) 6.28318)) (mod fm_amt)))

; Main Body (Sine)
(def body (sin (* (phasor (+ sweep_freq fm_mod)) 6.28318)))

; Ring Modulation (the "weird" Aphex part)
(def rm_osc (sin (* (phasor (mod ring_mod_freq)) 6.28318)))
(def mixed_body (mix body (* body rm_osc) (gt (mod ring_mod_freq) 10)))

; --- Noise & Impact ---
(def noise_src (noise))
(def impact_filt (biquad noise_src 4000 0.7 1 1)) ; High pass for the click
(def impact (* impact_filt i_env (mod impact_lvl)))

; --- Signal Chain ---
(def combined (+ mixed_body impact))

; Saturation / Drive
(def saturated (tanh (* combined (mod drive))))

; Bitcrush/Downsample logic
(defmacro sample_hold (sig speed)
  (def ph (phasor speed))
  (latch sig (lt ph 0.05)))

(def crushed (mix saturated (sample_hold saturated (- 20000 (* (mod crush) 19500))) (mod crush)))

; Output
(out (* crushed a_env velocity gain) 1 @name audio)
