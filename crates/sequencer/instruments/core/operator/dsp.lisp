; Operator-style 4-op FM synth modeled after Ableton Operator.
; Four operators (A carrier-most, D modulator-most) routed through 11
; branchless FM algorithms (per-edge gain selectors, computed D->C->B->A
; per sample). Operator level = carrier volume AND FM index; FM Drive
; offsets modulator levels only. Feedback is enabled only on operators
; with no incoming FM edges in the current algorithm. Ratio (coarse+fine)
; or fixed-Hz mode per op, per-op ADSR + velocity, pitch envelope, an
; LFO with Hz/ratio (audio-rate) modes, multimode filter (LP12/LP24/BP/
; HP/notch/morph) with drive + own envelope, post-filter waveshaper,
; tone damping, glide and stereo spread.

(def gate (in 1 @name gate))
(def pitch (in 2 @name pitch))
(def velocity (in 3 @name velocity))
(def trigger (in 4 @name trigger))
(def clock (in 5 @name clock))
(def mod1 (in 6 @name mod1 @modulator 1))
(def mod2 (in 7 @name mod2 @modulator 2))
(def mod3 (in 8 @name mod3 @modulator 3))
(def mod4 (in 9 @name mod4 @modulator 4))

(defmacro semi_ratio (semi)
  (exp (/ (* (log 2) semi) 12)))

(defmacro dbamp (db)
  (exp (* 0.1151292546 db)))

; Latch a signal when trig fires; hold it otherwise (per-voice randoms).
(defmacro latch_on_trigger (sig trig)
  (make-history hold_hist)
  (def held (read-history hold_hist))
  (def latched (gswitch (gt trig 0.5) sig held))
  (write-history hold_hist latched)
  latched)

; Resettable phase accumulator: restarts at 0 when reset fires.
(defmacro retrig_phasor (freq reset)
  (make-history ph_hist)
  (def prev_ph (read-history ph_hist))
  (def next_ph (wrap (+ prev_ph (/ freq samplerate)) 0 1))
  (def ph (gswitch (gt reset 0.5) 0.0 next_ph))
  (write-history ph_hist ph)
  ph)

; ---- operator params (A is the always-carrier, D the deepest modulator) ----
(param opa_on @default 1 @min 0 @max 1)
(param opa_wave @default 0 @min 0 @max 6)
(param opa_coarse @default 1 @min 0 @max 32)
(param opa_fine @default 0 @min -1 @max 1)
(param opa_fixed @default 0 @min 0 @max 1)
(param opa_freq_hz @default 440 @min 0.1 @max 20000 @unit Hz)
(param opa_level_db @default -6 @min -60 @max 0 @unit dB @mod true @mod-mode additive @mod-depth-min -24 @mod-depth-max 24 @mod-unit dB)
(param opa_vel @default 0.35 @min 0 @max 1)
(param opa_attack @default 2 @min 0.1 @max 10000 @unit ms)
(param opa_decay @default 800 @min 1 @max 20000 @unit ms)
(param opa_sustain @default 1 @min 0 @max 1)
(param opa_release @default 150 @min 1 @max 20000 @unit ms)

(param opb_on @default 1 @min 0 @max 1)
(param opb_wave @default 0 @min 0 @max 6)
(param opb_coarse @default 1 @min 0 @max 32)
(param opb_fine @default 0 @min -1 @max 1)
(param opb_fixed @default 0 @min 0 @max 1)
(param opb_freq_hz @default 440 @min 0.1 @max 20000 @unit Hz)
(param opb_level_db @default -60 @min -60 @max 0 @unit dB @mod true @mod-mode additive @mod-depth-min -24 @mod-depth-max 24 @mod-unit dB)
(param opb_vel @default 0 @min 0 @max 1)
(param opb_attack @default 2 @min 0.1 @max 10000 @unit ms)
(param opb_decay @default 800 @min 1 @max 20000 @unit ms)
(param opb_sustain @default 1 @min 0 @max 1)
(param opb_release @default 150 @min 1 @max 20000 @unit ms)

(param opc_on @default 1 @min 0 @max 1)
(param opc_wave @default 0 @min 0 @max 6)
(param opc_coarse @default 1 @min 0 @max 32)
(param opc_fine @default 0 @min -1 @max 1)
(param opc_fixed @default 0 @min 0 @max 1)
(param opc_freq_hz @default 440 @min 0.1 @max 20000 @unit Hz)
(param opc_level_db @default -60 @min -60 @max 0 @unit dB @mod true @mod-mode additive @mod-depth-min -24 @mod-depth-max 24 @mod-unit dB)
(param opc_vel @default 0 @min 0 @max 1)
(param opc_attack @default 2 @min 0.1 @max 10000 @unit ms)
(param opc_decay @default 800 @min 1 @max 20000 @unit ms)
(param opc_sustain @default 1 @min 0 @max 1)
(param opc_release @default 150 @min 1 @max 20000 @unit ms)

(param opd_on @default 1 @min 0 @max 1)
(param opd_wave @default 0 @min 0 @max 6)
(param opd_coarse @default 1 @min 0 @max 32)
(param opd_fine @default 0 @min -1 @max 1)
(param opd_fixed @default 0 @min 0 @max 1)
(param opd_freq_hz @default 440 @min 0.1 @max 20000 @unit Hz)
(param opd_level_db @default -60 @min -60 @max 0 @unit dB @mod true @mod-mode additive @mod-depth-min -24 @mod-depth-max 24 @mod-unit dB)
(param opd_vel @default 0 @min 0 @max 1)
(param opd_attack @default 2 @min 0.1 @max 10000 @unit ms)
(param opd_decay @default 800 @min 1 @max 20000 @unit ms)
(param opd_sustain @default 1 @min 0 @max 1)
(param opd_release @default 150 @min 1 @max 20000 @unit ms)

; ---- FM router ----
(param algorithm @default 0 @min 0 @max 10)
(param fm_drive_db @default 0 @min -24 @max 24 @unit dB @mod true @mod-mode additive @mod-depth-min -24 @mod-depth-max 24 @mod-unit dB)
(param feedback @default 0 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)

; ---- pitch envelope ----
(param penv_amount @default 0 @min -48 @max 48 @unit st @mod true @mod-mode additive @mod-depth-min -48 @mod-depth-max 48 @mod-unit st)
(param penv_attack @default 1 @min 0.1 @max 10000 @unit ms)
(param penv_decay @default 300 @min 1 @max 20000 @unit ms)
(param penv_sustain @default 0 @min 0 @max 1)
(param penv_release @default 200 @min 1 @max 20000 @unit ms)

; ---- LFO (ratio mode keytracks like a 5th audio-rate oscillator) ----
(param lfo_wave @default 0 @min 0 @max 5)
(param lfo_mode @default 0 @min 0 @max 1)
(param lfo_rate_hz @default 5 @min 0.01 @max 40 @unit Hz)
(param lfo_ratio @default 1 @min 0.01 @max 8)
(param lfo_retrig @default 1 @min 0 @max 1)
(param lfo_amount @default 0 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
(param lfo_to_pitch @default 0.3 @min -36 @max 36 @unit st)
(param lfo_to_filter @default 0 @min -8 @max 8 @unit oct)

; ---- filter + shaper ----
(param filter_on @default 1 @min 0 @max 1)
(param filter_type @default 0 @min 0 @max 5)
(param filter_freq @default 12000 @min 20 @max 18000 @unit Hz @mod true @mod-mode additive @mod-depth-min -9000 @mod-depth-max 9000 @mod-unit Hz)
(param filter_res @default 0.1 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
(param filter_drive @default 0 @min 0 @max 1)
(param filter_keytrack @default 0 @min 0 @max 1)
(param filter_morph @default 0 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
(param fenv_amt @default 0 @min -8 @max 8 @unit oct)
(param fenv_attack @default 1 @min 0.1 @max 10000 @unit ms)
(param fenv_decay @default 600 @min 1 @max 20000 @unit ms)
(param fenv_sustain @default 0 @min 0 @max 1)
(param fenv_release @default 200 @min 1 @max 20000 @unit ms)

(param shaper_type @default 0 @min 0 @max 3)
(param shaper_drive_db @default 0 @min -12 @max 36 @unit dB @mod true @mod-mode additive @mod-depth-min -24 @mod-depth-max 24 @mod-unit dB)
(param shaper_wet @default 0 @min 0 @max 1)

; ---- global ----
(param tone @default 1 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
(param transpose @default 0 @min -48 @max 48 @unit st)
(param glide_ms @default 0 @min 0 @max 1000 @unit ms)
(param spread @default 0 @min 0 @max 1)
(param voice_pan @default 0 @min -1 @max 1)
(param volume_db @default -12 @min -36 @max 6 @unit dB @mod true @mod-mode additive @mod-depth-min -24 @mod-depth-max 24 @mod-unit dB)

; ---- glide / base pitch (Hz) ----
(def glide_coeff (exp (/ -1.0 (max 1.0 (* glide_ms 0.001 samplerate)))))
(make-history glide_hist)
(def prev_glide (read-history glide_hist))
(def glide_pitch (+ (* pitch (- 1 glide_coeff)) (* prev_glide glide_coeff)))
(def base_pitch (gswitch (gt glide_ms 0.1) glide_pitch pitch))
(write-history glide_hist base_pitch)

; ---- per-voice spread randoms (latched at note start) ----
(def rnd_pan (latch_on_trigger (noise) trigger))
(def rnd_det (latch_on_trigger (noise) trigger))
(def spread_semis (* rnd_det spread spread 0.5))
(def note_hz (* base_pitch (semi_ratio (+ transpose spread_semis))))

; ---- envelopes ----
(def env_a (adsr gate trigger opa_attack opa_decay opa_sustain opa_release))
(def env_b (adsr gate trigger opb_attack opb_decay opb_sustain opb_release))
(def env_c (adsr gate trigger opc_attack opc_decay opc_sustain opc_release))
(def env_d (adsr gate trigger opd_attack opd_decay opd_sustain opd_release))
(def penv (adsr gate trigger penv_attack penv_decay penv_sustain penv_release))
(def fenv (adsr gate trigger fenv_attack fenv_decay fenv_sustain fenv_release))

; ---- LFO ----
(def lfo_freq (clip (gswitch (gte lfo_mode 0.5)
                             (* note_hz lfo_ratio)
                             lfo_rate_hz)
                    0.01 (* samplerate 0.45)))
(def lfo_reset (* lfo_retrig trigger))
(def lfo_ph (retrig_phasor lfo_freq lfo_reset))
(make-history lfo_ph_prev_hist)
(def lfo_prev_ph (read-history lfo_ph_prev_hist))
(def lfo_wrapped (lt lfo_ph lfo_prev_ph))
(write-history lfo_ph_prev_hist lfo_ph)
(def lfo_sh (latch_on_trigger (noise) (max lfo_wrapped lfo_reset)))
(def lfo_idx (clip (round lfo_wave) 0 5))
(def lfo_raw (selector (+ lfo_idx 1)
                (sin (* lfo_ph twopi))
                (triangle lfo_ph)
                (- (* lfo_ph 2) 1)
                (- 1 (* lfo_ph 2))
                (scale (lt lfo_ph 0.5) 0 1 -1 1)
                lfo_sh))
(def lfo (* lfo_raw (clip (mod lfo_amount) 0 1)))

; ---- shared pitch modulation ratio (pitch env + LFO vibrato/FM) ----
(def pitch_mod_ratio (semi_ratio (+ (* penv (clip (mod penv_amount) -48 48))
                                    (* lfo lfo_to_pitch))))

; ---- operator frequency: ratio (coarse+fine) or fixed Hz ----
(defmacro op_freq (coarse fine fixed fixed_hz)
  (def c (clip (round coarse) 0 32))
  (def cbase (gswitch (lt c 0.5) 0.5 c))
  (def ratio (max 0.0625 (+ cbase fine)))
  (clip (* (gswitch (gte fixed 0.5)
                    (clip fixed_hz 0.1 20000)
                    (* note_hz ratio))
           pitch_mod_ratio)
        0.0 (* samplerate 0.45)))

(def f_a (op_freq opa_coarse opa_fine opa_fixed opa_freq_hz))
(def f_b (op_freq opb_coarse opb_fine opb_fixed opb_freq_hz))
(def f_c (op_freq opc_coarse opc_fine opc_fixed opc_freq_hz))
(def f_d (op_freq opd_coarse opd_fine opd_fixed opd_freq_hz))

; ---- algorithm edge gains (alg 0..10), signals flow D -> C -> B -> A ----
(def alg1 (+ (clip (round algorithm) 0 10) 1))
(def g_dc (selector alg1 1 1 0 1 0 1 1 1 0 0 0))
(def g_cb (selector alg1 1 1 1 0 0 0 0 0 0 0 0))
(def g_ba (selector alg1 1 0 1 1 0 1 1 0 1 0 0))
(def g_db (selector alg1 0 0 1 0 0 1 0 0 0 0 0))
(def g_ca (selector alg1 0 0 0 0 1 1 1 0 0 0 0))
(def g_da (selector alg1 0 0 0 0 1 0 0 0 0 1 0))
(def out_b (selector alg1 0 1 0 0 0 0 0 1 0 1 1))
(def out_c (selector alg1 0 0 0 1 0 0 0 1 1 1 1))
(def out_d (selector alg1 0 0 0 0 0 0 0 0 1 0 1))
; feedback only on ops with no incoming FM edge
(def fb_c_ok (- 1 g_dc))
(def fb_b_ok (selector alg1 0 0 0 1 1 0 1 1 1 1 1))
(def fb_a_ok (selector alg1 0 1 0 0 0 0 0 1 0 0 1))

; ---- operator amplitudes ----
; carrier amp = level * env * velocity; modulator amp adds FM Drive.
(defmacro vel_scale (amt)
  (+ (- 1 amt) (* velocity amt)))

(def fm_drive (clip (mod fm_drive_db) -24 24))

(def lvl_a (clip (mod opa_level_db) -60 0))
(def lvl_b (clip (mod opb_level_db) -60 0))
(def lvl_c (clip (mod opc_level_db) -60 0))
(def lvl_d (clip (mod opd_level_db) -60 0))
(def act_a (* (gte opa_on 0.5) (gt lvl_a -59.5) env_a (vel_scale opa_vel)))
(def act_b (* (gte opb_on 0.5) (gt lvl_b -59.5) env_b (vel_scale opb_vel)))
(def act_c (* (gte opc_on 0.5) (gt lvl_c -59.5) env_c (vel_scale opc_vel)))
(def act_d (* (gte opd_on 0.5) (gt lvl_d -59.5) env_d (vel_scale opd_vel)))
(def amp_a (* act_a (dbamp lvl_a)))
(def amp_b (* act_b (dbamp lvl_b)))
(def amp_c (* act_c (dbamp lvl_c)))
(def amp_d (* act_d (dbamp lvl_d)))
(def mamp_b (* act_b (dbamp (clip (+ lvl_b fm_drive) -60 12))))
(def mamp_c (* act_c (dbamp (clip (+ lvl_c fm_drive) -60 12))))
(def mamp_d (* act_d (dbamp (clip (+ lvl_d fm_drive) -60 12))))

; ---- operator core ----
; waveforms: sine, sine 4-bit, sine 8-bit, triangle, saw, square, noise
(defmacro op_wave_sel (widx pm f)
  (def s (sin (* pm twopi)))
  (def s4 (* (round (* s 7.5)) 0.13333334))
  (def s8 (* (round (* s 63.5)) 0.015748031))
  (selector (+ (clip (round widx) 0 6) 1)
    s
    s4
    s8
    (triangle pm)
    (polyblep_saw pm f)
    (polyblep_pulse pm 0.5 f)
    (noise)))

; phase-modulated operator with self-feedback on its previous amped output
(defmacro fm_op (widx f fm_rad fb_gain amp)
  (make-history fb_hist)
  (def prev (read-history fb_hist))
  (def ph (phasor f))
  (def pm (wrap (+ ph (/ (+ fm_rad (* prev fb_gain)) twopi)) 0 1))
  (def raw (op_wave_sel widx pm f))
  (write-history fb_hist (* raw amp))
  raw)

; modulator level -> FM index scale, radians at 0 dB
(def fm_index 13.0)
(def fb_amt (* (clip (mod feedback) 0 1) 7.0))

(def raw_d (fm_op opd_wave f_d 0.0 fb_amt amp_d))
(def fm_c (* fm_index g_dc raw_d mamp_d))
(def raw_c (fm_op opc_wave f_c fm_c (* fb_amt fb_c_ok) amp_c))
(def fm_b (* fm_index (+ (* g_cb raw_c mamp_c) (* g_db raw_d mamp_d))))
(def raw_b (fm_op opb_wave f_b fm_b (* fb_amt fb_b_ok) amp_b))
(def fm_a (* fm_index (+ (* g_ba raw_b mamp_b)
                         (* g_ca raw_c mamp_c)
                         (* g_da raw_d mamp_d))))
(def raw_a (fm_op opa_wave f_a fm_a (* fb_amt fb_a_ok) amp_a))

; A is a carrier in every algorithm
(def carriers (+ (* raw_a amp_a)
                 (* raw_b amp_b out_b)
                 (* raw_c amp_c out_c)
                 (* raw_d amp_d out_d)))

; ---- filter ----
(def fdrive (+ 1.0 (* filter_drive 5.0)))
(def fin (/ (tanh (* carriers fdrive)) fdrive))
(def key_ratio (pow (/ (max base_pitch 8.0) 261.63) filter_keytrack))
(def cut (clip (* (clip (mod filter_freq) 20 18000)
                  key_ratio
                  (semi_ratio (* fenv_amt fenv 12))
                  (semi_ratio (* lfo_to_filter lfo 12)))
               20 18000))
(def fq (+ 0.5 (* (clip (mod filter_res) 0 1) 9.0)))
(def flt_lp (svf fin cut fq 0))
(def flt_lp24 (svf flt_lp cut fq 0))
(def flt_bp (svf fin cut fq 1))
(def flt_hp (svf fin cut fq 2))
(def flt_nt (svf fin cut fq 3))
; morph sweeps LP -> BP -> HP -> notch -> LP
(def mpos (* (clip (mod filter_morph) 0 1) 4.0))
(def mseg (clip (floor mpos) 0 3))
(def mfrac (- mpos mseg))
(def morph_from (selector (+ mseg 1) flt_lp flt_bp flt_hp flt_nt))
(def morph_to (selector (+ mseg 1) flt_bp flt_hp flt_nt flt_lp))
(def flt_morph (+ (* morph_from (- 1 mfrac)) (* morph_to mfrac)))
(def flt_out (selector (+ (clip (round filter_type) 0 5) 1)
               flt_lp flt_lp24 flt_bp flt_hp flt_nt flt_morph))
(def filtered (gswitch (gte filter_on 0.5) flt_out carriers))

; ---- waveshaper (soft / hard / fold / digital) ----
(def sdrv (dbamp (clip (mod shaper_drive_db) -12 36)))
(def shx (* filtered sdrv))
(def sh_soft (tanh shx))
(def sh_hard (clip shx -1 1))
(def sh_fold (sin (* shx 1.5707963)))
(def sh_digi (* (round (* (clip shx -1 1) 7.5)) 0.13333334))
(def shaped (selector (+ (clip (round shaper_type) 0 3) 1)
              sh_soft sh_hard sh_fold sh_digi))
(def swet (clip shaper_wet 0 1))
(def shaper_out (+ filtered (* swet (- shaped filtered))))

; ---- tone: gentle high-frequency damping (4 kHz .. 22 kHz) ----
(def tone_hz (* 4000.0 (exp (* (clip (mod tone) 0 1) 1.7047481))))
(def toned (svf shaper_out tone_hz 0.5 0))

; ---- output ----
(def vol (dbamp (clip (mod volume_db) -36 6)))
(def amp (* toned vol))
(def pan (clip (+ voice_pan (* rnd_pan spread)) -1 1))
(def left (* amp (clip (- 1 (* pan 0.5)) 0 1.5)))
(def right (* amp (clip (+ 1 (* pan 0.5)) 0 1.5)))

(out (tanh left) 1 @name left)
(out (tanh right) 2 @name right)
