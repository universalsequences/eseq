; 8-bit NES/Gameboy style synth - Modulatable Version
; Features pulse, triangle, and noise oscillators with bit-crushing and filtering.

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

(defmacro pulse_from_phase (phase width)
  (scale (lt phase width) 0 1 -1 1))

(defmacro crush (sig levels)
  (def safe_levels (max 2 levels))
  (def norm (* (+ sig 1) 0.5))
  (def stepped (/ (floor (* norm safe_levels)) (max 1 safe_levels)))
  (- (* stepped 2) 1))

(param amp_attack   @default 2    @min 1   @max 1000 @unit ms)
(param amp_decay    @default 100  @min 1   @max 2000 @unit ms)
(param amp_sustain  @default 0.5  @min 0   @max 1)
(param amp_release  @default 150  @min 1   @max 3000 @unit ms)

(param pulse_width  @default 0.5  @min 0.05 @max 0.95 @mod true @mod-mode additive)
(param pulse_mix    @default 0.8  @min 0    @max 1    @mod true @mod-mode additive)
(param tri_mix      @default 0.2  @min 0    @max 1    @mod true @mod-mode additive)
(param noise_mix    @default 0.0  @min 0    @max 1    @mod true @mod-mode additive)

(param bit_depth    @default 8    @min 1    @max 16   @mod true @mod-mode additive)
(param drive        @default 1.0  @min 1    @max 8    @mod true @mod-mode additive)

(param cutoff       @default 5000 @min 40   @max 12000 @unit Hz @mod true @mod-mode additive)
(param resonance    @default 1.0  @min 0.5  @max 4.0   @mod true @mod-mode additive)

(param pitch_bend   @default 0    @min -12  @max 12 @unit st @mod true @mod-mode additive)
(param gain         @default 0.3  @min 0    @max 1)

(def env (adsr gate trigger amp_attack amp_decay amp_sustain amp_release))

; Pitch with bend modulation
(def bend_ratio (exp (/ (* (log 2) (mod pitch_bend)) 12)))
(def final_pitch (* pitch bend_ratio))

(def ph (phasor final_pitch))
(def p_wave (pulse_from_phase ph (clip (mod pulse_width) 0.05 0.95)))
(def t_wave (triangle ph))
(def n_wave (noise))

; Mixing oscillators with modulation
(def combined (+ (* p_wave (clip (mod pulse_mix) 0 1)) 
                 (* t_wave (clip (mod tri_mix) 0 1)) 
                 (* n_wave (clip (mod noise_mix) 0 1))))

; Distortion and Bitcrushing
(def driven (tanh (* combined (mod drive))))
(def quantized (crush driven (pow 2 (clip (mod bit_depth) 1 16))))

; Multimode-ish filter (LP)
(def filtered (biquad quantized (clip (mod cutoff) 40 12000) (clip (mod resonance) 0.5 4.0) 1 0))

(out (* filtered env velocity gain) 1 @name audio)
