# MnM track chain — master copy

Canonical macro block for the Monomachine family (see `docs/monomachine-family-spec.md`).
The instrument lisp has no include mechanism, so every `monomachine/*` instrument copies
this block verbatim into its `dsp.lisp` between the `; === MNM CHAIN BEGIN ===` and
`; === MNM CHAIN END ===` markers. **Drift between an instrument and this file is a bug.**
When you change a macro here, re-copy it into every family member and re-run the harness
checks below.

Standard param block (copy too, tune defaults per machine):

```dgenlisp
(param amp_attack_ms  @default 1    @min 0.2 @max 2000 @unit ms)
(param amp_hold_ms    @default 60   @min 0   @max 4000 @unit ms)
(param amp_decay_ms   @default 260  @min 5   @max 8000 @unit ms)
(param amp_release_ms @default 80   @min 2   @max 8000 @unit ms)
(param glide_ms       @default 0    @min 0   @max 1500 @unit ms)

(param flt_base       @default 120  @min 20  @max 11000 @unit Hz @mod true @mod-mode additive)
(param flt_width      @default 5.5  @min 0.1 @max 9 @mod true @mod-mode additive)
(param flt_res_lo     @default 0.9  @min 0.5 @max 14)
(param flt_res_hi     @default 0.9  @min 0.5 @max 14)
(param fenv_attack_ms @default 2    @min 0.2 @max 4000 @unit ms)
(param fenv_decay_ms  @default 240  @min 5   @max 8000 @unit ms)
(param env_to_base    @default 0    @min -6  @max 6 @mod true @mod-mode additive)
(param env_to_width   @default 0    @min -8  @max 8 @mod true @mod-mode additive)
(param keytrack       @default 0    @min 0   @max 2)

(param am_rate        @default 55   @min 0.1 @max 2200 @unit Hz @mod true @mod-mode additive)
(param am_depth       @default 0    @min 0   @max 1 @mod true @mod-mode additive)
(param srr            @default 0    @min 0   @max 1 @mod true @mod-mode additive)
(param eq_freq        @default 800  @min 40  @max 11000 @unit Hz @mod true @mod-mode additive)
(param eq_gain_db     @default 0    @min -30 @max 30 @unit dB @mod true @mod-mode additive)
(param eq_q           @default 2.2  @min 0.5 @max 12)
(param drive          @default 1.0  @min 0.5 @max 8 @mod true @mod-mode additive)
(param pan_width      @default 0.15 @min 0   @max 1)
(param gain           @default 0.16 @min 0   @max 1 @mod true @mod-mode additive)
```

Macro block:

```dgenlisp
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

; AHD+R envelope: linear attack to 1, hold at 1 for a fixed time, one-pole
; decay to 0 (no sustain stage — the MnM way), one-pole release on gate-off.
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

; Simple AD envelope (attack to 1, decay to 0) for the filter.
(defmacro mnm_ad_env (gate_sig trigger_sig attack_ms decay_ms)
  (mnm_ahd_env gate_sig trigger_sig attack_ms 0 decay_ms decay_ms))

; One-pole portamento on the pitch input; snaps on the first note.
(defmacro mnm_glide (pitch_hz gl_ms)
  (make-history glide_h)
  (def gprev (read-history glide_h))
  (def gcoeff (- 1.0 (exp (/ -1.0 (max 1.0 (* gl_ms 0.001 samplerate))))))
  (def gnext (+ gprev (* gcoeff (- pitch_hz gprev))))
  (def gval (gswitch (lte gprev 0.01) pitch_hz gnext))
  (write-history glide_h gval)
  gval)

; Dual base/width filter: 24dB HP at base, 24dB LP at base * 2^width (octaves).
; Resonance on the first stage of each side, 0.55 on the second. Narrow width
; plus envelope on base/width = the gulp. (12dB slopes leak too much bass for
; the gulp to read — harness-verified.)
(defmacro mnm_filter (input base_hz width_oct res_lo res_hi)
  (def hp_cut (clip base_hz 20 11000))
  (def lp_cut (clip (* hp_cut (exp (* (log 2) (clip width_oct 0.05 9)))) 30 16000))
  (def lp24 (svf (svf input lp_cut res_lo 0) lp_cut 0.55 0))
  (svf (svf lp24 hp_cut res_hi 2) hp_cut 0.55 2))

; Amplitude modulation by an internal sine (depth 0 = bypass).
(defmacro mnm_am (sig rate_hz depth)
  (mix sig (* sig (sin (* twopi (phasor (clip rate_hz 0.05 3000))))) (clip depth 0 1)))

; Sample-rate reduction: latch at a variable hold clock. amount 0..1 maps the
; hold rate log-wise from clean down to ~300 Hz.
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

; Bit quantizer for 12-bit DPRO / 8-bit SID character.
(defmacro mnm_bits (sig bits)
  (def bit_lv (pow 2 bits))
  (- (* (/ (mnm_floor (* (* (+ (clip sig -1 1) 1) 0.5) bit_lv)) bit_lv) 2) 1))

; One-band parallel-resonator EQ; big gains encouraged (honk control, not mixing).
; The preamble svf's BP output peaks at gain q at cutoff (harness-measured), so
; divide by q for a unity-gain bandpass; center boost/cut then lands exactly.
(defmacro mnm_eq (sig freq q gain_db)
  (+ sig (* (- (dbamp gain_db) 1.0) (/ 1.0 q) (svf sig (clip freq 40 12000) q 1))))
; === MNM CHAIN END ===
```

Standard tail wiring (source → chain → stereo out):

```dgenlisp
(def gpitch (mnm_glide pitch glide_ms))
; ... machine-specific source using gpitch → (def src ...)
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
(out (* (- voiced (* wdel pan_width 0.35)) 1) 1 @name left)
(out (+ voiced (* wdel pan_width 0.35)) 2 @name right)
```

## Harness verification (run after any chain change)

Compile a test instrument (preamble + chain + saw source) with DGenLisp, drive the
dylib (see memory: dgenlisp-c-harness-method / custom-instrument-workflow), and check:

1. **AHD env**: RMS envelope shows linear attack, flat hold of `amp_hold_ms`,
   exponential decay to silence *while gate stays high*; release on gate-off.
2. **Gulp**: with narrow `flt_width` + `env_to_base` sweep, the spectral centroid
   sweeps per note; width env widens/narrows the passband.
3. **SRR**: srr=0 is bit-exact passthrough; srr>0 adds alias images.
4. **AM**: sidebands at carrier ± am_rate; depth 0 bypasses.
5. **EQ**: ±30 dB peak realized at eq_freq within ~2 dB.
