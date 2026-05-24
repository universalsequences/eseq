; Old School Dubstep Growl Bass
; Dubby UK bass voice with weighty sine sub, anti-aliased mid oscillator,
; wobble-driven lowpass movement, vowel-ish bandpass growl, and warm ladder drive.

(def gate (in 1 @name gate))
(def pitch (in 2 @name pitch))
(def velocity (in 3 @name velocity))
(def trigger (in 4 @name trigger))
(def mod1 (in 5 @name mod1 @modulator 1))
(def mod2 (in 6 @name mod2 @modulator 2))
(def mod3 (in 7 @name mod3 @modulator 3))
(def mod4 (in 8 @name mod4 @modulator 4))

(defmacro semi_ratio (semi)
  (exp (/ (* (log 2) semi) 12)))

(defmacro cent_ratio (cent)
  (exp (/ (* (log 2) cent) 1200)))

(defmacro soft_limit (x amount)
  (/ (tanh (* x amount)) (max amount 0.001)))

(param sub_level @default 0.82 @min 0 @max 1 @mod true @mod-mode additive)
(param mid_level @default 0.72 @min 0 @max 1 @mod true @mod-mode additive)
(param growl_level @default 0.64 @min 0 @max 1 @mod true @mod-mode additive)
(param wave_blend @default 0.38 @min 0 @max 1 @mod true @mod-mode additive)
(param pulse_width @default 0.43 @min 0.06 @max 0.94 @mod true @mod-mode additive)
(param detune_cents @default -9 @min -40 @max 40 @unit cents)
(param octave @default -12 @min -24 @max 12 @unit st)

(param cutoff @default 360 @min 45 @max 4200 @unit Hz @mod true @mod-mode additive)
(param resonance @default 0.50 @min 0 @max 0.95 @mod true @mod-mode additive)
(param filter_env_amt @default 950 @min -1600 @max 4200 @unit Hz)
(param wobble_to_cutoff @default 1150 @min 0 @max 3600 @unit Hz)
(param keytrack @default 0.16 @min 0 @max 1)
(param drive @default 3.1 @min 1 @max 8 @mod true @mod-mode additive)

(param growl_amount @default 0.72 @min 0 @max 1 @mod true @mod-mode additive)
(param formant_base @default 520 @min 120 @max 2400 @unit Hz @mod true @mod-mode additive)
(param formant_spread @default 1.65 @min 1.05 @max 3.8 @mod true @mod-mode additive)
(param formant_q @default 3.1 @min 0.7 @max 8 @mod true @mod-mode additive)
(param wobble_to_growl @default 0.46 @min 0 @max 1)

(param lfo_rate @default 2.0 @min 0.05 @max 12 @unit Hz)
(param lfo_skank @default 0.32 @min 0 @max 1)
(param lfo_shape @default 0.60 @min 0 @max 1)
(param pitch_wobble @default 2.0 @min 0 @max 18 @unit cents)

(param amp_attack @default 4 @min 1 @max 500 @unit ms)
(param amp_decay @default 160 @min 1 @max 2000 @unit ms)
(param amp_sustain @default 0.78 @min 0 @max 1)
(param amp_release @default 230 @min 1 @max 3500 @unit ms)

(param filt_attack @default 8 @min 1 @max 800 @unit ms)
(param filt_decay @default 520 @min 1 @max 3200 @unit ms)
(param filt_sustain @default 0.18 @min 0 @max 1)
(param filt_release @default 300 @min 1 @max 3200 @unit ms)

(param output_gain @default 0.34 @min 0 @max 1 @mod true @mod-mode additive)
(param dirt @default 0.42 @min 0 @max 1 @mod true @mod-mode additive)
(param sub_clean @default 0.62 @min 0 @max 1)

(def amp_env (adsr gate trigger amp_attack amp_decay amp_sustain amp_release))
(def filt_env (adsr gate trigger filt_attack filt_decay filt_sustain filt_release))

(def lfo_phase (phasor lfo_rate))
(def lfo_sine (* 0.5 (+ 1 (sin (* twopi lfo_phase)))))
(def lfo_ramp lfo_phase)
(def lfo_mixed (+ (* lfo_sine (- 1 lfo_shape)) (* lfo_ramp lfo_shape)))
(def lfo_skanked (clip (+ lfo_mixed (* lfo_skank (- (* lfo_mixed lfo_mixed) lfo_mixed))) 0 1))
(def lfo_bipolar (- (* 2 lfo_skanked) 1))

(def pitch_bend (cent_ratio (* lfo_bipolar pitch_wobble)))
(def base_freq (* pitch (semi_ratio octave) pitch_bend))
(def det_freq (* base_freq (cent_ratio detune_cents)))
(def sub_freq (* base_freq 0.5))

(def sub_phase (phasor sub_freq))
(def osc_phase (phasor base_freq))
(def det_phase (phasor det_freq))

(def sub_osc (sin (* twopi sub_phase)))
(def saw_a (polyblep_saw osc_phase base_freq))
(def saw_b (polyblep_saw det_phase det_freq))
(def pulse_a (polyblep_pulse osc_phase (clip (mod pulse_width) 0.06 0.94) base_freq))
(def pulse_b (polyblep_pulse det_phase (clip (- 1 (mod pulse_width)) 0.06 0.94) det_freq))
(def saw_pair (* 0.5 (+ saw_a saw_b)))
(def pulse_pair (* 0.5 (+ pulse_a pulse_b)))
(def blend (clip (mod wave_blend) 0 1))
(def mid_osc (+ (* saw_pair (- 1 blend)) (* pulse_pair blend)))

(def pre_mid (soft_limit (* mid_osc (+ 1 (* 4 (clip (mod dirt) 0 1)))) (+ 1 (* 3 (clip (mod dirt) 0 1)))))
(def source (+ (* sub_osc (clip (mod sub_level) 0 1)) (* pre_mid (clip (mod mid_level) 0 1))))

(def key_cut (* pitch keytrack 2.0))
(def cutoff_motion (+ (clip (mod cutoff) 45 4200) (* filt_env filter_env_amt) (* lfo_skanked wobble_to_cutoff) key_cut))
(def filt_cutoff (clip cutoff_motion 45 6500))
(def laddered (ladder source filt_cutoff (clip (mod resonance) 0 0.95) (clip (mod drive) 1 8)))

(def growl_drive (soft_limit (* laddered (+ 1 (* 7 (clip (mod growl_amount) 0 1)))) (+ 1 (* 2.5 (clip (mod growl_amount) 0 1)))))
(def formant_move (+ (* 0.65 filt_env) (* wobble_to_growl lfo_skanked)))
(def f1 (clip (+ (clip (mod formant_base) 120 2400) (* 680 formant_move)) 90 5000))
(def f2 (clip (* f1 (clip (mod formant_spread) 1.05 3.8)) 180 7800))
(def f3 (clip (* f2 1.72) 300 10000))
(def q (clip (mod formant_q) 0.7 8))
(def vowel_a (svf growl_drive f1 q 1))
(def vowel_b (svf growl_drive f2 q 1))
(def vowel_c (svf growl_drive f3 (* q 0.65) 1))
(def vowel_mix (+ (* 0.68 vowel_a) (* 0.48 vowel_b) (* 0.25 vowel_c)))
(def growl_mix_amt (clip (+ (mod growl_level) (* lfo_skanked (clip (mod growl_amount) 0 1) 0.18)) 0 1))
(def dirty_mix (+ (* laddered (- 1 growl_mix_amt)) (* vowel_mix growl_mix_amt)))

(def sub_filtered (svf sub_osc 95 0.75 0))
(def clean_sub_amt (clip sub_clean 0 1))
(def recombined (+ (* dirty_mix (- 1 clean_sub_amt)) (* (+ dirty_mix (* sub_filtered (clip (mod sub_level) 0 1))) clean_sub_amt)))
(def final_sat (tanh (* recombined (+ 1 (* 2.6 (clip (mod dirt) 0 1))))))
(def final (* final_sat amp_env velocity (clip (mod output_gain) 0 1)))

(out final 1 @name audio)
