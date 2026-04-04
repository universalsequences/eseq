; Orchestral strings "lux" — simple sustained waveguide string
; Rebuilt from the acoustic-guitar topology:
; stable compensated loop, continuous held excitation, and explicit attack.

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

(param bow_pressure    @default 0.34 @min 0    @max 1    @mod true @mod-mode additive)
(param bow_attack_ms   @default 180  @min 5    @max 2000 @unit ms @mod true @mod-mode additive)
(param brightness      @default 0.58 @min 0.05 @max 0.95 @mod true @mod-mode additive)
(param pick_pos        @default 0.16 @min 0.04 @max 0.42 @mod true @mod-mode additive)
(param decay_time_ms   @default 2200 @min 100  @max 8000 @unit ms @mod true @mod-mode additive)
(param soundboard      @default 0.62 @min 0    @max 1    @mod true @mod-mode additive)
(param sympathetic     @default 0.34 @min 0    @max 1    @mod true @mod-mode additive)
(param stereo_width    @default 0.50 @min 0    @max 1    @mod true @mod-mode additive)
(param vibrato_rate    @default 5.2  @min 0    @max 10   @unit Hz)
(param vibrato_depth   @default 0.10 @min 0    @max 2    @mod true @mod-mode additive)
(param ensemble        @default 0.28 @min 0    @max 1    @mod true @mod-mode additive)
(param gain            @default 0.16 @min 0    @max 1)

; ── Stable controls ──
(def bow_amt      (clip (mod bow_pressure) 0 1))
(def bright_amt   (clip (mod brightness) 0.05 0.95))
(def pick_amt     (clip (mod pick_pos) 0.04 0.42))
(def board_amt    (clip (mod soundboard) 0 1))
(def sym_amt      (clip (mod sympathetic) 0 1))
(def width_amt    (clip (mod stereo_width) 0 1))
(def ens_amt      (clip (mod ensemble) 0 1))
(def vibrato_lfo  (sin (* twopi (phasor vibrato_rate))))
(def pitch_mod    (+ pitch (* pitch (mod vibrato_depth) 0.01 vibrato_lfo)))
(def safe_pitch   (max pitch_mod 45.0))
(def delay_nominal (/ 44100.0 safe_pitch))
(def sustain_s    (max 0.10 (/ (mod decay_time_ms) 1000.0)))
(def vel_scale    (+ 0.25 (* 0.75 velocity)))

; ── Held excitation with attack control ──
; While the note is held, the excitation stays up. decay_time_ms is treated as
; release time after note-off, not as an in-note decay.
(def bow_env      (adsr gate trigger bow_attack_ms 1 1.0 decay_time_ms))
(def raw_noise    (noise))
(def noise_body   (biquad raw_noise
                          (+ 240.0 (* (- 1.0 bright_amt) 1800.0))
                          0.85
                          0.18
                          0))
(def noise_air    (biquad raw_noise
                          (+ 900.0 (* bright_amt 2600.0))
                          0.75
                          0.04
                          0))
(def held_noise   (+ noise_body noise_air))
(def pick_dly     (* pick_amt delay_nominal))
(def comb_noise   (- held_noise (delay held_noise pick_dly)))

; ── Main string loop: acoustic-guitar style compensated waveguide ──
(def lp_freq      (max (* safe_pitch 1.6)
                       (+ 180.0 (* bright_amt 12000.0))))
(def exp1         (exp (/ (* -1.0 twopi lp_freq) 44100.0)))
(def cos_term     (cos (* (/ safe_pitch 44100.0) twopi)))
(def mag_sq       (max 0.000001 (+ 1.0 (* exp1 exp1) (* -2.0 exp1 cos_term))))
(def mag_h        (/ (- 1.0 exp1) (sqrt mag_sq)))
(def stretch      (min 0.99998
                     (/ (pow 0.001 (/ 1.0 (* sustain_s safe_pitch)))
                        mag_h)))
(def lp_group_dly (/ exp1 (max 0.001 (- 1.0 exp1))))
(def delay_len    (max 1.5 (- delay_nominal lp_group_dly)))

(make-history ks_hist)
(def ks_prev      (read-history ks_hist))
(def sustain_body (biquad ks_prev (+ 180.0 (* bow_amt 900.0)) 1.0 0.12 2))
(def sustain_air  (biquad ks_prev (+ 700.0 (* bright_amt 2400.0)) 0.9 0.04 2))
(def sustain_exc  (* (+ sustain_body sustain_air (* comb_noise 0.12))
                     bow_env
                     bow_amt
                     vel_scale
                     0.10))
(def exciter      sustain_exc)
(def loop_input   (+ exciter (* ks_prev stretch)))
(def delayed      (delay loop_input delay_len))
(def ks_out       (+ (* delayed (- 1.0 exp1)) (* ks_prev exp1)))

; ── Light ensemble string for section width ──
(def ens_lfo         (sin (* twopi (phasor 0.31))))
(def detune_ratio    (+ 1.0 (* (+ (* ens_amt 0.004) (* sym_amt 0.006)) ens_lfo)))
(def ens_pitch       (max 45.0 (* safe_pitch detune_ratio)))
(def ens_lp_freq     (max (* ens_pitch 1.8)
                          (+ 220.0 (* bright_amt 9000.0))))
(def ens_exp         (exp (/ (* -1.0 twopi ens_lp_freq) 44100.0)))
(def ens_cos_term    (cos (* (/ ens_pitch 44100.0) twopi)))
(def ens_mag_sq      (max 0.000001 (+ 1.0 (* ens_exp ens_exp) (* -2.0 ens_exp ens_cos_term))))
(def ens_mag_h       (/ (- 1.0 ens_exp) (sqrt ens_mag_sq)))
(def ens_sustain_s   (max 0.10 (* sustain_s 0.85)))
(def ens_stretch     (min 0.99997
                       (/ (pow 0.001 (/ 1.0 (* ens_sustain_s ens_pitch)))
                          ens_mag_h)))
(def ens_group_dly   (/ ens_exp (max 0.001 (- 1.0 ens_exp))))
(def ens_delay_len   (max 1.5 (- (/ 44100.0 ens_pitch) ens_group_dly)))

(make-history ens_hist)
(def ens_prev        (read-history ens_hist))
(def ens_input       (+ (* exciter (+ 0.12 (* ens_amt 0.20) (* sym_amt 0.20)))
                        (* ks_out (+ 0.00 (* sym_amt 0.08)))
                        (* ens_prev ens_stretch)))
(def ens_delayed     (delay ens_input ens_delay_len))
(def ens_out         (+ (* ens_delayed (- 1.0 ens_exp)) (* ens_prev ens_exp)))

; ── Section body / stereo image ──
(def body_src        (+ ks_out (* ens_out (+ 0.15 (* ens_amt 0.85)))))
(def body1           (biquad body_src 140.0 1.25 0.24 2))
(def body2           (biquad body_src 280.0 1.20 0.22 2))
(def body3           (biquad body_src 520.0 1.30 0.18 2))
(def body4           (biquad body_src 1100.0 1.10 0.12 2))
(def body_sum        (+ body1 body2 (+ body3 body4)))
(def body_out        (* body_sum (+ (* board_amt 0.40) (* board_amt board_amt 1.10))))

(def left_mix        (delay (+ (* ks_out (+ 1.00 (* board_amt 0.08)))
                               (* ens_out (+ 0.10 (* ens_amt 0.22) (* width_amt 0.12)))
                               (* body_out (+ 0.22 (* width_amt 0.16))))
                            (+ 2.0 (* width_amt 30.0))))
(def right_mix       (delay (+ (* ks_out (+ 0.90 (* bright_amt 0.08)))
                               (* ens_out (+ 0.16 (* ens_amt 0.38) (* width_amt 0.26)))
                               (* body_out (+ 0.14 (* width_amt 0.22))))
                            (+ 6.0 (* width_amt 46.0))))

; ── Output smoothing / DC block ──
(def out_l_lp        (biquad left_mix (+ 5200.0 (* bright_amt 4200.0)) 0.72 0.88 0))
(def out_r_lp        (biquad right_mix (+ 5600.0 (* bright_amt 4600.0)) 0.72 0.88 0))

(make-history dc_hist_l)
(make-history dc_hist_r)
(def dc_l            (mix out_l_lp (read-history dc_hist_l) 0.9985))
(def dc_r            (mix out_r_lp (read-history dc_hist_r) 0.9985))
(def out_l           (- out_l_lp dc_l))
(def out_r           (- out_r_lp dc_r))

; ── Commit histories ──
(write-history ks_hist ks_out)
(write-history ens_hist ens_out)
(write-history dc_hist_l dc_l)
(write-history dc_hist_r dc_r)

(out (* out_l gain) 1 @name left)
(out (* out_r gain) 2 @name right)
