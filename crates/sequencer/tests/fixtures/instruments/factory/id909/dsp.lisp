; Factory ID909 — the identified SynthID TR-909 voice, rebuilt for p-locking.
;
; The body is the accepted Rung 3 identification patch (drums/synthid-909,
; 80.10% independent MR-STFT improvement); at the defaults below every hit
; reproduces the learned render. The 40 fixed harmonic coefficients are the
; backward-pruned learned timbre, expressed as Fourier/envelope structure.
; Everything added around the learned voice is a no-op at its default, so
; the instrument boots up AS the 909 — the same transformation applied to
; factory/id808:
;   - tune (semitones, host-modulatable) retunes the sounding voice
;     continuously — the body sweep integrates its instantaneous frequency,
;     so tune LFOs and pitch p-locks both track (the harmonic-correction
;     bank rides the same phase, so the whole learned timbre transposes);
;   - one classic ADSR: at the defaults (sustain 0, release == decay)
;     it IS the learned one-shot for any gate length; sustain up + short
;     release is gated bass. Glide slides between pitches;
;   - the BANK: the sc-filterbank (Sherman FB2) core with its cutoff on a
;     per-trigger decay envelope, keytrack + tube drive, reconstruction;
;   - a final HP/LP tone pair + level after the bank, exact bypass at the
;     defaults.
;
; Factory macro-vocabulary style (docs/factory-macro-library-spec.md): the
; top level is section macro nodes wired together, no bare math;
; host-modulatable params stay top-level and enter the graph as (mod p)
; macro arguments; the remaining p-lockable params live in bank sections
; that smooth them and return tuples.

(def gate (in 1 @name gate))
(def pitch (/ (in 2 @name pitch) 8))
(def velocity (in 3 @name velocity))
(def trigger (in 4 @name trigger))
(def clock (in 5 @name clock))
(def mod1 (in 6 @name mod1 @modulator 1))
(def mod2 (in 7 @name mod2 @modulator 2))
(def mod3 (in 8 @name mod3 @modulator 3))
(def mod4 (in 9 @name mod4 @modulator 4))

; ======================================================================
; host-modulatable params (must live at top level; see header)
; ======================================================================

; Learned defaults: sweep endpoints 276.55519 Hz / 46.922146 Hz (ratio
; kept, the sequencer pitch supplies the endpoint), body T60 624.4732 ms
; (ampDecay -11.0617325/s), drive 2.003162. Ranges are widened past the
; identified values; the defaults themselves are untouched.
(param tune @default 0 @min -24 @max 24 @unit st @mod true @mod-mode additive @mod-depth-min -24 @mod-depth-max 24 @mod-unit st)
(param start_ratio @default 5.893916 @min 1 @max 12 @mod true @mod-mode additive @mod-depth-min -5 @mod-depth-max 5)
(param decay @default 624.4732 @min 40 @max 8000 @unit ms @mod true @mod-mode additive @mod-depth-min -2000 @mod-depth-max 2000 @mod-unit ms)
(param release @default 624.4732 @min 40 @max 8000 @unit ms @mod true @mod-mode additive @mod-depth-min -2000 @mod-depth-max 2000 @mod-unit ms)
(param drive @default 2.003162 @min 1 @max 8 @mod true @mod-mode additive @mod-depth-min -3 @mod-depth-max 3)
(param click_amp @default 1.2 @min 0 @max 4 @mod true @mod-mode additive @mod-depth-min -2 @mod-depth-max 2)
(param noise_amp @default 0.0006846405 @min 0 @max 0.01 @mod true @mod-mode additive @mod-depth-min -0.005 @mod-depth-max 0.005)
(param bank @default 0 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
(param bank_env @default 0.31 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
(param bank_freq @default 0.03 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
(param bank_res @default 0.75 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
(param lpf @default 18000 @min 200 @max 18000 @unit Hz @mod true @mod-mode additive @mod-depth-min -9000 @mod-depth-max 9000 @mod-unit Hz)
(param hpf @default 20 @min 20 @max 500 @unit Hz @mod true @mod-mode additive @mod-depth-min -240 @mod-depth-max 240 @mod-unit Hz)
(param level @default 1 @min 0 @max 1.5 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)

; Shared time constant for editable voice parameters. Pitch is a note input,
; not a parameter, and remains immediate at each sequencer trigger.
(param smoothing @default 5 @min 0 @max 100 @unit ms)

; ======================================================================
; leaf helper macros (collapsed inside section layers)
; ======================================================================

(defmacro semi (s) (pow 2 (/ s 12)))

; History-based one-pole parameter smoother (from drums/synthid-909). Each
; expansion owns independent value and initialization history. The first
; sample adopts the current value directly, avoiding a startup ramp away
; from the identified defaults.
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

; Resettable exponential decay envelope (T60 in ms), value 1.0 on the
; trigger sample.
(defmacro id-env (trig decay_ms)
  (make-history e_h)
  (def coef (exp (/ -6.9077553 (max 1.0 (* decay_ms 0.001 samplerate)))))
  (def next (gswitch (gt trig 0.5) 1.0 (* (read-history e_h) coef)))
  (write-history e_h next)
  next)

; A correction is a scalar Fourier partial following the body sweep and
; body envelope, with one of the four pruned decay rates: 0, 15, 60, 240/s.
(defmacro harmonic-sin (coefficient harmonic decay sweep_phase body_envelope t)
  (* coefficient
     (sin (* sweep_phase harmonic twopi))
     body_envelope
     body_amp_v
     (exp (* -1.0 decay t))))

(defmacro harmonic-cos (coefficient harmonic decay sweep_phase body_envelope t)
  (* coefficient
     (cos (* sweep_phase harmonic twopi))
     body_envelope
     body_amp_v
     (exp (* -1.0 decay t))))

; ======================================================================
; parameter bank sections: declare + smooth the non-modulatable params
; (names stay global in the manifest so p-locks/presets/ui bind normally)
; ======================================================================

; -> (sweep sustain glide attack fade retrigger_fade)
(defmacro sweep-tail-params (smooth_ms)
  ; Body pitch-sweep time: the learned -52.411747/s exponential rate
  ; expressed as a T60 in ms (same convention as decay/release).
  (param sweep @default 131.79784 @min 50 @max 900 @unit ms)
  ; Envelope sustain level. 0 (learned) = one-shot; raised = the note
  ; holds while the gate is down, then releases — gated bass, no mode.
  (param sustain @default 0 @min 0 @max 1)
  (param glide @default 0 @min 0 @max 400 @unit ms)
  ; Sampler-style linear attack ramp (footwork 808s: ~300 ms). Zero
  ; preserves the identified instantaneous attack exactly.
  (param attack @default 0 @min 0 @max 1000 @unit ms)
  ; Post-everything sample-style fade-in (after the bank and tone): the
  ; static swell of the finished sound, vs `attack` which blooms INTO the
  ; bank drive. Zero = exact bypass.
  (param fade @default 0 @min 0 @max 1000 @unit ms)
  ; Crossfade time used only when a new trigger overlaps a playing hit. The
  ; first isolated trigger stays instantaneous (identified attack).
  (param retrigger_fade @default 5 @min 0.1 @max 50 @unit ms)
  (tuple (onepole-param sweep smooth_ms)
         (onepole-param sustain smooth_ms)
         (onepole-param glide smooth_ms)
         (onepole-param attack smooth_ms)
         (onepole-param fade smooth_ms)
         (onepole-param retrigger_fade smooth_ms)))

; -> (body_amp body_asymmetry click_freq click_decay noise_cutoff
;     noise_decay out_gain amp_curve body_harmonic)
(defmacro body-params (smooth_ms)
  (param body_amp @default 0.8939833 @min 0.05 @max 1.2)
  (param body_asymmetry @default 0.035932463 @min -0.6 @max 0.6)
  ; 909 body extras: quadratic amp-envelope curvature (learned) and the
  ; learned odd-harmonic (3rd/5th) content, both editable.
  (param amp_curve @default -3.3242447 @min -60 @max 0)
  (param body_harmonic @default -0.3666658 @min -2 @max 2)
  (param click_freq @default 637.39606 @min 200 @max 3000 @unit Hz)
  (param click_decay @default -453.86426 @min -2500 @max -100)
  (param noise_cutoff @default 16080.313 @min 500 @max 20000 @unit Hz)
  (param noise_decay @default -9.952022 @min -500 @max -0.001)
  (param out_gain @default 0.31624377 @min 0.1 @max 1.2)
  (tuple (onepole-param body_amp smooth_ms)
         (onepole-param body_asymmetry smooth_ms)
         (onepole-param click_freq smooth_ms)
         (onepole-param click_decay smooth_ms)
         (onepole-param noise_cutoff smooth_ms)
         (onepole-param (clip noise_decay -500 -0.001) smooth_ms)
         (onepole-param out_gain smooth_ms)
         (onepole-param amp_curve smooth_ms)
         (onepole-param body_harmonic smooth_ms)))

; ======================================================================
; trigger core: edge detect, two-slot alternation, first-hit bypass,
; equal-gain crossfade weights. -> (triggered trig_a trig_b gain_a gain_b)
; ======================================================================

; Exponential crossfade gain (from drums/synthid-909). first_hit bypasses
; the fade so an isolated initial hit keeps the exact identified attack.
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
  ; On an overlapping trigger, emit the previous weight for this exact
  ; sample and begin the transition in state for the next sample, keeping
  ; the gain contribution continuous at the retrigger boundary.
  (gswitch (gt first_hit 0.5) target previous_gain))

(defmacro trigger-core (trigger_in retrig_fade attack_ms)
  ; A long `attack` leaves nothing to mask the retrigger choke, and a 5 ms
  ; fade truncates a ~50 Hz cycle mid-wave (spectral splatter, heard as a
  ; tiny click). Stretch the choke with the attack — max(XFD, attack/10),
  ; capped at 40 ms — so instant attacks keep the exact learned 5 ms
  ; (masked by the transient) and slow swells choke over a full cycle.
  (def fade_eff (clip (max retrig_fade (* attack_ms 0.1)) 0.1 40))
  (def trigger_gate (gt trigger_in 0.5))
  (make-history trigger_h)
  (def previous_trigger (read-history trigger_h))
  (def triggered (max 0.0 (- trigger_gate previous_trigger)))
  (write-history trigger_h trigger_gate)

  (make-history selector_h)
  (def previous_selector (read-history selector_h))
  (def slot
    (gswitch (gt triggered 0.5) (- 1.0 previous_selector) previous_selector))
  (write-history selector_h slot)

  (make-history ever_triggered_h)
  (def was_triggered (read-history ever_triggered_h))
  (def first_hit (* triggered (lt was_triggered 0.5)))
  (write-history ever_triggered_h (gswitch (gt triggered 0.5) 1.0 was_triggered))

  (def trig_a (* triggered (lt slot 0.5)))
  (def trig_b (* triggered (gte slot 0.5)))
  (def target_a (gswitch (lt slot 0.5) 1.0 0.0))
  (def target_b (- 1.0 target_a))
  (def gain_a (retrigger-gain target_a first_hit fade_eff))
  (def gain_b (retrigger-gain target_b first_hit fade_eff))
  (tuple triggered trig_a trig_b gain_a gain_b))

; ======================================================================
; glide: shared one-pole pitch slide toward the played pitch; exact
; pass-through when glide time ~0 (voices then latch per hit as learned).
; ======================================================================

(defmacro glide-pitch (target glide_ms)
  (make-history g_h)
  (def coeff (exp (/ -1.0 (max 1.0 (* glide_ms 0.001 samplerate)))))
  (def prev (read-history g_h))
  (def glided (+ (* target (- 1 coeff)) (* prev coeff)))
  (def held (gswitch (gt glide_ms 0.1) glided target))
  (write-history g_h held)
  held)

; ======================================================================
; voice: one independent copy of the identified voice + layers. Macro
; expansion gives each retrigger slot its own clock, latches, noise
; sources, and filter state. Shaping values are the shared smoothed
; top-level signals, exactly like drums/synthid-909.
; ======================================================================

(defmacro id909-voice (voice_trigger input_pitch input_velocity)
  ; Resettable seconds-since-trigger clock: exactly t=0 on the trigger
  ; sample, then t=1/samplerate.
  (make-history time_h)
  (def previous_time (read-history time_h))
  (def t (gswitch (gt voice_trigger 0.5) 0.0 previous_time))
  (write-history time_h (+ t (/ 1.0 samplerate)))

  (make-history active_h)
  (def active (gswitch (gt voice_trigger 0.5) 1.0 (read-history active_h)))
  (write-history active_h active)

  ; Velocity belongs to this hit for its complete lifetime; later triggers
  ; may carry different velocities without changing the outgoing tail.
  (make-history velocity_h)
  (def previous_velocity (read-history velocity_h))
  (def hit_velocity
    (gswitch (gt voice_trigger 0.5)
             (clip input_velocity 0.0 1.0)
             previous_velocity))
  (write-history velocity_h hit_velocity)

  ; Pitch is latched for the same reason: an outgoing tail must not retune
  ; when the incoming sequencer event carries a different note. Glide mode
  ; tracks the shared slide instead; tune retunes continuously on purpose
  ; (it is the modulation surface).
  (make-history pitch_h)
  (def previous_pitch (read-history pitch_h))
  (def hit_pitch
    (gswitch (gt voice_trigger 0.5)
             (max input_pitch 1.0)
             previous_pitch))
  (write-history pitch_h hit_pitch)
  (def base_pitch (gswitch (gt glide_v 0.1) (max glided 1.0) hit_pitch))
  (def pitch_used (* base_pitch tune_ratio_s))

  ; Body sweep: the identified closed-form phase, accumulated one exact
  ; per-sample increment at a time. For a fixed pitch the sum telescopes to
  ; the closed form (verified against drums/synthid-909 in the audition
  ; harness); a moving pitch (tune modulation, glide) retunes with no phase
  ; discontinuity because each increment uses the current frequency.
  (def body_end pitch_used)
  (def body_start (* body_end start_ratio_s))
  (def pitch_rate (/ -6.9077553 (* (max sweep_v 50.0) 0.001)))
  (def dt (/ 1.0 samplerate))
  (def sweep_inc
    (+ (* body_end dt)
       (* (/ (- body_start body_end) pitch_rate)
          (exp (* pitch_rate t))
          (- (exp (* pitch_rate dt)) 1.0))))
  ; Wrapped to [0,1) so float32 accumulation stays precise over long tails
  ; (sin/cos are periodic in whole cycles; every integer-multiple partial
  ; of the correction bank too).
  (make-history sweep_h)
  (def sweep_phase
    (gswitch (gt voice_trigger 0.5) 0.0 (read-history sweep_h)))
  (write-history sweep_h (wrap (+ sweep_phase sweep_inc) 0 1))

  ; Classic exponential DSR (attack is the separate post-voice linear
  ; ramp): decay toward sustain while the gate is held, release toward 0
  ; after note-off, both T60 one-poles. At the defaults (sustain 0,
  ; release == decay) decay and release are the SAME curve, so note-off
  ; is seamless and this is exactly the learned one-shot exponential for
  ; any gate length. Sustain up + short release = gated bass, no mode.
  (def coef_d (exp (/ -6.9077553 (* (max decay_s 1.0) 0.001 samplerate))))
  (def coef_r (exp (/ -6.9077553 (* (max release_s 1.0) 0.001 samplerate))))
  (def sus (clip sustain_v 0 1))
  (make-history env_h)
  (def env_prev (read-history env_h))
  (def dsr_env
    (gswitch (gt voice_trigger 0.5) 1.0
             (gswitch (gt gate 0.5)
                      (+ sus (* (- env_prev sus) coef_d))
                      (* env_prev coef_r))))
  (write-history env_h dsr_env)
  (def body_envelope (* dsr_env (exp (* amp_curve_v t t))))

  (def body (* (sin (* sweep_phase twopi)) body_envelope body_amp_v))

  (def even_harmonic
    (* body_asym_v
       (sin (- (* sweep_phase 2.0 twopi) 0.62))
       body_envelope
       body_amp_v
       (exp (* -17.0 t))))

  ; Learned odd-harmonic (square-ish 3rd/5th) content.
  (def odd_harmonics
    (* body_harm_v
       (+ (* (sin (* sweep_phase 3.0 twopi)) (/ 1.0 9.0))
          (* (sin (* sweep_phase 5.0 twopi)) (/ 1.0 25.0)))
       body_envelope
       body_amp_v))

  ; The 40 backward-pruned learned correction partials (drums/synthid-909,
  ; verbatim). They ride sweep_phase and body_envelope, so tune/glide/tail
  ; carry the whole learned timbre with them.
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
    (* (sin (* click_freq_v t twopi))
       (exp (* click_decay_v t))
       click_amp_s))

  ; DGen scalar noise is [0,1); scale it to [-1,1) exactly as SynthID does.
  (def bipolar_noise (- (* (noise) 2.0) 1.0))
  (def filtered_noise (biquad bipolar_noise noise_cutoff_v 0.707 1.0 0.0))
  (def noise_burst
    (* filtered_noise
       (exp (* noise_decay_v t))
       noise_amp_s))

  (def mixed
    (+ body even_harmonic odd_harmonics harmonic_correction click noise_burst))
  ; 909 output stage: biased softsign (verbatim from drums/synthid-909).
  (def bias 0.05)
  (def shifted (+ (* mixed drive_s) bias))
  (def softsign (- (/ shifted (+ 1.0 (abs shifted)))
                   (/ bias (+ 1.0 (abs bias)))))
  (def learned_voice (* softsign out_gain_v))

  ; Sampler-style attack: a LINEAR amplitude ramp from exactly 0 at the
  ; trigger (click-free by construction) to full scale at `attack` ms —
  ; the footwork-808 swell, unlike the old T60 exponential which leapt
  ; toward full scale immediately. Exact bypass at 0.
  (def attack_envelope
    (gswitch (lt attack_v 0.5)
             1.0
             (clip (/ t (* (max attack_v 0.5) 0.001)) 0.0 1.0)))
  (* learned_voice attack_envelope hit_velocity active))

; ======================================================================
; voice mix + post chain
; ======================================================================

(defmacro voice-mix (va ga vb gb)
  (+ (* va ga) (* vb gb)))

; Final HP/LP tone pair + level, AFTER the bank so its screams, bleed and
; aliasing can be tamed. The filters always run (state stays warm) and are
; crossfaded out near the identity extremes: blend 0 at the defaults
; (exact dry pass-through), full filter by 35 Hz / 16000 Hz. A p-lock
; sweeping across the extreme therefore never hard-swaps signals.
(defmacro tone-stage (x lpf_hz hpf_hz lvl)
  (def hp_filtered (svf x (clip hpf_hz 20 500) 0.707 2))
  (def hp_blend (clip (* (- hpf_hz 20.0) 0.0667) 0 1))
  (def hp_out (mix x hp_filtered hp_blend))
  (def lp_filtered (svf hp_out (clip lpf_hz 200 18000) 0.707 0))
  (def lp_blend (clip (* (- 17500.0 lpf_hz) 0.000667) 0 1))
  (def lp_out (mix hp_out lp_filtered lp_blend))
  (* lp_out (clip lvl 0 1.5)))

; Post-everything linear fade-in, using the builtin Sampler's retrigger
; recipe (instruments/sampler.rs) adapted to a shared mono chain: a new
; trigger while the output is AUDIBLE first carries the envelope on from
; its current value through a 4 ms raised-cosine duck, then swells; a
; trigger into silence starts the ramp at 0 immediately (their
; last_env_amp test, approximated with a fast output-level follower, so
; the envelope only ever jumps when there is no signal to click).
(defmacro fade-stage (x triggered fade_ms)
  ; fast-attack / slow-release level follower of the input
  (make-history fd_folh)
  (def fd_mag (abs x))
  (def fd_fprev (read-history fd_folh))
  (def fd_fol (+ fd_fprev (* (gswitch (gt fd_mag fd_fprev) 0.05 0.0005)
                             (- fd_mag fd_fprev))))
  (write-history fd_folh fd_fol)

  ; seconds since trigger
  (make-history fd_th)
  (def fd_t (gswitch (gt triggered 0.5) 0.0 (read-history fd_th)))
  (write-history fd_th (+ fd_t (/ 1.0 samplerate)))

  ; duck start level: the envelope's value at the trigger. With a long
  ; `attack` the new voice starts from silence pre-bank, so a full-height
  ; duck cannot leak its transient — carry the envelope over with perfect
  ; continuity (no gain jump at all, the click-free path). Only for
  ; near-instant attacks is the duck gated by how loud the output
  ; actually was (silent -> 0, clean ramp; the 200x slope keeps any
  ; residual envelope jump far below audibility relative to the signal).
  (make-history fd_envh)
  (make-history fd_duckh)
  (def fd_prev_env (read-history fd_envh))
  (def fd_gate
    (clip (+ (* 200.0 fd_fol) (gswitch (gt attack_v 20.0) 1.0 0.0)) 0 1))
  (def fd_duck0
    (gswitch (gt triggered 0.5)
             (* fd_prev_env fd_gate)
             (read-history fd_duckh)))
  (write-history fd_duckh fd_duck0)

  ; raised-cosine 1 -> 0 over 24 ms (the sampler's retrigger_tail_gain
  ; shape, lengthened to cover a full cycle down to ~42 Hz: a duck faster
  ; than the content's period splatters spectrally and reads as a click,
  ; and unlike the sampler there is no instant new attack to mask it)
  (def fd_cg (* 0.5 (+ 1.0 (cos (* pi (clip (/ fd_t 0.024) 0 1))))))
  (def fd_ramp (clip (/ fd_t (* (max fade_ms 0.5) 0.001)) 0 1))
  (def fd_env (max fd_ramp (* fd_duck0 fd_cg)))
  (write-history fd_envh fd_env)
  (gswitch (lt fade_ms 0.5) x (* x fd_env)))

; ======================================================================
; BANK: the sc-filterbank core (Sherman FB2, content/effects/sc-filterbank)
; with its cutoff riding a per-trigger decay envelope — identical to
; factory/id808's bank. Serial F1(LP) -> F2(~BP through the /4 clock
; divider), van der Pol resonance, VCO slew + charge-injection thump,
; tube drive, clock-tracking reconstruction, shared compressing output.
; Exact bypass at bank 0 (the cores keep running so engaging is click-free).
; ======================================================================

; One switched-cap SVF core: input sampled on the tick, Chamberlin update
; gated to the tick, states held (ZOH) between ticks. Biased tanh on the
; bp state injection-locks the scream; amplitude-dependent damping (van
; der Pol) gives a hard self-osc threshold. (From sc-filterbank.)
(defmacro bank-svf (sig tick morph gcoef kbase)
  (make-history lp_h)
  (make-history bp_h)
  (def xs (latch sig tick))
  (def keff (+ kbase (* 1.2 (* (read-history bp_h) (read-history bp_h)))))
  (def hp (- xs (+ (read-history lp_h) (* keff (read-history bp_h)))))
  (def bpn (* 1.078 (- (tanh (+ (+ (read-history bp_h) (* gcoef hp)) 0.28)) (tanh 0.28))))
  (def lpn (+ (read-history lp_h) (* gcoef bpn)))
  (write-history bp_h (mix (read-history bp_h) bpn tick))
  (write-history lp_h (mix (read-history lp_h) lpn tick))
  (def lpw (clip (- 1 (* 2 morph)) 0 1))
  (def hpw (clip (- (* 2 morph) 1) 0 1))
  (+ (* (read-history lp_h) lpw)
     (+ (* (read-history bp_h) (- 1 (+ lpw hpw)))
        (* hp hpw))))

(defmacro bank-stage (sig triggered wet_a env_a freq_a res_a note_in)
  ; Defaults are the exact settings the id808 gesture was discovered with:
  ; freq 0.34 -> 0.03 (floor 0.03 + env 0.31), res 0.75, mode1 0.00,
  ; mode2 0.51, harm 5, crunch 0.00, ser 1.00, blend 0.50, drive 0.81.
  ; bank_freq (FLR) and bank_res (RES) are top-level @mod params, passed
  ; in as freq_a / res_a.
  (param bank_time @default 260 @min 20 @max 2000 @unit ms)
  (param bank_harm @default 5 @min 0 @max 7)
  (param bank_crunch @default 0 @min 0 @max 1)
  (param bank_drive @default 0.81 @min 0 @max 1)
  ; Keytrack MODE (default key): 1 shifts the whole sweep (floor, start,
  ; both resonances, and the clock — so the ZOH/aliasing artifacts too)
  ; with the note, in the log-cutoff domain, relative to the learned
  ; 46.92 Hz endpoint. At that reference pitch the two modes are
  ; identical. 0 = free (fixed frequencies). Follows tune and glide;
  ; intermediates blend.
  (param bank_track @default 1 @min 0 @max 1)
  ; Reconstruction filter (the thing after the chip that Sherman barely
  ; has): two cascaded one-poles tracking the CLOCK at 0.35*fclk — above
  ; the passband (cutoff = fclk/ratio, tone untouched) but below the ZOH
  ; image bands, so it eats the staircase aliasing wherever the sweep
  ; sits. 0 = raw hardware grit, 1 = fully reconstructed (default).
  (param bank_recon @default 1 @min 0 @max 1)

  (def wet_amt (clip wet_a 0 1))
  (def bk_hitp (max (latch (max note_in 1.0) triggered) 1.0))
  (def bk_note (* (gswitch (gt glide_v 0.1) (max glided 1.0) bk_hitp) tune_ratio_s))
  (def bk_key_off (* (clip bank_track 0 1) (/ (log (/ bk_note 46.922146)) 5.586)))
  ; input drive: the builtin Filterbank's drive circuit
  ; (effects/filterbank.rs §2) — dynamic-bias coupling-cap sag, +6 dB
  ; pre-emphasis @ 3 kHz, 0.55·tube + 0.45·diode asymmetric shaper (roar
  ; transfer bank), matched de-emphasis, 10 Hz DC blocker. The builtin's
  ; 4x oversampling is deliberately omitted: this bank aliases by design,
  ; and bank_recon is the cleanup control.
  (def gained (* sig (+ 1 (* bank_drive 24))))

  ; dynamic bias — a 2 ms / 80 ms follower
  ; of the driven signal shifts the operating point into the asymmetric
  ; curve, so transients bloom and sustained material sits down
  (make-history bk_biash)
  (def bmag (abs gained))
  (def bprev (read-history bk_biash))
  (def bcoef (gswitch (gt bmag bprev)
                      (- 1.0 (exp (/ -1.0 (* 0.002 samplerate))))
                      (- 1.0 (exp (/ -1.0 (* 0.080 samplerate))))))
  (def benv (+ bprev (* bcoef (- bmag bprev))))
  (write-history bk_biash benv)
  (def dbias (* 0.22 (tanh benv)))
  ; pre-emphasis: +6 dB above 3 kHz so the highs clip first
  (def ecoef (- 1.0 (exp (/ (* -2.0 pi 3000.0) samplerate))))
  (make-history bk_emph)
  (def emph_lp (+ (read-history bk_emph) (* ecoef (- gained (read-history bk_emph)))))
  (write-history bk_emph emph_lp)
  (def sh_in (+ gained (- gained emph_lp) dbias))
  ; 0.55 tube + 0.45 diode, unity small-signal slope (roar transfer bank)
  (def tube_u (max sh_in -2.4))
  (def sh_tube (tanh (+ tube_u (* 0.2 tube_u tube_u))))
  ; exp argument clamped at 0 so the unselected branch stays finite for
  ; negative inputs (gswitch evaluates both sides)
  (def dpos (gswitch (lt sh_in 0.35)
                     sh_in
                     (+ 0.35 (/ (- 1.0 (exp (* -3.0 (max (- sh_in 0.35) 0.0)))) 3.0))))
  (def sh_diode (gswitch (gte sh_in 0.0) dpos (* 1.2 (tanh (/ sh_in 1.2)))))
  (def shaped_drv (+ (* 0.55 sh_tube) (* 0.45 sh_diode)))
  ; matched de-emphasis (product ~ flat when clean), then 10 Hz DC block
  ; (the asymmetric curve + bias ride on an offset)
  (make-history bk_deemph)
  (def deemph_lp (+ (read-history bk_deemph) (* ecoef (- shaped_drv (read-history bk_deemph)))))
  (write-history bk_deemph deemph_lp)
  (def de_drv (- shaped_drv (* 0.5 (- shaped_drv deemph_lp))))
  (def dc_r (exp (/ (* -2.0 pi 10.0) samplerate)))
  (make-history bk_dcx)
  (make-history bk_dcy)
  (def dcy (+ (- de_drv (read-history bk_dcx)) (* dc_r (read-history bk_dcy))))
  (write-history bk_dcx de_drv)
  (write-history bk_dcy dcy)
  (def x dcy)
  ; input envelope (charge-injection bleed keying), ~10 ms follower
  (make-history bk_envh)
  (def bk_env (+ (read-history bk_envh) (* 0.003 (- (abs x) (read-history bk_envh)))))
  (write-history bk_envh bk_env)

  ; cutoff position: floor + per-trigger decay sweep (replaces the LFO)
  (def sweep_env (id-env triggered bank_time))
  (def fpos_target (clip (+ (clip freq_a 0 1) bk_key_off (* (clip env_a 0 1) sweep_env)) 0 1))
  ; VCO slew: the expo converter lags, asymmetrically (up faster than down)
  (make-history bk_fposh)
  (def fpos_diff (- fpos_target (read-history bk_fposh)))
  (def fpos (+ (read-history bk_fposh)
               (* (mix 0.0015 0.006 (> fpos_diff 0)) fpos_diff)))
  (write-history bk_fposh fpos)
  (def fc (* 30 (exp (* 5.586 fpos))))

  ; switched-cap clock: crunch morphs ratio 100:1 -> 25:1 (log)
  (def ratio (* 100 (exp (* bank_crunch (log 0.25)))))
  (def gcoef (* 2 (sin (/ pi ratio))))
  (def kbase (- (* 2.08 (- 1 (clip res_a 0 1))) 0.22))

  ; clock jitter, depth keyed to crunch
  (make-history bk_nzh)
  (def bk_nz (+ (read-history bk_nzh) (* 0.05 (- (noise) (read-history bk_nzh)))))
  (write-history bk_nzh bk_nz)
  (def fclk (clip (* (* fc ratio) (+ 1 (* (* 0.012 (+ 0.3 bank_crunch)) bk_nz)))
                  200 (* samplerate 0.99)))
  (def ph1 (phasor fclk))
  ; explicit wrap detector: ramp2trig misses wraps near the host rate
  (make-history bk_prevph)
  (def tick1 (< ph1 (read-history bk_prevph)))
  (write-history bk_prevph ph1)

  ; clock divider: F2's clock is F1's through the selected ratio
  ; (selector is 1-based; floor needs dgenlisp >= v0.1.6). The knob moves
  ; in 0.5 steps: halves land midway between adjacent tap ratios, which
  ; the subtract-N accumulator divides as happily as the named taps.
  (def harm_q (/ (round (* (clip bank_harm 0 7) 2)) 2))
  (def harm_i (floor harm_q))
  (def harm_f (- harm_q harm_i))
  (def div_a (selector (+ 1 harm_i) 1 1.2 1.5 2 3 4 5 7))
  (def div_b (selector (+ 1 (clip (+ harm_i 1) 0 7)) 1 1.2 1.5 2 3 4 5 7))
  (def divisor (mix div_a div_b harm_f))
  (make-history bk_divcnt)
  (def cnt (+ (read-history bk_divcnt) tick1))
  (def fire2 (>= cnt divisor))
  (write-history bk_divcnt (- cnt (* divisor fire2)))
  (def tick2 (* tick1 fire2))

  ; sweep thump: charge injection puts a moving DC offset into the loop
  (def thump (* 60 fpos_diff (mix 0.0015 0.006 (> fpos_diff 0))))
  (def xin (+ x thump))

  (def f1 (bank-svf xin tick1 0.0 gcoef kbase))
  ; serial: F1's resonance overdrives the stage feeding F2
  (def f2in (tanh (* 1.7 f1)))
  (def f2 (bank-svf f2in tick2 0.51 gcoef kbase))

  ; clock bleed as charge injection, rising as the clock falls audible.
  ; Deviation from the effect port: the hardware's constant 0.3 idle-bleed
  ; floor is removed — an instrument must go silent between hits, so the
  ; bleed is keyed entirely to the input envelope (1.9 keeps the same
  ; peak level the effect has at full program).
  (def bleed (* (* (* (* bank_crunch bank_crunch)
                      (* 0.02 (clip (- 1 (/ fclk 6000)) 0 1)))
                   (* 1.9 bk_env))
                (- (* 2 (< ph1 0.5)) 1)))

  ; shared output stage: envelope-coupled gain into ONE tanh (the scream
  ; eats headroom and the program ducks under it)
  (def pre (+ (* 0.5 (+ f1 f2)) bleed))
  (make-history bk_cmph)
  (def cmpa (abs pre))
  (write-history bk_cmph (+ (read-history bk_cmph)
                            (* (mix 0.0004 0.02 (> cmpa (read-history bk_cmph)))
                               (- cmpa (read-history bk_cmph)))))
  (def cmp (/ 1 (+ 1 (* 3.2 (read-history bk_cmph)))))
  (def wet (* 0.85 (tanh (* 1.7 (* pre cmp)))))

  ; clock-tracking reconstruction filter: two cascaded one-poles
  ; (12 dB/oct) at 0.35*fclk (see bank_recon above)
  (def rc_cut (clip (* fclk 0.35) 60 18000))
  (def rc_coef (exp (/ (* -2.0 pi rc_cut) samplerate)))
  (make-history bk_rch1)
  (def rc_1 (+ (* (- 1.0 rc_coef) wet) (* rc_coef (read-history bk_rch1))))
  (write-history bk_rch1 rc_1)
  (make-history bk_rch2)
  (def rc_2 (+ (* (- 1.0 rc_coef) rc_1) (* rc_coef (read-history bk_rch2))))
  (write-history bk_rch2 rc_2)
  (def wet_recon (mix wet rc_2 (clip bank_recon 0 1)))
  (mix sig wet_recon wet_amt))

; ======================================================================
; top level: section nodes only
; ======================================================================

(def (sweep_v sustain_v glide_v attack_v fade_v retrig_fade_v)
     (sweep-tail-params smoothing))
(def (body_amp_v body_asym_v click_freq_v click_decay_v
      noise_cutoff_v noise_decay_v out_gain_v
      amp_curve_v body_harm_v)
     (body-params smoothing))
; Smoothed host-modulatable signals shared by both voice slots.
(def tune_ratio_s (onepole-param (semi (mod tune)) smoothing))
(def start_ratio_s (onepole-param (mod start_ratio) smoothing))
(def decay_s (onepole-param (mod decay) smoothing))
(def release_s (onepole-param (mod release) smoothing))
(def drive_s (onepole-param (mod drive) smoothing))
(def click_amp_s (onepole-param (mod click_amp) smoothing))
(def noise_amp_s (onepole-param (mod noise_amp) smoothing))
(def lpf_s (onepole-param (mod lpf) smoothing))
(def hpf_s (onepole-param (mod hpf) smoothing))
(def level_s (onepole-param (mod level) smoothing))
(def bank_s (onepole-param (mod bank) smoothing))
(def bank_env_s (onepole-param (mod bank_env) smoothing))
(def bank_freq_s (onepole-param (mod bank_freq) smoothing))
(def bank_res_s (onepole-param (mod bank_res) smoothing))

(def (triggered trig_a trig_b gain_a gain_b)
     (trigger-core trigger retrig_fade_v attack_v))

(def glided (glide-pitch pitch glide_v))

(def voice_a (id909-voice trig_a pitch velocity))
(def voice_b (id909-voice trig_b pitch velocity))

(def mixed_voices (voice-mix voice_a gain_a voice_b gain_b))

(def banked (bank-stage mixed_voices triggered bank_s bank_env_s bank_freq_s bank_res_s pitch))

(def toned (tone-stage banked lpf_s hpf_s level_s))

(def faded (fade-stage toned triggered fade_v))

(out faded 1 @name audio)
