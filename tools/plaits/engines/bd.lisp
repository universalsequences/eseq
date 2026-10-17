; ======================================================================
; Bass drum engine (plaits/dsp/engine/bass_drum_engine.cc):
;   OUT = AnalogBassDrum (808-style bridged-T resonator) -> Overdrive
;   AUX = SyntheticBassDrum (distorted-sine "909-ish" kick)
; Unpatched trigger (patched = 0) = sustain mode: free-running oscillators
; with a sustain gain of accent * decay.
; ======================================================================

; stmlib ParameterInterpolator emulated exactly, including the case where
; Next() is not called every sample (the state then lags its target):
; increment = (target - state) / 12 latched at the block start; `called`
; = 1 on the samples where the C++ calls Next(). `init` is the C++ initial
; state (histories start at 0, so the history holds value - init).
(defmacro bd-pi (target bstart called init)
  (make-history h_v)
  (make-history h_inc)
  (def v0 (read-history h_v))
  (def inc (gswitch bstart (/ (- (- target init) v0) 12) (read-history h_inc)))
  (write-history h_inc inc)
  (def v (+ v0 (* called inc)))
  (write-history h_v v)
  (+ v init))

; stmlib Svf whose state only advances while `en` is 1; (tuple lp bp hp).
(defmacro bd-svf (in g r en)
  (make-history s1)
  (make-history s2)
  (def h (/ 1 (+ 1 (* r g) (* g g))))
  (def st1 (read-history s1))
  (def st2 (read-history s2))
  (def hp (* (- in (* r st1) (* g st1) st2) h))
  (def bp (+ (* g hp) st1))
  (def lp (+ (* g bp) st2))
  (write-history s1 (gswitch en (+ (* g hp) bp) st1))
  (write-history s2 (gswitch en (+ (* g bp) lp) st2))
  (tuple lp bp hp))

; stmlib SoftClip.
(defmacro bd-softclip (x)
  (def c (clip x -3 3))
  (/ (* c (+ 27 (* c c))) (+ 27 (* 9 c c))))

(defmacro bd-diode (x)
  (def x2 (* 2 x))
  (gswitch (gte x 0) x (/ (* 0.7 x2) (+ 1 (abs x2)))))

; ---------------------------------------------------------------- analog
(defmacro bd-analog (bstart trig sustain accent f0 tone decay afm sfm)
  (make-history h_pr)      ; pulse_remaining_samples_
  (make-history h_fr)      ; fm_pulse_remaining_samples_
  (make-history h_pulse)   ; pulse_
  (make-history h_height)  ; pulse_height_
  (make-history h_plp)     ; pulse_lp_
  (make-history h_fmlp)    ; fm_pulse_lp_
  (make-history h_retrig)  ; retrig_pulse_
  (make-history h_lpout)   ; lp_out_
  (make-history h_tone)    ; tone_lp_
  (make-history h_ph)      ; oscillator_ phase
  (def ns (- 1 sustain))
  (def scale (/ 0.001 (max f0 0.0000001)))
  (def q (* 1500 (pl-semi-ratio (* decay 80))))
  (def tone_f (min (* 4 f0 (pl-semi-ratio (* tone 108))) 1))
  (def exciter_leak (* 0.08 (+ tone 0.25)))
  (def height (gswitch trig (+ 3 (* 7 accent)) (read-history h_height)))
  (write-history h_height height)
  ; Q39 / Q40 trigger pulse
  (def pr0 (gswitch trig 48 (read-history h_pr)))
  (def pon (gt pr0 0))
  (def pr1 (- pr0 pon))
  (write-history h_pr pr1)
  (def pulse_s (gswitch pon (gswitch (gt pr1 0) height (- height 1))
                            (* (read-history h_pulse) (- 1 (/ 1 9.6)))))
  (write-history h_pulse pulse_s)
  (def pulse_a (* pulse_s ns))
  (def plp (+ (read-history h_plp) (* (/ 1 4.8) (- pulse_a (read-history h_plp)))))
  (write-history h_plp plp)
  (def pulse (bd-diode (+ (- pulse_a plp) (* pulse_a 0.044))))
  ; Q41 / Q42 FM pulse
  (def fr0 (gswitch trig 288 (read-history h_fr)))
  (def fon (gt fr0 0))
  (def fr1 (- fr0 fon))
  (write-history h_fr fr1)
  (def retrig (gswitch fon (gswitch (gt fr1 0) 0 -0.8)
                           (* (read-history h_retrig) (- 1 (/ 1 2400)))))
  (write-history h_retrig retrig)
  (def fm_pulse (* fon ns))
  (def fmlp (+ (read-history h_fmlp) (* (/ 1 4.8) (- fm_pulse (read-history h_fmlp)))))
  (write-history h_fmlp fmlp)
  ; Q43 and R170 leakage
  (def lp_prev (gswitch trig 0 (read-history h_lpout)))
  (def punch (+ 0.7 (bd-diode (- (* 10 lp_prev) 1))))
  (def f (clip (* f0 (+ 1 (* fmlp 1.7 afm) (* punch 0.08 sfm))) 0 0.4))
  ; sustain: sine oscillator
  (def sgain (bd-pi (* accent decay) bstart sustain 0))
  (def ph_prev (read-history h_ph))
  (def ph_inc (+ ph_prev (min f 0.5)))
  (def ph (gswitch sustain (- ph_inc (gte ph_inc 1)) ph_prev))
  (write-history h_ph ph)
  (def osc_sin (* sgain (sin (* twopi ph))))
  (def osc_cos (* sgain (cos (* twopi ph))))
  ; triggered: resonator
  (def (r_lp r_bp r_hp) (bd-svf (* (- pulse (* retrig 0.2)) scale)
                                (pl-tan-dirty f) (/ 1 (+ 1 (* q f))) ns))
  (def res_out (gswitch sustain osc_sin r_bp))
  (write-history h_lpout (gswitch sustain osc_cos r_lp))
  (def tone_lp (+ (read-history h_tone)
                  (* tone_f (- (+ (* pulse exciter_leak) res_out) (read-history h_tone)))))
  (write-history h_tone tone_lp)
  tone_lp)

; plaits Overdrive.
(defmacro bd-overdrive (in drive bstart)
  (def d2 (* drive drive))
  (def pre_a (* drive 0.5))
  (def pre_b (* d2 d2 drive 24))
  (def pre (+ pre_a (* (- pre_b pre_a) d2)))
  (def squashed (* drive (- 2 drive)))
  (def post (/ 1 (max (bd-softclip (+ 0.33 (* squashed (- pre 0.33)))) 0.000001)))
  (def pre_i (bd-pi pre bstart 1 0))
  (def post_i (bd-pi post bstart 1 0))
  (* (bd-softclip (* pre_i in)) post_i))

; ------------------------------------------------------------- synthetic
(defmacro bd-distorted-sine (phase noise dirt)
  (def p0 (+ phase (* noise dirt)))
  (def p (- p0 (pl-trunc p0)))
  (def tri (- (* 4 (gswitch (lt p 0.5) p (- 1 p))) 1))
  (def s (/ (* 2 tri) (+ 1 (abs tri))))
  (+ s (* (- 1 dirt) (- (sin (* twopi (+ p 0.75))) s))))

(defmacro bd-transistor-vca (s gain)
  (def x (* (- s 0.6) gain))
  (+ (/ (* 3 x) (+ 2 (abs x))) (* gain 0.3)))

(defmacro bd-synthetic (bstart bphase trig sustain accent f0 tone decay0 dirt0 fm_amount fm_decay0 u_a u_b)
  (make-history h_ph)       ; phase_
  (make-history h_pn)       ; phase_noise_
  (make-history h_fm)       ; fm_
  (make-history h_fmlp)     ; fm_lp_
  (make-history h_body)     ; body_env_
  (make-history h_bodylp)   ; body_env_lp_
  (make-history h_tr)       ; transient_env_
  (make-history h_trlp)     ; transient_env_lp_
  (make-history h_bpw)      ; body_env_pulse_width_
  (make-history h_fpw)      ; fm_pulse_width_
  (make-history h_tone)     ; tone_lp_
  (make-history h_clp)      ; click lp_
  (make-history h_chp)      ; click hp_
  (make-history h_nlp)      ; attack noise lp_
  (make-history h_nhp)      ; attack noise hp_
  (def ns (- 1 sustain))
  (def decay (* decay0 decay0))
  (def fm_env_decay (* fm_decay0 fm_decay0))
  (def dirt (* dirt0 (max (- 1 (* 8 f0)) 0)))
  (def fm_decay (- 1 (/ 1 (* 0.008 (+ 1 (* fm_env_decay 4)) 48000))))
  (def body_decay (- 1 (* (/ 1 960) (pl-semi-ratio (* decay -60)))))
  (def transient_decay (- 1 (/ 1 240)))
  (def tone_f (min (* 4 f0 (pl-semi-ratio (* tone 108))) 1))
  (def level0 (+ 0.3 (* 0.7 accent)))
  ; phase noise (every sample)
  (def pn (+ (read-history h_pn) (* 0.002 (- (- u_a 0.5) (read-history h_pn)))))
  (write-history h_pn pn)
  ; pulse widths
  (def fpw0 (gswitch trig 62 (read-history h_fpw)))
  (def bpw0 (gswitch trig 48 (read-history h_bpw)))
  (def fpw_on (* ns (gt fpw0 0)))
  (def bpw_on (* ns (gt bpw0 0)))
  (write-history h_fpw (- fpw0 fpw_on))
  (def bpw1 (- bpw0 bpw_on))
  (write-history h_bpw bpw1)
  ; f0 interpolator: Next() in sustain, and when the FM pulse is over
  (def f0_called (+ sustain (* ns (- 1 fpw_on))))
  (def f0_mod (bd-pi f0 bstart f0_called 0))
  ; envelopes (triggered mode only)
  (def fm_prev (gswitch trig 1 (read-history h_fm)))
  (def fmlp_prev (read-history h_fmlp))
  (def dec_on (* ns (- 1 fpw_on)))
  (def fm_v (gswitch dec_on (* fm_prev fm_decay) fm_prev))
  (write-history h_fm fm_v)
  (def fm_mult (+ 1 (* fm_amount 3.5 fmlp_prev)))
  (def ph_prev (read-history h_ph))
  (def ph_t (+ ph_prev (min (* f0_mod fm_mult) 0.5)))
  (def ph_s (+ ph_prev f0_mod))
  (def ph_adv (gswitch sustain ph_s ph_t))
  (def ph_wr (- ph_adv (gte ph_adv 1)))
  (def ph (gswitch fpw_on 0.25 (gswitch f0_called ph_wr ph_prev)))
  (write-history h_ph ph)
  (def body_prev (gswitch trig level0 (read-history h_body)))
  (def tr_prev (gswitch trig level0 (read-history h_tr)))
  (def env_dec (* ns (- 1 bpw_on)))
  (def body_env (gswitch env_dec (* body_prev body_decay) body_prev))
  (def tr_env (gswitch env_dec (* tr_prev transient_decay) tr_prev))
  (write-history h_body body_env)
  (write-history h_tr tr_env)
  (def bodylp (gswitch ns (+ (read-history h_bodylp) (* 0.1 (- body_env (read-history h_bodylp)))) (read-history h_bodylp)))
  (def trlp (gswitch ns (+ (read-history h_trlp) (* 0.1 (- tr_env (read-history h_trlp)))) (read-history h_trlp)))
  (def fmlp (gswitch ns (+ fmlp_prev (* 0.1 (- fm_v fmlp_prev))) fmlp_prev))
  (write-history h_bodylp bodylp)
  (write-history h_trlp trlp)
  (write-history h_fmlp fmlp)
  (def body (bd-distorted-sine ph pn dirt))
  ; click: SLOPE -> ONE_POLE hp -> Svf LP (5 kHz, q 2)
  (def click_in (- 1 (gt bpw1 0)))
  (def clp_prev (read-history h_clp))
  (def cerr (- click_in clp_prev))
  (def clp (gswitch ns (+ clp_prev (* cerr (gswitch (gt cerr 0) 0.5 0.1))) clp_prev))
  (write-history h_clp clp)
  (def chp (gswitch ns (+ (read-history h_chp) (* 0.04 (- clp (read-history h_chp)))) (read-history h_chp)))
  (write-history h_chp chp)
  (def (c_lp c_bp c_hp) (bd-svf (- clp chp) (pl-tan-fast (/ 5000 samplerate)) 0.5 ns))
  ; attack noise
  (def nlp (gswitch ns (+ (read-history h_nlp) (* 0.05 (- u_b (read-history h_nlp)))) (read-history h_nlp)))
  (write-history h_nlp nlp)
  (def nhp (gswitch ns (+ (read-history h_nhp) (* 0.005 (- nlp (read-history h_nhp)))) (read-history h_nhp)))
  (write-history h_nhp nhp)
  (def transient (+ c_lp (- nlp nhp)))
  (def sgain (bd-pi (* accent decay) bstart sustain 0))
  (def mix_s (- (bd-transistor-vca body sgain)))
  (def mix_t (- (- (bd-transistor-vca body bodylp)) (* transient trlp tone)))
  (def mix (gswitch sustain mix_s mix_t))
  (def tone_lp (+ (read-history h_tone) (* tone_f (- mix (read-history h_tone)))))
  (write-history h_tone tone_lp)
  tone_lp)

(defmacro plaits-bd (bstart bphase rise high patched note timbre morph harmonics accent)
  (def nz (noise))   ; per-sample uniform draw (identical (noise) nodes merge)
  (def f0 (pl-note-to-freq note))
  (def sustain (- 1 patched))
  (def trig (* bstart rise))
  (def afm (min (* harmonics 4) 1))
  (def sfm (clip (- (* harmonics 4) 1) 0 1))
  (def drive (* (max (- (* harmonics 2) 1) 0) (max (- 1 (* 16 f0)) 0)))
  (def a_out (bd-analog bstart trig sustain accent f0 timbre morph afm sfm))
  (def out (bd-overdrive a_out (+ 0.5 (* 0.5 drive)) bstart))
  (def dirt0 (gswitch sustain harmonics (- 0.4 (* 0.25 morph morph))))
  (def aux (bd-synthetic bstart bphase trig sustain accent f0 timbre morph dirt0
                         (min (* harmonics 2) 1) (max (- (* harmonics 2) 1) 0)
                         nz (pl-noise-tap nz)))
  (tuple out aux))
