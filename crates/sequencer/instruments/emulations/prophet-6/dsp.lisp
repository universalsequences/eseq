; Prophet-6 inspired analog polysynth voice
; Two VCO-style oscillators plus sub/noise, dual envelopes, simple LFO,
; resonant lowpass shaping, and modulation-ready params.

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
(def ext1 (in 11 @name ext1 @modulator 7))
(def ext2 (in 12 @name ext2 @modulator 8))
(def ext3 (in 13 @name ext3 @modulator 9))
(def ext4 (in 14 @name ext4 @modulator 10))

(defmacro pulse_from_phase (phase width)
  (scale (lt phase width) 0 1 -1 1))

(defmacro saw_from_phase (phase)
  (scale phase 0 1 -1 1))

(defmacro semi_ratio (semi)
  (exp (/ (* (log 2) semi) 12)))

(param amp_attack_ms   @default 8    @min 1    @max 4000 @unit ms)
(param amp_decay_ms    @default 180  @min 1    @max 4000 @unit ms)
(param amp_sustain     @default 0.75 @min 0    @max 1)
(param amp_release_ms  @default 420  @min 1    @max 8000 @unit ms)

(param filt_attack_ms  @default 4    @min 1    @max 4000 @unit ms)
(param filt_decay_ms   @default 260  @min 1    @max 4000 @unit ms)
(param filt_sustain    @default 0.2  @min 0    @max 1)
(param filt_release_ms @default 500  @min 1    @max 8000 @unit ms)

(param osc1_shape      @default 0.0  @min 0    @max 1    @mod true @mod-mode additive)
(param osc1_pw         @default 0.5  @min 0.05 @max 0.95 @mod true @mod-mode additive)
(param osc1_mix        @default 0.75 @min 0    @max 1    @mod true @mod-mode additive)

(param osc2_shape      @default 0.35 @min 0    @max 1    @mod true @mod-mode additive)
(param osc2_pw         @default 0.5  @min 0.05 @max 0.95 @mod true @mod-mode additive)
(param osc2_mix        @default 0.7  @min 0    @max 1    @mod true @mod-mode additive)
(param osc2_detune     @default 0.08 @min -0.5 @max 0.5  @unit st @mod true @mod-mode additive)
(param osc2_fine       @default 0.0  @min -0.1 @max 0.1  @unit st @mod true @mod-mode additive)

(param sub_mix         @default 0.18 @min 0    @max 1    @mod true @mod-mode additive)
(param noise_mix       @default 0.02 @min 0    @max 1    @mod true @mod-mode additive)

(param cutoff          @default 2200 @min 40   @max 14000 @unit Hz @mod true @mod-mode additive)
(param resonance       @default 1.1  @min 0.5  @max 4.5  @mod true @mod-mode additive)
(param filter_env_amt  @default 2600 @min -8000 @max 8000 @unit Hz @mod true @mod-mode additive)
(param keytrack        @default 0.35 @min 0    @max 1    @mod true @mod-mode additive)

(param lfo_rate        @default 4.5  @min 0.05 @max 20   @unit Hz)
(param lfo_pitch_amt   @default 0.0  @min 0    @max 0.08 @mod true @mod-mode additive)
(param lfo_shape_amt   @default 0.0  @min 0    @max 0.8  @mod true @mod-mode additive)
(param vibrato_amt     @default 0.0  @min 0    @max 0.15 @mod true @mod-mode additive)

(param drift           @default 0.01 @min 0    @max 0.08)
(param drive           @default 1.4  @min 1    @max 8    @mod true @mod-mode additive)
(param gain            @default 0.18 @min 0    @max 1)

(def amp_env (adsr gate trigger amp_attack_ms amp_decay_ms amp_sustain amp_release_ms))
(def filt_env (adsr gate trigger filt_attack_ms filt_decay_ms filt_sustain filt_release_ms))
(def lfo (triangle (phasor lfo_rate)))

(def key_follow_hz (* pitch (clip (mod keytrack) 0 1)))
(def pitch_mod_semi (+ (* lfo (mod lfo_pitch_amt)) (* mod1 (mod vibrato_amt))))
(def pitch_ratio (semi_ratio pitch_mod_semi))
(def drift1 (+ 1 (* drift 0.17)))
(def drift2 (+ 1 (* drift -0.13)))

(def osc1_pitch (* pitch pitch_ratio drift1))
(def osc2_ratio (semi_ratio (+ (mod osc2_detune) (mod osc2_fine))))
(def osc2_pitch (* pitch pitch_ratio osc2_ratio drift2))

(def ph1 (phasor osc1_pitch))
(def ph2 (phasor osc2_pitch))
(def phsub (phasor (* pitch 0.5)))

(def shape1 (clip (+ (mod osc1_shape) (* lfo (mod lfo_shape_amt)) (* mod2 0.5)) 0 1))
(def shape2 (clip (+ (mod osc2_shape) (* lfo (mod lfo_shape_amt)) (* mod2 0.5)) 0 1))

(def saw1 (polyblep_saw ph1 osc1_pitch))
(def saw2 (polyblep_saw ph2 osc2_pitch))
(def pulse1 (polyblep_pulse ph1 (clip (mod osc1_pw) 0.05 0.95) osc1_pitch))
(def pulse2 (polyblep_pulse ph2 (clip (mod osc2_pw) 0.05 0.95) osc2_pitch))
(def sub (polyblep_saw phsub (* pitch 0.5)))

(def osc1 (+ (* saw1 (- 1 shape1)) (* pulse1 shape1)))
(def osc2 (+ (* saw2 (- 1 shape2)) (* pulse2 shape2)))

(def raw_mix (+ (* osc1 (clip (mod osc1_mix) 0 1))
                (* osc2 (clip (mod osc2_mix) 0 1))
                (* sub (clip (mod sub_mix) 0 1))
                (* (noise) (clip (mod noise_mix) 0 1))))

(def filter_cutoff (clip (+ (mod cutoff)
                            key_follow_hz
                            (* filt_env (mod filter_env_amt))
                            (* mod3 4000))
                         40 16000))
(def filt (biquad raw_mix filter_cutoff (clip (mod resonance) 0.5 4.5) 1 0))
(def driven (tanh (* filt (mod drive))))
(def voiced (* driven amp_env velocity gain))

(out voiced 1 @name audio)
