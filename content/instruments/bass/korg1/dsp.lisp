(def gate (in 1 @name gate))
(def pitch (in 2 @name pitch))
(def velocity (in 3 @name velocity))
(def trigger (in 4 @name trigger))
(def mod1 (in 5 @name mod1 @modulator 1))
(def mod2 (in 6 @name mod2 @modulator 2))
(def mod3 (in 7 @name mod3 @modulator 3))
(def mod4 (in 8 @name mod4 @modulator 4))

(param amp_attack @default 2 @min 1 @max 1000 @unit ms)
(param amp_decay @default 135 @min 1 @max 2500 @unit ms)
(param amp_sustain @default 0.70 @min 0 @max 1)
(param amp_release @default 80 @min 1 @max 4000 @unit ms)

(param filt_attack @default 1 @min 1 @max 1000 @unit ms)
(param filt_decay @default 185 @min 1 @max 3000 @unit ms)
(param filt_sustain @default 0.08 @min 0 @max 1)
(param filt_release @default 115 @min 1 @max 4000 @unit ms)

(param vco1_saw @default 0.82 @min 0 @max 1)
(param vco1_pulse @default 0.36 @min 0 @max 1)
(param vco2_level @default 0.58 @min 0 @max 1)
(param vco2_interval @default -12 @min -24 @max 24)
(param vco2_fine @default 4 @min -50 @max 50)
(param sub_level @default 0.32 @min 0 @max 1)
(param noise_level @default 0.025 @min 0 @max 1)

(param pulse_width @default 0.48 @min 0.05 @max 0.95 @mod true @mod-mode additive)
(param pwm_amount @default 0.10 @min 0 @max 0.45)
(param pitch_env_amount @default 0 @min -2400 @max 2400)
(param analog_drift @default 3.5 @min 0 @max 35)
(param ring_level @default 0.08 @min 0 @max 1)

(param cutoff @default 820 @min 30 @max 18000 @unit Hz @mod true @mod-mode additive)
(param resonance @default 0.42 @min 0 @max 1 @mod true @mod-mode additive)
(param hp_cutoff @default 42 @min 20 @max 5000 @unit Hz)
(param hp_resonance @default 0.12 @min 0 @max 1)
(param filter_env_amount @default 3100 @min -9000 @max 9000)
(param keytrack @default 0.28 @min 0 @max 4)
(param scream @default 0.18 @min 0 @max 1)

(param lfo_rate @default 5.7 @min 0.05 @max 40 @unit Hz)
(param lfo_filter_amount @default 90 @min -5000 @max 5000)
(param lfo_pitch @default 0 @min -1200 @max 1200)

(param input_drive @default 2.2 @min 0.5 @max 10)
(param filter_drive @default 2.7 @min 0.5 @max 10)
(param output_bite @default 1.35 @min 0.5 @max 6)
(param gain @default 0.42 @min 0 @max 1)

(def amp_env (adsr gate trigger amp_attack amp_decay amp_sustain amp_release))
(def filt_env (adsr gate trigger filt_attack filt_decay filt_sustain filt_release))

(def lfo_phase (phasor lfo_rate))
(def lfo (sin (* lfo_phase twopi)))

(def drift_a (sin (* (phasor 0.137) twopi)))
(def drift_b (sin (* (phasor 0.211) twopi)))
(def drift_c (sin (* (phasor 0.073) twopi)))

(def pitch_snap (* filt_env pitch_env_amount))
(def vib (* lfo lfo_pitch))
(def cents1 (+ pitch_snap vib (* drift_a analog_drift)))
(def cents2 (+ pitch_snap vib vco2_fine (* drift_b analog_drift -1.35)))
(def cents_sub (* drift_c analog_drift 0.35))

(def freq1 (* pitch (pow 2 (/ cents1 1200))))
(def freq2 (* pitch (pow 2 (/ (+ (* vco2_interval 100) cents2) 1200))))
(def freq_sub (* pitch 0.5 (pow 2 (/ cents_sub 1200))))

(def phase1 (phasor freq1))
(def phase2 (phasor freq2))
(def phase_sub (phasor freq_sub))

(def pw (clip (+ (mod pulse_width) (* lfo pwm_amount) (* drift_c 0.012)) 0.05 0.95))

(def saw1 (polyblep_saw phase1 freq1))
(def pulse1 (polyblep_pulse phase1 pw freq1))
(def saw2 (polyblep_saw phase2 freq2))
(def pulse2 (polyblep_pulse phase2 (- 1 pw) freq2))
(def sub (polyblep_pulse phase_sub 0.5 freq_sub))
(def hiss (noise))

(def vco1 (+ (* vco1_saw saw1) (* vco1_pulse pulse1)))
(def vco2 (* vco2_level (+ (* 0.62 saw2) (* 0.38 pulse2))))
(def ring (* ring_level pulse1 pulse2 1.6))
(def raw_mix (+ vco1 vco2 (* sub_level sub) (* noise_level hiss) ring))
(def pre_drive (tanh (* raw_mix input_drive 0.62)))

(def hp_q (+ 0.55 (* hp_resonance 5.0)))
(def hp_stage (svf pre_drive (clip hp_cutoff 20 5000) hp_q 2))
(def hp_sat (tanh (* hp_stage filter_drive 0.72)))

(def lp_cut (clip (+ (mod cutoff) (* filter_env_amount filt_env velocity) (* lfo lfo_filter_amount) (* keytrack pitch)) 30 18000))
(def lp_q (+ 0.62 (* (clip (mod resonance) 0 1) 5.2)))
(def lp_stage (svf hp_sat lp_cut lp_q 0))
(def bp_stage (svf hp_sat lp_cut lp_q 1))

(def ms_body (+ lp_stage (* scream bp_stage)))
(def post (tanh (* ms_body output_bite)))
(def signal (* post amp_env velocity gain))

(out signal 1 @name audio)