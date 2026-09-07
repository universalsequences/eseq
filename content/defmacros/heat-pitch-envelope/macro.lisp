; Finite exponential pitch decay. Initial is semitones, time is milliseconds.
; Development law (k=5); measured Analog timing-map calibration is pending.
(defmacro heat-pitch-envelope (note_on initial time_ms)
  (make-history elapsed_hist)
  (make-history initialized_hist)
  (def restart (max (eq (read-history initialized_hist) 0) (gt note_on 0.5)))
  ; Quantize duration to whole samples so the endpoint is exactly zero.
  (def frames (max 0 (round (* time_ms (/ samplerate 1000)))))
  (def elapsed (gswitch restart 0 (read-history elapsed_hist)))
  (def t (clip (/ elapsed (max 1 frames)) 0 1))
  (def curve (/ (- (exp (* -5 t)) 0.006737946999) 0.993262053001))
  (write-history elapsed_hist (min frames (+ elapsed 1)))
  (write-history initialized_hist 1)
  (* initial curve (lt elapsed frames)))
