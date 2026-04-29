; Elektron Monomachine-inspired SID family
; Approximates the Monomachine's SID machine with three digital oscillators,
; sync/ring interactions, quantized output and multimode filtering.

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

(defmacro semi_ratio (semi)
  (exp (/ (* (log 2) semi) 12)))

(defmacro crush (sig levels)
  (def safe_levels (max 4 levels))
  (def norm (* (+ sig 1) 0.5))
  (def stepped (/ (floor (* norm safe_levels)) safe_levels))
  (- (* stepped 2) 1))

(defmacro smooth (sig amt)
  (make-history h)
  (def y (mix sig (read-history h) amt))
  (write-history h y))

(param amp_attack_ms   @default 2    @min 1     @max 5000 @unit ms)
(param amp_decay_ms    @default 90   @min 1     @max 5000 @unit ms)
(param amp_sustain     @default 0.72 @min 0     @max 1)
(param amp_release_ms  @default 110  @min 1     @max 5000 @unit ms)

(param cutoff          @default 2800 @min 40    @max 12000 @unit Hz @mod true @mod-mode additive)
(param resonance       @default 1.2  @min 0.35  @max 3.8)
(param filter_mode     @default 0.0  @min 0     @max 2)
(param keytrack        @default 0.18 @min 0     @max 2)

(param osc1_semi       @default 0    @min -24   @max 24 @unit st)
(param osc2_semi       @default 12   @min -24   @max 24 @unit st @mod true @mod-mode additive)
(param osc3_semi       @default -12  @min -36   @max 24 @unit st @mod true @mod-mode additive)
(param osc2_detune     @default 3    @min -25   @max 25 @unit cents @mod true @mod-mode additive)
(param osc3_detune     @default -2   @min -25   @max 25 @unit cents @mod true @mod-mode additive)

(param tri_mix         @default 0.3  @min 0     @max 1)
(param saw_mix         @default 0.75 @min 0     @max 1)
(param pulse_mix       @default 0.55 @min 0     @max 1)
(param noise_mix       @default 0.0  @min 0     @max 1)
(param pulse_width     @default 0.42 @min 0.04  @max 0.96 @mod true @mod-mode additive)

(param osc1_level      @default 0.9  @min 0     @max 1)
(param osc2_level      @default 0.5  @min 0     @max 1)
(param osc3_level      @default 0.35 @min 0     @max 1)
(param sync_amt        @default 0.0  @min 0     @max 1 @mod true @mod-mode additive)
(param ring_amt        @default 0.0  @min 0     @max 1 @mod true @mod-mode additive)

(param bit_depth       @default 8    @min 4     @max 14)
(param drive           @default 1.4  @min 0.5   @max 4)
(param glitch_rate     @default 0.0  @min 0     @max 60 @unit Hz)
(param glitch_amt      @default 0.0  @min 0     @max 1 @mod true @mod-mode additive)
(param fold_amt        @default 0.0  @min 0     @max 2 @mod true @mod-mode additive)
(param filter_fm       @default 0.0  @min 0     @max 6000 @unit Hz @mod true @mod-mode additive)
(param buzz            @default 0.0  @min 0     @max 1 @mod true @mod-mode additive)
(param gain            @default 0.14 @min 0     @max 1)

(def amp_env (adsr gate trigger amp_attack_ms amp_decay_ms amp_sustain amp_release_ms))
(def ln2 (log 2))

(def osc1_ratio (semi_ratio osc1_semi))
(def osc2_ratio (semi_ratio (+ (mod osc2_semi) (/ (mod osc2_detune) 100))))
(def osc3_ratio (semi_ratio (+ (mod osc3_semi) (/ (mod osc3_detune) 100))))

(def ph2 (phasor (* pitch osc2_ratio)))
(make-history ph2_hist)
(def ph2_prev (read-history ph2_hist))
(def ph2_wrap (lt ph2 ph2_prev))
(write-history ph2_hist ph2)

(def ph1 (phasor (* pitch osc1_ratio) (* ph2_wrap (clip (mod sync_amt) 0 1))))
(def ph3 (phasor (* pitch osc3_ratio)))

(def tri1 (triangle ph1))
(def saw1 (scale ph1 0 1 -1 1))
(def pulse1 (pulse_from_phase ph1 (clip (mod pulse_width) 0.04 0.96)))
(def tri2 (triangle ph2))
(def saw2 (scale ph2 0 1 -1 1))
(def pulse2 (pulse_from_phase ph2 (clip (mod pulse_width) 0.04 0.96)))
(def tri3 (triangle ph3))
(def saw3 (scale ph3 0 1 -1 1))
(def pulse3 (pulse_from_phase ph3 (clip (mod pulse_width) 0.04 0.96)))

(def wave1 (+ (* tri1 tri_mix) (* saw1 saw_mix) (* pulse1 pulse_mix) (* (noise) noise_mix)))
(def wave2 (+ (* tri2 tri_mix) (* saw2 saw_mix) (* pulse2 pulse_mix)))
(def wave3 (+ (* tri3 tri_mix) (* saw3 saw_mix) (* pulse3 pulse_mix)))
(def glitch_trig (lt (phasor (+ 0.01 glitch_rate)) 0.001))
(def glitch_noise (latch (noise) glitch_trig))

(def ring (* wave1 wave3))
(def xorish (* (sign wave1) (sign wave2) (sign wave3)))
(def sid_core (+ (* wave1 osc1_level)
                 (* wave2 osc2_level)
                 (* wave3 osc3_level)
                 (* ring (clip (mod ring_amt) 0 1))
                 (* xorish (clip (mod buzz) 0 1) 0.65)
                 (* glitch_noise (clip (mod glitch_amt) 0 1) 0.35)))
(def quantized (crush sid_core (exp (* ln2 bit_depth))))
(def fold_ctl (clip (mod fold_amt) 0 2))
(def folded (- (* 2 (tanh (* quantized (+ 1 (* fold_ctl 3))))) quantized))
(def driven (tanh (* (+ quantized (* folded fold_ctl)) drive)))

(def smooth_filter_mod (tanh (* (smooth sid_core 0.992) 0.85)))
(def smooth_glitch_mod (tanh (* (smooth glitch_noise 0.996) 0.65)))
(def safe_cutoff_base (max 60 cutoff))
(def safe_keytrack (max 0 (* pitch keytrack)))
(def raw_cut_mod (+ (* smooth_filter_mod (min (mod filter_fm) 1800))
                    (* smooth_glitch_mod (clip (mod glitch_amt) 0 1) 320)))
(def safe_cut_mod (max (- 60 (+ safe_cutoff_base safe_keytrack)) raw_cut_mod))
(def safe_resonance (min 1.35 (max 0.35 resonance)))
(def sid_cutoff
  (min 11000
       (max 60
            (+ (mod cutoff)
               safe_keytrack
               safe_cut_mod))))
(def lp (biquad driven sid_cutoff safe_resonance 1 0))
(def bp (biquad driven sid_cutoff safe_resonance 1 2))
(def hp (biquad driven sid_cutoff safe_resonance 1 1))
(def lp_bp (mix lp bp (clip filter_mode 0 1)))
(def mode_sel (clip (- filter_mode 1) 0 1))
(def filtered (mix lp_bp hp mode_sel))

(out (* filtered amp_env velocity gain) 1 @name audio)
