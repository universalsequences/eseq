;; Tape (Jiles–Atherton hysteresis) built-in FX panel.
(module eseq.effects.builtin.tape)

(import eseq.effects.builtin.dynamics :as dyn)
(import eseq.effects.builtin.filter-core :refer (builtin-fx-param))
(import eseq.effects.param-grid :refer (fx-param-grid))

(export tape-ui)

(def tape-ui (fx)
  (let ((params (get fx :params)))
    (let ((drive-p (eseq.effects.builtin.filter-core/builtin-fx-param params "drive"))
          (bias-p (eseq.effects.builtin.filter-core/builtin-fx-param params "bias"))
          (speed-p (eseq.effects.builtin.filter-core/builtin-fx-param params "speed"))
          (output-p (eseq.effects.builtin.filter-core/builtin-fx-param params "output"))
          (mix-p (eseq.effects.builtin.filter-core/builtin-fx-param params "mix"))
          (wow-p (eseq.effects.builtin.filter-core/builtin-fx-param params "wow"))
          (flutter-p (eseq.effects.builtin.filter-core/builtin-fx-param params "flutter"))
          (hiss-p (eseq.effects.builtin.filter-core/builtin-fx-param params "hiss")))
      (if (and drive-p bias-p speed-p output-p mix-p)
        (v-stack :gap 0.34
          (h-stack :gap 0.45 :align :center
            (dyn/option-row fx "spd" speed-p 6.4))
          (h-stack :gap 0.5 :align :center
            (dyn/number-knob fx "drive" drive-p 1)
            (dyn/percent-knob fx "bias" bias-p)
            (dyn/number-knob fx "out" output-p 1)
            (dyn/percent-knob fx "mix" mix-p))
          (if (and wow-p flutter-p hiss-p)
            (h-stack :gap 0.5 :align :center
              (dyn/percent-knob fx "wow" wow-p)
              (dyn/percent-knob fx "flut" flutter-p)
              (dyn/percent-knob fx "hiss" hiss-p))
            (box :width 0 :height 0)))
        (eseq.effects.param-grid/fx-param-grid params fx)))))
