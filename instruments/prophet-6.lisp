; Sequential Prophet-6 inspired poly analog synth
; 2 VCO-style oscillators + sub + noise, Curtis-style 4-pole LP with feedback drive,
; poly-mod style modulation routing via sequencer modulators, and vintage voice drift.

(def gate     (in 1  @name gate))
(def pitch    (in 2  @name pitch))
(def velocity (in 3  @name velocity))
(def trigger  (in 4  @name trigger))
(def mod1     (in 5  @name mod1 @modulator 1))
(def mod2     (in 6  @name mod2 @modulator 2))
(def mod3     (in 7  @name mod3 @modulator 3))
(def mod4     (in 8  @name mod4 @modulator 4))
(def mod5     (in 9  @name mod5 @modulator 5))
(def mod6     (in 10 @name mod6 @modulator 6))

(defmacro semi_ratio (semi)
  (exp (/ (* (log 2) semi) 12)))

(defmacro pulse_from_phase (phase width)
  (scale (lt phase width) 0 1 -1 1))

(defmacro saw_from_phase (phase)
  (scale phase 0 1 -1 1))

; --- Amp envelope ---
(param amp_attack_ms    @default 6    @min 1    @max 5000 @unit ms)
(param amp_decay_ms     @default 280  @min 1    @max 5000 @unit ms)
(param amp_sustain      @default 0.78 @min 0    @max 1)
(param amp_release_ms   @default 220  @min 1    @max 5000 @unit ms)

; --- Filter envelope ---
(param filt_attack_ms   @default 5    @min 1    @max 5000 @unit ms)
(param filt_decay_ms    @default 340  @min 1    @max 5000 @unit ms)
(param filt_sustain     @default 0.0  @min 0    @max 1)
(param filt_release_ms  @default 260  @min 1    @max 5000 @unit ms)

; --- Oscillators / mixer ---
(param osc_a_shape      @default 0.0  @min 0    @max 1   @mod true @mod-mode additive) ; 0=saw 1=pulse
(param osc_b_shape      @default 0.18 @min 0    @max 1   @mod true @mod-mode additive)
(param pulse_width      @default 0.5  @min 0.05 @max 0.95 @mod true @mod-mode additive)
(param osc_b_semi       @default 0    @min -24  @max 24  @unit st @mod true @mod-mode semitone)
(param osc_b_detune     @default 3    @min -30  @max 30  @unit cents @mod true @mod-mode additive)
(param osc_a_level      @default 0.72 @min 0    @max 1   @mod true @mod-mode additive)
(param osc_b_level      @default 0.68 @min 0    @max 1   @mod true @mod-mode additive)
(param sub_level        @default 0.18 @min 0    @max 1   @mod true @mod-mode additive)
(param noise_level      @default 0.015 @min 0   @max 0.5 @mod true @mod-mode additive)
(param sync_amount      @default 0.0  @min 0    @max 1)

; --- Filter / character ---
(param cutoff           @default 1400 @min 30   @max 12000 @unit Hz @mod true @mod-mode additive)
(param resonance        @default 0.95 @min 0.2  @max 4.4   @mod true @mod-mode additive)
(param filter_env_amt   @default 2600 @min -6000 @max 6000 @unit Hz @mod true @mod-mode additive)
(param keytrack         @default 0.55 @min 0    @max 2)
(param filter_vel_amt   @default 0.28 @min 0    @max 1)
(param filter_drive     @default 1.55 @min 0.5  @max 4.5   @mod true @mod-mode additive)
(param filter_feedback  @default 0.22 @min 0    @max 0.75)
(param resonance_loss   @default 0.18 @min 0    @max 0.6)

; --- Poly-mod / motion ---
(param poly_pwm_amt     @default 0.0  @min -0.45 @max 0.45 @mod true @mod-mode additive)
(param poly_cutoff_amt  @default 0.0  @min -3000 @max 3000 @unit Hz @mod true @mod-mode additive)
(param lfo_rate         @default 3.8  @min 0.01 @max 30    @unit Hz)
(param lfo_to_pw        @default 0.0  @min -0.45 @max 0.45 @mod true @mod-mode additive)
(param lfo_to_cutoff    @default 0.0  @min -2500 @max 2500 @unit Hz @mod true @mod-mode additive)
(param lfo_to_pitch     @default 0.0  @min -0.12 @max 0.12 @mod true @mod-mode additive)

; --- Vintage / output ---
(param vintage          @default 0.14 @min 0    @max 1)
(param amp_vel_amt      @default 0.32 @min 0    @max 1)
(param gain             @default 0.11 @min 0    @max 1)

; --- Envelopes and note variation ---
(def amp_env   (adsr gate trigger amp_attack_ms amp_decay_ms amp_sustain amp_release_ms))
(def filt_env  (adsr gate trigger filt_attack_ms filt_decay_ms filt_sustain filt_release_ms))
(def lfo       (sin (* twopi (phasor lfo_rate))))
(def drift_a   (* (latch (noise) trigger) vintage 3.5))
(def drift_b   (* (latch (noise) trigger) vintage 3.5))
(def drift_pw  (* (latch (noise) trigger) vintage 0.035))
(def drift_cut (* (latch (noise) trigger) vintage 110))

; --- Oscillator frequencies ---
(def pitch_a   (* pitch (semi_ratio (+ (/ drift_a 100.0) (* lfo (mod lfo_to_pitch) 12.0)))))
(def pitch_b   (* pitch (semi_ratio (+ osc_b_semi (/ (+ (mod osc_b_detune) drift_b) 100.0) (* lfo (mod lfo_to_pitch) 12.0)))))

; Osc A is master for sync, osc B slave for Prophet-style sync sweeps
(def ph_a      (phasor pitch_a))
(make-history ph_a_hist)
(def ph_a_prev (read-history ph_a_hist))
(def ph_a_wrap (lt ph_a ph_a_prev))
(write-history ph_a_hist ph_a)
(def sync_trig (* (gt sync_amount 0.5) ph_a_wrap))
(def ph_b      (phasor pitch_b sync_trig))
(def ph_sub    (phasor (* pitch_a 0.5)))

(def raw_pw    (+ (mod pulse_width)
                  drift_pw
                  (* lfo (mod lfo_to_pw))
                  (mod poly_pwm_amt)))
(def pw        (clip raw_pw 0.05 0.95))

(def osc_a     (mix (saw_from_phase ph_a) (pulse_from_phase ph_a pw) (clip (mod osc_a_shape) 0 1)))
(def osc_b     (mix (saw_from_phase ph_b) (pulse_from_phase ph_b pw) (clip (mod osc_b_shape) 0 1)))
(def sub_osc   (pulse_from_phase ph_sub 0.5))

(def mixer (+ (* osc_a   (clip (mod osc_a_level) 0 1))
              (* osc_b   (clip (mod osc_b_level) 0 1))
              (* sub_osc (clip (mod sub_level) 0 1))
              (* (noise) (clip (mod noise_level) 0 0.5))))

; --- Curtis-style 24 dB LP approximation ---
; Slight level-dependent drive, feedback, and resonance bass loss compensation
(def filter_vel  (+ (- 1 filter_vel_amt) (* filter_vel_amt velocity)))
(def cutoff_hz   (clip (+ (mod cutoff)
                          (* pitch keytrack)
                          (* filt_env (mod filter_env_amt) filter_vel)
                          (* lfo (mod lfo_to_cutoff))
                          (mod poly_cutoff_amt)
                          drift_cut)
                       30 11000))
(def res         (clip (mod resonance) 0.2 4.4))

(make-history fb_hist)
(def fb_prev     (read-history fb_hist))
(def pre_emph    (+ mixer (* fb_prev filter_feedback res 0.22)))
(def driven1     (tanh (* pre_emph (mod filter_drive))))
(def lp1         (biquad driven1 cutoff_hz res 1 0))
(def lp2_in      (tanh (* lp1 1.08)))
(def lp2         (biquad lp2_in (* cutoff_hz 0.99) (* res 0.9) 1 0))
(write-history fb_hist lp2)

(def bass_makeup (+ 1.0 (* resonance_loss (clip (- res 1.0) 0 3.4))))
(def filtered    (tanh (* lp2 bass_makeup 1.18)))

(def amp_vel     (+ (- 1 amp_vel_amt) (* amp_vel_amt velocity)))
(out (* filtered amp_env amp_vel gain) 1 @name audio)
