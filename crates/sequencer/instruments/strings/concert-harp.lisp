; Concert harp — richer waveguide plucked string
; Built from the working gate-edge / direct-delay topology used by acoustic-guitar.

(def gate     (in 1  @name gate))
(def pitch    (in 2  @name pitch))
(def velocity (in 3  @name velocity))
(def trigger  (in 4  @name trigger))
(def mod1     (in 5  @name mod1 @modulator 1))
(def mod2     (in 6  @name mod2 @modulator 2))
(def mod3     (in 7  @name mod3 @modulator 3))
(def mod4     (in 8  @name mod4 @modulator 4))

(param nail         @default 0.35 @min 0    @max 1    @mod true @mod-mode additive)
  ; 0=flesh pluck, 1=nail/bright attack
(param brightness   @default 0.58 @min 0.05 @max 0.95 @mod true @mod-mode additive)
  ; Loop brightness and overtone bloom
(param pluck_pos    @default 0.18 @min 0.04 @max 0.38)
  ; Harp strings are typically plucked a bit farther from the bridge
(param sustain_s    @default 4.2  @min 0.4  @max 10   @unit s   @mod true @mod-mode additive)
  ; T60-style decay target
(param soundboard   @default 0.65 @min 0    @max 1    @mod true @mod-mode additive)
  ; Resonant body contribution
(param sympathetic  @default 0.35 @min 0    @max 1    @mod true @mod-mode additive)
  ; Secondary string/halo resonance
(param stereo_width @default 0.55 @min 0    @max 1    @mod true @mod-mode additive)
  ; Asymmetric body pickup and early-air spread
(param weirdness    @default 0.00 @min 0    @max 1    @mod true @mod-mode additive)
  ; Crossfades toward synthetic metallic/FM-like bloom
(param vel_bright   @default 0.18 @min 0    @max 1)
  ; Velocity influence on brightness
(param gain         @default 0.20 @min 0    @max 1)

; ── Safe pitch ──
(def safe_pitch (max pitch 25.0))
(def delay_nominal (/ 44100.0 safe_pitch))

; ── Note-on detector / exciter window ──
(make-history gate_hist)
(make-history counter_hist)
(def prev_gate    (read-history gate_hist))
(def counter_prev (read-history counter_hist))
(def note_on      (gt (- gate prev_gate) 0.0))
(def counter      (gswitch note_on 0.0 (+ counter_prev 1.0)))

; ── Multi-stage pluck exciter ──
(def nail_amt     (clip (mod nail) 0 1))
(def bright_amt   (clip (+ (mod brightness) (* vel_bright velocity)) 0.05 0.99))
(def dark_amt     (- 1.0 bright_amt))
(def burst_len    (+ 12.0 (* (- 1.0 nail_amt) 78.0)))
(def burst_gate   (lt counter burst_len))
(def exc_cutoff   (+ 500.0 (* nail_amt 12000.0)))
(def exc_q        (+ 0.45 (* nail_amt 0.55)))
(def raw_burst    (biquad (* (noise) burst_gate) exc_cutoff exc_q 1 0))
(def body_burst   (biquad raw_burst (+ 450.0 (* (- 1.0 nail_amt) 3000.0)) 0.8 0.85 0))
(def edge_burst   (biquad raw_burst (+ 2200.0 (* nail_amt 12500.0)) 0.7 1 0))
(def exciter      (+ (* body_burst (+ 1.05 (* (- 1.0 nail_amt) 0.55)))
                     (* edge_burst (+ 0.03 (* nail_amt 0.90)))))

; Pluck position comb
(def pick_dly     (* (clip pluck_pos 0.04 0.38) delay_nominal))
(def comb_exc     (- exciter (delay exciter pick_dly)))

; ── Main string loop ──
(def lp_freq      (max (* safe_pitch 1.8)
                       (+ 120.0 (* bright_amt 9800.0))))
(def exp1         (exp (/ (* -1.0 twopi lp_freq) 44100.0)))
(def cos_term     (cos (* (/ safe_pitch 44100.0) twopi)))
(def mag_sq       (max 0.000001 (+ 1.0 (* exp1 exp1) (* -2.0 exp1 cos_term))))
(def mag_h        (/ (- 1.0 exp1) (sqrt mag_sq)))
(def sustain_main (max 0.15 (mod sustain_s)))
(def stretch      (min 0.99999
                     (/ (pow 0.001 (/ 1.0 (* sustain_main safe_pitch)))
                        mag_h)))
(def lp_group_dly (/ exp1 (max 0.001 (- 1.0 exp1))))
(def delay_len    (max 1.5 (- delay_nominal lp_group_dly)))

(make-history ks_hist)
(def ks_prev      (read-history ks_hist))
(def loop_input   (+ comb_exc (* ks_prev stretch)))
(def delayed      (delay loop_input delay_len))
(def ks_out       (+ (* delayed (- 1.0 exp1)) (* ks_prev exp1)))

; ── Sympathetic halo string ──
(def sym_amt         (clip (mod sympathetic) 0 1))
(def halo_ratio      (+ 1.0010 (* sym_amt 0.010)))
(def halo_pitch      (* safe_pitch halo_ratio))
(def halo_lp_freq    (max (* halo_pitch 2.0)
                          (+ 260.0 (* bright_amt 7200.0))))
(def halo_exp        (exp (/ (* -1.0 twopi halo_lp_freq) 44100.0)))
(def halo_cos_term   (cos (* (/ halo_pitch 44100.0) twopi)))
(def halo_mag_sq     (max 0.000001 (+ 1.0 (* halo_exp halo_exp) (* -2.0 halo_exp halo_cos_term))))
(def halo_mag_h      (/ (- 1.0 halo_exp) (sqrt halo_mag_sq)))
(def halo_sustain    (max 0.12 (* sustain_main 0.72)))
(def halo_stretch    (min 0.99997
                       (/ (pow 0.001 (/ 1.0 (* halo_sustain halo_pitch)))
                          halo_mag_h)))
(def halo_group_dly  (/ halo_exp (max 0.001 (- 1.0 halo_exp))))
(def halo_delay_len  (max 1.5 (- (/ 44100.0 halo_pitch) halo_group_dly)))

(make-history halo_hist)
(def halo_prev    (read-history halo_hist))
(def halo_input   (+ (* comb_exc (+ 0.02 (* sym_amt 0.42)))
                     (* ks_out (+ 0.01 (* sym_amt 0.28)))
                     (* halo_prev halo_stretch)))
(def halo_delayed (delay halo_input halo_delay_len))
(def halo_out     (+ (* halo_delayed (- 1.0 halo_exp)) (* halo_prev halo_exp)))

; ── Soundboard / cavity resonances ──
(def board_src    (+ ks_out (* halo_out (+ 0.25 (* sym_amt 0.95)))))
(def body1        (biquad board_src 110.0 1.3 0.35 2))
(def body2        (biquad board_src 230.0 1.5 0.30 2))
(def body3        (biquad board_src 470.0 1.2 0.24 2))
(def body4        (biquad board_src 920.0 1.1 0.18 2))
(def body_sum     (+ body1 body2 (+ body3 body4)))
(def board_amt    (clip (mod soundboard) 0 1))
(def body_out     (* body_sum (+ (* board_amt 0.45) (* board_amt board_amt 1.25))))

; ── Optional synthetic metallic/FM bloom ──
(def weird_amt    (clip (mod weirdness) 0 1))
(def fm_index     (+ 0.04 (* weird_amt 5.2)))
(def fm_pitch     (+ (* safe_pitch (+ 1.0 (* weird_amt 1.7)))
                     (* ks_out safe_pitch fm_index 0.45)
                     (* halo_out safe_pitch fm_index 0.30)))
(def fm_core      (sin (* twopi (phasor fm_pitch))))
(def fm_upper1    (sin (* twopi (phasor (+ (* safe_pitch 2.73)
                                           (* ks_out safe_pitch weird_amt 0.55))))))
(def fm_upper2    (sin (* twopi (phasor (+ (* safe_pitch 4.11)
                                           (* halo_out safe_pitch weird_amt 0.42))))))
(def weird_env    (+ (* ks_out 0.65) (* halo_out 0.55) (* body_out 0.20)))
(def weird_raw    (+ (* fm_core 0.85) (* fm_upper1 0.36) (* fm_upper2 0.24)))
(def weird_tone   (biquad (* weird_raw weird_env)
                          (+ 1800.0 (* weird_amt 9000.0))
                          0.85
                          0.9
                          0))

; ── Stereo pickup image ──
(def width        (clip (mod stereo_width) 0 1))
(def early_l      (delay (+ (* ks_out (+ 1.00 (* dark_amt 0.08)))
                            (* halo_out (+ 0.12 (* width 0.18)))
                            (* body_out (+ 0.82 (* width 0.22))))
                         (+ 1.0 (* width 34.0))))
(def early_r      (delay (+ (* ks_out (+ 0.86 (* bright_amt 0.10)))
                            (* halo_out (+ 0.16 (* width 0.48)))
                            (* body_out (+ 0.58 (* width 0.58))))
                         (+ 5.0 (* width 58.0))))
(def weird_l      (* weird_tone (+ (* weird_amt 0.10) (* weird_amt width 0.18))))
(def weird_r      (* weird_tone (+ (* weird_amt 0.14) (* weird_amt width 0.34))))
(def left_mix     (+ early_l (* halo_out (+ 0.04 (* width 0.12))) (* body_out (+ 0.18 (* width 0.24))) weird_l))
(def right_mix    (+ early_r (* halo_out (+ 0.08 (* width 0.54))) (* body_out (+ 0.04 (* width 0.10))) weird_r))

; ── Output smoothing / DC block ──
(def out_l_lp     (biquad left_mix (+ 4200.0 (* bright_amt 5200.0)) 0.72 0.88 0))
(def out_r_lp     (biquad right_mix (+ 4600.0 (* bright_amt 6200.0)) 0.72 0.88 0))

(make-history dc_hist_l)
(make-history dc_hist_r)
(def dc_l         (mix out_l_lp (read-history dc_hist_l) 0.9985))
(def dc_r         (mix out_r_lp (read-history dc_hist_r) 0.9985))
(def out_l        (- out_l_lp dc_l))
(def out_r        (- out_r_lp dc_r))

; ── Commit histories ──
(write-history gate_hist gate)
(write-history counter_hist counter)
(write-history ks_hist ks_out)
(write-history halo_hist halo_out)
(write-history dc_hist_l dc_l)
(write-history dc_hist_r dc_r)

(def vel_scale    (+ 0.25 (* 0.75 velocity)))
(out (* out_l vel_scale gain) 1 @name left)
(out (* out_r vel_scale gain) 2 @name right)
