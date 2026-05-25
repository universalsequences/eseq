;; DJ Mixer built-in FX panel.
(def builtin-fx-dj-mixer-knob (fx label-text p decimals)
  (knob-number :label label-text
    :value (fx-param-value p)
    :min (get p :min) :max (get p :max) :decimals decimals
    :font-size 10.0 :label-font-size 10.0
    :text-color :fg :label-color :dim
    :width 10.2 :height 2.65 :knob-size 1.9
    :on-change (lambda (v) (fx-set-effect-value fx p v))))

(def builtin-fx-dj-mixer-loop-button (fx p)
  (button "Loop"
    :width 10.2 :height 1.65 :padding 0 :font-size 11.0
    :background-color (if (> (get p :value) 0.5) (rgba 1.0 0.62 0.25 1.0) :mixer-control-bg)
    :color (if (> (get p :value) 0.5) :black :dim)
    :on-click |x y r| (fx-toggle-effect-value fx p)))

(def builtin-fx-dj-mixer-ui (fx)
  (let ((params (get fx :params)))
    (let ((speed-p (builtin-fx-param params "speed"))
          (length-p (builtin-fx-param params "length"))
          (loop-p (builtin-fx-param params "loop")))
      (if (and speed-p length-p loop-p)
        (box :width 11.2 :height 8.3 :padding 0.28
             :background-color :fx-inner-panel-bg :corner-radius 7
          (v-stack :gap 0.26 :align :center
            (builtin-fx-dj-mixer-knob fx "speed" speed-p 2)
            (builtin-fx-dj-mixer-knob fx "length" length-p 3)
            (box :height 0.32 :width 10.2)
            (builtin-fx-dj-mixer-loop-button fx loop-p)))
        (fx-param-grid params fx)))))
