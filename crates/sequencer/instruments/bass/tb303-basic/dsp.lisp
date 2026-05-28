; Acid 303 inspired mono bass synth
; Single anti-aliased saw/pulse oscillator, slide, accent response, snappy filter envelope,
; and driven ladder low-pass for squelchy acid basslines.

(def gate     (in 1  @name gate))
(def pitch    (in 2  @name pitch))
(def velocity (in 3  @name velocity))
(def trigger  (in 4  @name trigger))
(def mod1     (in 5  @name mod1 @modulator 1))
(def mod2     (in 6  @name mod2 @modulator 2))
(def mod3     (in 7  @name mod3 @modulator 3))
(def mod4     (in 8  @name mod4 @modulator 4))

(defmacro semi_ratio (semi)
  (exp (/ (* (log 2) semi) 12)))

; Oscillator / performance
(param tune_semitones @default 0 @min -24 @max 24 @unit st)
(param fine_cents @default 0 @min -50 @max 50 @unit cents)
(param slide_time @default 65 @min 0 @max 600 @unit ms)
(param wave_mix @default 0 @min 0 @max 1 @mod true @mod-mode additive)
(param pulse_width @default 0.50 @min 0.08 @max 0.92 @mod true @mod-mode additive)
(param osc_level @default 0.95 @min 0 @max 1 @mod true @mod-mode additive)

; Amp envelope
(param amp_attack @default 1 @min 1 @max 250 @unit ms)
(param amp_decay @default 45 @min 1 @max 1000 @unit ms)
(param amp_sustain @default 0.90 @min 0 @max 1)
(param amp_release @default 45 @min 1 @max 1500 @unit ms)

; Filter envelope
(param filt_attack @default 1 @min 1 @max 250 @unit ms)
(param filt_decay @default 260 @min 8 @max 2500 @unit ms)
(param filt_sustain @default 0.0 @min 0 @max 1)
(param filt_release @default 80 @min 1 @max 2500 @unit ms)
(param env_mod @default 3100 @min -2500 @max 7000 @unit Hz)
(param accent_to_env @default 0.65 @min 0 @max 2)

; Filter / tone
(param cutoff @default 520 @min 35 @max 9000 @unit Hz @mod true @mod-mode additive)
(param resonance @default 0.58 @min 0 @max 0.96 @mod true @mod-mode additive)
(param keytrack @default 0.18 @min 0 @max 1.5)
(param drive @default 2.3 @min 0.5 @max 7 @mod true @mod-mode additive)
(param accent_to_cutoff @default 1150 @min 0 @max 5000 @unit Hz)
(param accent_to_drive @default 1.05 @min 0 @max 4)
(param post_tone @default 0.58 @min 0 @max 1)
(param output_gain @default 0.30 @min 0 @max 1 @mod true @mod-mode additive)

(def amp_env (adsr gate trigger amp_attack amp_decay amp_sustain amp_release))
(def filt_env (adsr gate trigger filt_attack filt_decay filt_sustain filt_release))

; Portamento / slide smoothing. Larger slide_time gives the classic liquid note smear.
(def glide_alpha (- 1.0 (exp (/ -3.0 (max 0.1 (* slide_time 44.1))))))
(make-history glide_hist)
(def glide_pitch (+ (* glide_alpha pitch)
                    (* (- 1.0 glide_alpha) (read-history glide_hist))))
(write-history glide_hist glide_pitch)

(def tuned_freq (* glide_pitch (semi_ratio (+ tune_semitones (/ fine_cents 100)))))
(def phase (phasor tuned_freq))
(def saw (polyblep_saw phase tuned_freq))
(def pulse (polyblep_pulse phase (clip (mod pulse_width) 0.08 0.92) tuned_freq))
(def wave (clip (mod wave_mix) 0 1))
(def osc (* (clip (mod osc_level) 0 1)
            (+ (* (- 1 wave) saw)
               (* wave pulse))))

; Velocity acts like the 303 accent lane: higher velocity opens the filter and hits the drive harder.
(def accent (clip velocity 0 1))
(def accent_env_scale (+ 1 (* accent accent_to_env)))
(def env_cut (* filt_env env_mod accent_env_scale))
(def key_cut (* glide_pitch keytrack))
(def accent_cut (* accent accent_to_cutoff))
(def safe_cutoff (clip (+ (mod cutoff) env_cut key_cut accent_cut) 35 11000))
(def safe_res (clip (mod resonance) 0 0.96))
(def safe_drive (clip (+ (mod drive) (* accent accent_to_drive)) 0.5 9))

(def pre (tanh (* osc safe_drive)))
(def acid (ladder pre safe_cutoff safe_res safe_drive))
(def bright (svf acid (clip (* safe_cutoff 2.4) 90 14000) 0.7 4))
(def tone_mix (clip post_tone 0 1))
(def toned (+ (* (- 1 tone_mix) acid) (* tone_mix bright)))
(def amp_accent (+ 0.55 (* 0.45 accent)))

(out (* (tanh (* toned 1.7)) amp_env amp_accent (clip (mod output_gain) 0 1)) 1 @name audio)
