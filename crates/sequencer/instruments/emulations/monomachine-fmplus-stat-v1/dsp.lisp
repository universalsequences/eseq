; Elektron Monomachine FM+STATIC-inspired v1
; Three-operator FM: two listed-frequency modulators into one carrier.

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

(defmacro semi_ratio (semi)
  (exp (/ (* (log 2) semi) 12)))

(defmacro listed_ratio (idx)
  (selector (clip (floor idx) 1 32)
    0.015625 0.03125 0.0625 0.09375
    0.125 0.1875 0.25 0.3125
    0.375 0.4375 0.5 0.625
    0.75 0.875 1.0 1.25
    1.5 1.75 2.0 2.5
    3.0 3.5 4.0 5.0
    6.0 7.0 8.0 10.0
    12.0 16.0 24.0 32.0))

(param amp_attack_ms     @default 4    @min 1     @max 5000 @unit ms)
(param amp_decay_ms      @default 360  @min 1     @max 5000 @unit ms)
(param amp_sustain       @default 0.78 @min 0     @max 1)
(param amp_release_ms    @default 180  @min 1     @max 5000 @unit ms)

(param op1_attack_ms     @default 1    @min 1     @max 5000 @unit ms)
(param op1_decay_ms      @default 740  @min 1     @max 5000 @unit ms)
(param op1_sustain       @default 0.16 @min 0     @max 1)
(param op1_release_ms    @default 90   @min 1     @max 5000 @unit ms)

(param filter_attack_ms  @default 2    @min 1     @max 5000 @unit ms)
(param filter_decay_ms   @default 300  @min 1     @max 5000 @unit ms)
(param filter_sustain    @default 0.22 @min 0     @max 1)
(param filter_release_ms @default 160  @min 1     @max 5000 @unit ms)

(param op1_frq           @default 15   @min 1     @max 32 @mod true @mod-mode additive)
(param op1_fin           @default 0    @min -100  @max 100 @unit cents @mod true @mod-mode additive)
(param op1_fb            @default 0.10 @min 0     @max 1 @mod true @mod-mode additive)
(param op1_env           @default 0.42 @min 0     @max 1 @mod true @mod-mode additive)

(param op2_frq           @default 19   @min 1     @max 32 @mod true @mod-mode additive)
(param op2_vol           @default 0.26 @min 0     @max 1 @mod true @mod-mode additive)
(param tone              @default 0.48 @min 0     @max 1 @mod true @mod-mode additive)
(param tune_cents        @default 0    @min -100  @max 100 @unit cents @mod true @mod-mode additive)

(param cutoff            @default 7800 @min 80    @max 12000 @unit Hz @mod true @mod-mode additive)
(param resonance         @default 0.707 @min 0.5  @max 2.5 @mod true @mod-mode additive)
(param keytrack          @default 0.12 @min 0     @max 2)
(param filter_env_amt    @default 900  @min -8000 @max 8000 @unit Hz @mod true @mod-mode additive)
(param drive             @default 1.0  @min 0.5   @max 8 @mod true @mod-mode additive)
(param gain              @default 0.18 @min 0     @max 1 @mod true @mod-mode additive)

(def amp_env (adsr gate trigger amp_attack_ms amp_decay_ms amp_sustain amp_release_ms))
(def op1_env_shape (adsr gate trigger op1_attack_ms op1_decay_ms op1_sustain op1_release_ms))
(def filt_env (adsr gate trigger filter_attack_ms filter_decay_ms filter_sustain filter_release_ms))
(def tone_amt (clip (mod tone) 0 1))
(def tuned_pitch (* pitch (semi_ratio (/ (mod tune_cents) 100))))

(def op1_amt (clip (mod op1_env) 0 1))
(def op2_amt (clip (mod op2_vol) 0 1))
(def op1_freq (* tuned_pitch (listed_ratio (mod op1_frq)) (semi_ratio (/ (mod op1_fin) 100))))
(def op2_freq (* tuned_pitch (listed_ratio (mod op2_frq)) (+ 1 (* 0.004 tone_amt))))
(def carrier_freq (* tuned_pitch (+ 1 (* 0.0015 tone_amt))))

(make-history op1_hist)
(make-history op2_hist)
(make-history car_hist)
(def op1_prev (read-history op1_hist))
(def op2_prev (read-history op2_hist))
(def car_prev (read-history car_hist))

(def op1_env_sig (* op1_env_shape op1_amt))
(def op2_env_sig (adsr gate trigger 1 (+ 18 (* 1100 op2_amt op2_amt)) (* op2_amt op2_amt 0.42) 75))
(def op1_fb_amt (* (clip (mod op1_fb) 0 1) (+ 0.35 (* 2.4 tone_amt))))
(def op2_fb_amt (* (max 0 (- op2_amt 0.58)) (+ 1.0 (* 2.2 tone_amt))))

(def op1_phase (phasor op1_freq))
(def op2_phase (phasor op2_freq))
(def car_phase (phasor carrier_freq))
(def op1 (sin (+ (* twopi op1_phase) (* op1_prev op1_fb_amt))))
(def op2_base (sin (+ (* twopi op2_phase) (* op2_prev op2_fb_amt) (* car_prev op2_fb_amt 0.28))))
(def op2 (* op2_base (+ 0.55 (* 0.45 (tanh (* op2_base op2_fb_amt))))))
(write-history op1_hist op1)
(write-history op2_hist op2)

(def idx1 (* op1_env_sig (+ 0.15 (* 12.0 op1_amt)) (+ 0.55 (* 1.65 tone_amt))))
(def idx2 (* op2_env_sig (+ 0.05 (* 9.0 op2_amt)) (+ 0.65 (* 1.35 tone_amt))))
(def cross_amt (* tone_amt op1_amt op2_amt 2.0))
(def carrier (sin (+ (* twopi car_phase) (* op1 idx1) (* op2 idx2) (* op1 op2 cross_amt))))
(write-history car_hist carrier)

(def bright (* carrier (+ 0.92 (* 0.08 (sin (+ (* twopi car_phase) (* op1 idx1 1.8)))))))
(def driven (tanh (* bright (clip (mod drive) 0.5 8))))
(def tone_cut (+ (mod cutoff) (* tuned_pitch keytrack) (* filt_env (mod filter_env_amt)) (* tone_amt 2600)))
(def filtered (biquad driven (clip tone_cut 80 12000) (clip (mod resonance) 0.5 2.5) 1 0))
(out (* filtered amp_env velocity (clip (mod gain) 0 1)) 1 @name audio)
