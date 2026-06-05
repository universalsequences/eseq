(midi-fx-param "rate"
  :default 4
  :min 0
  :max 12
  :role :clock-rate
  :enum "1" "1/2" "1/4" "1/8" "1/16" "1/32" "1/64"
        "1/2T" "1/4T" "1/8T" "1/16T" "1/32T" "1/64T")

(midi-fx-param "direction"
  :default 0
  :min 0
  :max 3
  :enum "up" "down" "up-down" "random")

(midi-fx-param "octave" :default 1 :min 1 :max 4)
(midi-fx-param "gate" :default 0.90 :min 0.05 :max 1.00)
(midi-fx-param "velocity" :default 0.80 :min 0.00 :max 1.00)

(def-midi-fx "arp"
  (let ((notes (fx-notes-octaves (fx-notes) (fx-param "octave")))
        (rate (fx-param "rate"))
        (direction (fx-param "direction"))
        (gate (fx-param "gate"))
        (velocity (fx-param "velocity")))
    (do
      (fx-suppress)
      (for-each |i|
        (fx-arp-emit-from notes rate i direction gate velocity)
        (range 0 (fx-arp-count-for notes rate))))))
