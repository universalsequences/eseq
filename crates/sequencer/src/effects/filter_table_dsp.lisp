; Filter Table — stereo, magnitude-table-driven spectral filter.
;
; Contract with filter_table.rs:
;   table_magnitudes is a mutable [64, 1025] tensor containing normalized
;   half-spectrum magnitudes. User audio is transformed into this representation
;   offline; phase is deliberately discarded so frame interpolation cannot
;   cancel harmonics.
;
; N=2048 and hop=512 use the same 4x-overlap STFT convention as the existing
; shipping spectral effects. The wrapped Hann around sample zero bounds the
; synthesized zero-phase IR to roughly 768 taps and prevents circular
; convolution from wrapping a long response into time-aliased output.

(def N 2048)
(def HOP 512)
(def NBINS 1025)
(def LASTBIN 1024)
(def LASTFRAME 63)
(def IRHALF 384)
(def PI 3.141592653589793)

; `last` is the highest valid row index. The interpolation base row is clamped
; to last-1 so the second gather never reads past the tensor: at pos=last the
; pair collapses to rows last-1/last with frac=1, which selects exactly the
; final row instead of gathering out of range.
(defmacro peek-vec (table pos last cols col)
  (def clamped (max (min pos last) 0))
  (def i0 (min (floor clamped) (- last 1)))
  (def frac (- clamped i0))
  (def base (+ (* i0 cols) col))
  (def a (gather table base))
  (def b (gather table (+ base cols)))
  (+ (* a (- 1 frac)) (* b frac)))

(def fold-index (min (iota 2048) (- 2048 (iota 2048))))
(def bin-index (iota 1025))

(defmacro mirror-spectrum (half-spectrum)
  (gather half-spectrum fold-index))

; Response controls are smoothed with a ~30 ms scalar one-pole *before* the
; hop-hold, so hop-quantized automation cannot staircase the rebuilt response:
; successive hop responses differ by near-continuous parameter increments and
; the 4x-overlap synthesis crossfades between them. The smoother is seeded to
; the incoming value on the very first sample so static parameters have no
; startup glide. SMOOTH-MS 0 collapses alpha to 1 (legacy instant behavior).
(def SMOOTH-MS 30)
(defmacro smooth-control (h hseed sig)
  (make-history h)
  (make-history hseed)
  (def alpha (min 1 (/ 1.0 (max 1 (* samplerate (* SMOOTH-MS 0.001))))))
  (def seeded (read-history hseed))
  (write-history hseed 1)
  (def prev (read-history h))
  (write-history h (+ (* (- 1 seeded) sig)
                      (* seeded (+ prev (* alpha (- sig prev)))))))

; Dilated 3-tap smoothing pass used to build progressively band-limited
; variants of the table row for the anti-aliased cutoff resample below.
(defmacro aa-box (c d)
  (def il (max (- bin-index d) 0))
  (def ir (min (+ bin-index d) LASTBIN))
  (+ (* 0.5 (gather c bin-index))
     (* 0.25 (+ (gather c il) (gather c ir)))))

(defmacro ir-window ()
  (def distance fold-index)
  (* (* 0.5 (+ 1 (cos (* PI (/ distance IRHALF)))))
     (lte distance IRHALF)))

; frame is normalized 0..1. Cutoff is the frequency assigned to table harmonic
; 24, matching the musical control model: moving cutoff translates the table's
; character along the frequency axis rather than imposing a low-pass slope.
(defmacro filter-response (table frame cutoff resonance)
  (def columns (iota 1025))
  (def curve (peek-vec table (* (ones 1025) (* frame LASTFRAME)) LASTFRAME NBINS columns))

  ; Closing cutoff below the reference frequency resamples the row with a
  ; uniform stride > 1 in bin space, which silently drops (aliases) features
  ; narrower than the stride. Pre-smooth the row with an a-trous cascade whose
  ; effective width doubles per level and blend the two levels bracketing
  ; log2(stride); stride <= 1 selects the untouched row. AA-MAX-LEVEL 0
  ; restores the naive resample.
  (def AA-MAX-LEVEL 4)
  (def m1 (aa-box curve 1))
  (def m2 (aa-box m1 2))
  (def m3 (aa-box m2 4))
  (def m4 (aa-box m3 8))
  (def stride (/ (* 24 (/ samplerate N)) cutoff))
  (def lvl (clip (/ (log (max stride 1)) 0.6931471805599453) 0 AA-MAX-LEVEL))
  (def w0 (max 0 (- 1 (abs lvl))))
  (def w1 (max 0 (- 1 (abs (- lvl 1)))))
  (def w2 (max 0 (- 1 (abs (- lvl 2)))))
  (def w3 (max 0 (- 1 (abs (- lvl 3)))))
  (def w4 (max 0 (- 1 (abs (- lvl 4)))))
  (def banded (+ (+ (* w0 curve) (* w1 m1))
                 (+ (* w2 m2) (+ (* w3 m3) (* w4 m4)))))

  (def bin-hz (* fold-index (/ samplerate N)))
  (def harmonic-pos (min LASTBIN (* 24 (/ bin-hz cutoff))))
  (def shifted (peek-vec banded harmonic-pos LASTBIN 1 0))

  ; Resonance increases spectral contrast without becoming an output-gain
  ; control. Raising normalized magnitudes directly keeps peaks bounded before
  ; makeup; RMS makeup restores quieter sparse responses but is capped at +18 dB
  ; so high-resonance curves cannot produce runaway narrow-bin boosts. Table
  ; rows are peak-normalized at import, so shaped/sparse curves routinely need
  ; 5-20x makeup to sit near the dry level; the old +6 dB cap left the wet path
  ; 10-20 dB under dry for most presets.
  (def exponent (+ 1 (* 3 resonance)))
  (def contrasted (pow (max shifted 0.0001) exponent))
  (def response-rms (sqrt (+ (mean (* contrasted contrasted)) 0.000001)))
  (def makeup (min 8 (/ 1 response-rms)))
  (def shaped (* contrasted makeup))

  (def full (mirror-spectrum shaped))
  (def ir (ifft full (* full 0) @N 2048 @backend accelerated))
  (def bounded (* ir (ir-window)))
  (fft bounded @N 2048 @backend accelerated))

(def in-l (in 1 @name left))
(def in-r (in 2 @name right))
(def mod1 (in 3 @name mod1 @modulator 1))
(def mod2 (in 4 @name mod2 @modulator 2))
(def mod3 (in 5 @name mod3 @modulator 3))
(def mod4 (in 6 @name mod4 @modulator 4))

(param frame @min 0 @max 1 @default 0 @unit % @mod true @mod-mode additive)
(param cutoff @min 40 @max 18000 @default 1000 @unit Hz @mod true @mod-mode additive)
(param resonance @min 0 @max 1 @default 0 @unit % @mod true @mod-mode additive)
(param mix @min 0 @max 1 @default 0.7 @unit % @mod true @mod-mode additive)
(param output @min 0.25 @max 2 @default 1)

(def table (tensor-param @shape [64 1025] @name table_magnitudes))

; Resolve host modulation before holding the response controls. `@mod true`
; declares the routing metadata and hidden depth lanes; `(mod ...)` is the DSP
; accessor that actually applies those lanes. Clamping keeps additive
; modulation inside each control's valid signal domain.
(def frame-h (hop-hold (smooth-control sm-frame sm-frame-seed (clip (mod frame) 0 1)) HOP))
(def cutoff-h (hop-hold (smooth-control sm-cutoff sm-cutoff-seed (clip (mod cutoff) 40 18000)) HOP))
(def resonance-h (hop-hold (smooth-control sm-res sm-res-seed (clip (mod resonance) 0 1)) HOP))
(def (h-re h-im) (filter-response table frame-h cutoff-h resonance-h))

; sqrt-Hann analysis/synthesis with the established 0.707 normalization gives
; unity overlap-add at N/4. The bypass traverses the same STFT and therefore has
; exactly the wet path's one-window latency.
(def win (* 0.70710678 (sqrt (hann 2048))))

(def frame-l (* (reshape (buffer in-l 2048 512) @shape [2048]) win))
(def frame-r (* (reshape (buffer in-r 2048 512) @shape [2048]) win))
(def (x-l-re x-l-im) (fft frame-l @N 2048 @backend accelerated))
(def (x-r-re x-r-im) (fft frame-r @N 2048 @backend accelerated))
(def (y-l-re y-l-im) (complex-mul x-l-re x-l-im h-re h-im))
(def (y-r-re y-r-im) (complex-mul x-r-re x-r-im h-re h-im))

(def dry-l (overlap-add (* (ifft x-l-re x-l-im @N 2048 @backend accelerated) win) HOP))
(def dry-r (overlap-add (* (ifft x-r-re x-r-im @N 2048 @backend accelerated) win) HOP))
(def wet-l (overlap-add (* (ifft y-l-re y-l-im @N 2048 @backend accelerated) win) HOP))
(def wet-r (overlap-add (* (ifft y-r-re y-r-im @N 2048 @backend accelerated) win) HOP))

; Equal-power dry/wet law. Both branches are latency-aligned above. Mix remains
; sample-rate modulatable because it does not rebuild the spectral response.
(def mix-mod (clip (mod mix) 0 1))
(def dry-gain (sqrt (- 1 mix-mod)))
(def wet-gain (sqrt mix-mod))
(out (* output (+ (* dry-l dry-gain) (* wet-l wet-gain))) 1 @name left)
(out (* output (+ (* dry-r dry-gain) (* wet-r wet-gain))) 2 @name right)
