; filterbank — the builtin Filterbank (Sherman Filterbank 2 style,
; crates/sequencer/src/effects/filterbank.rs) as patchable DGenLisp, for one
; mono voice or effect channel.
;
;   (filterbank x f1 f2 mode1 mode2 res drive_db crunch fm corr par harm mix)
; and, in its own library entry (content/defmacros/filterbank-sc):
;   (filterbank-sc x fm_in f1 f2 mode1 mode2 res drive_db crunch fm corr par harm mix)
;
; LEVEL MATTERS: like the effect, it expects a track-level input (peaks
; around 0.1-0.3, i.e. -10..-20 dBFS). A full-scale oscillator already
; saturates the drive at 0 dB, so drive_db then changes little; scale the
; source down (or apply the amp envelope/gain) before the filterbank.
;
; Everything is a signal, so any argument can be an envelope, LFO or knob:
;   x        input
;   fm_in    (filterbank-sc only) FM sidechain; filterbank uses the driven input
;   f1 f2    filter cutoffs in Hz (f2 ignored when harm > 0)
;   mode1/2  0 LP .. 0.5 BP .. 1 HP morph per filter
;   res      resonance in percent, 0..110 (self-oscillates near 100)
;   drive_db input drive, -6..30 dB (tube/diode shaper with dynamic bias)
;   crunch   0..1, switched-capacitor clock ratio 100:1 -> 25:1 (stepping,
;            aliasing and clock bleed as cutoffs fall)
;   fm       0..1, audio-rate cutoff FM depth (+-4 octaves at 1)
;   corr     0..1, correction: subtracts band-pass from each output
;   par      0 serial (F1 -> F2) .. 1 parallel
;   harm     0 free, 1..11 link F2 = F1 / {1 1.5 2 3 4 5 6 8 9 12 16}
;   mix      0..1 equal-power dry/wet
;
; Building blocks for custom chains: filterbank-drive (input stage),
; filterbank-core (two clocked filters from an already-driven signal),
; filterbank-svf (one switched-capacitor SVF).
; Deliberate differences from the builtin: no 4x oversampling in the drive
; (it aliases by design), no Sense trigger / internal envelope / LFO / AM —
; patch your own modulation into f1/f2/res/mode instead.

(defmacro filterbank-coeff (ms)
  (- 1.0 (exp (/ -1.0 (max 0.01 (* ms 0.001 samplerate))))))

(defmacro filterbank-pole (hz)
  (- 1.0 (exp (/ (* -6.283185307 hz) samplerate))))

; Input stage (filterbank.rs §2): dynamic-bias coupling-cap sag, +6 dB
; pre-emphasis at 3 kHz, 0.55 tube + 0.45 diode asymmetric shaper, matched
; de-emphasis, 10 Hz DC block.
(defmacro filterbank-drive (sig drive_db)
  (def gained (* sig (exp (* 0.1151292546 drive_db))))
  (make-history dbias_h)
  (def bmag (abs gained))
  (def bprev (read-history dbias_h))
  (def benv (+ bprev (* (gswitch (gt bmag bprev) (filterbank-coeff 2.0) (filterbank-coeff 80.0)) (- bmag bprev))))
  (write-history dbias_h benv)
  (def dbias (* 0.22 (tanh benv)))
  (def ecoef (filterbank-pole 3000.0))
  (make-history demph_h)
  (def emph_lp (+ (read-history demph_h) (* ecoef (- gained (read-history demph_h)))))
  (write-history demph_h emph_lp)
  (def sh_in (+ gained (- gained emph_lp) dbias))
  (def tube_u (max sh_in -2.4))
  (def sh_tube (tanh (+ tube_u (* 0.2 tube_u tube_u))))
  (def dpos (gswitch (lt sh_in 0.35) sh_in
              (+ 0.35 (/ (- 1.0 (exp (* -3.0 (max (- sh_in 0.35) 0.0)))) 3.0))))
  (def sh_diode (gswitch (gte sh_in 0.0) dpos (* 1.2 (tanh (/ sh_in 1.2)))))
  (def shaped (+ (* 0.55 sh_tube) (* 0.45 sh_diode)))
  (make-history ddeemph_h)
  (def deemph_lp (+ (read-history ddeemph_h) (* ecoef (- shaped (read-history ddeemph_h)))))
  (write-history ddeemph_h deemph_lp)
  (def de (- shaped (* 0.5 (- shaped deemph_lp))))
  (def dc_r (exp (/ (* -6.283185307 10.0) samplerate)))
  (make-history ddcx_h)
  (make-history ddcy_h)
  (def y (+ (- de (read-history ddcx_h)) (* dc_r (read-history ddcy_h))))
  (write-history ddcx_h de)
  (write-history ddcy_h y)
  y)

; One switched-capacitor SVF (svf_clocked_tick + filter_channel). While
; ratio*fc >= sr it is a plain per-sample Simper SVF; below, the input is
; band-limited at 0.45 fclk, the core updates only on clock ticks with a
; constant g = tan(pi/ratio), states hold between ticks and are reconstructed
; at 0.5 fclk, with live input feedthrough on hp. Output = LP->BP->HP morph
; minus corr*bp plus clock bleed (bleed_amt, a square at the clock).
(defmacro filterbank-svf (x fc ratio k mode corr bleed_amt)
  (def fclk_raw (* ratio fc))
  (def over (gte fclk_raw (* samplerate 0.999)))
  (def fclk (clip fclk_raw 200 (* samplerate 0.999)))
  (make-history aa_h)
  (def aa (+ (read-history aa_h) (* (filterbank-pole (* 0.45 fclk)) (- x (read-history aa_h)))))
  (write-history aa_h aa)
  (make-history clk_h)
  (def cph (+ (read-history clk_h) (/ fclk samplerate)))
  (def tick_raw (gte cph 1.0))
  (write-history clk_h (- cph tick_raw))
  (def tick (max over tick_raw))
  (def xin (gswitch over x aa))
  (def g (gswitch over
           (tan (* pi (/ (clip fc 1 (* samplerate 0.49)) samplerate)))
           (tan (/ pi (max ratio 2.1)))))
  (def kk (max k (/ -0.9 (max g 0.0001))))
  (make-history ic1_h)
  (make-history ic2_h)
  (def ic1 (read-history ic1_h))
  (def ic2 (read-history ic2_h))
  (def a1 (/ 1.0 (+ 1.0 (* g (+ g kk)))))
  (def a2 (* g a1))
  (def a3 (* g a2))
  (def v3 (- xin ic2))
  (def v1 (+ (* a1 ic1) (* a2 v3)))
  (def v2 (+ ic2 (* a2 ic1) (* a3 v3)))
  (def n1 (* 4.0 (tanh (/ (- (* 2.0 v1) ic1) 4.0))))
  (def n2 (- (* 2.0 v2) ic2))
  (write-history ic1_h (gswitch tick n1 ic1))
  (write-history ic2_h (gswitch tick n2 ic2))
  (make-history hlp_h)
  (make-history hbp_h)
  (def hold_lp (gswitch tick v2 (read-history hlp_h)))
  (def hold_bp (gswitch tick v1 (read-history hbp_h)))
  (write-history hlp_h hold_lp)
  (write-history hbp_h hold_bp)
  (def rc (filterbank-pole (* 0.5 fclk)))
  (make-history slp_h)
  (make-history sbp_h)
  (def slp (+ (read-history slp_h) (* rc (- hold_lp (read-history slp_h)))))
  (def sbp (+ (read-history sbp_h) (* rc (- hold_bp (read-history sbp_h)))))
  (write-history slp_h (gswitch over v2 slp))
  (write-history sbp_h (gswitch over v1 sbp))
  (def lp (gswitch over v2 slp))
  (def bp (gswitch over v1 sbp))
  (def hpo (- x (* kk bp) lp))
  (def m (clip mode 0 1))
  (def w_lp (clip (- 1.0 (* 2.0 m)) 0 1))
  (def w_hp (clip (- (* 2.0 m) 1.0) 0 1))
  (def w_bp (- 1.0 w_lp w_hp))
  (+ (* w_lp lp) (* w_bp bp) (* w_hp hpo) (* -1.0 corr bp)
     (* bleed_amt (- (* 2.0 (lt cph 0.5)) 1.0))))

; Two clocked filters on an already-driven signal `post`, FM'd by fm_sig.
; Returns the wet signal (no dry mix).
(defmacro filterbank-core (post fm_sig f1 f2 harm mode1 mode2 res crunch fm corr par)
  (def fm_oct (* (tanh fm_sig) (clip fm 0 1) 4.0))
  (def cr (clip crunch 0 1))
  (def ratio (* 100.0 (exp (* cr -1.386294361))))
  (def f1_hz (clip (* f1 (exp (* 0.6931471806 fm_oct))) 20 (* samplerate 0.45)))
  (def harm_ratio (selector (+ (clip (round harm) 0 11) 1) 0 1 1.5 2 3 4 5 6 8 9 12 16))
  (def f2_free (clip (* f2 (exp (* 0.6931471806 fm_oct))) 20 (* samplerate 0.45)))
  ; linked: F2 follows the MODULATED F1 down by the ratio
  (def f2_hz (gswitch (gt harm_ratio 0.5) (clip (/ f1_hz (max harm_ratio 1)) 20 (* samplerate 0.45)) f2_free))
  (def k (max (- 2.0 (* (clip res 0 110) 0.02)) -0.25))
  (make-history pres_h)
  (def pa (abs post))
  (def pprev (read-history pres_h))
  (def pres_env (+ pprev (* (gswitch (gt pa pprev) (filterbank-coeff 1.0) (filterbank-coeff 50.0)) (- pa pprev))))
  (write-history pres_h pres_env)
  (def presence (clip (* 320.0 pres_env) 0 1))
  (def bleed1 (* cr 0.02 presence (clip (- 1.0 (/ (* ratio f1_hz) 12000.0)) 0 1)))
  (def bleed2 (* cr 0.02 presence (clip (- 1.0 (/ (* ratio f2_hz) 12000.0)) 0 1)))
  (def sp (clip par 0 1))
  (def o1 (filterbank-svf post f1_hz ratio k mode1 (clip corr 0 1) bleed1))
  (def o2 (filterbank-svf (mix o1 post sp) f2_hz ratio k mode2 (clip corr 0 1) bleed2))
  (+ (* (- 1.0 sp) o2) (* sp 0.5 (+ o1 o2))))

(defmacro filterbank (x f1 f2 mode1 mode2 res drive_db crunch fm corr par harm mix)
  (def post (filterbank-drive x drive_db))
  (def wet (filterbank-core post post f1 f2 harm mode1 mode2 res crunch fm corr par))
  (def theta (* (clip mix 0 1) 1.570796327))
  (+ (* x (cos theta)) (* wet (sin theta))))
