;@ungated
; ======================================================================
; String machine engine (plaits/dsp/engine2/string_machine_engine.cc):
; 4 divide-down voices (ChordBank chord, no inversion) split L/R, two
; NaiveSvf low-passes, cross-mix, Ensemble chorus.
; Requires engines/chordbank.lisp (cb-* helpers) loaded first.
; ======================================================================
(def strmach_lut_sine (tensor @shape [641 1] @file "lut_sine.json"))
; registrations[11][6], index row * 6 + k.
(def strmach_reg (tensor @shape [66 1] @data [1 0 0 0 0 0  0.5 0 0.5 0 0 0  0.4 0 0.2 0 0.4 0  0.3 0 0 0.3 0 0.4  0.3 0 0 0 0 0.7  0.2 0 0 0.2 0 0.6  0 0.2 0.1 0 0.2 0.5  0 0.3 0 0.3 0 0.4  0 0.4 0 0.3 0 0.3  0 0.5 0 0.5 0 0  0 1 0 0 0 0]))

(defmacro strmach-reg-at (ri rf k)
  (def a (peek strmach_reg (+ (* ri 6) k) 0))
  (def b (peek strmach_reg (+ (* ri 6) 6 k) 0))
  (+ a (* (- b a) rf)))

; stmlib NaiveSvf, FREQUENCY_DIRTY, low-pass output; f = 2 pi min(fc, 0.158).
(defmacro strmach-naive-lp (in f damp)
  (make-history h_lp)
  (make-history h_bp)
  (def bp0 (read-history h_bp))
  (def lp (+ (read-history h_lp) (* f bp0)))
  (def hp (- in (* bp0 damp) lp))
  (def bp (+ bp0 (* f hp)))
  (write-history h_lp lp)
  (write-history h_bp bp)
  lp)

; uint32 LFO phase kept exactly as (hi, lo) 16-bit halves; returns hi after
; adding inc_hi * 65536 + inc_lo per sample.
(defmacro strmach-phase32 (inc_hi inc_lo)
  (make-history h_hi)
  (make-history h_lo)
  (def lo1 (+ (read-history h_lo) inc_lo))
  (def c (gte lo1 65536))
  (def lo (- lo1 (* c 65536)))
  (def hi1 (+ (read-history h_hi) inc_hi c))
  (def hi (- hi1 (* (gte hi1 65536) 65536)))
  (write-history h_lo lo)
  (write-history h_hi hi)
  (tuple hi lo))

; SineRaw(phase + (off_hi, off_lo)): lut_sine[(phase + off) >> 23].
(defmacro strmach-sine-raw (hi lo off_hi off_lo)
  (def c (gte (+ lo off_lo) 65536))
  (def h1 (+ hi off_hi c))
  (def h (- h1 (* (gte h1 65536) 65536)))
  (peek strmach_lut_sine (floor (* h 0.0078125)) 0))

; plaits Ensemble (FxEngine<1024, FORMAT_32_BIT>, two 511-sample lines).
; Split for block-gate: the LFO delay times and the six delay taps run
; after the gate.
(defmacro strmach-ensemble-times (depth)
  (def (p1h p1l) (strmach-phase32 1 1753))
  (def (p2h p2l) (strmach-phase32 9 156))
  (def slow_0 (strmach-sine-raw p1h p1l 0 0))
  (def slow_120 (strmach-sine-raw p1h p1l 21626 57671))
  (def slow_240 (strmach-sine-raw p1h p1l 43253 49807))
  (def fast_0 (strmach-sine-raw p2h p2l 0 0))
  (def fast_120 (strmach-sine-raw p2h p2l 21626 57671))
  (def fast_240 (strmach-sine-raw p2h p2l 43253 49807))
  (def a (* depth 160))
  (def b (* depth 16))
  (def d1 (+ 192 (* slow_0 a) (* fast_0 b)))
  (def d2 (+ 192 (* slow_120 a) (* fast_120 b)))
  (def d3 (+ 192 (* slow_240 a) (* fast_240 b)))
  (tuple d1 d2 d3))

(defmacro strmach-ensemble (left right amount d1 d2 d3)
  (def dry (- 1 (* amount 0.5)))
  (def l1 (delay left d1 @max-delay 512))
  (def l2 (delay left d2 @max-delay 512))
  (def l3 (delay left d3 @max-delay 512))
  (def r1 (delay right d1 @max-delay 512))
  (def r2 (delay right d2 @max-delay 512))
  (def r3 (delay right d3 @max-delay 512))
  (def wet_l (+ (* l1 0.33) (* l2 0.33) (* r3 0.33)))
  (def wet_r (+ (* r1 0.33) (* r2 0.33) (* l3 0.33)))
  (tuple (+ (* wet_l amount) (* left dry)) (+ (* wet_r amount) (* right dry))))

(defmacro strmach-voices (bstart note timbre morph harmonics bphase)
  (def morph_lp (cb-block-pole morph 0.1 bstart))
  (def chord (cb-chord-index harmonics bstart))
  (def reg (* (max morph_lp 0) 9.999))
  (def ri (floor reg))
  (def rf (- reg ri))
  (def h0 (strmach-reg-at ri rf 0))
  (def h1 (strmach-reg-at ri rf 1))
  (def h2 (strmach-reg-at ri rf 2))
  (def h3 (strmach-reg-at ri rf 3))
  (def h4 (strmach-reg-at ri rf 4))
  (def h5 (strmach-reg-at ri rf 5))
  (def f0 (* (pl-note-to-freq note) 0.998))
  (def nf0 (* f0 (peek cb_ratios (* chord 4) 0)))
  (def nf1 (* f0 (peek cb_ratios (+ (* chord 4) 1) 0)))
  (def nf2 (* f0 (peek cb_ratios (+ (* chord 4) 2) 0)))
  (def nf3 (* f0 (peek cb_ratios (+ (* chord 4) 3) 0)))
  (def v0 (cb-string-synth nf0 h0 h1 h2 h3 h4 h5 (* 0.25 (clip (- 4 (* nf0 32)) 0 1)) 1 bstart bphase))
  (def v1 (cb-string-synth nf1 h0 h1 h2 h3 h4 h5 (* 0.25 (clip (- 4 (* nf1 32)) 0 1)) 1 bstart bphase))
  (def v2 (cb-string-synth nf2 h0 h1 h2 h3 h4 h5 (* 0.25 (clip (- 4 (* nf2 32)) 0 1)) 1 bstart bphase))
  (def v3 (cb-string-synth nf3 h0 h1 h2 h3 h4 h5 (* 0.25 (clip (- 4 (* nf3 32)) 0 1)) 1 bstart bphase))
  (def cutoff (* 2.2 f0 (pl-semi-ratio (* 120 timbre))))
  (def l (strmach-naive-lp (+ v0 v2) (* twopi (min cutoff 0.158)) 1))
  (def r (strmach-naive-lp (+ v1 v3) (* twopi (min (* cutoff 1.5) 0.158)) 1))
  (def mo (+ (* 0.66 l) (* 0.33 r)))
  (def ma (+ (* 0.66 r) (* 0.33 l)))
  (tuple mo ma))

(defmacro plaits-strmach (bstart bphase rise high patched note timbre morph harmonics accent active)
  (def (mo ma) (block-gate active (strmach-voices bstart note timbre morph harmonics bphase)))
  ; A delay time may not come out of a block-gate, so the ensemble LFOs run
  ; ungated.
  (def (d1 d2 d3) (strmach-ensemble-times (+ 0.35 (* 0.65 timbre))))
  (def (l r) (strmach-ensemble mo ma (* (abs (- timbre 0.5)) 2) d1 d2 d3))
  (tuple (* active l) (* active r)))
