; Hammer-Stein Physical Modeling Piano Synthesizer
; An advanced 2-string unison physical modeling digital waveguide piano.
; Simulates hammer-strike stiffness, noise components, damper behavior, 
; soundboard/sympathetic resonances, duplex scale chime, and dynamic panning.

(def gate     (in 1  @name gate))
(def pitch    (in 2  @name pitch))
(def velocity (in 3  @name velocity))
(def trigger  (in 4  @name trigger))
(def mod1     (in 5  @name mod1 @modulator 1))
(def mod2     (in 6  @name mod2 @modulator 2))
(def mod3     (in 7  @name mod3 @modulator 3))
(def mod4     (in 8  @name mod4 @modulator 4))

; ── HAMMER PARAMETERS ──
(param hardness        @default 0.52 @min 0.05 @max 1.0  @mod true @mod-mode additive)
  ; Hammer stiffness (felt soft vs hard metal clack)
(param hardness_vel    @default 0.35 @min 0.0  @max 1.0)
  ; Influence of velocity on hammer stiffness
(param hammer_noise    @default 0.25 @min 0.0  @max 1.0  @mod true @mod-mode additive)
  ; Level of mechanical soundboard hammer thud
(param strike_pos      @default 0.14 @min 0.05 @max 0.25)
  ; Strike position fraction (comb notch, typically 1/7th ≈ 0.14)

; ── TIMBRE & OVERTONE PARAMETERS ──
(param brightness      @default 0.65 @min 0.10 @max 0.95 @mod true @mod-mode additive)
  ; Overall string brightness and loop cutoff
(param brightness_vel  @default 0.25 @min 0.0  @max 1.0)
  ; Velocity influence on overtones
(param duplex_ring     @default 0.18 @min 0.0  @max 1.0  @mod true @mod-mode additive)
  ; High-frequency chime simulation (duplex scale)

; ── DAMPER & SUSTAIN PARAMETERS ──
(param sustain_s       @default 4.5  @min 0.5  @max 12.0 @unit s   @mod true @mod-mode additive)
  ; Decay time when string is free (sustain)
(param damper_decay    @default 0.22 @min 0.05 @max 0.80 @unit s)
  ; Decay time when damper is applied (key release)
(param pedal_sustain   @default 0.0  @min 0    @max 1)
  ; Simulate sustain pedal (0 = Off, 1 = Down)

; ── ACOUSTIC RESONANCE PARAMETERS ──
(param soundboard      @default 0.50 @min 0.0  @max 1.0  @mod true @mod-mode additive)
  ; Soundboard wood cavity resonance contribution
(param sympathetic     @default 0.35 @min 0.0  @max 1.0  @mod true @mod-mode additive)
  ; Secondary string sympathetic halo resonance

; ── VOICING & IMAGING PARAMETERS ──
(param unison_detune   @default 2.5  @min 0.0  @max 10.0)
  ; Micro-detuning between string pairs (cents equivalent)
(param stereo_width    @default 0.60 @min 0.0  @max 1.0)
  ; Stereo spread of the unison string pair
(param key_pan         @default 0.45 @min 0.0  @max 1.0)
  ; Panning sensitivity across keyboard (bass left, treble right)

; ── GLOBAL & EQ PARAMETERS ──
(param eq_bass         @default 0.0  @min -12.0 @max 12.0 @unit dB)
(param eq_treble       @default 1.5  @min -12.0 @max 12.0 @unit dB)
(param gain            @default 0.22 @min 0.0   @max 1.0)

; ── Safe Pitch Clamping ──
(def safe_pitch (max pitch 35.0))
(def delay_nominal (/ samplerate safe_pitch))

; ── Trigger & Note-On Action Detector ──
(make-history gate_hist)
(make-history counter_hist)
(def prev_gate    (read-history gate_hist))
(def counter_prev (read-history counter_hist))
(def note_on      (gt (- gate prev_gate) 0.0))
(def counter      (gswitch note_on 0.0 (+ counter_prev 1.0)))

; ── Hammer Dynamics Engine ──
; Higher velocity increases stiffness and brightness of the strike
(def eff_hardness (clip (+ (mod hardness) (* hardness_vel velocity)) 0.05 1.0))

; Noise burst is longer for softer hammers, very snappy for hard hammers
(def burst_len    (+ 12.0 (* (- 1.0 eff_hardness) 100.0)))
(def noise_env    (lt counter burst_len))

; Hammer noise frequencies scale with stiffness
(def exc_cutoff   (+ 280.0 (* eff_hardness 9200.0)))
(def exc_q        (+ 0.42 (* (- 1.0 eff_hardness) 0.58)))
(def raw_burst    (biquad (* (noise) noise_env) exc_cutoff exc_q 1 0))

; Hammer wood/felt "thud" body mode
(def thud_sig     (biquad raw_burst 160.0 0.85 0.75 0))

; Hammer sharp click strike transient (short metallic spike)
(def click_amp    (* eff_hardness (+ 0.12 (* velocity 0.88)) (gt counter 0.0) (lt counter 4.0)))
(def click_sig    (biquad click_amp 5200.0 0.45 1.0 0))

; Combine excitation components
(def noise_amt    (clip (mod hammer_noise) 0 1))
(def raw_strike   (+ (* thud_sig 1.25)
                     (* click_sig 0.95)
                     (* raw_burst noise_amt 0.70)))

; Apply strike position comb-filter
(def strike_dly   (* (clip strike_pos 0.05 0.25) delay_nominal))
(def comb_exc     (- raw_strike (delay raw_strike strike_dly)))

; ── Damper and Sustain Mechanics ──
; If sustain pedal is down or gate is active, string is free. Otherwise dampened.
(def pedal_active (gt pedal_sustain 0.5))
(def is_sustained (max gate pedal_active))
(def current_sustain (gswitch is_sustained damper_decay (mod sustain_s)))

; Natural piano scale decay: treble decays much faster than bass
(def pitch_scale  (pow (/ 261.63 safe_pitch) 0.35))
(def scaled_sustain (* current_sustain (clip pitch_scale 0.12 3.5)))

; Effective brightness
(def eff_bright  (clip (+ (mod brightness) (* brightness_vel velocity)) 0.05 0.99))

; ── Dual Waveguide Strings (Unison A & B) ──

; String A (nominal pitch)
(def lp_freq_a     (max (* safe_pitch 1.4) (+ 150.0 (* eff_bright 16500.0))))
(def exp_a         (exp (/ (* -1.0 twopi lp_freq_a) samplerate)))
(def cos_term_a    (cos (* (/ safe_pitch samplerate) twopi)))
(def mag_sq_a      (max 0.000001 (+ 1.0 (* exp_a exp_a) (* -2.0 exp_a cos_term_a))))
(def mag_h_a       (/ (- 1.0 exp_a) (sqrt mag_sq_a)))
(def stretch_a     (min 0.99999
                     (/ (pow 0.001 (/ 1.0 (* scaled_sustain safe_pitch)))
                        mag_h_a)))
(def lp_group_dly_a (/ exp_a (max 0.001 (- 1.0 exp_a))))
(def delay_len_a   (max 1.5 (- delay_nominal lp_group_dly_a)))

; String B (micro-detuned)
(def detune_ratio  (+ 1.0 (* unison_detune 0.00015)))
(def pitch_b       (* safe_pitch detune_ratio))
(def delay_nominal_b (/ samplerate pitch_b))
(def lp_freq_b     (max (* pitch_b 1.4) (+ 150.0 (* eff_bright 16500.0))))
(def exp_b         (exp (/ (* -1.0 twopi lp_freq_b) samplerate)))
(def cos_term_b    (cos (* (/ pitch_b samplerate) twopi)))
(def mag_sq_b      (max 0.000001 (+ 1.0 (* exp_b exp_b) (* -2.0 exp_b cos_term_b))))
(def mag_h_b       (/ (- 1.0 exp_b) (sqrt mag_sq_b)))
(def stretch_b     (min 0.99999
                     (/ (pow 0.001 (/ 1.0 (* scaled_sustain pitch_b)))
                        mag_h_b)))
(def lp_group_dly_b (/ exp_b (max 0.001 (- 1.0 exp_b))))
(def delay_len_b   (max 1.5 (- delay_nominal_b lp_group_dly_b)))

; Run waveguide loops
(make-history ks_hist_a)
(def ks_prev_a    (read-history ks_hist_a))
(def loop_input_a (+ comb_exc (* ks_prev_a stretch_a)))
(def delayed_a    (delay loop_input_a delay_len_a))
(def ks_out_a     (+ (* delayed_a (- 1.0 exp_a)) (* ks_prev_a exp_a)))

(make-history ks_hist_b)
(def ks_prev_b    (read-history ks_hist_b))
(def loop_input_b (+ comb_exc (* ks_prev_b stretch_b)))
(def delayed_b    (delay loop_input_b delay_len_b))
(def ks_out_b     (+ (* delayed_b (- 1.0 exp_b)) (* ks_prev_b exp_b)))

; ── Sympathetic Resonator Halo ──
(def sym_amt        (clip (mod sympathetic) 0 1))
(def halo_pitch     (* safe_pitch 1.0003)) ; slight detuning for build-up bloom
(def halo_lp_freq   (max (* halo_pitch 1.6) (+ 280.0 (* eff_bright 5500.0))))
(def halo_exp       (exp (/ (* -1.0 twopi halo_lp_freq) samplerate)))
(def halo_cos_term  (cos (* (/ halo_pitch samplerate) twopi)))
(def halo_mag_sq    (max 0.000001 (+ 1.0 (* halo_exp halo_exp) (* -2.0 halo_exp halo_cos_term))))
(def halo_mag_h     (/ (- 1.0 halo_exp) (sqrt halo_mag_sq)))
(def halo_sustain   (max 1.2 (* scaled_sustain 1.3)))
(def halo_stretch   (min 0.99998
                      (/ (pow 0.001 (/ 1.0 (* halo_sustain halo_pitch)))
                         halo_mag_h)))
(def halo_group_dly (/ halo_exp (max 0.001 (- 1.0 halo_exp))))
(def halo_delay_len (max 1.5 (- (/ samplerate halo_pitch) halo_group_dly)))

(make-history halo_hist)
(def halo_prev      (read-history halo_hist))
(def halo_input     (+ (* comb_exc (+ 0.01 (* sym_amt 0.15)))
                       (* (+ ks_out_a ks_out_b) (+ 0.02 (* sym_amt 0.22)))
                       (* halo_prev halo_stretch)))
(def halo_delayed   (delay halo_input halo_delay_len))
(def halo_out       (+ (* halo_delayed (- 1.0 halo_exp)) (* halo_prev halo_exp)))

; ── High-Register Duplex Scale Ring ──
(def duplex_amt     (clip (mod duplex_ring) 0 1))
(def duplex_freq    (* safe_pitch 4.0)) ; 2 octaves up
(def duplex_phase   (phasor duplex_freq))
(def duplex_osc     (sin (* duplex_phase twopi)))
(def duplex_env     (+ (* ks_out_a 0.5) (* ks_out_b 0.5)))
(def duplex_sig     (* duplex_osc duplex_env duplex_amt 0.32))

; ── Soundboard Cavity Resonances (Parallel Bandpass Filters) ──
(def ks_mixed (+ ks_out_a ks_out_b))
(def sb1 (biquad ks_mixed 68.0 1.5 0.28 2))
(def sb2 (biquad ks_mixed 125.0 1.7 0.25 2))
(def sb3 (biquad ks_mixed 190.0 1.4 0.22 2))
(def sb4 (biquad ks_mixed 320.0 1.2 0.18 2))
(def sb5 (biquad ks_mixed 580.0 1.0 0.12 2))
(def sb_sum (+ sb1 sb2 sb3 sb4 sb5))
(def board_amt (clip (mod soundboard) 0 1))
(def body_out (* sb_sum board_amt 0.78))

; ── Stereo Panning & Imaging ──
(def width (clip stereo_width 0 1))

; Key-based panning: bass left, treble right
(def pan_center 0.5)
(def key_pan_sens (clip key_pan 0 1))
(def raw_pan_offset (* 0.14 (log (/ safe_pitch 261.63))))
(def pan_key (clip (+ pan_center (* key_pan_sens raw_pan_offset)) 0.15 0.85))

; String A and String B pulled apart slightly left & right of key-panned center
(def pan_l (clip (- pan_key (* width 0.15)) 0.02 0.98))
(def pan_r (clip (+ pan_key (* width 0.15)) 0.02 0.98))

(def left_src  (+ (* ks_out_a (- 1.0 pan_l)) 
                  (* ks_out_b (- 1.0 pan_r))
                  (* halo_out 0.40) 
                  (* body_out 0.32)
                  (* duplex_sig 0.5)))

(def right_src (+ (* ks_out_a pan_l) 
                  (* ks_out_b pan_r)
                  (* halo_out 0.50) 
                  (* body_out 0.58)
                  (* duplex_sig 0.5)))

; ── 3-Band EQ Shelf Crossover (Bass, Mid, Treble) ──
(def eq_bass_gain   (pow 10.0 (/ eq_bass 20.0)))
(def eq_treble_gain (pow 10.0 (/ eq_treble 20.0)))

; Bass band extraction (Lowpass at 200Hz)
(def bass_l (biquad left_src 200.0 0.5 1.0 0))
(def bass_r (biquad right_src 200.0 0.5 1.0 0))

; Treble band extraction (Highpass at 2000Hz)
(def treble_l (biquad left_src 2000.0 0.5 1.0 1))
(def treble_r (biquad right_src 2000.0 0.5 1.0 1))

; Mid band extraction
(def mid_l (- left_src (+ bass_l treble_l)))
(def mid_r (- right_src (+ bass_r treble_r)))

; Reconstruct EQ sum
(def eq_l (+ (* bass_l eq_bass_gain) mid_l (* treble_l eq_treble_gain)))
(def eq_r (+ (* bass_r eq_bass_gain) mid_r (* treble_r eq_treble_gain)))

; ── Output DC Blocker ──
(make-history dc_l_hist)
(make-history dc_r_hist)
(def dc_lp_l (mix eq_l (read-history dc_l_hist) 0.9995))
(def dc_lp_r (mix eq_r (read-history dc_r_hist) 0.9995))
(def clean_l (- eq_l dc_lp_l))
(def clean_r (- eq_r dc_lp_r))

; Commit state histories
(write-history gate_hist gate)
(write-history counter_hist counter)
(write-history ks_hist_a ks_out_a)
(write-history ks_hist_b ks_out_b)
(write-history halo_hist halo_out)
(write-history dc_l_hist dc_lp_l)
(write-history dc_r_hist dc_lp_r)

; Velocity scaling
(def vel_scale (+ 0.25 (* 0.75 velocity)))
(out (* clean_l vel_scale gain) 1 @name left)
(out (* clean_r vel_scale gain) 2 @name right)
