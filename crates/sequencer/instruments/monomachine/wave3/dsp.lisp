; Monomachine DPRO-spirit wavetable machine (wave3): dpro-wave-v2 modernized.
; Real 512x32 factory bank + scan slew + hard sync (from v2), plus DPRO phase
; morph, 12-bit quantize, a stacked stepped-interval second voice, and the
; shared MnM track chain. See docs/monomachine-family-spec.md and CHAIN.md.

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

(def waves (wavetable @shape [512 32] @file "waves/factory.json"))

; ---- DPRO wave machine params ----
(param wave          @default 7    @min 1    @max 32 @mod true @mod-mode additive)
(param wp            @default 0    @min 0    @max 127 @mod true @mod-mode additive)
(param phase_morph   @default 0    @min 0    @max 1 @mod true @mod-mode additive)
(param sync_mode     @default 0    @min 0    @max 2)
(param sfrq          @default 440  @min 20   @max 8000 @unit Hz @mod true @mod-mode additive)
(param bits          @default 12   @min 4    @max 16 @mod true @mod-mode additive)
(param stack_semi    @default 12   @min -24  @max 24 @mod true @mod-mode additive)
(param stack_level   @default 0    @min 0    @max 1 @mod true @mod-mode additive)
(param stack_detune  @default 6    @min -50  @max 50 @unit cents)

(param amp_attack_ms  @default 1    @min 0.2 @max 2000 @unit ms)
(param amp_hold_ms    @default 60   @min 0   @max 4000 @unit ms)
(param amp_decay_ms   @default 300  @min 5   @max 8000 @unit ms)
(param amp_release_ms @default 80   @min 2   @max 8000 @unit ms)
(param glide_ms       @default 0    @min 0   @max 1500 @unit ms)

(param flt_base       @default 80   @min 20  @max 11000 @unit Hz @mod true @mod-mode additive)
(param flt_width      @default 6.5  @min 0.1 @max 9 @mod true @mod-mode additive)
(param flt_res_lo     @default 1.0  @min 0.5 @max 14)
(param flt_res_hi     @default 0.7  @min 0.5 @max 14)
(param fenv_attack_ms @default 2    @min 0.2 @max 4000 @unit ms)
(param fenv_decay_ms  @default 260  @min 5   @max 8000 @unit ms)
(param env_to_base    @default 1.0  @min -6  @max 6 @mod true @mod-mode additive)
(param env_to_width   @default 0    @min -8  @max 8 @mod true @mod-mode additive)
(param keytrack       @default 0.2  @min 0   @max 2)

(param am_rate        @default 55   @min 0.1 @max 2200 @unit Hz @mod true @mod-mode additive)
(param am_depth       @default 0    @min 0   @max 1 @mod true @mod-mode additive)
(param srr            @default 0    @min 0   @max 1 @mod true @mod-mode additive)
(param eq_freq        @default 1200 @min 40  @max 11000 @unit Hz @mod true @mod-mode additive)
(param eq_gain_db     @default 0    @min -30 @max 30 @unit dB @mod true @mod-mode additive)
(param eq_q           @default 2.2  @min 0.5 @max 12)
(param drive          @default 1.3  @min 0.5 @max 8 @mod true @mod-mode additive)
(param pan_width      @default 0.15 @min 0   @max 1)
(param gain           @default 0.3  @min 0   @max 1 @mod true @mod-mode additive)

; ---- wavetable voice ----
(def gpitch (mnm_glide pitch glide_ms))

; hard sync source (off / fixed Hz / keytracked offset), from dpro-wave-v2
(def sync_on (gt sync_mode 0.5))
(def sync_master (phasor gpitch))
(make-history sync_hist)
(def sync_prev (read-history sync_hist))
(def sync_wrap (* (lt sync_master sync_prev) sync_on))
(write-history sync_hist sync_master)
(def sync_fixed_freq (clip (mod sfrq) 20 8000))
(def sync_keytrack_freq (clip (+ gpitch (mod sfrq)) 20 12000))
(def sync_slave_freq (gswitch (gt sync_mode 1.5) sync_keytrack_freq sync_fixed_freq))
(def osc_freq (gswitch sync_on sync_slave_freq gpitch))
(def osc_phase (phasor osc_freq sync_wrap))

; wave scan: base wave + wp sweeps toward the end of the bank, slewed except
; at note start (snap so p-locked waves land instantly), from dpro-wave-v2
(def wp_steps (clip (mod wp) 0 127))
(def base_wave (clip (mnm_floor (- (mod wave) 1)) 0 31))
(def scan_span (- 31 base_wave))
(def scan_target (+ base_wave (* (/ wp_steps 127) scan_span)))
(make-history scan_hist)
(def scan_prev (read-history scan_hist))
(def scan_slew_coeff (- 1.0 (exp (/ -1.0 (* 0.012 samplerate)))))
(def scan_pos
  (gswitch trigger
    scan_target
    (+ scan_prev (* scan_slew_coeff (- scan_target scan_prev)))))
(write-history scan_hist scan_pos)

; DPRO-style phase morph: sine-warped table readout
(def pm (clip (mod phase_morph) 0 1))
(def warped_phase (wrap (+ osc_phase (* (sin (* twopi osc_phase)) pm 0.35)) 0 1))
(def main_wave (wavetable-read-512 waves (clip scan_pos 0 31) warped_phase))

; stacked second voice at a stepped interval, same table position
(def st_semi (clip (mnm_floor (mod stack_semi)) -24 24))
(def st_freq (clip (* gpitch (semi_ratio (+ st_semi (/ stack_detune 100)))) 1 16000))
(def st_phase (phasor st_freq))
(def st_warped (wrap (+ st_phase (* (sin (* twopi st_phase)) pm 0.35)) 0 1))
(def st_level (clip (mod stack_level) 0 1))
(def stack_wave (* (wavetable-read-512 waves (clip scan_pos 0 31) st_warped) st_level))

(def summed (/ (+ main_wave stack_wave) (+ 1 (* st_level 0.7))))
(def bits_ctl (clip (mod bits) 4 16))
(def src (gswitch (gte bits_ctl 15.5) summed (mnm_bits summed bits_ctl)))

; ---- MnM track chain tail ----
(def amped (mnm_am src (mod am_rate) (mod am_depth)))
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
