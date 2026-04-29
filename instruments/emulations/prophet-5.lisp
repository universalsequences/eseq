; Prophet-5 inspired polysynth
; Refactored for the shared modulation system:
; synth params define the base voice, Mod routes mod1..mod6 into tagged destinations.

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

(defmacro pulse_from_phase (phase width)
  (scale (lt phase width) 0 1 -1 1))

(defmacro smooth (sig amt)
  (make-history h)
  (def y (mix sig (read-history h) amt))
  (write-history h y))

(defmacro weighted_mix3 (a wa b wb c wc)
  (def total (+ wa wb wc))
  (/ (+ (* a wa) (* b wb) (* c wc)) (max 1 total)))

(param amp_attack_ms    @default 6    @min 1    @max 5000 @unit ms)
(param amp_decay_ms     @default 140  @min 1    @max 5000 @unit ms)
(param amp_sustain      @default 0.78 @min 0    @max 1)
(param amp_release_ms   @default 260  @min 1    @max 5000 @unit ms)

(param filt_attack_ms   @default 5    @min 1    @max 5000 @unit ms)
(param filt_decay_ms    @default 220  @min 1    @max 5000 @unit ms)
(param filt_sustain     @default 0.0  @min 0    @max 1)
(param filt_release_ms  @default 320  @min 1    @max 5000 @unit ms)

(param cutoff           @default 920   @min 30    @max 12000 @unit Hz @mod true @mod-mode additive)
(param resonance        @default 1.0   @min 0.3   @max 4.5 @mod true @mod-mode additive)
(param filter_env_amt   @default 1800  @min -5000 @max 5000 @unit Hz @mod true @mod-mode additive)
(param keytrack         @default 0.4   @min 0     @max 2)
(param filter_vel_amt   @default 0.35  @min 0     @max 1)
(param filter_drive     @default 1.25  @min 0.5   @max 4 @mod true @mod-mode additive)

(param osc_a_level      @default 0.8   @min 0     @max 1 @mod true @mod-mode additive)
(param osc_b_level      @default 0.75  @min 0     @max 1 @mod true @mod-mode additive)
(param noise_level      @default 0.02  @min 0     @max 0.4 @mod true @mod-mode additive)

(param osc_a_semi       @default 0     @min -24   @max 24 @unit st @mod true @mod-mode additive)
(param osc_b_semi       @default 0     @min -36   @max 36 @unit st @mod true @mod-mode additive)
(param osc_a_fine_cents @default 0     @min -50   @max 50 @unit cents @mod true @mod-mode additive)
(param osc_b_fine_cents @default 2     @min -50   @max 50 @unit cents @mod true @mod-mode additive)

(param osc_a_saw        @default 1     @min 0     @max 1 @mod true @mod-mode additive)
(param osc_a_pulse      @default 0     @min 0     @max 1 @mod true @mod-mode additive)
(param osc_b_saw        @default 1     @min 0     @max 1 @mod true @mod-mode additive)
(param osc_b_tri        @default 0     @min 0     @max 1 @mod true @mod-mode additive)
(param osc_b_pulse      @default 0     @min 0     @max 1 @mod true @mod-mode additive)

(param osc_a_pw         @default 0.5   @min 0.05  @max 0.95 @mod true @mod-mode additive)
(param osc_b_pw         @default 0.5   @min 0.05  @max 0.95 @mod true @mod-mode additive)
(param sync             @default 0     @min 0     @max 1 @mod true @mod-mode additive)

(param env_pitch_amt    @default 0     @min -24   @max 24 @unit st @mod true @mod-mode additive)
(param poly_pitch_amt   @default 0     @min -24   @max 24 @unit st @mod true @mod-mode additive)
(param env_pw_amt       @default 0     @min -0.45 @max 0.45 @mod true @mod-mode additive)
(param poly_pw_amt      @default 0     @min -0.45 @max 0.45 @mod true @mod-mode additive)
(param poly_filter_amt  @default 0     @min -4000 @max 4000 @unit Hz @mod true @mod-mode additive)

(param osc_b_keytrack   @default 1     @min 0     @max 1)
(param amp_vel_amt      @default 0.35  @min 0     @max 1)
(param vintage          @default 0.12  @min 0     @max 1)
(param gain             @default 0.16  @min 0     @max 1)

(def amp_env (adsr gate trigger amp_attack_ms amp_decay_ms amp_sustain amp_release_ms))
(def filt_env (adsr gate trigger filt_attack_ms filt_decay_ms filt_sustain filt_release_ms))

(def ln2 (log 2))

(def note_rand_a (latch (noise) trigger))
(def note_rand_b (latch (noise) trigger))
(def note_rand_pw (latch (noise) trigger))
(def note_rand_cutoff (latch (noise) trigger))

(def drift_a_cents (* note_rand_a vintage 5))
(def drift_b_cents (* note_rand_b vintage 5))
(def drift_pw (* note_rand_pw vintage 0.04))
(def drift_cutoff (* note_rand_cutoff vintage 180))

(def osc_b_pitch_semi (+ (mod osc_b_semi) (/ (+ (mod osc_b_fine_cents) drift_b_cents) 100)))
(def osc_b_ratio (exp (* ln2 (/ osc_b_pitch_semi 12))))
(def osc_b_freq (* (mix 440.0 pitch osc_b_keytrack) osc_b_ratio))

(def osc_b_phase (phasor osc_b_freq))
(make-history osc_b_phase_hist)
(def osc_b_phase_prev (read-history osc_b_phase_hist))
(def osc_b_wrap (* (gt (mod sync) 0.5) (lt osc_b_phase osc_b_phase_prev)))
(write-history osc_b_phase_hist osc_b_phase)

(def osc_b_pw_ctl (clip (+ (mod osc_b_pw) drift_pw) 0.04 0.96))
(def osc_b_saw_sig (scale osc_b_phase 0 1 -1 1))
(def osc_b_tri_sig (triangle osc_b_phase))
(def osc_b_pulse_sig (pulse_from_phase osc_b_phase osc_b_pw_ctl))
(def osc_b_mix
  (weighted_mix3
    osc_b_saw_sig (clip (mod osc_b_saw) 0 1)
    osc_b_tri_sig (clip (mod osc_b_tri) 0 1)
    osc_b_pulse_sig (clip (mod osc_b_pulse) 0 1)))
(def osc_b_filter_mod (smooth osc_b_mix 0.995))

(def osc_a_pitch_mod (+ (* filt_env (mod env_pitch_amt))
                        (* osc_b_mix (mod poly_pitch_amt))))
(def osc_a_pitch_semi (+ (mod osc_a_semi) (/ (+ (mod osc_a_fine_cents) drift_a_cents) 100) osc_a_pitch_mod))
(def osc_a_ratio (exp (* ln2 (/ osc_a_pitch_semi 12))))
(def osc_a_freq (* pitch osc_a_ratio))
(def osc_a_phase (phasor osc_a_freq osc_b_wrap))
(def osc_a_pw_ctl
  (clip (+ (mod osc_a_pw)
           (* filt_env (mod env_pw_amt))
           (* osc_b_mix (mod poly_pw_amt))
           drift_pw)
        0.04 0.96))
(def osc_a_saw_sig (scale osc_a_phase 0 1 -1 1))
(def osc_a_pulse_sig (pulse_from_phase osc_a_phase osc_a_pw_ctl))
(def osc_a_mix
  (weighted_mix3
    osc_a_saw_sig (clip (mod osc_a_saw) 0 1)
    osc_a_pulse_sig (clip (mod osc_a_pulse) 0 1)
    0 0))

(def mixer (+ (* osc_a_mix (clip (mod osc_a_level) 0 1))
              (* osc_b_mix (clip (mod osc_b_level) 0 1))
              (* (noise) (clip (mod noise_level) 0 0.4))))
(def driven (tanh (* mixer (mod filter_drive))))

(def filter_env_scaled (* filt_env (* (mod filter_env_amt) (+ (- 1 filter_vel_amt) (* filter_vel_amt velocity)))))
(def filter_mod (+ (mod cutoff)
                   (* pitch keytrack)
                   filter_env_scaled
                   (* osc_b_filter_mod (mod poly_filter_amt))
                   drift_cutoff))
(def safe_filter_cutoff (min 10000 (max 30 filter_mod)))
(def safe_resonance (min 3.2 (max 0.35 (mod resonance))))

(def filter_stage1 (biquad driven safe_filter_cutoff safe_resonance 1 0))
(def filter_stage2
  (biquad (tanh (* filter_stage1 (+ 0.6 (* 0.4 (mod filter_drive)))))
          safe_filter_cutoff
          safe_resonance
          1
          0))

(def amp_velocity (+ (- 1 amp_vel_amt) (* amp_vel_amt velocity)))
(out (* (tanh (* filter_stage2 1.15)) amp_env amp_velocity gain) 1 @name audio)
