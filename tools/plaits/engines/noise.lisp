; ======================================================================
; Noise engine (plaits/dsp/engine/noise_engine.cc): two clocked
; (sample-and-hold, PolyBLEP'd) noise sources into an LP->BP->HP multimode
; SVF (OUT) and a pair of band-passes at f0 / f1 (AUX).
; ======================================================================

; plaits ClockedNoise. `raw` is this sample's uniform [0,1) draw.
(defmacro noise-clocked (sync freq raw bstart bphase)
  (make-history h_phase)
  (make-history h_sample)
  (make-history h_next)
  (def f (pl-interp (clip freq 0 1) bstart bphase 0.001))
  (def rs (- (* 2 raw) 1))
  (def raw_amount (clip (* 4 (- f 0.25)) 0 1))
  (def ph0 (gswitch (* bstart sync) 1 (read-history h_phase)))
  (def pa (+ ph0 f))
  (def w (gte pa 1))
  (def p1 (- pa w))
  (def t (/ p1 (max f 0.000000001)))
  (def sample (read-history h_sample))
  (def disc (* w (- rs sample)))
  (def this_s (+ (read-history h_next) (* disc (pl-this-blep t))))
  (def sample1 (gswitch w rs sample))
  (write-history h_phase p1)
  (write-history h_sample sample1)
  (write-history h_next (+ (* disc (pl-next-blep t)) sample1))
  (+ this_s (* raw_amount (- rs this_s))))

; Engine body with the two noise draws (uniform [0,1)) passed in.
(defmacro noise-core (bstart bphase rise patched note timbre morph harmonics ra rb)
  (def f0 (pl-note-to-freq note))
  (def f1 (pl-note-to-freq (- (+ note (* harmonics 48)) 24)))
  (def low (gswitch patched -24 0))
  (def clock_f (pl-note-to-freq (+ (* timbre (- 128 low)) low)))
  (def q_t (* 0.5 (pl-hold (pl-semi-ratio (* morph 120)) bstart)))
  (def n1 (noise-clocked rise clock_f ra bstart bphase))
  (def n2 (noise-clocked rise (/ (* clock_f f1) f0) rb bstart bphase))
  (def f0i (pl-interp f0 bstart bphase 0))
  (def f1i (pl-interp f1 bstart bphase 0))
  (def qi (pl-interp q_t bstart bphase 0))
  (def mode (pl-interp harmonics bstart bphase 0))
  (def gain (/ 1 (sqrt (max (* (+ 0.5 qi) 40 f0i) 0.0000000001))))
  (def r (/ 1 (max qi 0.000001)))
  (def g0 (pl-tan-accurate f0i))
  (def g1 (pl-tan-accurate f1i))
  (def in1 (* n1 gain))
  (def in2 (* n2 gain))
  ; lp_hp_filter_ and bp_filter_[0] share coefficients and input, so their
  ; states are identical: one SVF serves both.
  (def (lp bp hp) (pl-svf in1 g0 r))
  (def hp_gain (min (+ (* -2 mode) 1) 0))
  (def bp_gain (- 1 (* 2 (abs (- mode 0.5)))))
  (def lp_gain (max (- 1 (* 2 mode)) 0))
  (def (lpb bpb hpb) (pl-svf in2 g1 r))
  (tuple (+ (* hp_gain hp) (* bp_gain bp) (* lp_gain lp))
         (+ bp bpb)))

(defmacro plaits-noise (bstart bphase rise high patched note timbre morph harmonics accent)
  ; per-sample uniform draw (same node as the driver's u0: identical
  ; (noise) expressions are merged)
  (def nz (noise))
  ; second independent draw: hash of this and the previous driver sample
  (noise-core bstart bphase rise patched note timbre morph harmonics
              nz (pl-wrap (+ (* nz 4093.137) (pl-noise-tap nz)))))
