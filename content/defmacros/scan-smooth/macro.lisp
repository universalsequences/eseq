; scan-smooth — the core Wavetable synth's wave-position chase: a one-pole
; lowpass (~8 ms time constant) glides toward the target so knob moves and
; mod sweeps morph between waves without zipper noise, while a trigger snap
; jumps straight to the target at note start so a new note never inherits a
; glide from the previous one. Feed the (fractional) result to a table read's
; wave index — sample interpolates the wave axis. Typical use:
;   (sample table phase (+ (* set 16) (scan-smooth wave trigger)))
(defmacro scan-smooth (target trigger)
  (make-history scan_hist)
  (def prev (read-history scan_hist))
  (def coeff (- 1.0 (exp (/ -1.0 (* 0.008 samplerate)))))
  (def scan (gswitch trigger target (+ prev (* coeff (- target prev)))))
  (write-history scan_hist scan)
  scan)
