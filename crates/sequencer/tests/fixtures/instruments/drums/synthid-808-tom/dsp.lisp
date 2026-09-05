; SynthID 808 Tom - the Rung 3 TR-808 low tom identification patch.
;
; Same DGenLisp port of Examples/SynthID/Patch.swift as drums/synthid-808 (the
; TR-808 tanh voice: closed-form swept-phase body, attack-localized even
; harmonic, odd-harmonic term, click, filtered noise burst, tanh drive). The
; identified defaults come from the 808-tom profile run on
; Assets/808-tom-low.wav (output/rung3_808tom_v1/recovered_params.json,
; 78.07% independent MR-STFT improvement against the deterministic midpoint
; baseline - just under the 80% gate - with an absolute learned distance of
; 0.014963, between the accepted 808 (0.0116) and 909 (0.0223); the port
; reproduces learned.wav to 4e-5 max abs). At an end pitch of 92.99951 Hz
; and velocity 1.0 they reproduce the learned render; velocity is applied
; only as host-level gain.
;
; What the measurement said (and the profile bounds encode): a near-pure sine
; sweeping 113 -> 93 Hz with a slow ~-20/s pitch decay, a uniform ~-13/s
; amplitude decay (no steepening), H2/H3 40/33 dB down at onset, and a high
; band 70 dB below the fundamental - the tom is almost all body, so the click
; and noise sections sit near zero at their identified defaults and are there
; to be turned up.
;
; MIDI pitch defines the endpoint of the body sweep, through the same /8
; mapping as synthid-808 (the identified 92.99951 Hz end pitch sounds at
; F#5 = 744 Hz on the keyboard; the audition harness wants --pitch 743.996).
; start_ratio preserves the learned relationship between its endpoints while
; allowing the sweep depth to be edited independently. The gate input is deliberately not part of the
; identified one-shot voice. A trigger resets the analytic time origin, while a
; small active latch prevents an unsolicited hit when the instrument is loaded.

(def gate (in 1 @name gate))
(def pitch (/ (in 2 @name pitch) 8))
(def velocity (in 3 @name velocity))
(def trigger (in 4 @name trigger))
(def mod1 (in 5 @name mod1 @modulator 1))
(def mod2 (in 6 @name mod2 @modulator 2))
(def mod3 (in 7 @name mod3 @modulator 3))
(def mod4 (in 8 @name mod4 @modulator 4))

; History-based one-pole parameter smoother. Each expansion owns independent
; value and initialization history. The first sample adopts the current value
; directly, avoiding a startup ramp away from the identified defaults.
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

; Shared time constant for editable voice parameters. Pitch is a note input,
; not a parameter, and remains immediate at each sequencer trigger.
(param smoothing @default 5 @min 0 @max 100 @unit ms)

; The learned sweep endpoints were 128.40256 Hz and 92.99951 Hz. Their ratio is
; retained as the default while the sequencer pitch supplies the endpoint
; (92.99951 Hz is a hair under F#2 = 92.5 Hz).
(param start_ratio @default 1.3806800 @min 1 @max 6)
(param pitch_decay @default -25.772999 @min -80 @max -5)

(param body_amp @default 0.3182151 @min 0.2 @max 1.0)
; Body release is expressed as T60 milliseconds. The learned ampDecay value of
; -12.9586170/s maps exactly to 533.06269 ms via -ln(1000)/(release*0.001).
(param release @default 533.06269 @min 100 @max 4000 @unit ms)
(param body_asymmetry @default 0.01700413 @min -0.5 @max 0.5)
; Odd-harmonic term (3rd/9 + 5th/25 on the body envelope), the profile's
; zero-default capacity scalar; the tom's H3 is louder than its H2.
(param body_harmonic @default 0.04795937 @min -1 @max 1)

(param click_freq @default 429.5722 @min 300 @max 3000 @unit Hz)
(param click_amp @default 0.1907173 @min 0 @max 1.5)
(param click_decay @default -1049.25790 @min -1600 @max -100)

(param noise_cutoff @default 1560.879 @min 1000 @max 20000 @unit Hz)
(param noise_amp @default 0.00033538853 @min 0 @max 0.05)
(param noise_decay @default -4.5932680 @min -400 @max -0.001)

(param drive @default 2.2058423 @min 1 @max 3)
(param out_gain @default 0.3753786 @min 0.1 @max 1)
; Optional post-voice fade-in. Zero preserves the identified instantaneous
; attack. Nonzero values are T60-style rise times: at attack milliseconds the
; envelope is within 0.1% of full scale.
(param fade_in @default 0 @min 0 @max 100 @unit ms)
; Crossfade time used only when a new trigger overlaps a playing hit. The first
; isolated trigger remains instantaneous and preserves the identified attack.
(param retrigger_fade @default 5 @min 0.1 @max 50 @unit ms)

; Smooth every editable synthesis parameter before it enters the voice graph.
(def start_ratio_s (onepole-param start_ratio smoothing))
(def pitch_decay_s (onepole-param pitch_decay smoothing))
(def body_amp_s (onepole-param body_amp smoothing))
(def release_s (onepole-param release smoothing))
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

; One independent copy of the identified voice. Macro expansion gives each
; retrigger slot its own clock, velocity latch, noise source, and filter state.
(defmacro synthid-voice (voice_trigger input_pitch input_velocity)
  ; Resettable seconds-since-trigger clock. This explicit history form avoids
  ; the accumulator's previous-value convention: a trigger produces exactly
  ; t=0, followed by t=1/samplerate.
  (make-history time_h)
  (def previous_time (read-history time_h))
  (def t (gswitch (gt voice_trigger 0.5) 0.0 previous_time))
  (write-history time_h (+ t (/ 1.0 samplerate)))

  (make-history active_h)
  (def active (gswitch (gt voice_trigger 0.5) 1.0 (read-history active_h)))
  (write-history active_h active)

  ; Velocity belongs to this hit for its complete lifetime; later triggers may
  ; carry different velocities without changing the outgoing tail.
  (make-history velocity_h)
  (def previous_velocity (read-history velocity_h))
  (def hit_velocity
    (gswitch (gt voice_trigger 0.5)
             (clip input_velocity 0.0 1.0)
             previous_velocity))
  (write-history velocity_h hit_velocity)

  ; Pitch is latched for the same reason: an outgoing tail must not retune when
  ; the incoming sequencer event carries a different note.
  (make-history pitch_h)
  (def previous_pitch (read-history pitch_h))
  (def hit_pitch
    (gswitch (gt voice_trigger 0.5)
             (max input_pitch 1.0)
             previous_pitch))
  (write-history pitch_h hit_pitch)

  ; Closed-form integral of the exponential pitch sweep.
  (def body_end hit_pitch)
  (def body_start (* body_end start_ratio_s))
  (def sweep_phase
    (+ (* body_end t)
       (* (/ (- body_start body_end) pitch_decay_s)
          (- (exp (* pitch_decay_s t)) 1.0))))

  (def amp_decay (/ -6.9077553 (* (max release_s 1.0) 0.001)))
  (def body_envelope (exp (* amp_decay t)))
  (def body
    (* (sin (* sweep_phase twopi))
       body_envelope
       body_amp_s))

  (def even_harmonic
    (* body_asymmetry_s
       (sin (- (* sweep_phase 2.0 twopi) 0.62))
       body_envelope
       body_amp_s
       (exp (* -17.0 t))))

  ; Triangle-series odd harmonics of the swept phase on the body envelope
  ; (Patch.swift oddHarmonics); inert at body_harmonic 0.
  (def odd_harmonics
    (* body_harmonic_s
       (+ (* (sin (* sweep_phase 3.0 twopi)) 0.11111111)
          (* (sin (* sweep_phase 5.0 twopi)) 0.04))
       body_envelope
       body_amp_s))

  (def click
    (* (sin (* click_freq_s t twopi))
       (exp (* click_decay_s t))
       click_amp_s))

  ; DGen scalar noise is [0,1); scale it to [-1,1) exactly as SynthID does.
  (def bipolar_noise (- (* (noise) 2.0) 1.0))
  (def filtered_noise (biquad bipolar_noise noise_cutoff_s 0.707 1.0 0.0))
  (def noise_burst
    (* filtered_noise
       (exp (* noise_decay_s t))
       noise_amp_s))

  (def mixed (+ body even_harmonic odd_harmonics click noise_burst))
  (def learned_voice (* (tanh (* mixed drive_s)) out_gain_s))
  (def attack_seconds (* (max fade_in_s 0.001) 0.001))
  (def attack_envelope
    (gswitch (lt fade_in_s 0.001)
             1.0
             (- 1.0 (exp (/ (* -6.9077553 t) attack_seconds)))))
  (* learned_voice attack_envelope hit_velocity active))

; Exponential crossfade gain. Unlike general parameter smoothing, this starts
; from zero by design. first_hit bypasses the fade so an isolated initial hit
; retains the exact hard attack of the identified recording.
(defmacro retrigger-gain (target first_hit time_ms)
  (make-history gain_h)
  (def previous_gain (read-history gain_h))
  (def safe_seconds (* (max time_ms 0.001) 0.001))
  (def coefficient (exp (/ -6.9077553 (* samplerate safe_seconds))))
  (def filtered_gain
    (+ target (* coefficient (- previous_gain target))))
  (def next_gain
    (gswitch (gt first_hit 0.5)
             target
             (gswitch (lt time_ms 0.001) target filtered_gain)))
  (write-history gain_h next_gain)
  ; On an overlapping trigger, emit the previous weight for this exact sample
  ; and begin the transition in state for the next sample. This makes the gain
  ; contribution continuous at the retrigger boundary.
  (gswitch (gt first_hit 0.5) target previous_gain))

; Convert the host trigger to a single-sample rising edge before toggling slots.
(def trigger_gate (gt trigger 0.5))
(make-history trigger_h)
(def previous_trigger (read-history trigger_h))
(def triggered (max 0.0 (- trigger_gate previous_trigger)))
(write-history trigger_h trigger_gate)

; Alternate between two voice slots. The outgoing slot keeps running while its
; crossfade gain falls, and the incoming slot starts at t=0 with latched velocity.
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
(def voice_a (synthid-voice trigger_a pitch velocity))
(def voice_b (synthid-voice trigger_b pitch velocity))

(def target_a (gswitch (lt selector 0.5) 1.0 0.0))
(def target_b (- 1.0 target_a))
(def gain_a (retrigger-gain target_a first_hit retrigger_fade_s))
(def gain_b (retrigger-gain target_b first_hit retrigger_fade_s))

(out (+ (* voice_a gain_a) (* voice_b gain_b)) 1 @name audio)
