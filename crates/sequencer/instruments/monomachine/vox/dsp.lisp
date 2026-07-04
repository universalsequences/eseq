; Monomachine VO-6-spirit formant vocal machine ("vox").
; Glottal pulse / breath-noise source -> growl AM -> 4-formant vowel bank with
; stepped vowel table + morph + pitch-independent formant shift -> consonant
; burst generator -> shared MnM track chain (AM, base/width gulp filter, SRR,
; big one-band EQ, drive, AHD+R amp env). See docs/monomachine-family-spec.md.
;
; Vowel order (index 0-9), arranged for musical morph neighborhoods:
;   a ae e i y uh er o aw u   (morph wraps u -> a)

(def gate (in 1 @name gate))
(def pitch (in 2 @name pitch))
(def velocity (in 3 @name velocity))
(def trigger (in 4 @name trigger))
(def clock (in 5 @name clock))
(def mod1 (in 6 @name mod1 @modulator 1))
(def mod2 (in 7 @name mod2 @modulator 2))
(def mod3 (in 8 @name mod3 @modulator 3))
(def mod4 (in 9 @name mod4 @modulator 4))

; === MNM CHAIN BEGIN === (master copy: instruments/monomachine/CHAIN.md)
(defmacro semi_ratio (semi)
  (exp (/ (* (log 2) semi) 12)))

(defmacro dbamp (db)
  (exp (* 0.1151292546 db)))

; Runtime floor: the dgen build constant-folds literal floor but emits identity
; for runtime values (floor is a NO-OP at runtime — verified 2026-07). The %
; operator's codegen does real flooring, so derive floor from it. Works for
; negative inputs too. Use this, never bare floor, on any runtime value.
(defmacro mnm_floor (x)
  (- x (% x 1)))

(defmacro mnm_ahd_env (gate_sig trigger_sig attack_ms hold_ms decay_ms release_ms)
  (make-history ahd_env_h)
  (make-history ahd_t_h)
  (make-history ahd_gate_h)
  (def prev (read-history ahd_env_h))
  (def prev_t (read-history ahd_t_h))
  (def prev_gate (read-history ahd_gate_h))
  (def gate_on (gt gate_sig 0.5))
  (def rising (* gate_on (lte prev_gate 0.5)))
  (def retrig (max rising trigger_sig))
  (def t (gswitch retrig 0.0 (+ prev_t 1.0)))
  (def atk_smp (max 1.0 (* attack_ms 0.001 samplerate)))
  (def hold_end (+ atk_smp (* hold_ms 0.001 samplerate)))
  (def dec_coeff (- 1.0 (exp (/ -6.907755 (max 1.0 (* decay_ms 0.001 samplerate))))))
  (def rel_coeff (- 1.0 (exp (/ -6.907755 (max 1.0 (* release_ms 0.001 samplerate))))))
  (def attack_level (min 1.0 (+ prev (/ 1.0 atk_smp))))
  (def decayed (* prev (- 1.0 dec_coeff)))
  (def released (* prev (- 1.0 rel_coeff)))
  (def level
    (gswitch gate_on
      (gswitch (lt t atk_smp)
        attack_level
        (gswitch (lt t hold_end) 1.0 decayed))
      released))
  (write-history ahd_env_h level)
  (write-history ahd_t_h t)
  (write-history ahd_gate_h gate_sig)
  level)

(defmacro mnm_ad_env (gate_sig trigger_sig attack_ms decay_ms)
  (mnm_ahd_env gate_sig trigger_sig attack_ms 0 decay_ms decay_ms))

(defmacro mnm_glide (pitch_hz gl_ms)
  (make-history glide_h)
  (def gprev (read-history glide_h))
  (def gcoeff (- 1.0 (exp (/ -1.0 (max 1.0 (* gl_ms 0.001 samplerate))))))
  (def gnext (+ gprev (* gcoeff (- pitch_hz gprev))))
  (def gval (gswitch (lte gprev 0.01) pitch_hz gnext))
  (write-history glide_h gval)
  gval)

(defmacro mnm_filter (input base_hz width_oct res_lo res_hi)
  (def hp_cut (clip base_hz 20 11000))
  (def lp_cut (clip (* hp_cut (exp (* (log 2) (clip width_oct 0.05 9)))) 30 16000))
  (def lp24 (svf (svf input lp_cut res_lo 0) lp_cut 0.55 0))
  (svf (svf lp24 hp_cut res_hi 2) hp_cut 0.55 2))

(defmacro mnm_am (sig rate_hz depth)
  (mix sig (* sig (sin (* twopi (phasor (clip rate_hz 0.05 3000))))) (clip depth 0 1)))

(defmacro mnm_srr (sig amount)
  (def srr_amt (clip amount 0 1))
  (def hold_hz (* samplerate (pow (/ 300.0 samplerate) srr_amt)))
  (make-history srr_ph_h)
  (make-history srr_val_h)
  (def srr_prev_ph (read-history srr_ph_h))
  (def srr_ph (wrap (+ srr_prev_ph (/ hold_hz samplerate)) 0 1))
  (def srr_wrapped (lt srr_ph srr_prev_ph))
  (write-history srr_ph_h srr_ph)
  (def srr_held
    (gswitch (max srr_wrapped (lte srr_amt 0.001)) sig (read-history srr_val_h)))
  (write-history srr_val_h srr_held)
  srr_held)

(defmacro mnm_bits (sig bits)
  (def bit_lv (pow 2 bits))
  (- (* (/ (mnm_floor (* (* (+ (clip sig -1 1) 1) 0.5) bit_lv)) bit_lv) 2) 1))

; The preamble svf's BP output peaks at gain q at cutoff (harness-measured), so
; divide by q for a unity-gain bandpass; center boost/cut then lands exactly.
(defmacro mnm_eq (sig freq q gain_db)
  (+ sig (* (- (dbamp gain_db) 1.0) (/ 1.0 q) (svf sig (clip freq 40 12000) q 1))))
; === MNM CHAIN END ===

; Vowel formant tables (F1-F4 Hz), 1-based selector over 10 vowels.
(defmacro vowel_f1 (vi)
  (selector (+ vi 1) 650 660 400 290 270 520 490 400 570 350))
(defmacro vowel_f2 (vi)
  (selector (+ vi 1) 1080 1720 1700 1870 2140 1190 1350 800 840 600))
(defmacro vowel_f3 (vi)
  (selector (+ vi 1) 2650 2410 2600 2800 2950 2390 1690 2600 2410 2700))
(defmacro vowel_f4 (vi)
  (selector (+ vi 1) 2900 2750 3000 3250 3300 2900 2700 2800 2700 2900))

; Unity-gain bandpass formant (svf bp peaks at q, so normalize).
(defmacro formant_bp (sig freq q)
  (* (/ 1.0 q) (svf sig (clip freq 60 9000) q 1)))

; Consonant burst tables: filter freq / q / mode (0 LP, 1 BP, 2 HP) / length scale.
; Types: 0 none, 1 s, 2 sh, 3 f, 4 t, 5 k, 6 p, 7 h
(defmacro cons_freq (ci)
  (selector (+ ci 1) 1000 6200 2900 1500 4200 1400 300 1800))
(defmacro cons_q (ci)
  (selector (+ ci 1) 1.0 1.2 2.5 1.6 2.2 3.0 1.0 0.8))
(defmacro cons_mode (ci)
  (selector (+ ci 1) 1 2 1 1 1 1 0 1))
(defmacro cons_len_scale (ci)
  (selector (+ ci 1) 1.0 1.0 1.0 1.0 0.3 0.3 0.25 1.4))

(param amp_attack_ms  @default 2    @min 0.2 @max 2000 @unit ms)
(param amp_hold_ms    @default 120  @min 0   @max 4000 @unit ms)
(param amp_decay_ms   @default 500  @min 5   @max 8000 @unit ms)
(param amp_release_ms @default 120  @min 2   @max 8000 @unit ms)
(param glide_ms       @default 40   @min 0   @max 1500 @unit ms)

(param glottis        @default 0.35 @min 0.02 @max 0.5 @mod true @mod-mode additive)
(param breath         @default 0.1  @min 0   @max 1 @mod true @mod-mode additive)
(param growl          @default 0    @min 0   @max 1 @mod true @mod-mode additive)
(param growl_hz       @default 28   @min 8   @max 90 @unit Hz)

(param vowel          @default 0    @min 0   @max 9 @mod true @mod-mode additive)
(param vowel_morph    @default 0    @min 0   @max 1 @mod true @mod-mode additive)
(param formant_shift  @default 1.0  @min 0.25 @max 4 @mod true @mod-mode additive)
(param formant_q      @default 10   @min 3   @max 24)

(param cons_type      @default 0    @min 0   @max 7)
(param cons_level     @default 0.5  @min 0   @max 1.5 @mod true @mod-mode additive)
(param cons_len_ms    @default 70   @min 5   @max 400 @unit ms)
(param sibilance      @default 0.3  @min 0   @max 1)
(param cons_duck      @default 0.5  @min 0   @max 1)

(param flt_base       @default 60   @min 20  @max 11000 @unit Hz @mod true @mod-mode additive)
(param flt_width      @default 7.5  @min 0.1 @max 9 @mod true @mod-mode additive)
(param flt_res_lo     @default 0.7  @min 0.5 @max 14)
(param flt_res_hi     @default 0.7  @min 0.5 @max 14)
(param fenv_attack_ms @default 2    @min 0.2 @max 4000 @unit ms)
(param fenv_decay_ms  @default 240  @min 5   @max 8000 @unit ms)
(param env_to_base    @default 0    @min -6  @max 6 @mod true @mod-mode additive)
(param env_to_width   @default 0    @min -8  @max 8 @mod true @mod-mode additive)
(param keytrack       @default 0    @min 0   @max 2)

(param am_rate        @default 55   @min 0.1 @max 2200 @unit Hz @mod true @mod-mode additive)
(param am_depth       @default 0    @min 0   @max 1 @mod true @mod-mode additive)
(param srr            @default 0    @min 0   @max 1 @mod true @mod-mode additive)
(param eq_freq        @default 1400 @min 40  @max 11000 @unit Hz @mod true @mod-mode additive)
(param eq_gain_db     @default 0    @min -30 @max 30 @unit dB @mod true @mod-mode additive)
(param eq_q           @default 2.2  @min 0.5 @max 12)
(param drive          @default 1.4  @min 0.5 @max 8 @mod true @mod-mode additive)
(param pan_width      @default 0.15 @min 0   @max 1)
(param gain           @default 0.35 @min 0   @max 1 @mod true @mod-mode additive)

; ---- voice source ----
(def gpitch (mnm_glide pitch glide_ms))
(def glot_w (clip (mod glottis) 0.02 0.5))
(def glot_raw (polyblep_pulse (phasor gpitch) glot_w gpitch))
; narrow pulse duty leaves a DC offset; recenter roughly and tame level
(def glot (* (- glot_raw (- (* glot_w 2) 1)) 0.7))
(def breath_amt (clip (mod breath) 0 1))
(def voiced_src (mix glot (* (noise) 0.5) breath_amt))
(def growl_amt (clip (mod growl) 0 1))
(def growl_lfo (* 0.5 (+ 1 (sin (* twopi (phasor growl_hz))))))
(def src (* voiced_src (- 1 (* growl_amt growl_lfo 0.85))))

; ---- vowel formant bank ----
(def vi (clip (mnm_floor (mod vowel)) 0 9))
(def vnext (% (+ vi 1) 10))
(def vm (clip (mod vowel_morph) 0 1))
(def fshift (clip (mod formant_shift) 0.25 4))
(def f1 (* (mix (vowel_f1 vi) (vowel_f1 vnext) vm) fshift))
(def f2 (* (mix (vowel_f2 vi) (vowel_f2 vnext) vm) fshift))
(def f3 (* (mix (vowel_f3 vi) (vowel_f3 vnext) vm) fshift))
(def f4 (* (mix (vowel_f4 vi) (vowel_f4 vnext) vm) fshift))
(def fq (clip formant_q 3 24))
(def vowel_sig
  (* 2.2 (+ (formant_bp src f1 fq)
            (* 0.55 (formant_bp src f2 fq))
            (* 0.28 (formant_bp src f3 fq))
            (* 0.15 (formant_bp src f4 fq)))))

; ---- consonant burst ----
(def ci (clip (mnm_floor cons_type) 0 7))
(def cons_on (gt ci 0.5))
(def cons_env (mnm_ad_env gate trigger 0.5 (* cons_len_ms (cons_len_scale ci))))
(def cons_noise (noise))
(def cons_core (svf cons_noise (* (cons_freq ci) fshift) (cons_q ci) (cons_mode ci)))
(def cons_hiss (* sibilance (svf cons_noise 6800 0.8 2)))
(def cons_sig (* cons_on (clip (mod cons_level) 0 1.5) cons_env (+ cons_core cons_hiss) 0.9))

(def vox_sum (+ (* vowel_sig (- 1 (* cons_duck cons_env cons_on))) cons_sig))

; ---- MnM track chain tail ----
(def amped (mnm_am vox_sum (mod am_rate) (mod am_depth)))
(def fenv (mnm_ad_env gate trigger fenv_attack_ms fenv_decay_ms))
(def base_eff (clip (* (+ (mod flt_base) (* gpitch keytrack))
                       (exp (* (log 2) (* fenv (mod env_to_base))))) 20 11000))
(def width_eff (clip (+ (mod flt_width) (* fenv (mod env_to_width))) 0.05 9))
(def filtered (mnm_filter amped base_eff width_eff flt_res_lo flt_res_hi))
(def crushed (mnm_srr filtered (mod srr)))
(def eqd (mnm_eq crushed (mod eq_freq) eq_q (mod eq_gain_db)))
(def driven (tanh (* eqd (mod drive))))
(def amp_env (mnm_ahd_env gate trigger amp_attack_ms amp_hold_ms amp_decay_ms amp_release_ms))
(def voiced (* driven amp_env velocity (clip (mod gain) 0 1)))
(def wdel (delay voiced (+ 4 (* pan_width 36))))
(out (- voiced (* wdel pan_width 0.35)) 1 @name left)
(out (+ voiced (* wdel pan_width 0.35)) 2 @name right)
