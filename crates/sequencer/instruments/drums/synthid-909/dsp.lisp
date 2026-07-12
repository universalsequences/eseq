; SynthID 909 - accepted Rung 3 TR-909 identification patch.
;
; Direct DGenLisp port of the passing SynthID v4 voice. The defaults come from
; output/rung3_909_v4/recovered_params.json (80.10% independent MR-STFT
; improvement). MIDI pitch supplies the endpoint of the exponential sweep;
; start_ratio preserves the identified 276.55519 / 46.922146 relationship.
; The 40 fixed harmonic coefficients are the backward-pruned learned timbre,
; expressed as Fourier/envelope structure rather than samples or lookup tables.

(def gate (in 1 @name gate))
(def pitch (/ (in 2 @name pitch) 8))
(def velocity (in 3 @name velocity))
(def trigger (in 4 @name trigger))
(def mod1 (in 5 @name mod1 @modulator 1))
(def mod2 (in 6 @name mod2 @modulator 2))
(def mod3 (in 7 @name mod3 @modulator 3))
(def mod4 (in 8 @name mod4 @modulator 4))

(defmacro onepole-param (input time_ms)
  (make-history value_h)
  (make-history initialized_h)
  (def previous (read-history value_h))
  (def initialized (read-history initialized_h))
  (def safe_seconds (* (max time_ms 0.001) 0.001))
  (def coefficient (exp (/ -1.0 (* samplerate safe_seconds))))
  (def filtered (+ (* (- 1.0 coefficient) input) (* coefficient previous)))
  (def initialized_value (gswitch (lt initialized 0.5) input filtered))
  (def output (gswitch (lt time_ms 0.001) input initialized_value))
  (write-history value_h output)
  (write-history initialized_h 1.0)
  output)

(param smoothing @default 5 @min 0 @max 100 @unit ms)
(param start_ratio @default 5.893916 @min 1 @max 10)
(param pitch_decay @default -52.411747 @min -80 @max -20)

(param body_amp @default 0.8939833 @min 0.05 @max 1.0)
; T60 representation of the learned ampDecay=-11.0617325/s.
(param release @default 624.4732 @min 100 @max 4000 @unit ms)
(param amp_curve @default -3.3242447 @min -60 @max 0)
(param body_asymmetry @default 0.035932463 @min -0.5 @max 0.5)
(param body_harmonic @default -0.3666658 @min -1 @max 1)

(param click_freq @default 637.39606 @min 200 @max 1000 @unit Hz)
(param click_amp @default 1.2 @min 0 @max 1.2)
(param click_decay @default -453.86426 @min -800 @max -150)

(param noise_cutoff @default 16080.313 @min 1000 @max 18000 @unit Hz)
(param noise_amp @default 0.0006846405 @min 0 @max 0.01)
(param noise_decay @default -9.952022 @min -150 @max -5)

(param drive @default 2.003162 @min 1 @max 6)
(param out_gain @default 0.31624377 @min 0.1 @max 1)
; Optional T60-style post-voice attack. Zero preserves the learned transient.
(param fade_in @default 0 @min 0 @max 100 @unit ms)
(param retrigger_fade @default 5 @min 0.1 @max 50 @unit ms)

(def start_ratio_s (onepole-param start_ratio smoothing))
(def pitch_decay_s (onepole-param pitch_decay smoothing))
(def body_amp_s (onepole-param body_amp smoothing))
(def release_s (onepole-param release smoothing))
(def amp_curve_s (onepole-param amp_curve smoothing))
(def body_asymmetry_s (onepole-param body_asymmetry smoothing))
(def body_harmonic_s (onepole-param body_harmonic smoothing))
(def click_freq_s (onepole-param click_freq smoothing))
(def click_amp_s (onepole-param click_amp smoothing))
(def click_decay_s (onepole-param click_decay smoothing))
(def noise_cutoff_s (onepole-param noise_cutoff smoothing))
(def noise_amp_s (onepole-param noise_amp smoothing))
(def noise_decay_s (onepole-param noise_decay smoothing))
(def drive_s (onepole-param drive smoothing))
(def out_gain_s (onepole-param out_gain smoothing))
(def fade_in_s (onepole-param fade_in smoothing))
(def retrigger_fade_s (onepole-param retrigger_fade smoothing))

; A correction is a scalar Fourier partial following the body sweep and body
; envelope, with one of the four pruned decay rates: 0, 15, 60, or 240 /s.
(defmacro harmonic-sin (coefficient harmonic decay sweep_phase body_envelope t)
  (* coefficient
     (sin (* sweep_phase harmonic twopi))
     body_envelope
     body_amp_s
     (exp (* -1.0 decay t))))

(defmacro harmonic-cos (coefficient harmonic decay sweep_phase body_envelope t)
  (* coefficient
     (cos (* sweep_phase harmonic twopi))
     body_envelope
     body_amp_s
     (exp (* -1.0 decay t))))

(defmacro synthid909-voice (voice_trigger input_pitch input_velocity)
  (make-history time_h)
  (def previous_time (read-history time_h))
  (def t (gswitch (gt voice_trigger 0.5) 0.0 previous_time))
  (write-history time_h (+ t (/ 1.0 samplerate)))

  (make-history active_h)
  (def active (gswitch (gt voice_trigger 0.5) 1.0 (read-history active_h)))
  (write-history active_h active)

  (make-history velocity_h)
  (def previous_velocity (read-history velocity_h))
  (def hit_velocity
    (gswitch (gt voice_trigger 0.5)
             (clip input_velocity 0.0 1.0)
             previous_velocity))
  (write-history velocity_h hit_velocity)

  (make-history pitch_h)
  (def previous_pitch (read-history pitch_h))
  (def hit_pitch
    (gswitch (gt voice_trigger 0.5)
             (max input_pitch 1.0)
             previous_pitch))
  (write-history pitch_h hit_pitch)

  (def body_end hit_pitch)
  (def body_start (* body_end start_ratio_s))
  (def sweep_phase
    (+ (* body_end t)
       (* (/ (- body_start body_end) pitch_decay_s)
          (- (exp (* pitch_decay_s t)) 1.0))))

  (def amp_decay (/ -6.9077553 (* (max release_s 1.0) 0.001)))
  (def body_envelope
    (exp (+ (* amp_decay t) (* amp_curve_s t t))))
  (def body
    (* (sin (* sweep_phase twopi)) body_envelope body_amp_s))

  (def even_harmonic
    (* body_asymmetry_s
       (sin (- (* sweep_phase 2.0 twopi) 0.62))
       body_envelope body_amp_s (exp (* -17.0 t))))

  (def odd_harmonics
    (* body_harmonic_s
       (+ (* (sin (* sweep_phase 3.0 twopi)) (/ 1.0 9.0))
          (* (sin (* sweep_phase 5.0 twopi)) (/ 1.0 25.0)))
       body_envelope body_amp_s))

  (def harmonic_correction
    (+
      (harmonic-sin -0.0015310477 10 0 sweep_phase body_envelope t)
      (harmonic-sin 0.0010460267 12 0 sweep_phase body_envelope t)
      (harmonic-cos -0.0012142119 16 0 sweep_phase body_envelope t)
      (harmonic-cos -0.01941547 2 0 sweep_phase body_envelope t)
      (harmonic-sin 0.023703808 2 0 sweep_phase body_envelope t)
      (harmonic-cos -0.016709592 3 0 sweep_phase body_envelope t)
      (harmonic-sin 0.08386365 3 0 sweep_phase body_envelope t)
      (harmonic-cos -0.0051835985 4 0 sweep_phase body_envelope t)
      (harmonic-cos 0.0051175904 5 0 sweep_phase body_envelope t)
      (harmonic-sin 0.009158693 5 0 sweep_phase body_envelope t)
      (harmonic-sin 0.0030010177 6 0 sweep_phase body_envelope t)
      (harmonic-cos 0.007857386 7 0 sweep_phase body_envelope t)
      (harmonic-cos 0.00272979 8 0 sweep_phase body_envelope t)
      (harmonic-cos -0.0034662898 9 0 sweep_phase body_envelope t)
      (harmonic-sin -0.0020758086 9 0 sweep_phase body_envelope t)
      (harmonic-sin -0.002200256 10 15 sweep_phase body_envelope t)
      (harmonic-sin -0.00060076453 14 15 sweep_phase body_envelope t)
      (harmonic-sin 0.0022033039 16 15 sweep_phase body_envelope t)
      (harmonic-cos 0.05482698 2 15 sweep_phase body_envelope t)
      (harmonic-sin -0.0023214894 2 15 sweep_phase body_envelope t)
      (harmonic-cos -0.010733163 3 15 sweep_phase body_envelope t)
      (harmonic-sin -0.22469933 3 15 sweep_phase body_envelope t)
      (harmonic-cos 0.020118007 5 15 sweep_phase body_envelope t)
      (harmonic-sin -0.038221404 5 15 sweep_phase body_envelope t)
      (harmonic-cos -0.0067852763 6 15 sweep_phase body_envelope t)
      (harmonic-sin -0.003176011 9 15 sweep_phase body_envelope t)
      (harmonic-sin -0.021203622 12 240 sweep_phase body_envelope t)
      (harmonic-cos -0.021953242 15 240 sweep_phase body_envelope t)
      (harmonic-cos -0.18868186 3 240 sweep_phase body_envelope t)
      (harmonic-sin 0.32736433 3 240 sweep_phase body_envelope t)
      (harmonic-sin 0.21922709 4 240 sweep_phase body_envelope t)
      (harmonic-sin 0.006498365 10 60 sweep_phase body_envelope t)
      (harmonic-sin -0.0012741693 12 60 sweep_phase body_envelope t)
      (harmonic-cos -0.007484256 14 60 sweep_phase body_envelope t)
      (harmonic-cos 0.008631823 16 60 sweep_phase body_envelope t)
      (harmonic-cos 0.016201887 2 60 sweep_phase body_envelope t)
      (harmonic-sin -0.115740165 2 60 sweep_phase body_envelope t)
      (harmonic-cos 0.0018314002 3 60 sweep_phase body_envelope t)
      (harmonic-sin 0.05478942 5 60 sweep_phase body_envelope t)
      (harmonic-sin -0.022988245 6 60 sweep_phase body_envelope t)))

  (def click
    (* (sin (* click_freq_s t twopi))
       (exp (* click_decay_s t)) click_amp_s))

  (def bipolar_noise (- (* (noise) 2.0) 1.0))
  (def filtered_noise (biquad bipolar_noise noise_cutoff_s 0.707 1.0 0.0))
  (def noise_burst
    (* filtered_noise (exp (* noise_decay_s t)) noise_amp_s))

  (def mixed
    (+ body even_harmonic odd_harmonics harmonic_correction click noise_burst))
  (def bias 0.05)
  (def shifted (+ (* mixed drive_s) bias))
  (def softsign (- (/ shifted (+ 1.0 (abs shifted)))
                   (/ bias (+ 1.0 (abs bias)))))
  (def learned_voice (* softsign out_gain_s))

  (def attack_seconds (* (max fade_in_s 0.001) 0.001))
  (def attack_envelope
    (gswitch (lt fade_in_s 0.001)
             1.0
             (- 1.0 (exp (/ (* -6.9077553 t) attack_seconds)))))
  (* learned_voice attack_envelope hit_velocity active))

(defmacro retrigger-gain (target first_hit time_ms)
  (make-history gain_h)
  (def previous_gain (read-history gain_h))
  (def safe_seconds (* (max time_ms 0.001) 0.001))
  (def coefficient (exp (/ -6.9077553 (* samplerate safe_seconds))))
  (def filtered_gain (+ target (* coefficient (- previous_gain target))))
  (def next_gain
    (gswitch (gt first_hit 0.5)
             target
             (gswitch (lt time_ms 0.001) target filtered_gain)))
  (write-history gain_h next_gain)
  (gswitch (gt first_hit 0.5) target previous_gain))

(def trigger_gate (gt trigger 0.5))
(make-history trigger_h)
(def previous_trigger (read-history trigger_h))
(def triggered (max 0.0 (- trigger_gate previous_trigger)))
(write-history trigger_h trigger_gate)

(make-history selector_h)
(def previous_selector (read-history selector_h))
(def selector
  (gswitch (gt triggered 0.5) (- 1.0 previous_selector) previous_selector))
(write-history selector_h selector)

(make-history ever_triggered_h)
(def was_triggered (read-history ever_triggered_h))
(def first_hit (* triggered (lt was_triggered 0.5)))
(write-history ever_triggered_h (gswitch (gt triggered 0.5) 1.0 was_triggered))

(def trigger_a (* triggered (lt selector 0.5)))
(def trigger_b (* triggered (gte selector 0.5)))
(def voice_a (synthid909-voice trigger_a pitch velocity))
(def voice_b (synthid909-voice trigger_b pitch velocity))

(def target_a (gswitch (lt selector 0.5) 1.0 0.0))
(def target_b (- 1.0 target_a))
(def gain_a (retrigger-gain target_a first_hit retrigger_fade_s))
(def gain_b (retrigger-gain target_b first_hit retrigger_fade_s))

(out (+ (* voice_a gain_a) (* voice_b gain_b)) 1 @name audio)
