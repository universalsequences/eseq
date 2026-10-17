;@ungated
; ======================================================================
; String engine (plaits/dsp/engine/string_engine.cc): three StringVoices
; (physical_modelling/string_voice.cc + string.cc) that the engine rotates
; through on each rising trigger edge, so a new pluck does not cut the
; previous strings' decay. The string is an extended Karplus-Strong loop
; (1024-sample delay line, Hermite read) with either a curved-bridge
; nonlinearity (harmonics < 0.25) or allpass dispersion (harmonics > 0.25),
; a DC blocker and an SVF damping lowpass in the loop.
; Excitation: a 1/f0-sample noise burst on trigger (white noise, lowpassed),
; or continuous dust when the trigger input is unpatched (the active string
; sustains). out = sum of the strings, aux = sum of the excitations.
; Uses the driver's u0 (uniform [0,1)) as noise source.
; Block-gate split (;@ungated): the delay-line loops (str-string) always
; run; all per-block control math and the exciters run inside
; (block-gate active ...). While inactive the loops are fed zeros and their
; state is cleared (StringEngine::Reset on re-selection), outputs are 0.
; ======================================================================
(def str_lut_svf_shift (tensor @shape [257 1] @file "lut_svf_shift.json"))

; Mutable delay-line memory: String::string_ (1024) and String::stretch_
; (256) for each of the 3 strings.
(def str_line0 (tensor @shape [1024]))
(def str_line1 (tensor @shape [1024]))
(def str_line2 (tensor @shape [1024]))
(def str_ap0 (tensor @shape [256]))
(def str_ap1 (tensor @shape [256]))
(def str_ap2 (tensor @shape [256]))

; Non-negative integer x modulo n (x may be negative but > -n * big).
(defmacro str-imod (x n)
  (- x (* n (floor (/ x n)))))

; History cell that starts at `init` instead of 0. Returns the previous value;
; write the new one with (str-wr ...).
(defmacro str-rd (h ready init)
  (gswitch (read-history ready) (read-history h) init))

; plaits DelayLine::Read(d) for a line whose next write goes to counter c:
; the sample written d writes ago (d = 1 is the most recent).
(defmacro str-tap (buf c d n)
  (peek buf (str-imod (- c d) n)))

; Per-block String coefficients (string.cc ProcessInternal preamble), pure
; scalar: runs inside the block-gate.
(defmacro str-string-ctl (f0 nl brightness damping)
  (def disp (gt nl 0))
  (def amt (abs nl))
  (def dly (clip (/ 1 (max f0 0.000000001)) 4 1020))
  (def src_raw (* dly f0))
  (def src_full (gte src_raw 0.9999))
  (def src_ratio (gswitch src_full 1 src_raw))
  (def dcut0 (min (+ 12 (* damping damping 60) (* brightness 24)) 84))
  (def df0 (min (* f0 (pl-semi-ratio dcut0)) 0.499))
  (def to_inf (gswitch (gte damping 0.95) (* 20 (- damping 0.95)) 0))
  (def br (+ brightness (* to_inf (- 1 brightness))))
  (def df (+ df0 (* to_inf (- 0.4999 df0))))
  (def dcut (+ dcut0 (* to_inf (- 128 dcut0))))
  (def g (pl-tan-fast df))
  (def h (/ 1 (+ 1 (* 2 g) (* g g))))
  (def target (* dly (pl-lut str_lut_svf_shift dcut 1)))
  (def sp (* amt (- 2 amt) 0.225))
  (def sc (clip (* (/ 160 48000) dly) 1 2.1))
  (def nas (gswitch (gt amt 0.75) (* 4 (- amt 0.75)) 0))
  (def namt (* nas nas 0.1))
  (def nfilt (+ 0.06 (* 0.94 br br)))
  (def bcurv (* amt amt 0.01))
  (def apg (/ (* -0.618 amt) (+ 0.15 amt)))
  (tuple disp src_full src_ratio g h target sp sc namt nfilt bcurv apg))

; The String loop (string.cc ProcessInternal per-sample body) with its own
; delay memory `line`/`ap`. Ungated. `active` = 0 forces a tick every sample,
; writes zeros and clears the filter state, so the lines are silent after
; 1024 inactive samples (and the output is 0 meanwhile).
(defmacro str-string (line ap exc_in u bstart active disp src_full src_ratio g h target sp sc namt nfilt bcurv apg)
  (make-history ready)
  (make-history h_ph)
  (make-history h_d)
  (make-history h_inc)
  (make-history h_dn)
  (make-history h_cb)
  (make-history h_c)
  (make-history h_ca)
  (make-history h_dcx)
  (make-history h_dcy)
  (make-history h_s1)
  (make-history h_s2)
  (make-history h_o0)
  (make-history h_o1)
  ; linear-interpolation SRC for f0 below 48000/1020 Hz
  (def ph1 (+ (gswitch src_full 1 (read-history h_ph)) src_ratio))
  (def tick (max (gt ph1 1) (- 1 active)))
  (def ph (- ph1 (gt ph1 1)))
  ; ParameterInterpolator on delay_ (initial 100), advanced once per tick
  (def d_prev (str-rd h_d ready 100))
  (def inc (gswitch bstart (/ (- target d_prev) 12) (read-history h_inc)))
  (def d (+ d_prev (* tick inc)))
  ; dispersion noise / curved bridge
  (def dn_prev (read-history h_dn))
  (def dn_new (+ dn_prev (* nfilt (- (- u 0.5) dn_prev))))
  (def cb_prev (read-history h_cb))
  (def dl (* d (gswitch disp (+ 1 (* dn_new namt)) (- 1 (* cb_prev bcurv)))))
  (def c (read-history h_c))
  (def ca (read-history h_ca))
  ; dispersion: allpass-stretched read
  (def apd (* dl sp))
  (def maind (- dl (* apd (- 0.408 (* sp 0.308)) sc)))
  (def use_ap (* disp (gte apd 4) (gte maind 4)))
  (def mi (floor (max maind 0)))
  (def mf (- maind mi))
  (def la (str-tap line c mi 1024))
  (def lb (str-tap line c (+ mi 1) 1024))
  (def lin (+ la (* (- lb la) mf)))
  (def api (floor (max apd 0)))
  (def ap_read (str-tap ap ca api 256))
  (def ap_write (+ lin (* apg ap_read)))
  (def ap_out (+ (* (- ap_write) apg) ap_read))
  ; Hermite read
  (def hi (floor (max dl 0)))
  (def hf (- dl hi))
  (def xm1 (str-tap line c (- hi 1) 1024))
  (def x0 (str-tap line c hi 1024))
  (def x1 (str-tap line c (+ hi 1) 1024))
  (def x2 (str-tap line c (+ hi 2) 1024))
  (def hc (* (- x1 xm1) 0.5))
  (def hv (- x0 x1))
  (def hw (+ hc hv))
  (def ha (+ hw hv (* (- x2 x0) 0.5)))
  (def hb (+ hw ha))
  (def herm (+ (* (+ (* (- (* ha hf) hb) hf) hc) hf) x0))
  (def s0 (gswitch use_ap ap_out herm))
  (def bv (- (abs s0) 0.025))
  (def cb_new (* (+ (abs bv) bv) (gswitch (gt s0 0) 1 -1.5)))
  (def s1 (clip (+ s0 exc_in) -20 20))
  ; DCBlocker(1 - 20 / 48000)
  (def dcy (+ (* (read-history h_dcy) 0.99958333) s1 (- (read-history h_dcx))))
  ; damping SVF lowpass, r = 2
  (def st1 (read-history h_s1))
  (def st2 (read-history h_s2))
  (def fhp (* (- dcy (* 2 st1) (* g st1) st2) h))
  (def fbp (+ (* g fhp) st1))
  (def flp (* active (+ (* g fbp) st2)))
  (def o0_prev (read-history h_o0))
  (def o1_prev (read-history h_o1))
  (def o0 (* active (gswitch tick flp o0_prev)))
  (def o1 (* active (gswitch tick o0_prev o1_prev)))
  ; Writes. Off-tick pokes land on the slot the next tick overwrites (the
  ; oldest sample, never read), so they are harmless.
  (def wr (seq (poke line c flp)
               (poke ap ca (* active ap_write))
               (+ o1 (* (- o0 o1) ph))))
  (def run (* tick active))
  (write-history ready 1)
  (write-history h_ph (* active ph))
  (write-history h_d (gswitch active d d_prev))
  (write-history h_inc inc)
  (write-history h_dn (* active (gswitch (* tick disp) dn_new dn_prev)))
  (write-history h_cb (* active (gswitch (* tick (- 1 disp)) cb_new cb_prev)))
  (write-history h_c (str-imod (+ c tick) 1024))
  (write-history h_ca (str-imod (+ ca (* tick (max use_ap (- 1 active)))) 256))
  (write-history h_dcx (gswitch run s1 (* active (read-history h_dcx))))
  (write-history h_dcy (gswitch run dcy (* active (read-history h_dcy))))
  (write-history h_s1 (gswitch run (+ (* g fhp) fbp) (* active st1)))
  (write-history h_s2 (gswitch run (+ (* g fbp) flp) (* active st2)))
  (write-history h_o0 o0)
  (write-history h_o1 o1)
  wr)

; One StringVoice (string_voice.cc) minus the String loop: returns the
; excitation and the String's per-block coefficients.
(defmacro str-voice (f0 sus trig accent structure brightness damping u bstart)
  (def density (* brightness brightness))
  (def b (+ brightness (* 0.25 accent (- 1 brightness))))
  (def dmp (+ damping (* 0.25 accent (- 1 damping))))
  (def eset (max sus trig))
  (def cutoff (min (* 4 f0 (pl-semi-ratio (* (- (* b (- 2 b)) 0.5) 72))) 0.499))
  (make-history ready)
  (make-history h_g)
  (make-history h_r)
  (make-history h_rem)
  (make-history h_e1)
  (make-history h_e2)
  ; excitation_filter_ coefficients: set on trigger/sustain, else kept
  ; (Init: f = 0.01, q = 100)
  (def eg (gswitch eset (pl-tan-dirty cutoff) (str-rd h_g ready 0.0314275104)))
  (def er (gswitch eset (gswitch sus 1 2) (str-rd h_r ready 0.01)))
  (def eh (/ 1 (+ 1 (* er eg) (* eg eg))))
  ; remaining_noise_samples_
  (def rem0 (gswitch (* bstart eset) (floor (/ 1 (max f0 0.000001))) (read-history h_rem)))
  (def burst (* (gt rem0 0) (- 1 sus)))
  (def dust_f (+ 0.00005 (* 0.99995 density density)))
  (def dust (gswitch (lt u dust_f) (/ u dust_f) 0))
  (def exc (gswitch sus (* dust (- 8 (* dust_f 6)) accent) (* burst (- (* 2 u) 1))))
  (def e1 (read-history h_e1))
  (def e2 (read-history h_e2))
  (def ehp (* (- exc (* er e1) (* eg e1) e2) eh))
  (def ebp (+ (* eg ehp) e1))
  (def elp (+ (* eg ebp) e2))
  (write-history ready 1)
  (write-history h_g eg)
  (write-history h_r er)
  (write-history h_rem (- rem0 burst))
  (write-history h_e1 (+ (* eg ehp) ebp))
  (write-history h_e2 (+ (* eg ebp) elp))
  (def nl (gswitch (lt structure 0.24) (* (- structure 0.24) 4.166)
                   (gswitch (gt structure 0.26) (* (- structure 0.26) 1.35135) 0)))
  (def (disp src_full src_ratio g h target sp sc namt nfilt bcurv apg) (str-string-ctl f0 nl b dmp))
  (tuple elp disp src_full src_ratio g h target sp sc namt nfilt bcurv apg))

; f0_delay_ (DelayLine<float, 16>) written once per block: Read(14) on the
; first sample of a block = the value written 14 blocks ago. Scalar
; histories instead of `delay` so it can sit inside the block-gate.
(defmacro str-block-delay14 (x bstart)
  (make-history d1) (make-history d2) (make-history d3) (make-history d4)
  (make-history d5) (make-history d6) (make-history d7) (make-history d8)
  (make-history d9) (make-history d10) (make-history d11) (make-history d12)
  (make-history d13) (make-history d14)
  (def r14 (read-history d14))
  (write-history d14 (gswitch bstart (read-history d13) r14))
  (write-history d13 (gswitch bstart (read-history d12) (read-history d13)))
  (write-history d12 (gswitch bstart (read-history d11) (read-history d12)))
  (write-history d11 (gswitch bstart (read-history d10) (read-history d11)))
  (write-history d10 (gswitch bstart (read-history d9) (read-history d10)))
  (write-history d9 (gswitch bstart (read-history d8) (read-history d9)))
  (write-history d8 (gswitch bstart (read-history d7) (read-history d8)))
  (write-history d7 (gswitch bstart (read-history d6) (read-history d7)))
  (write-history d6 (gswitch bstart (read-history d5) (read-history d6)))
  (write-history d5 (gswitch bstart (read-history d4) (read-history d5)))
  (write-history d4 (gswitch bstart (read-history d3) (read-history d4)))
  (write-history d3 (gswitch bstart (read-history d2) (read-history d3)))
  (write-history d2 (gswitch bstart (read-history d1) (read-history d2)))
  (write-history d1 (gswitch bstart x (read-history d1)))
  r14)

; Gated part of the engine: string rotation, f0s, exciters, coefficients.
(defmacro str-control (bstart rise patched note timbre morph harmonics accent u0)
  (def f0 (pl-note-to-freq note))
  (def f0_old (str-block-delay14 f0 bstart))
  (def edge (* bstart rise))
  ; active_string_ starts at 2 and advances on each rising edge
  (make-history h_k)
  (def a_prev (str-imod (+ (read-history h_k) 2) 3))
  (def a (gswitch edge (str-imod (+ a_prev 1) 3) a_prev))
  (write-history h_k (str-imod (- a 2) 3))
  (make-history ready)
  (make-history h_f0)
  (make-history h_f1)
  (make-history h_f2)
  (def f_0 (gswitch (eq a 0) f0 (gswitch (* edge (eq a_prev 0)) f0_old (str-rd h_f0 ready 0.01))))
  (def f_1 (gswitch (eq a 1) f0 (gswitch (* edge (eq a_prev 1)) f0_old (str-rd h_f1 ready 0.01))))
  (def f_2 (gswitch (eq a 2) f0 (gswitch (* edge (eq a_prev 2)) f0_old (str-rd h_f2 ready 0.01))))
  (write-history ready 1)
  (write-history h_f0 f_0)
  (write-history h_f1 f_1)
  (write-history h_f2 f_2)
  (def sus (- 1 patched))
  (def br (* timbre timbre))
  (def u1 (pl-noise-tap u0))
  (def u2 (pl-noise-tap u1))
  (def (e0 dp0 sf0 sr0 g0 h0 t0 sp0 sc0 na0 nf0 bc0 ag0)
    (str-voice f_0 (* sus (eq a 0)) (* rise (eq a 0)) accent harmonics br morph u0 bstart))
  (def (e1 dp1 sf1 sr1 g1 h1 t1 sp1 sc1 na1 nf1 bc1 ag1)
    (str-voice f_1 (* sus (eq a 1)) (* rise (eq a 1)) accent harmonics br morph u1 bstart))
  (def (e2 dp2 sf2 sr2 g2 h2 t2 sp2 sc2 na2 nf2 bc2 ag2)
    (str-voice f_2 (* sus (eq a 2)) (* rise (eq a 2)) accent harmonics br morph u2 bstart))
  (tuple e0 dp0 sf0 sr0 g0 h0 t0 sp0 sc0 na0 nf0 bc0 ag0
         e1 dp1 sf1 sr1 g1 h1 t1 sp1 sc1 na1 nf1 bc1 ag1
         e2 dp2 sf2 sr2 g2 h2 t2 sp2 sc2 na2 nf2 bc2 ag2))

(defmacro plaits-string (bstart bphase rise high patched note timbre morph harmonics accent active)
  (def (e0 dp0 sf0 sr0 g0 h0 t0 sp0 sc0 na0 nf0 bc0 ag0
        e1 dp1 sf1 sr1 g1 h1 t1 sp1 sc1 na1 nf1 bc1 ag1
        e2 dp2 sf2 sr2 g2 h2 t2 sp2 sc2 na2 nf2 bc2 ag2)
    (block-gate active (str-control bstart rise patched note timbre morph harmonics accent u0)))
  ; dispersion-noise draws (ungated: the loops consume them per tick)
  (def u3 (pl-noise-tap (pl-noise-tap (pl-noise-tap u0))))
  (def u4 (pl-noise-tap u3))
  (def u5 (pl-noise-tap u4))
  (def o0 (str-string str_line0 str_ap0 e0 u3 bstart active dp0 sf0 sr0 g0 h0 t0 sp0 sc0 na0 nf0 bc0 ag0))
  (def o1 (str-string str_line1 str_ap1 e1 u4 bstart active dp1 sf1 sr1 g1 h1 t1 sp1 sc1 na1 nf1 bc1 ag1))
  (def o2 (str-string str_line2 str_ap2 e2 u5 bstart active dp2 sf2 sr2 g2 h2 t2 sp2 sc2 na2 nf2 bc2 ag2))
  (tuple (+ o0 o1 o2) (* active (+ e0 e1 e2))))
