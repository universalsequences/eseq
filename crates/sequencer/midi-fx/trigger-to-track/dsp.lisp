(midi-fx-param "track"
  :default 2
  :min 1
  :max 64)

(def-midi-fx "trigger-to-track"
  (let ((target (- (fx-positive-int (fx-param "track")) 1)))
    (if (= target (fx-track))
      false
      (fx-emit 0 :track target))))
