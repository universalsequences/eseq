; Elektron Monomachine FM+PARALLEL-inspired v1
; Three parallel FM+ blocks, each with listed-frequency modulator and envelope.

(def gate (in 1 @name gate))
(def pitch (in 2 @name pitch))
(def velocity (in 3 @name velocity))
(def trigger (in 4 @name trigger))
(def mod1 (in 5 @name mod1 @modulator 1))
(def mod2 (in 6 @name mod2 @modulator 2))
(def mod3 (in 7 @name mod3 @modulator 3))
(def mod4 (in 8 @name mod4 @modulator 4))

(def waves (tensor @shape [512 64] @file "waves/user-bank.json"))

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

(defmacro fm_op (mod_phase wave_idx wave_mix)
  (mix (sin (* twopi mod_phase))
       (wavetable-read waves (clip (- wave_idx 1) 0 63) mod_phase)
       (clip wave_mix 0 1)))

(defmacro fm_car (phase wave_idx wave_mix)
  (mix (sin (* twopi phase))
       (wavetable-read waves (clip (- wave_idx 1) 0 63) phase)
       (clip wave_mix 0 1)))

(defmacro fm_block (car_phase mod_phase env amount tone prev_fb fb_scale phase_offset wave_idx wave_mix car_wave_idx car_wave_mix)
  (def fb_amt (* amount (+ 0.25 (* tone fb_scale))))
  (def modulator (fm_op (wrap (+ mod_phase (* prev_fb fb_amt 0.159154943)) 0 1) wave_idx wave_mix))
  (def idx (* env (+ 0.1 (* amount 10.5)) (+ 0.55 (* tone 1.7))))
  (fm_car (wrap (+ car_phase phase_offset (* modulator idx 0.159154943)) 0 1) car_wave_idx car_wave_mix))

(param amp_attack_ms     @default 4    @min 1     @max 5000 @unit ms)
(param amp_decay_ms      @default 360  @min 1     @max 5000 @unit ms)
(param amp_sustain       @default 0.76 @min 0     @max 1)
(param amp_release_ms    @default 220  @min 1     @max 5000 @unit ms)

(param op1_attack_ms     @default 1    @min 1     @max 5000 @unit ms)
(param op1_decay_ms      @default 520  @min 1     @max 5000 @unit ms)
(param op1_sustain       @default 0.16 @min 0     @max 1)
(param op1_release_ms    @default 85   @min 1     @max 5000 @unit ms)

(param op2_attack_ms     @default 1    @min 1     @max 5000 @unit ms)
(param op2_decay_ms      @default 720  @min 1     @max 5000 @unit ms)
(param op2_sustain       @default 0.10 @min 0     @max 1)
(param op2_release_ms    @default 90   @min 1     @max 5000 @unit ms)

(param op3_attack_ms     @default 1    @min 1     @max 5000 @unit ms)
(param op3_decay_ms      @default 980  @min 1     @max 5000 @unit ms)
(param op3_sustain       @default 0.06 @min 0     @max 1)
(param op3_release_ms    @default 120  @min 1     @max 5000 @unit ms)

(param filter_attack_ms  @default 3    @min 1     @max 5000 @unit ms)
(param filter_decay_ms   @default 360  @min 1     @max 5000 @unit ms)
(param filter_sustain    @default 0.18 @min 0     @max 1)
(param filter_release_ms @default 180  @min 1     @max 5000 @unit ms)

(param op1_frq           @default 15   @min 1     @max 32 @mod true @mod-mode additive)
(param op1_env           @default 0.52 @min 0     @max 1 @mod true @mod-mode additive)
(param op1_wave          @default 15   @min 1     @max 64 @mod true @mod-mode additive)
(param op1_mix           @default 0.0  @min 0     @max 1 @mod true @mod-mode additive)
(param op2_frq           @default 19   @min 1     @max 32 @mod true @mod-mode additive)
(param op2_env           @default 0.38 @min 0     @max 1 @mod true @mod-mode additive)
(param op2_wave          @default 32   @min 1     @max 64 @mod true @mod-mode additive)
(param op2_mix           @default 0.0  @min 0     @max 1 @mod true @mod-mode additive)
(param op3_frq           @default 23   @min 1     @max 32 @mod true @mod-mode additive)
(param op3_env           @default 0.26 @min 0     @max 1 @mod true @mod-mode additive)
(param op3_wave          @default 47   @min 1     @max 64 @mod true @mod-mode additive)
(param op3_mix           @default 0.0  @min 0     @max 1 @mod true @mod-mode additive)
(param car_wave          @default 1    @min 1     @max 64 @mod true @mod-mode additive)
(param car_mix           @default 0.0  @min 0     @max 1 @mod true @mod-mode additive)
(param tone              @default 0.54 @min 0     @max 1 @mod true @mod-mode additive)
(param tune_cents        @default 0    @min -100  @max 100 @unit cents @mod true @mod-mode additive)

(param cutoff            @default 8200 @min 80    @max 12000 @unit Hz @mod true @mod-mode additive)
(param resonance         @default 0.707 @min 0.5  @max 2.5 @mod true @mod-mode additive)
(param keytrack          @default 0.10 @min 0     @max 2)
(param filter_env_amt    @default 1200 @min -8000 @max 8000 @unit Hz @mod true @mod-mode additive)
(param drive             @default 1.0  @min 0.5   @max 8 @mod true @mod-mode additive)
(param gain              @default 0.16 @min 0     @max 1 @mod true @mod-mode additive)

(def amp_env (adsr gate trigger amp_attack_ms amp_decay_ms amp_sustain amp_release_ms))
(def op1_env_shape (adsr gate trigger op1_attack_ms op1_decay_ms op1_sustain op1_release_ms))
(def op2_env_shape (adsr gate trigger op2_attack_ms op2_decay_ms op2_sustain op2_release_ms))
(def op3_env_shape (adsr gate trigger op3_attack_ms op3_decay_ms op3_sustain op3_release_ms))
(def filt_env (adsr gate trigger filter_attack_ms filter_decay_ms filter_sustain filter_release_ms))

(def tone_amt (clip (mod tone) 0 1))
(def tuned_pitch (* pitch (semi_ratio (/ (mod tune_cents) 100))))
(def carrier_phase (phasor (* tuned_pitch (+ 1 (* tone_amt 0.001)))))
(def op1_amt (clip (mod op1_env) 0 1))
(def op2_amt (clip (mod op2_env) 0 1))
(def op3_amt (clip (mod op3_env) 0 1))

(def op1_freq (* tuned_pitch (listed_ratio (mod op1_frq)) (+ 1 (* tone_amt 0.002))))
(def op2_freq (* tuned_pitch (listed_ratio (mod op2_frq)) (+ 1 (* tone_amt 0.004))))
(def op3_freq (* tuned_pitch (listed_ratio (mod op3_frq)) (+ 1 (* tone_amt 0.006))))
(def op1_phase (phasor op1_freq))
(def op2_phase (phasor op2_freq))
(def op3_phase (phasor op3_freq))

(make-history op1_fb_hist)
(make-history op2_fb_hist)
(make-history op3_fb_hist)
(def op1_prev (read-history op1_fb_hist))
(def op2_prev (read-history op2_fb_hist))
(def op3_prev (read-history op3_fb_hist))

(def block1 (fm_block carrier_phase op1_phase op1_env_shape op1_amt tone_amt op1_prev 1.5 0.00 (mod op1_wave) (mod op1_mix) (mod car_wave) (mod car_mix)))
(def block2 (fm_block carrier_phase op2_phase op2_env_shape op2_amt tone_amt op2_prev 2.0 0.17 (mod op2_wave) (mod op2_mix) (mod car_wave) (mod car_mix)))
(def block3 (fm_block carrier_phase op3_phase op3_env_shape op3_amt tone_amt op3_prev 2.6 0.37 (mod op3_wave) (mod op3_mix) (mod car_wave) (mod car_mix)))
(write-history op1_fb_hist block1)
(write-history op2_fb_hist block2)
(write-history op3_fb_hist block3)

(def mix_norm (+ 0.18 op1_amt op2_amt op3_amt))
(def raw_fm (/ (+ (* block1 (+ 0.15 op1_amt))
                  (* block2 (+ 0.15 op2_amt))
                  (* block3 (+ 0.15 op3_amt)))
               mix_norm))
(def shimmer (* tone_amt 0.12 (sin (+ (* twopi carrier_phase) (* raw_fm 2.4)))))
(def driven (tanh (* (+ raw_fm shimmer) (clip (mod drive) 0.5 8))))
(def tone_cut (+ (mod cutoff) (* tuned_pitch keytrack) (* filt_env (mod filter_env_amt)) (* tone_amt 3000)))
(def filtered (biquad driven (clip tone_cut 80 12000) (clip (mod resonance) 0.5 2.5) 1 0))
(out (* filtered amp_env velocity (clip (mod gain) 0 1)) 1 @name audio)
