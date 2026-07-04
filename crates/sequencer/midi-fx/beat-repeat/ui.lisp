(def beat-repeat-rate-labels
  (list "1" "1/2" "1/4" "1/8" "1/16" "1/32" "1/64"
        "1/2T" "1/4T" "1/8T" "1/16T" "1/32T" "1/64T"))

(def-midi-fx-ui
  (v-stack :gap 0.25
    (midi-fx-param "rate" :as :dropdown :items beat-repeat-rate-labels)
    (midi-fx-param "gate" :as :slider)
    (midi-fx-param "velocity" :as :slider)))
