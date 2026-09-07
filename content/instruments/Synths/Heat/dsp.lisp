; Heat: dual-lane subtractive synthesis with shared host expression.
; Physical-unit controls and original DSP; the current sound is user-approved.
; Each unison copy owns its complete oscillators, filters, envelopes and LFOs.
; Named host signals distinguish a physical note-on from an envelope trigger.
(use-defmacro heat-envelope)
(use-defmacro heat-pitch-envelope)
(use-defmacro heat-lfo)
(use-defmacro heat-glide)
(use-defmacro heat-sync)
(use-defmacro heat-unison-onset)
(use-defmacro heat-linear-filter)
(use-defmacro heat-drive)

(def gate (in 1 @name gate))
(def pitch (in 2 @name pitch))
(def velocity (in 3 @name velocity))
(def trigger (in 4 @name trigger))
(def note_on (in 5 @name note_on))
(def legato (in 6 @name legato))
(def pressure (in 7 @name pressure))
(def pitch_bend (in 12 @name pitch_bend))
(def mod_wheel (in 13 @name mod_wheel))
(def mod1 (in 8 @name mod1 @modulator 1))
(def mod2 (in 9 @name mod2 @modulator 2))
(def mod3 (in 10 @name mod3 @modulator 3))
(def mod4 (in 11 @name mod4 @modulator 4))

(defmacro heat-db (db) (exp (* 0.11512925465 db)))
(defmacro heat-octaves (octaves) (exp (* 0.69314718056 octaves)))

; First-order target slew, initialized directly to the first target.
; Smooth continuous level/pan controls without smearing note/event signals.
(defmacro heat-control (target)
  (make-history ready_hist)
  (make-history value_hist)
  (def old (read-history value_hist))
  (def coefficient (- 1 (exp (/ -1 (* 0.002 samplerate)))))
  (def value (gswitch (eq (read-history ready_hist) 0) target
    (+ old (* coefficient (- target old)))))
  (write-history ready_hist 1)
  (write-history value_hist value)
  value)

; Finite 2 ms enable ramp. Continuous levels retain their exponential slew.
; A disabled section finishes its fade before its DSP state is frozen.
(defmacro heat-enable (target)
  (make-history ready)
  (make-history level)
  (def old (read-history level))
  (def step (/ 1 (* 0.002 samplerate)))
  (def value (gswitch (eq (read-history ready) 0) target
    (+ old (clip (- target old) (- 0 step) step))))
  (write-history ready 1)
  (write-history level value)
  value)

; Execution window for a section that owns released state. It rises to exact
; one immediately and falls to exact zero only after hold_ms, so a section can
; be kept executing after its audible fade has finished and then be frozen at
; a known-idle state rather than mid-release.
(defmacro heat-run (target hold_ms)
  (make-history ready)
  (make-history level)
  (def old (read-history level))
  (def step (/ 1000 (* (max 1 hold_ms) samplerate)))
  (def value (gswitch (eq (read-history ready) 0) target
    (gswitch (gt target 0.5) 1 (max 0 (- old step)))))
  (write-history ready 1)
  (write-history level value)
  value)

; Analytical source family, normalized peak amplitude. The two oscillators
; own separate phases and sub phases; no sampled reference waveforms are used.
(defmacro heat-source (frequency wave duty sub_level sync_mode sync_semitones)
  (def hz (clip frequency 0.01 (* 0.45 samplerate)))
  (def phase (phasor hz))
  (def sub_hz (* 0.5 hz))
  (def sub_phase (phasor sub_hz))
  (def shape (clip (round wave) 0 3))
  (def main (selector (+ 1 shape)
    (block-gate (eq shape 0) (sin (* 6.28318530718 phase)))
    (block-gate (eq shape 1) (polyblep_saw phase hz))
    (block-gate (eq shape 2) (polyblep_pulse phase (clip duty 0.01 0.99) hz))
    (block-gate (eq shape 3) (noise))))
  (def synced (block-gate (* (gt sync_mode 0.5) (lt wave 2.5)) (heat-sync hz (heat-octaves (/ sync_semitones 12)) wave duty)))
  ; Noise has no periodic master/slave relationship; mode does not color it.
  (gswitch (* (gt sync_mode 0.5) (lt wave 2.5)) synced
    (block-gate (eq (* (gt sync_mode 0.5) (lt wave 2.5)) 0)
    (+ main (* (clip sub_level 0 1) (block-gate sub_level (polyblep_pulse sub_phase 0.5 sub_hz)))))))

(param volume_db @default -18 @min -60 @max 6 @unit dB @mod true @mod-mode additive)
(param tune_semitones @default 0 @min -48 @max 48 @unit st @mod true @mod-mode additive)
(param pressure_pitch_semitones @default 0 @min -24 @max 24 @unit st)
(param pressure_filter_octaves @default 0 @min -8 @max 8)
(param pressure_amp_db @default 0 @min -36 @max 12 @unit dB)

(param unison_voices @default 1 @min 1 @max 4)
(param unison_detune_cents @default 10 @min 0 @max 100 @unit ct)
(param unison_delay_ms @default 0 @min 0 @max 100 @unit ms)
(param unison_spread @default 0.5 @min 0 @max 1)
(param octave @default 0 @min -4 @max 4)
(param detune_cents @default 0 @min -100 @max 100 @unit ct)
(param stretch_cents @default 0 @min -100 @max 100 @unit ct/oct)
(param tuning_error_cents @default 0 @min 0 @max 100 @unit ct)
(param bend_range_semitones @default 2 @min 0 @max 24 @unit st)
(param glide_mode @default 0 @min 0 @max 2)
(param glide_time_ms @default 100 @min 0 @max 15000 @unit ms)
(param glide_rate_mode @default 0 @min 0 @max 1)
(param vibrato_rate_hz @default 5 @min 0.1 @max 20 @unit Hz)
(param vibrato_amount_cents @default 0 @min 0 @max 200 @unit ct)
(param vibrato_wheel_cents @default 0 @min 0 @max 200 @unit ct)
(param vibrato_delay_ms @default 0 @min 0 @max 10000 @unit ms)
(param vibrato_attack_ms @default 0 @min 0 @max 10000 @unit ms)

; Lane 1: independently stored source, modulation and articulation.
(param osc1_enabled @default 1 @min 0 @max 1)
(param osc1_wave @default 1 @min 0 @max 3)
(param osc1_level_db @default -6 @min -60 @max 12 @unit dB @mod true @mod-mode additive)
(param osc1_to_filter1 @default 1 @min 0 @max 1 @mod true @mod-mode additive)
(param osc1_pitch_env_initial @default 0 @min -48 @max 48 @unit st)
(param osc1_pitch_env_time_ms @default 500 @min 0 @max 15000 @unit ms)
(param osc1_semitones @default 0 @min -48 @max 48 @unit st @mod true @mod-mode additive)
(param osc1_cents @default 0 @min -300 @max 300 @unit ct @mod true @mod-mode additive)
(param osc1_keytrack @default 1 @min -2 @max 2)
(param osc1_pulse_duty @default 0.5 @min 0.01 @max 0.99 @mod true @mod-mode additive)
(param osc1_sub_sync @default 0 @min 0 @max 1)
(param osc1_sync_semitones @default 12 @min 0 @max 48 @unit st @mod true @mod-mode additive)
(param osc1_sub_level @default 0 @min 0 @max 1 @mod true @mod-mode additive)
(param osc1_lfo_pitch_semitones @default 0 @min -24 @max 24 @unit st)
(param osc1_lfo_pw @default 0 @min -0.49 @max 0.49)
(param lfo1_enabled @default 0 @min 0 @max 1)
(param lfo1_rate_hz @default 1 @min 0.01 @max 100)
(param lfo1_shape @default 0 @min 0 @max 4)
(param lfo1_width @default 0.5 @min 0 @max 1)
(param lfo1_retrigger @default 1 @min 0 @max 1)
(param lfo1_phase @default 0 @min 0 @max 1)
(param lfo1_delay_ms @default 0 @min 0 @max 10000)
(param lfo1_fade_ms @default 0 @min 0 @max 10000)
(param filter1_enabled @default 1 @min 0 @max 1)
(param filter1_mode @default 0 @min 0 @max 7)
(param filter1_cutoff_hz @default 1800 @min 30 @max 22000 @unit Hz @mod true @mod-mode additive)
(param filter1_q @default 0.707 @min 0.1 @max 100 @mod true @mod-mode additive)
(param filter1_drive @default 0 @min 0 @max 6)
(param filter1_keytrack @default 0 @min -2 @max 2)
(param filter1_env_octaves @default 2 @min -8 @max 8 @mod true @mod-mode additive)
(param filter1_lfo_octaves @default 0 @min -8 @max 8)
(param filter1_env_q @default 0 @min -50 @max 50)
(param filter1_lfo_q @default 0 @min -50 @max 50)
(param amp1_enabled @default 1 @min 0 @max 1)
(param amp1_level_db @default 0 @min -60 @max 12 @unit dB @mod true @mod-mode additive)
(param amp1_pan @default 0 @min -1 @max 1 @mod true @mod-mode additive)
(param amp1_lfo_level @default 0 @min -1 @max 1)
(param amp1_lfo_pan @default 0 @min -1 @max 1)
(param amp1_env_pan @default 0 @min -1 @max 1)
(param amp1_key_pan @default 0 @min -1 @max 1)
(param amp1_key_level_db @default 0 @min -24 @max 24 @unit dB)
(param filter1_env_attack_ms @default 5 @min 0.01 @max 15000)
(param filter1_env_decay_ms @default 350 @min 0.01 @max 15000)
(param filter1_env_sustain @default 0.25 @min 0 @max 1)
(param filter1_env_sustain_seconds @default -1 @min -1 @max 1000)
(param filter1_env_release_ms @default 250 @min 0.01 @max 15000)
(param filter1_env_exponential @default 1 @min 0 @max 1)
(param filter1_env_loop @default 0 @min 0 @max 3)
(param filter1_env_free @default 0 @min 0 @max 1)
(param filter1_env_legato @default 1 @min 0 @max 1)
(param filter1_env_velocity @default 0 @min 0 @max 1)
(param amp1_env_attack_ms @default 5 @min 0.01 @max 15000)
(param amp1_env_decay_ms @default 150 @min 0.01 @max 15000)
(param amp1_env_sustain @default 0.8 @min 0 @max 1)
(param amp1_env_sustain_seconds @default -1 @min -1 @max 1000)
(param amp1_env_release_ms @default 250 @min 0.01 @max 15000)
(param amp1_env_exponential @default 1 @min 0 @max 1)
(param amp1_env_loop @default 0 @min 0 @max 3)
(param amp1_env_free @default 0 @min 0 @max 1)
(param amp1_env_legato @default 1 @min 0 @max 1)
(param amp1_env_velocity @default 0.5 @min 0 @max 1)

; Lane 2: independently stored source, modulation and articulation.
(param osc2_enabled @default 0 @min 0 @max 1)
(param osc2_wave @default 1 @min 0 @max 3)
(param osc2_level_db @default -6 @min -60 @max 12 @unit dB @mod true @mod-mode additive)
(param osc2_to_filter1 @default 0 @min 0 @max 1 @mod true @mod-mode additive)
(param osc2_pitch_env_initial @default 0 @min -48 @max 48 @unit st)
(param osc2_pitch_env_time_ms @default 500 @min 0 @max 15000 @unit ms)
(param osc2_semitones @default 0 @min -48 @max 48 @unit st @mod true @mod-mode additive)
(param osc2_cents @default 0 @min -300 @max 300 @unit ct @mod true @mod-mode additive)
(param osc2_keytrack @default 1 @min -2 @max 2)
(param osc2_pulse_duty @default 0.5 @min 0.01 @max 0.99 @mod true @mod-mode additive)
(param osc2_sub_sync @default 0 @min 0 @max 1)
(param osc2_sync_semitones @default 12 @min 0 @max 48 @unit st @mod true @mod-mode additive)
(param osc2_sub_level @default 0 @min 0 @max 1 @mod true @mod-mode additive)
(param osc2_lfo_pitch_semitones @default 0 @min -24 @max 24 @unit st)
(param osc2_lfo_pw @default 0 @min -0.49 @max 0.49)
(param lfo2_enabled @default 0 @min 0 @max 1)
(param lfo2_rate_hz @default 1 @min 0.01 @max 100)
(param lfo2_shape @default 0 @min 0 @max 4)
(param lfo2_width @default 0.5 @min 0 @max 1)
(param lfo2_retrigger @default 1 @min 0 @max 1)
(param lfo2_phase @default 0 @min 0 @max 1)
(param lfo2_delay_ms @default 0 @min 0 @max 10000)
(param lfo2_fade_ms @default 0 @min 0 @max 10000)
(param filter2_enabled @default 1 @min 0 @max 1)
(param filter2_mode @default 0 @min 0 @max 7)
(param filter2_cutoff_hz @default 1800 @min 30 @max 22000 @unit Hz @mod true @mod-mode additive)
(param filter2_q @default 0.707 @min 0.1 @max 100 @mod true @mod-mode additive)
(param filter2_drive @default 0 @min 0 @max 6)
(param filter2_keytrack @default 0 @min -2 @max 2)
(param filter2_env_octaves @default 2 @min -8 @max 8 @mod true @mod-mode additive)
(param filter2_lfo_octaves @default 0 @min -8 @max 8)
(param filter2_env_q @default 0 @min -50 @max 50)
(param filter2_lfo_q @default 0 @min -50 @max 50)
(param amp2_enabled @default 1 @min 0 @max 1)
(param amp2_level_db @default 0 @min -60 @max 12 @unit dB @mod true @mod-mode additive)
(param amp2_pan @default 0 @min -1 @max 1 @mod true @mod-mode additive)
(param amp2_lfo_level @default 0 @min -1 @max 1)
(param amp2_lfo_pan @default 0 @min -1 @max 1)
(param amp2_env_pan @default 0 @min -1 @max 1)
(param amp2_key_pan @default 0 @min -1 @max 1)
(param amp2_key_level_db @default 0 @min -24 @max 24 @unit dB)
(param filter2_env_attack_ms @default 5 @min 0.01 @max 15000)
(param filter2_env_decay_ms @default 350 @min 0.01 @max 15000)
(param filter2_env_sustain @default 0.25 @min 0 @max 1)
(param filter2_env_sustain_seconds @default -1 @min -1 @max 1000)
(param filter2_env_release_ms @default 250 @min 0.01 @max 15000)
(param filter2_env_exponential @default 1 @min 0 @max 1)
(param filter2_env_loop @default 0 @min 0 @max 3)
(param filter2_env_free @default 0 @min 0 @max 1)
(param filter2_env_legato @default 1 @min 0 @max 1)
(param filter2_env_velocity @default 0 @min 0 @max 1)
(param amp2_env_attack_ms @default 5 @min 0.01 @max 15000)
(param amp2_env_decay_ms @default 150 @min 0.01 @max 15000)
(param amp2_env_sustain @default 0.8 @min 0 @max 1)
(param amp2_env_sustain_seconds @default -1 @min -1 @max 1000)
(param amp2_env_release_ms @default 250 @min 0.01 @max 15000)
(param amp2_env_exponential @default 1 @min 0 @max 1)
(param amp2_env_loop @default 0 @min 0 @max 3)
(param amp2_env_free @default 0 @min 0 @max 1)
(param amp2_env_legato @default 1 @min 0 @max 1)
(param amp2_env_velocity @default 0.5 @min 0 @max 1)
(param filter1_to_filter2 @default 0 @min 0 @max 1 @mod true @mod-mode additive)
(param filter2_follow @default 0 @min 0 @max 1)
(param filter2_offset_octaves @default 0 @min -8 @max 8 @mod true @mod-mode additive)
(param noise_enabled @default 0 @min 0 @max 1)
(param noise_level_db @default -24 @min -60 @max 12 @unit dB @mod true @mod-mode additive)
(param noise_color_hz @default 8000 @min 30 @max 22000 @unit Hz @mod true @mod-mode additive)
(param noise_to_filter1 @default 0.5 @min 0 @max 1 @mod true @mod-mode additive)

(def expression (clip pressure 0 1))
(def keyboard_octave (/ (log (/ (max pitch 0.01) 261.625565)) 0.69314718056))
(def played_octave (heat-glide keyboard_octave note_on legato glide_mode glide_time_ms glide_rate_mode))
(def vibrato (* (heat-lfo vibrato_rate_hz 0.5 0 note_on 0 0 vibrato_delay_ms vibrato_attack_ms)
  (+ vibrato_amount_cents (* (heat-control (clip mod_wheel 0 1)) vibrato_wheel_cents))))
(def tuning (+ (mod tune_semitones) (* 12 (round octave))
  (/ (+ detune_cents vibrato) 100)
  (* (heat-control (clip pitch_bend -1 1)) bend_range_semitones)
  (* expression pressure_pitch_semitones)))

; A unison copy owns the complete dual source/filter/amp path, all four
; envelopes and both LFOs. Modulated values enter explicitly so each host
; destination remains a single shared parameter with independent DSP history.
(defmacro heat-voice (gate note_on trigger velocity played_octave tuning expression unison_pan
  m_osc1_semitones m_osc1_cents m_osc1_level_db m_osc1_pulse_duty
  m_osc1_sub_level m_osc1_sync_semitones m_osc1_to_filter1 m_osc2_semitones
  m_osc2_cents m_osc2_level_db m_osc2_pulse_duty m_osc2_sub_level
  m_osc2_sync_semitones m_osc2_to_filter1 m_noise_level_db m_noise_color_hz
  m_noise_to_filter1 m_filter1_cutoff_hz m_filter1_env_octaves m_filter1_q
  m_filter1_to_filter2 m_amp1_level_db m_amp1_pan m_filter2_offset_octaves
  m_filter2_cutoff_hz m_filter2_env_octaves m_filter2_q m_amp2_level_db
  m_amp2_pan)
  ; Key-dependent tuning follows this copy's delayed onset, not the newest key
  ; while the old contour is still releasing. Error is stable for each hold.
  (def tuning_error (latch (noise) (gt note_on 0.5)))
  (def voice_tuning (+ tuning (/ (+ (* played_octave stretch_cents)
    (* tuning_error tuning_error_cents)) 100)))
  (def lfo1 (block-gate lfo1_enabled (* lfo1_enabled
    (heat-lfo lfo1_rate_hz lfo1_width lfo1_shape note_on
      lfo1_retrigger lfo1_phase lfo1_delay_ms lfo1_fade_ms))))
  (def filter1_env
    (* (+ (- 1 filter1_env_velocity) (* filter1_env_velocity (clip velocity 0 1)))
      (heat-envelope gate (gswitch (gt filter1_env_legato 0.5) trigger note_on)
        filter1_env_attack_ms filter1_env_decay_ms filter1_env_sustain filter1_env_sustain_seconds
        filter1_env_release_ms filter1_env_exponential filter1_env_loop filter1_env_free)))
  (def amp1_env
    (* (+ (- 1 amp1_env_velocity) (* amp1_env_velocity (clip velocity 0 1)))
      (heat-envelope gate (gswitch (gt amp1_env_legato 0.5) trigger note_on)
        amp1_env_attack_ms amp1_env_decay_ms amp1_env_sustain amp1_env_sustain_seconds
        amp1_env_release_ms amp1_env_exponential amp1_env_loop amp1_env_free)))
  (def osc1_hz (* 261.625565
    (heat-octaves (+ (* played_octave osc1_keytrack)
      (/ (+ voice_tuning m_osc1_semitones (/ m_osc1_cents 100)
        (* lfo1 osc1_lfo_pitch_semitones)
        (heat-pitch-envelope note_on osc1_pitch_env_initial osc1_pitch_env_time_ms)) 12)))))
  (def osc1_fade (heat-enable osc1_enabled))
  (def osc1 (block-gate osc1_fade (* (* osc1_fade (heat-control (heat-db m_osc1_level_db)))
    (heat-source osc1_hz osc1_wave
      (+ m_osc1_pulse_duty (* lfo1 osc1_lfo_pw)) m_osc1_sub_level osc1_sub_sync m_osc1_sync_semitones))))
  (def balance1 (heat-control (clip m_osc1_to_filter1 0 1)))
  (def lfo2 (block-gate lfo2_enabled (* lfo2_enabled
    (heat-lfo lfo2_rate_hz lfo2_width lfo2_shape note_on
      lfo2_retrigger lfo2_phase lfo2_delay_ms lfo2_fade_ms))))
  (def filter2_env
    (* (+ (- 1 filter2_env_velocity) (* filter2_env_velocity (clip velocity 0 1)))
      (heat-envelope gate (gswitch (gt filter2_env_legato 0.5) trigger note_on)
        filter2_env_attack_ms filter2_env_decay_ms filter2_env_sustain filter2_env_sustain_seconds
        filter2_env_release_ms filter2_env_exponential filter2_env_loop filter2_env_free)))
  (def amp2_env
    (* (+ (- 1 amp2_env_velocity) (* amp2_env_velocity (clip velocity 0 1)))
      (heat-envelope gate (gswitch (gt amp2_env_legato 0.5) trigger note_on)
        amp2_env_attack_ms amp2_env_decay_ms amp2_env_sustain amp2_env_sustain_seconds
        amp2_env_release_ms amp2_env_exponential amp2_env_loop amp2_env_free)))
  (def osc2_hz (* 261.625565
    (heat-octaves (+ (* played_octave osc2_keytrack)
      (/ (+ voice_tuning m_osc2_semitones (/ m_osc2_cents 100)
        (* lfo2 osc2_lfo_pitch_semitones)
        (heat-pitch-envelope note_on osc2_pitch_env_initial osc2_pitch_env_time_ms)) 12)))))
  (def osc2_fade (heat-enable osc2_enabled))
  (def osc2 (block-gate osc2_fade (* (* osc2_fade (heat-control (heat-db m_osc2_level_db)))
    (heat-source osc2_hz osc2_wave
      (+ m_osc2_pulse_duty (* lfo2 osc2_lfo_pw)) m_osc2_sub_level osc2_sub_sync m_osc2_sync_semitones))))
  (def balance2 (heat-control (clip m_osc2_to_filter1 0 1)))

  (def noise_fade (heat-enable noise_enabled))
  (def colored_noise (block-gate noise_fade (* (* noise_fade (heat-control (heat-db m_noise_level_db)))
    (svf (noise) m_noise_color_hz 0.70710678 0))))
  (def noise_balance (heat-control (clip m_noise_to_filter1 0 1)))
  (def input1 (+ (* osc1 balance1) (* osc2 balance2) (* colored_noise noise_balance)))
  (def input2 (+ (* osc1 (- 1 balance1)) (* osc2 (- 1 balance2))
    (* colored_noise (- 1 noise_balance))))

  (def cutoff1 (clip (* m_filter1_cutoff_hz
    (heat-octaves (+ (* played_octave filter1_keytrack)
      (* filter1_env m_filter1_env_octaves) (* lfo1 filter1_lfo_octaves)
      (* expression pressure_filter_octaves)))) 30 (min 22000 (* 0.49 samplerate))))
  (def resonance1 (clip (+ m_filter1_q (* filter1_env filter1_env_q)
    (* lfo1 filter1_lfo_q)) 0.1 100))
  (def filtered1 (gswitch (gt filter1_enabled 0.5)
    (heat-drive (heat-linear-filter input1 cutoff1 resonance1 filter1_mode) filter1_drive)
    input1))
  (def send1 (heat-control (clip m_filter1_to_filter2 0 1)))
  (def amplitude1 (* amp1_env
    (heat-control (* amp1_enabled (heat-db (+ m_amp1_level_db
      (* played_octave amp1_key_level_db)))))
    (max 0 (+ 1 (* lfo1 amp1_lfo_level)))))
  (def pan1 (clip (+ unison_pan (heat-control m_amp1_pan) (* lfo1 amp1_lfo_pan)
   (* amp1_env amp1_env_pan) (* played_octave amp1_key_pan)) -1 1))
  (def lane1 (* (* filtered1 (- 1 send1)) amplitude1))
  (def cutoff2 (clip (* (gswitch (gt filter2_follow 0.5) (* cutoff1 (heat-octaves m_filter2_offset_octaves)) m_filter2_cutoff_hz)
    (heat-octaves (+ (* played_octave filter2_keytrack)
      (* filter2_env m_filter2_env_octaves) (* lfo2 filter2_lfo_octaves)
      (* expression pressure_filter_octaves)))) 30 (min 22000 (* 0.49 samplerate))))
  (def resonance2 (clip (+ m_filter2_q (* filter2_env filter2_env_q)
    (* lfo2 filter2_lfo_q)) 0.1 100))
  (def filtered2 (gswitch (gt filter2_enabled 0.5)
    (heat-drive (heat-linear-filter (+ input2 (* filtered1 send1)) cutoff2 resonance2 filter2_mode) filter2_drive)
    (+ input2 (* filtered1 send1))))
  (def amplitude2 (* amp2_env
    (heat-control (* amp2_enabled (heat-db (+ m_amp2_level_db
      (* played_octave amp2_key_level_db)))))
    (max 0 (+ 1 (* lfo2 amp2_lfo_level)))))
  (def pan2 (clip (+ unison_pan (heat-control m_amp2_pan) (* lfo2 amp2_lfo_pan)
   (* amp2_env amp2_env_pan) (* played_octave amp2_key_pan)) -1 1))
  (def lane2 (* filtered2 amplitude2))

  (tuple (+ (* lane1 (cos (* 0.7853981634 (+ 1 pan1))))
      (* lane2 (cos (* 0.7853981634 (+ 1 pan2)))))
    (+ (* lane1 (sin (* 0.7853981634 (+ 1 pan1))))
      (* lane2 (sin (* 0.7853981634 (+ 1 pan2))))))
)

; A unison copy is skipped while it is disabled, and block-gate freezes rather
; than releases the state it skips. A copy frozen part-way through its release
; resumes there: heat-envelope starts every attack from its previous value, so
; raising unison_voices during a later note would begin that copy's attack from
; the frozen level instead of from idle. Keep the copy executing, with its own
; gate already low, for as long as its contours can still be sounding, and only
; then freeze it -- at idle, which is indistinguishable from a copy that has
; never run, so its next onset attacks from zero and latches its own tuning
; error. The window is the slowest complete attack/decay/release the four
; contours can be configured for, plus 5 ms covering the enable fade. It is
; paid once per change of unison_voices, never in the steady disabled state
; that the skip exists to make cheap.
(def contour_hold_ms (+ 5
  (max (max amp1_env_attack_ms amp2_env_attack_ms)
    (max filter1_env_attack_ms filter2_env_attack_ms))
  (max (max amp1_env_decay_ms amp2_env_decay_ms)
    (max filter1_env_decay_ms filter2_env_decay_ms))
  (max (max amp1_env_release_ms amp2_env_release_ms)
    (max filter1_env_release_ms filter2_env_release_ms))))
; Free-running loop contours ignore note-off and never return to idle, so a
; copy configured that way must not be frozen at all.
(defmacro heat-contour-loops (loop_mode free_run)
  (* (gt free_run 0.5) (gt loop_mode 0.5) (lt loop_mode 2.5)))
(def contour_never_idle
  (max (max (heat-contour-loops amp1_env_loop amp1_env_free)
      (heat-contour-loops amp2_env_loop amp2_env_free))
    (max (heat-contour-loops filter1_env_loop filter1_env_free)
      (heat-contour-loops filter2_env_loop filter2_env_free))))

(def copies (clip (round unison_voices) 1 4))

(def enabled0 (gt copies 0))
(def position0 (gswitch (gt copies 1) (- (/ 0 (max 1 (- copies 1))) 1) 0))
(def (gate0 on0 trigger0 legato0 pitch0 velocity0)
  (heat-unison-onset gate note_on trigger legato played_octave velocity enabled0 (* 0 unison_delay_ms)))
(def fade0 (heat-enable enabled0))
(def run0 (max (heat-run enabled0 contour_hold_ms) contour_never_idle))
(def (left0 right0) (block-gate run0 (heat-voice gate0 on0 trigger0 velocity0 pitch0
  (+ tuning (/ (* position0 unison_detune_cents) 100)) expression (* position0 unison_spread)
  (mod osc1_semitones) (mod osc1_cents) (mod osc1_level_db) (mod osc1_pulse_duty)
  (mod osc1_sub_level) (mod osc1_sync_semitones) (mod osc1_to_filter1) (mod osc2_semitones)
  (mod osc2_cents) (mod osc2_level_db) (mod osc2_pulse_duty) (mod osc2_sub_level)
  (mod osc2_sync_semitones) (mod osc2_to_filter1) (mod noise_level_db) (mod noise_color_hz)
  (mod noise_to_filter1) (mod filter1_cutoff_hz) (mod filter1_env_octaves) (mod filter1_q)
  (mod filter1_to_filter2) (mod amp1_level_db) (mod amp1_pan) (mod filter2_offset_octaves)
  (mod filter2_cutoff_hz) (mod filter2_env_octaves) (mod filter2_q) (mod amp2_level_db)
  (mod amp2_pan)
)))

(def enabled1 (gt copies 1))
(def position1 (gswitch (gt copies 1) (- (/ 2 (max 1 (- copies 1))) 1) 0))
(def (gate1 on1 trigger1 legato1 pitch1 velocity1)
  (heat-unison-onset gate note_on trigger legato played_octave velocity enabled1 (* 1 unison_delay_ms)))
(def fade1 (heat-enable enabled1))
(def run1 (max (heat-run enabled1 contour_hold_ms) contour_never_idle))
(def (left1 right1) (block-gate run1 (heat-voice gate1 on1 trigger1 velocity1 pitch1
  (+ tuning (/ (* position1 unison_detune_cents) 100)) expression (* position1 unison_spread)
  (mod osc1_semitones) (mod osc1_cents) (mod osc1_level_db) (mod osc1_pulse_duty)
  (mod osc1_sub_level) (mod osc1_sync_semitones) (mod osc1_to_filter1) (mod osc2_semitones)
  (mod osc2_cents) (mod osc2_level_db) (mod osc2_pulse_duty) (mod osc2_sub_level)
  (mod osc2_sync_semitones) (mod osc2_to_filter1) (mod noise_level_db) (mod noise_color_hz)
  (mod noise_to_filter1) (mod filter1_cutoff_hz) (mod filter1_env_octaves) (mod filter1_q)
  (mod filter1_to_filter2) (mod amp1_level_db) (mod amp1_pan) (mod filter2_offset_octaves)
  (mod filter2_cutoff_hz) (mod filter2_env_octaves) (mod filter2_q) (mod amp2_level_db)
  (mod amp2_pan)
)))

(def enabled2 (gt copies 2))
(def position2 (gswitch (gt copies 1) (- (/ 4 (max 1 (- copies 1))) 1) 0))
(def (gate2 on2 trigger2 legato2 pitch2 velocity2)
  (heat-unison-onset gate note_on trigger legato played_octave velocity enabled2 (* 2 unison_delay_ms)))
(def fade2 (heat-enable enabled2))
(def run2 (max (heat-run enabled2 contour_hold_ms) contour_never_idle))
(def (left2 right2) (block-gate run2 (heat-voice gate2 on2 trigger2 velocity2 pitch2
  (+ tuning (/ (* position2 unison_detune_cents) 100)) expression (* position2 unison_spread)
  (mod osc1_semitones) (mod osc1_cents) (mod osc1_level_db) (mod osc1_pulse_duty)
  (mod osc1_sub_level) (mod osc1_sync_semitones) (mod osc1_to_filter1) (mod osc2_semitones)
  (mod osc2_cents) (mod osc2_level_db) (mod osc2_pulse_duty) (mod osc2_sub_level)
  (mod osc2_sync_semitones) (mod osc2_to_filter1) (mod noise_level_db) (mod noise_color_hz)
  (mod noise_to_filter1) (mod filter1_cutoff_hz) (mod filter1_env_octaves) (mod filter1_q)
  (mod filter1_to_filter2) (mod amp1_level_db) (mod amp1_pan) (mod filter2_offset_octaves)
  (mod filter2_cutoff_hz) (mod filter2_env_octaves) (mod filter2_q) (mod amp2_level_db)
  (mod amp2_pan)
)))

(def enabled3 (gt copies 3))
(def position3 (gswitch (gt copies 1) (- (/ 6 (max 1 (- copies 1))) 1) 0))
(def (gate3 on3 trigger3 legato3 pitch3 velocity3)
  (heat-unison-onset gate note_on trigger legato played_octave velocity enabled3 (* 3 unison_delay_ms)))
(def fade3 (heat-enable enabled3))
(def run3 (max (heat-run enabled3 contour_hold_ms) contour_never_idle))
(def (left3 right3) (block-gate run3 (heat-voice gate3 on3 trigger3 velocity3 pitch3
  (+ tuning (/ (* position3 unison_detune_cents) 100)) expression (* position3 unison_spread)
  (mod osc1_semitones) (mod osc1_cents) (mod osc1_level_db) (mod osc1_pulse_duty)
  (mod osc1_sub_level) (mod osc1_sync_semitones) (mod osc1_to_filter1) (mod osc2_semitones)
  (mod osc2_cents) (mod osc2_level_db) (mod osc2_pulse_duty) (mod osc2_sub_level)
  (mod osc2_sync_semitones) (mod osc2_to_filter1) (mod noise_level_db) (mod noise_color_hz)
  (mod noise_to_filter1) (mod filter1_cutoff_hz) (mod filter1_env_octaves) (mod filter1_q)
  (mod filter1_to_filter2) (mod amp1_level_db) (mod amp1_pan) (mod filter2_offset_octaves)
  (mod filter2_cutoff_hz) (mod filter2_env_octaves) (mod filter2_q) (mod amp2_level_db)
  (mod amp2_pan)
)))

; Equal-power pan and explicit master gain. Normalize the sum by copy count;
; continuous controls are smoothed, with no hidden limiter on the result.
(def gain (heat-control (/ (heat-db (+ (mod volume_db) (* expression pressure_amp_db))) copies)))
(out (* gain (+ (* fade0 left0) (* fade1 left1) (* fade2 left2) (* fade3 left3))) 1)
(out (* gain (+ (* fade0 right0) (* fade1 right1) (* fade2 right2) (* fade3 right3))) 2)
