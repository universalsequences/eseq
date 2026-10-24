; Which modes of a modal object sound: the first `count` mode numbers, faded
; out between 0.35 and 0.45 of the sample rate instead of cut (a stiff object
; at a high note puts a mode near Nyquist, which rang as a whine).
(defmacro modal-band (n freqs count)
  (def top-taper (max (min (/ (- (* 0.45 samplerate) freqs) (* 0.1 samplerate)) 1) 0))
  (* (lte n (+ count 0.5)) top-taper))
