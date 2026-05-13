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

(param amp_attack @default 2 @min 1 @max 250 @unit ms)
(param amp_decay @default 190 @min 5 @max 1500 @unit ms)
(param amp_sustain @default 0.08 @min 0 @max 1)
(param amp_release @default 95 @min 5 @max 2000 @unit ms)

(param filt_attack @default 1 @min 1 @max 250 @unit ms)
(param filt_decay @default 165 @min 5 @max 2000 @unit ms)
(param filt_sustain @default 0.0 @min 0 @max 1)
(param filt_release @default 120 @min 5 @max 2500 @unit ms)

(param cutoff @default 520 @min 35 @max 12000 @unit Hz @mod true @mod-mode additive)
(param resonance @default 2.8 @min 0.5 @max 6.0 @mod true @mod-mode additive)
(param filter_env_amount @default 3900 @min -5000 @max 9000 @unit Hz @mod true @mod-mode additive)
(param keytrack @default 0.32 @min 0 @max 2)

(param detune @default 7 @min -30 @max 30 @unit cents)
(param osc_blend @default 0.62 @min 0 @max 1)
(param sub_level @default 0.22 @min 0 @max 1)
(param noise_level @default 0.055 @min 0 @max 0.5)
(param snap @default 0.18 @min 0 @max 1)

(param drive @default 2.4 @min 0.5 @max 8 @mod true @mod-mode additive)
(param gain @default 0.28 @min 0 @max 1)

(def amp_env (adsr gate trigger amp_attack amp_decay amp_sustain amp_release))
(def filt_env (adsr gate trigger filt_attack filt_decay filt_sustain filt_release))
(def snap_env (adsr gate trigger 1 24 0 35))

(def detune_ratio (pow 2 (/ detune 1200)))
(def phase_a (phasor pitch))
(def phase_b (phasor (* pitch detune_ratio)))
(def phase_sub (phasor (* pitch 0.5)))

(def saw_a (- (* phase_a 2) 1))
(def saw_b (- (* phase_b 2) 1))
(def sub_sine (sin (* phase_sub twopi)))
(def airy_noise (noise))

(def dual_saw (+ (* saw_a osc_blend) (* saw_b (- 1 osc_blend))))
(def raw_mix (+ dual_saw (* sub_sine sub_level) (* airy_noise noise_level) (* airy_noise snap snap_env)))

(def tracked_cutoff (+ (mod cutoff) (* filt_env (mod filter_env_amount)) (* pitch keytrack)))
(def safe_cutoff (clip tracked_cutoff 35 14000))
(def safe_resonance (clip (mod resonance) 0.5 6.0))

(def filtered_a (biquad raw_mix safe_cutoff safe_resonance 1 0))
(def filtered_b (biquad filtered_a safe_cutoff safe_resonance 1 0))

(def driven (tanh (* filtered_b (clip (mod drive) 0.5 8))))
(def signal (* driven amp_env velocity gain))

(out signal 1 @name audio)