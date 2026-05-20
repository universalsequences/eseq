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

; --- Parameters ---
(param amp_attack @default 60 @min 1 @max 1000 @unit ms)
(param amp_decay @default 250 @min 1 @max 2000 @unit ms)
(param amp_sustain @default 0.85 @min 0 @max 1)
(param amp_release @default 180 @min 1 @max 3000 @unit ms)

(param pressure @default 0.9 @min 0.1 @max 2.0 @mod true @mod-mode additive)
(param noise_amt @default 0.16 @min 0.0 @max 0.8 @mod true @mod-mode additive)
(param noise_color @default 2500 @min 500 @max 8000 @unit Hz)
(param chiff_amt @default 0.35 @min 0.0 @max 1.5)
(param chiff_decay @default 90 @min 10 @max 400 @unit ms)

(param resonance @default 35.0 @min 5.0 @max 180.0 @mod true @mod-mode additive)
(param drive @default 1.5 @min 0.5 @max 4.0)
(param overblow @default 0.0 @min 0.0 @max 1.0 @mod true @mod-mode additive)
(param air_bleed @default 0.08 @min 0.0 @max 0.5)

(param vibRate @default 5.5 @min 1.0 @max 12.0 @unit Hz)
(param vibDepth @default 0.15 @min 0.0 @max 1.0 @unit semitones)

(param mode1_gain @default 0.80 @min 0.0 @max 1.5)
(param mode2_gain @default 0.50 @min 0.0 @max 1.5)
(param mode3_gain @default 0.30 @min 0.0 @max 1.5)
(param mode4_gain @default 0.15 @min 0.0 @max 1.5)

(param brightness @default 6500 @min 800 @max 16000 @unit Hz @mod true @mod-mode additive)
(param gain @default 0.35 @min 0.0 @max 1.0)

; --- DSP Logic ---

; Envelopes
(def env (adsr gate trigger amp_attack amp_decay amp_sustain amp_release))
(def chiff_env (adsr gate trigger 2.0 chiff_decay 0.0 10.0))

; Vibrato
(def vib_phase (phasor vibRate))
(def vibrato (* (sin (* vib_phase twopi)) vibDepth))
(def mod_pitch (+ pitch vibrato))
(def freq (* 440.0 (pow 2.0 (/ (- mod_pitch 69.0) 12.0))))

; Noise Generation (Breath Turbulences)
(def breath_noise (noise))
(def filtered_noise (svf breath_noise noise_color 0.5 0))

; Low Frequency Jitter (Human Breath fluctuation)
(def jitter_phase (phasor 3.5))
(def jitter (sin (* jitter_phase twopi)))

; Combine components into breath amp
(def breath_amp (+ (* env (mod pressure)) (* chiff_env chiff_amt)))
(def modulated_breath (* breath_amp (+ 1.0 (+ (* jitter 0.08) (* filtered_noise (mod noise_amt))))))

; Non-linear Jet (Embouchure displacement)
(def jet_sig (* modulated_breath 1.2))
(def non_lin (tanh (* jet_sig drive)))

; Overblow filter routing
(def excitation_hp (svf non_lin (* freq (+ 1.0 (* (mod overblow) 1.5))) 0.7 2))
(def mixed_excitation (+ (* non_lin (- 1.0 (mod overblow))) (* excitation_hp (mod overblow))))

; Bore Modal Resonator Bank
; Four tuned harmonics corresponding to tube acoustic modes
(def m1 (svf mixed_excitation freq (clip (mod resonance) 2.0 180.0) 1))
(def m2 (svf mixed_excitation (min (* freq 2.0) 20000.0) (clip (* (mod resonance) 0.8) 2.0 150.0) 1))
(def m3 (svf mixed_excitation (min (* freq 3.0) 20000.0) (clip (* (mod resonance) 0.6) 2.0 120.0) 1))
(def m4 (svf mixed_excitation (min (* freq 4.0) 20000.0) (clip (* (mod resonance) 0.4) 2.0 100.0) 1))

; Harmonic mixing
(def poly_modes (+ (* m1 mode1_gain) (* m2 mode2_gain) (* m3 mode3_gain) (* m4 mode4_gain)))

; Mix directly coupled air leak
(def output_mix (+ poly_modes (* filtered_noise air_bleed)))

; Passive body/wood peak resonator around 220Hz
(def body_resonance (svf output_mix 220.0 1.5 4))

; Global brightness lowpass filter
(def final_filtered (svf body_resonance (clip (mod brightness) 400.0 18000.0) 0.5 0))

; Final amp scaling
(out (* final_filtered env velocity gain) 1 @name audio)
