; 808 Kick DSR convention: decay toward sustain while held, then release
; from the current value. Both times are T60; note-off never resets amplitude.
(defmacro drum-envelope (gate trigger decay_ms sustain release_ms)
  (make-history value_h)
  (make-history active_h)
  (def hit (gt trigger 0.5))
  (def active (max (read-history active_h) hit))
  (def previous (read-history value_h))
  (def target (* active (clip sustain 0 1)))
  (def decay_coef (exp (/ -6.9077553 (max 1 (* decay_ms 0.001 samplerate)))))
  (def release_coef (exp (/ -6.9077553 (max 1 (* release_ms 0.001 samplerate)))))
  (def next (gswitch hit 1
    (gswitch (gt gate 0.5)
      (+ target (* (- previous target) decay_coef))
      (* previous release_coef))))
  (def value (gswitch (lt (abs next) 0.00000000000000000001) 0 next))
  (write-history active_h active)
  (write-history value_h value)
  value)
