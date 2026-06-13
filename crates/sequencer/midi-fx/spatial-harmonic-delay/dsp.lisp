(midi-fx-param "rate"
  :default 4
  :min 0
  :max 12
  :enum "1" "1/2" "1/4" "1/8" "1/16" "1/32" "1/64"
        "1/2T" "1/4T" "1/8T" "1/16T" "1/32T" "1/64T")

(midi-fx-param "taps" :default 3 :min 0 :max 6)

(midi-fx-param "delay-1" :default 1 :min 0 :max 16)
(midi-fx-param "transpose-1" :default 0 :min -48 :max 48)
(midi-fx-param "velocity-1" :default 0.70 :min 0 :max 2)
(midi-fx-param "pan-1" :default -0.70 :min -1 :max 1)

(midi-fx-param "delay-2" :default 2 :min 0 :max 16)
(midi-fx-param "transpose-2" :default 7 :min -48 :max 48)
(midi-fx-param "velocity-2" :default 0.50 :min 0 :max 2)
(midi-fx-param "pan-2" :default 0.70 :min -1 :max 1)

(midi-fx-param "delay-3" :default 3 :min 0 :max 16)
(midi-fx-param "transpose-3" :default 12 :min -48 :max 48)
(midi-fx-param "velocity-3" :default 0.35 :min 0 :max 2)
(midi-fx-param "pan-3" :default -0.35 :min -1 :max 1)

(midi-fx-param "delay-4" :default 4 :min 0 :max 16)
(midi-fx-param "transpose-4" :default 19 :min -48 :max 48)
(midi-fx-param "velocity-4" :default 0.25 :min 0 :max 2)
(midi-fx-param "pan-4" :default 0.35 :min -1 :max 1)

(midi-fx-param "delay-5" :default 5 :min 0 :max 16)
(midi-fx-param "transpose-5" :default 24 :min -48 :max 48)
(midi-fx-param "velocity-5" :default 0.18 :min 0 :max 2)
(midi-fx-param "pan-5" :default -0.20 :min -1 :max 1)

(midi-fx-param "delay-6" :default 6 :min 0 :max 16)
(midi-fx-param "transpose-6" :default 31 :min -48 :max 48)
(midi-fx-param "velocity-6" :default 0.12 :min 0 :max 2)
(midi-fx-param "pan-6" :default 0.20 :min -1 :max 1)

(def shd-note-duration-steps (start end)
  (/ (fx-max 0 (- end start)) (fx-source-time)))

(def shd-emit-tap-note (rate delay transpose velocity pan note-index)
  (let ((start (fx-note-start note-index))
        (end (fx-note-end note-index)))
    (fx-emit :beats (+ (fx-time rate delay) start)
      :note (+ (fx-note note-index) transpose)
      :vel (* (fx-velocity) velocity)
      :pan pan
      :dur (shd-note-duration-steps start end))))

(def shd-emit-tap (rate delay transpose velocity pan)
  (for-each |i|
    (shd-emit-tap-note rate delay transpose velocity pan i)
    (range 0 (fx-note-count))))

(def shd-emit-if-active (tap rate delay transpose velocity pan)
  (if (<= tap (round (fx-param "taps")))
    (shd-emit-tap rate delay transpose velocity pan)
    false))

(def-midi-fx "spatial-harmonic-delay"
  (let ((rate (fx-param "rate")))
    (do
      (shd-emit-if-active 1 rate
        (fx-param "delay-1")
        (fx-param "transpose-1")
        (fx-param "velocity-1")
        (fx-param "pan-1"))
      (shd-emit-if-active 2 rate
        (fx-param "delay-2")
        (fx-param "transpose-2")
        (fx-param "velocity-2")
        (fx-param "pan-2"))
      (shd-emit-if-active 3 rate
        (fx-param "delay-3")
        (fx-param "transpose-3")
        (fx-param "velocity-3")
        (fx-param "pan-3"))
      (shd-emit-if-active 4 rate
        (fx-param "delay-4")
        (fx-param "transpose-4")
        (fx-param "velocity-4")
        (fx-param "pan-4"))
      (shd-emit-if-active 5 rate
        (fx-param "delay-5")
        (fx-param "transpose-5")
        (fx-param "velocity-5")
        (fx-param "pan-5"))
      (shd-emit-if-active 6 rate
        (fx-param "delay-6")
        (fx-param "transpose-6")
        (fx-param "velocity-6")
        (fx-param "pan-6")))))
