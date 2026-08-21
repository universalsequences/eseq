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

(defmacro mtof (p)
  (* 440.0 (exp (* (/ (- p 69.0) 12.0) (log 2)))))

; Parameters
; VCO1
(param vco1_wave @default 0 @min 0 @max 1) ; 0=saw, 1=pulse
(param vco1_pw @default 0.50 @min 0.08 @max 0.92 @mod true @mod-mode additive)
(param vco1_level @default 0.70 @min 0.0 @max 1.0 @mod true @mod-mode additive)

; VCO2
(param vco2_wave @default 0 @min 0 @max 1) ; 0=saw, 1=tri
(param vco2_pitch @default 7.0 @min -24.0 @max 24.0 @unit st @mod true @mod-mode additive)
(param vco2_fine @default 0.0 @min -50.0 @max 50.0 @unit cents)
(param vco2_level @default 0.50 @min 0.0 @max 1.0 @mod true @mod-mode additive)
(param vco_cross_mod @default 0.0 @min 0.0 @max 10.0 @mod true @mod-mode additive) ; FM amount
(param vco2_to_cutoff @default 0.0 @min -2000.0 @max 2000.0 @unit Hz)

; Mix
(param ring_level @default 0.0 @min 0.0 @max 1.0 @mod true @mod-mode additive)
(param noise_level @default 0.0 @min 0.0 @max 1.0 @mod true @mod-mode additive)

; HP Filter
(param hpf_cutoff @default 100.0 @min 20.0 @max 5000.0 @unit Hz @mod true @mod-mode additive)
(param hpf_resonance @default 1.0 @min 0.5 @max 8.0 @mod true @mod-mode additive)

; LP Filter
(param lpf_cutoff @default 2000.0 @min 40.0 @max 18000.0 @unit Hz @mod true @mod-mode additive)
(param lpf_resonance @default 1.0 @min 0.5 @max 8.0 @mod true @mod-mode additive)

; Envelopes & LFO
(param eg1_attack @default 2 @min 1 @max 2000 @unit ms)
(param eg1_decay @default 300 @min 1 @max 4000 @unit ms)
(param eg1_sustain @default 0.0 @min 0.0 @max 1.0)
(param eg1_release @default 300 @min 1 @max 5000 @unit ms)
(param eg1_to_lpf @default 4000.0 @min -8000.0 @max 8000.0 @unit Hz)
(param eg1_to_hpf @default 0.0 @min -4000.0 @max 4000.0 @unit Hz)

(param eg2_attack @default 5 @min 1 @max 2000 @unit ms)
(param eg2_decay @default 800 @min 1 @max 4000 @unit ms)
(param eg2_sustain @default 0.70 @min 0.0 @max 1.0)
(param eg2_release @default 500 @min 1 @max 5000 @unit ms)

(param lfo_rate @default 2.0 @min 0.1 @max 50.0 @unit Hz)
(param lfo_to_lpf @default 0.0 @min -4000.0 @max 4000.0 @unit Hz)

(param gain @default 0.25 @min 0.0 @max 1.0)

; DSP Signals
(def lfo_phase (phasor lfo_rate))
(def lfo_sig (sin (* lfo_phase (* 2.0 3.141592653589793))))

(def eg1_sig (adsr gate trigger eg1_attack eg1_decay eg1_sustain eg1_release))
(def eg2_sig (adsr gate trigger eg2_attack eg2_decay eg2_sustain eg2_release))

; VCO1 Generator
(def f1 (mtof pitch))
(def phase1 (phasor pitch))
(def osc1_saw (polyblep_saw phase1 f1))
(def osc1_pulse (polyblep_pulse phase1 (clip (mod vco1_pw) 0.05 0.95) f1))
(def osc1_raw (+ (* osc1_saw (- 1.0 vco1_wave)) (* osc1_pulse vco1_wave)))

; VCO2 Generator (with pitch modulation / FM from VCO1)
(def pitch2_mod (+ (+ pitch (mod vco2_pitch) (/ vco2_fine 100.0)) (* osc1_raw (mod vco_cross_mod) 2.0)))
(def f2_mod (mtof pitch2_mod))
(def phase2 (phasor pitch2_mod))
(def osc2_saw (polyblep_saw phase2 (clip f2_mod 10.0 20000.0)))
(def osc2_tri (- (* (abs (- (* phase2 2.0) 1.0)) 2.0) 1.0))
(def osc2_raw (+ (* osc2_saw (- 1.0 vco2_wave)) (* osc2_tri vco2_wave)))

; Ring & Noise
(def ring_sig (* osc1_raw osc2_raw))
(def noise_sig (noise))

; Mixer
(def mix_sig (+ (* osc1_raw (mod vco1_level)) (+ (* osc2_raw (mod vco2_level)) (+ (* ring_sig (mod ring_level)) (* noise_sig (mod noise_level))))))

; Chained MS-20 style filters (HPF followed by LPF)
(def hpf_cut_val (+ (mod hpf_cutoff) (* eg1_sig eg1_to_hpf)))
(def hpf_out (svf mix_sig (clip hpf_cut_val 20.0 10000.0) (clip (mod hpf_resonance) 0.5 8.0) 2))

(def lpf_cut_val (+ (mod lpf_cutoff) (+ (* eg1_sig eg1_to_lpf) (+ (* lfo_sig lfo_to_lpf) (* osc2_raw vco2_to_cutoff)))))
(def lpf_out (svf hpf_out (clip lpf_cut_val 40.0 18000.0) (clip (mod lpf_resonance) 0.5 8.0) 0))

; Output gain stage with EG2
(out (* lpf_out eg2_sig velocity gain) 1 @name audio)
