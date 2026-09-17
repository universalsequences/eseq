;; ES Compressor built-in FX panel: sampler-style sustain compressor.
(module eseq.effects.builtin.es-compressor)

(import eseq.effects.builtin.dynamics :as dyn)
(import eseq.effects.builtin.filter-core :refer (builtin-fx-param))
(import eseq.effects.param-grid :refer (fx-param-grid))

(export es-compressor-ui)

(def es-compressor-caption (text)
  (label text :font-size 8.2 :height 0.78 :color :dim :bg :transparent))

(def es-compressor-ui (fx)
  (let ((params (get fx :params)))
    (let ((amount-p (eseq.effects.builtin.filter-core/builtin-fx-param params "amount"))
          (mix-p (eseq.effects.builtin.filter-core/builtin-fx-param params "mix"))
          (attack-p (eseq.effects.builtin.filter-core/builtin-fx-param params "attack"))
          (release-p (eseq.effects.builtin.filter-core/builtin-fx-param params "release"))
          (input-p (eseq.effects.builtin.filter-core/builtin-fx-param params "input-db"))
          (drive-p (eseq.effects.builtin.filter-core/builtin-fx-param params "drive"))
          (detector-p (eseq.effects.builtin.filter-core/builtin-fx-param params "detector-db"))
          (output-p (eseq.effects.builtin.filter-core/builtin-fx-param params "output-db")))
      (if (and amount-p mix-p attack-p release-p input-p drive-p detector-p output-p)
        (v-stack :gap 0.3 :debug-name "es-compressor-panel"
          (h-stack :gap 0.5 :align :center
            (dyn/number-knob fx "sustain" amount-p 0)
            (dyn/percent-knob fx "mix" mix-p)
            (dyn/number-knob fx "attack" attack-p 1)
            (dyn/number-knob fx "release" release-p 0))
          (es-compressor-caption "sustain lifts tails · grip only near full scale")
          (h-stack :gap 0.5 :align :center
            (dyn/number-knob fx "input" input-p 1)
            (dyn/number-knob fx "drive" drive-p 1)
            (dyn/number-knob fx "detect" detector-p 1)
            (dyn/number-knob fx "output" output-p 1))
          (es-compressor-caption "dB trims · drive feeds the shaper · detector backs the compressor off"))
        (eseq.effects.param-grid/fx-param-grid params fx)))))
