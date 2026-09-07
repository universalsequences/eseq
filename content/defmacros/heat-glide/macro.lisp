; Portamento in octaves. Mode: off, every note, overlapping notes only.
; Time law: fixed duration or fixed rate (time_ms per octave). The first
; physical note always starts at its own pitch, including a reused DSP voice
; whose initialization ran before a note arrived. Later notes retain pitch
; across release for always-glide. A new destination starts from the current
; interpolated position, so interrupted glides remain continuous.
(defmacro heat-glide (target note_on legato mode time_ms rate_mode)
  (make-history ready_hist)
  (make-history position_hist)
  (make-history target_hist)
  (make-history origin_hist)
  (make-history elapsed_hist)
  (make-history duration_hist)
  (def ready (read-history ready_hist))
  (def old (read-history position_hist))
  (def changed (eq (eq target (read-history target_hist)) 0))
  (def start (max (gt note_on 0.5) changed))
  (def enabled (gt mode 0.5))
  (def snap (max (eq ready 0) (max (eq enabled 0)
    (max (lte time_ms 0) (* (gt note_on 0.5) (* (gt mode 1.5) (lt legato 0.5)))))))
  (def origin (gswitch start old (read-history origin_hist)))
  (def distance (abs (- target origin)))
  (def duration (gswitch start
    (max 1 (round (* (* (max 0 time_ms) (/ samplerate 1000))
      (gswitch (gt rate_mode 0.5) distance 1))))
    (read-history duration_hist)))
  (def elapsed (gswitch start 0 (min duration (+ 1 (read-history elapsed_hist)))))
  (def result (gswitch snap target
    (+ origin (* (- target origin) (clip (/ elapsed (max 1 duration)) 0 1)))))
  (write-history ready_hist (max ready (gt note_on 0.5)))
  (write-history position_hist result)
  (write-history target_hist target)
  (write-history origin_hist origin)
  (write-history elapsed_hist elapsed)
  (write-history duration_hist duration)
  result)
