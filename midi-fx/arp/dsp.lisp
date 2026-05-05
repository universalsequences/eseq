(midi-fx-param "rate"
  :default 4
  :min 0
  :max 12
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
  (do
    (fx-suppress)
    (for-each |i|
      (fx-arp-emit-directed
        (fx-param "rate")
        i
        (fx-param "direction")
        :octave (fx-param "octave")
        :vel (fx-param "velocity")
        :dur (* (fx-param "gate")
                (/ (fx-time (fx-param "rate")) (fx-source-time))))
      (range 0 (fx-arp-count (fx-param "rate"))))))
