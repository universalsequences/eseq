; TB-303 style monosynth

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

(defmacro saw_from_phase (ph)
  (scale ph 0 1 -1 1))

(defmacro pulse_from_phase (ph width)
  (scale (lt ph width) 0 1 -1 1))

(param wave_mix        @default 0.0  @min 0    @max 1)
(param pulse_width     @default 0.5  @min 0.05 @max 0.95 @mod true @mod-mode additive)

(param amp_attack_ms   @default 1    @min 1    @max 200 @unit ms)
(param amp_decay_ms    @default 180  @min 1    @max 2000 @unit ms)
(param amp_sustain     @default 0.0  @min 0    @max 1)
(param amp_release_ms  @default 80   @min 1    @max 2000 @unit ms)

(param filt_attack_ms  @default 1    @min 1    @max 200 @unit ms)
(param filt_decay_ms   @default 220  @min 1    @max 3000 @unit ms)
(param filt_sustain    @default 0.0  @min 0    @max 1)
(param filt_release_ms @default 120  @min 1    @max 3000 @unit ms)

(param cutoff          @default 700  @min 40   @max 8000 @unit Hz @mod true @mod-mode additive)
(param resonance       @default 3.2  @min 0.5  @max 4.5 @mod true @mod-mode additive)
(param env_amount      @default 2800 @min 0    @max 8000 @unit Hz @mod true @mod-mode additive)
(param accent          @default 0.25 @min 0    @max 1 @mod true @mod-mode additive)
(param drive           @default 1.6  @min 1    @max 8 @mod true @mod-mode additive)
(param keytrack        @default 0.35 @min 0    @max 1 @mod true @mod-mode additive)
(param slide_time_ms   @default 40   @min 1    @max 250 @unit ms)
(param gain            @default 0.25 @min 0    @max 1)

(def amp_env (adsr gate trigger amp_attack_ms amp_decay_ms amp_sustain amp_release_ms))
(def filt_env (adsr gate trigger filt_attack_ms filt_decay_ms filt_sustain filt_release_ms))

(def base_hz pitch)
(def tracked_cutoff (+ (mod cutoff) (* base_hz (mod keytrack))))
(def accent_gain (+ 1 (* velocity (mod accent))))

(def ph (phasor pitch))
(def saw_osc (saw_from_phase ph))
(def pulse_osc (pulse_from_phase ph (clip (mod pulse_width) 0.05 0.95)))
(def osc (mix saw_osc pulse_osc wave_mix))

(def filt_cutoff (clip (+ tracked_cutoff (* filt_env (mod env_amount) accent_gain)) 40 12000))
(def filtered (biquad osc filt_cutoff (mod resonance) 1 0))
(def driven (tanh (* filtered (mod drive) accent_gain)))
(def voiced (* driven amp_env velocity gain))

(out voiced 1 @name audio)
