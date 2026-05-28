;; Tape (Jiles–Atherton hysteresis) built-in FX panel.
(def builtin-fx-tape-ui (fx)
  (let ((params (get fx :params)))
    (let ((drive-p (builtin-fx-param params "drive"))
          (bias-p (builtin-fx-param params "bias"))
          (speed-p (builtin-fx-param params "speed"))
          (output-p (builtin-fx-param params "output"))
          (mix-p (builtin-fx-param params "mix")))
      (if (and drive-p bias-p speed-p output-p mix-p)
        (v-stack :gap 0.34
          (h-stack :gap 0.45 :align :center
            (builtin-fx-dynamics-option fx "spd" speed-p 6.4))
          (h-stack :gap 0.5 :align :center
            (builtin-fx-dynamics-number-knob fx "drive" drive-p 1)
            (builtin-fx-dynamics-percent-knob fx "bias" bias-p)
            (builtin-fx-dynamics-number-knob fx "out" output-p 1)
            (builtin-fx-dynamics-percent-knob fx "mix" mix-p)))
        (fx-param-grid params fx)))))
