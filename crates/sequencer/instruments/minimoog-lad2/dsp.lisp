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

(param osc1_level @default 0.85 @min 0 @max 1)
(param osc2_level @default 0.70 @min 0 @max 1)
(param osc3_level @default 0.55 @min 0 @max 1)
(param noise_level @default 0.04 @min 0 @max 1)

(param osc1_oct @default 0 @min -2 @max 2)
(param osc2_oct @default 0 @min -2 @max 2)
(param osc3_oct @default -1 @min -2 @max 2)
(param osc2_detune @default 7 @min -50 @max 50)
(param osc3_detune @default -5 @min -50 @max 50)

(param osc1_wave @default 0.15 @min 0 @max 1)
(param osc2_wave @default 0.35 @min 0 @max 1)
(param osc3_wave @default 0.70 @min 0 @max 1)
(param pulse_width @default 0.50 @min 0.05 @max 0.95 @mod true @mod-mode additive)

(param cutoff @default 950 @min 40 @max 12000 @unit Hz @mod true @mod-mode additive)
(param resonance @default 0.32 @min 0 @max 0.95 @mod true @mod-mode additive)
(param filter_env_amount @default 2800 @min -6000 @max 9000 @unit Hz @mod true @mod-mode additive)
(param key_track @default 0.30 @min 0 @max 2)
(param drive @default 1.8 @min 0.5 @max 5 @mod true @mod-mode additive)

(param amp_attack @default 5 @min 1 @max 2000 @unit ms)
(param amp_decay @default 230 @min 1 @max 3000 @unit ms)
(param amp_sustain @default 0.72 @min 0 @max 1)
(param amp_release @default 160 @min 1 @max 5000 @unit ms)

(param filt_attack @default 8 @min 1 @max 2000 @unit ms)
(param filt_decay @default 420 @min 1 @max 4000 @unit ms)
(param filt_sustain @default 0.18 @min 0 @max 1)
(param filt_release @default 260 @min 1 @max 5000 @unit ms)

(param gain @default 0.32 @min 0 @max 1)

(def amp_env (adsr gate trigger amp_attack amp_decay amp_sustain amp_release))
(def filt_env (adsr gate trigger filt_attack filt_decay filt_sustain filt_release))

(def osc1_freq (* pitch (pow 2 osc1_oct)))
(def osc2_freq (* pitch (pow 2 osc2_oct) (pow 2 (/ osc2_detune 1200))))
(def osc3_freq (* pitch (pow 2 osc3_oct) (pow 2 (/ osc3_detune 1200))))

(def pw (clip (mod pulse_width) 0.05 0.95))

(def phase1 (phasor osc1_freq))
(def phase2 (phasor osc2_freq))
(def phase3 (phasor osc3_freq))

(def saw1 (polyblep_saw phase1 osc1_freq))
(def saw2 (polyblep_saw phase2 osc2_freq))
(def saw3 (polyblep_saw phase3 osc3_freq))

(def pulse1 (polyblep_pulse phase1 pw osc1_freq))
(def pulse2 (polyblep_pulse phase2 pw osc2_freq))
(def pulse3 (polyblep_pulse phase3 pw osc3_freq))

(def osc1 (+ (* saw1 (- 1 osc1_wave)) (* pulse1 osc1_wave)))
(def osc2 (+ (* saw2 (- 1 osc2_wave)) (* pulse2 osc2_wave)))
(def osc3 (+ (* saw3 (- 1 osc3_wave)) (* pulse3 osc3_wave)))

(def mixed
  (* 0.34
    (+ (* osc1 osc1_level)
       (* osc2 osc2_level)
       (* osc3 osc3_level)
       (* (noise) noise_level))))

(def contour (* filt_env (mod filter_env_amount)))
(def tracked (* pitch key_track))
(def filt_cutoff (clip (+ (mod cutoff) contour tracked) 40 12000))
(def filt_res (clip (mod resonance) 0 0.95))
(def filt_drive (clip (mod drive) 0.5 5))

(def filtered (ladder mixed filt_cutoff filt_res filt_drive))
(def warmed (tanh (* filtered 1.25)))

(out (* warmed amp_env velocity gain) 1 @name audio)