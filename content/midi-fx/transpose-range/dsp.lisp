(midi-fx-param "min" :default -12 :min -96 :max 96)
(midi-fx-param "max" :default 12 :min -96 :max 96)

(def-midi-fx "transpose-range"
  (let ((low (fx-param "min"))
        (high (fx-param "max"))
        (source-beats (fx-source-time)))
    (do
      (fx-suppress)
      (for-each |i|
        (let ((start (fx-note-start i))
              (end (fx-note-end i)))
          (fx-emit :beats start
            :note (fx-wrap-transpose-into-range (fx-note i) low high)
            :dur (/ (fx-max 0 (- end start)) source-beats)))
        (range 0 (fx-note-count))))))
