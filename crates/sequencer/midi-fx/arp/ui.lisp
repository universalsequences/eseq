(def arp-rate-labels
  (list "1" "1/2" "1/4" "1/8" "1/16" "1/32" "1/64"
        "1/2T" "1/4T" "1/8T" "1/16T" "1/32T" "1/64T"))

(def arp-direction-labels
  (list "up" "down" "up-down" "random"))

(def-midi-fx-ui
  (v-stack :gap 0.25
    (midi-fx-param "rate" :as :dropdown :items arp-rate-labels)
    (midi-fx-param "direction" :as :dropdown :items arp-direction-labels)
    (midi-fx-param "octave" :as :slider)
    (midi-fx-param "gate" :as :slider)
    (midi-fx-param "velocity" :as :slider)))
