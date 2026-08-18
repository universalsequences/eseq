; Spectral engine tail — zero-phase STFT convolution (the original engine).
; Consumes response-mag [2048] from the shared head. One-window (N) latency,
; compensated by PDC; the dry path traverses the same STFT so mix stays
; aligned.

(defmacro ir-window ()
  (def distance fold-index)
  (* (* 0.5 (+ 1 (cos (* PI (/ distance IRHALF)))))
     (lte distance IRHALF)))

(def ir (ifft response-mag (* response-mag 0) @N 2048 @backend accelerated))
(def bounded (* ir (ir-window)))
(def (h-re h-im) (fft bounded @N 2048 @backend accelerated))


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
