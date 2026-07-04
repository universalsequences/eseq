(midi-fx-param "grid"
  :default 4
  :min 0
  :max 12
  :role :quantize-grid
  :enum "1" "1/2" "1/4" "1/8" "1/16" "1/32" "1/64"
        "1/2T" "1/4T" "1/8T" "1/16T" "1/32T" "1/64T")

(def-midi-fx "quantizer"
  (do
    (fx-suppress)
    false))
