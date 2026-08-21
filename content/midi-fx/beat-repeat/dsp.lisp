(midi-fx-param "rate"
  :default 4
  :min 0
  :max 12
  :role :clock-rate
  :enum "1" "1/2" "1/4" "1/8" "1/16" "1/32" "1/64"
        "1/2T" "1/4T" "1/8T" "1/16T" "1/32T" "1/64T")

(midi-fx-param "gate" :default 0.90 :min 0.05 :max 1.00)
(midi-fx-param "velocity" :default 1.00 :min 0.00 :max 2.00)

(def beat-repeat-note-duration (rate gate)
  (* gate (/ (fx-time rate) (fx-source-time))))

(def beat-repeat-emit-note (rate gate velocity note-index)
  (fx-emit :beats (fx-note-start note-index)
    :note (fx-note note-index)
    :vel (* (fx-velocity) velocity)
    :dur (beat-repeat-note-duration rate gate)))

(def-midi-fx "beat-repeat"
  (let ((rate (fx-param "rate"))
        (gate (fx-param "gate"))
        (velocity (fx-param "velocity")))
    (do
      (fx-suppress)
      (for-each |i|
        (beat-repeat-emit-note rate gate velocity i)
        (range 0 (fx-note-count))))))
