; Physical model of a saxophone / woodwind instrument.
; Uses a conical/cylindrical waveguide, non-linear reed table,
; physical body resonator, key click noise, growl, and throat vibrato.

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

(param amp_attack @default 35 @min 1 @max 500 @unit ms)
(param amp_decay @default 150 @min 5 @max 1500 @unit ms)
(param amp_sustain @default 0.85 @min 0 @max 1.0)
(param amp_release @default 120 @min 10 @max 2000 @unit ms)

(param pressure_ctrl @default 1.25 @min 0.2 @max 2.5 @mod true @mod-mode additive)
(param reed_stiffness @default 0.95 @min 0.2 @max 3.0 @mod true @mod-mode additive)
(param impedance @default 0.85 @min 0.1 @max 2.0)
(param reflection_cutoff @default 2800 @min 200 @max 12000 @unit Hz @mod true @mod-mode additive)
(param bore_type @default 0.85 @min 0.0 @max 1.0 @mod true @mod-mode additive)

(param growl_amt @default 0.12 @min 0.0 @max 0.8 @mod true @mod-mode additive)
(param growl_rate @default 68 @min 25 @max 160 @unit Hz)
(param vib_depth @default 0.08 @min 0.0 @max 0.8 @mod true @mod-mode additive)
(param vib_rate @default 5.5 @min 1.5 @max 10.0 @unit Hz)
(param glide @default 35 @min 0 @max 500 @unit ms)

(param breath_noise @default 0.15 @min 0.0 @max 0.6)
(param key_click @default 0.20 @min 0.0 @max 1.0)

(param body_cutoff @default 1450 @min 200 @max 8000 @unit Hz @mod true @mod-mode additive)
(param body_q @default 1.5 @min 0.5 @max 5.0)
(param saturation @default 1.8 @min 1.0 @max 8.0 @mod true @mod-mode additive)
(param gain @default 0.65 @min 0.0 @max 1.0 @mod true @mod-mode additive)

; Pitch slide / glide lag
(make-history pitch_lag)
(def glide_val (clip glide 0 500))
(def glide_coef (/ 1.0 (+ 1.0 (* glide_val 0.1))))
(def prev_pitch (read-history pitch_lag))
(def current_pitch (+ prev_pitch (* glide_coef (- pitch prev_pitch))))
(write-history pitch_lag current_pitch)

; Pitch modulation (throat vibrato)
(def v_depth (clip (mod vib_depth) 0 1))
(def vib_osc (sin (* (phasor vib_rate) twopi)))
(def pitch_mod (pow 2.0 (/ (* vib_osc v_depth) 12.0)))
(def pitch_freq (clip (* current_pitch pitch_mod) 20 20000))

; Delay calculation (compensation for cylinder/cone shift)
(def b_type (clip (mod bore_type) 0 1))
(def delay_scale (+ 0.5 (* b_type 0.5)))
(def delay_samps (clip (* (/ 44100.0 pitch_freq) delay_scale) 2 2000))

; Breath & Blow excitation
(def env (adsr gate trigger amp_attack amp_decay amp_sustain amp_release))
(def vel (clip velocity 0 1))
(def p_ctrl (clip (mod pressure_ctrl) 0.1 2.5))
(def base_pressure (* env vel p_ctrl))

; Throat Growl
(def g_amt (clip (mod growl_amt) 0 1))
(def growl_osc (sin (* (phasor growl_rate) twopi)))
(def growl_factor (+ 1.0 (* growl_osc g_amt 0.35)))

; Breath noise
(def b_noise (* (noise) breath_noise 0.15))
(def pressure (* base_pressure growl_factor (+ 1.0 b_noise)))

; Physical Reed-Bore Feedback Loop
(make-history bore_fb)
(def reflected (delay (read-history bore_fb) delay_samps))

; Reflection Low-Pass Filter (acoustic attenuation)
(def ref_cut (clip (mod reflection_cutoff) 100 12000))
(def reflected_lp (svf reflected ref_cut 0.5 0))

; Acoustic reflection coefficient (conical positive, cylindrical negative)
(def reflection_coeff (- (* b_type 2.0) 1.0))
(def reflected_wave (* reflected_lp reflection_coeff))

; Pressure differential across the reed
(def delta_p (- pressure reflected_wave))

; Nonlinear Reed Flow Table
(def stiffness (clip (mod reed_stiffness) 0.1 3.0))
(def clip_dp (clip delta_p 0.0 1.0))
(def reed_factor (- 1.0 clip_dp))
(def flow (* delta_p reed_factor reed_factor stiffness))

; Bore pressure wave entry
(def imp (clip impedance 0.05 2.0))
(def bore_input (+ reflected_wave (* flow imp)))
(write-history bore_fb bore_input)

; Sound output formulation (reed bleed-through + acoustic column)
(def air_bleed (* flow 0.25))
(def sound_source (+ reflected_lp air_bleed))

; Brass body resonator and formant shaping
(def b_cutoff (clip (mod body_cutoff) 100 8000))
(def body_lp (svf sound_source b_cutoff (clip body_q 0.5 5.0) 0))
(def body_bp (svf sound_source b_cutoff (clip body_q 0.5 5.0) 1))
(def filtered_sound (+ (* body_lp 0.75) (* body_bp 0.45)))

; Warm brass drive saturation
(def sat (clip (mod saturation) 1.0 8.0))
(def saturated (tanh (* filtered_sound sat)))

; Key click transients
(def click_env (adsr gate trigger 1 12 0 1))
(def click_sig (* (noise) click_env key_click 0.12))

; Final assembly & Master Gain
(def final_out (+ saturated click_sig))
(def out_gain (clip (mod gain) 0 1))
(out (* final_out out_gain 0.42) 1 @name audio)
