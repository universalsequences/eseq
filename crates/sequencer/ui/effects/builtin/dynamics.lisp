;; Dynamics, compressor, and limiter built-in FX panels.
(def builtin-fx-dynamics-percent-knob (fx label-text p)
  (knob-number :label label-text
    :value (fx-param-value p)
    :min (get p :min) :max (get p :max) :value-scale 100 :decimals 0
    :font-size 9.5 :label-font-size 9.5
    :text-color (param-plock-text-color fx p) :label-color :dim
    :plock-active (if (param-plock-active? fx p) 1 0)
    :plock-default (param-plock-default fx p)
    :plock-color-r (param-plock-color-r)
    :plock-color-g (param-plock-color-g)
    :plock-color-b (param-plock-color-b)
    :width 6.4 :height 3.2 :knob-size 2.0
	    :track-color '(rgba 0.4, 0.4, 0.4, 1)
    :on-change (lambda (v) (fx-set-effect-value fx p v))))

(def builtin-fx-dynamics-number-knob (fx label-text p decimals)
  (knob-number :label label-text
    :value (fx-param-value p)
    :min (get p :min) :max (get p :max) :decimals decimals
    :font-size 9.5 :label-font-size 9.5
    :text-color (param-plock-text-color fx p) :label-color :dim
    :plock-active (if (param-plock-active? fx p) 1 0)
    :plock-default (param-plock-default fx p)
    :plock-color-r (param-plock-color-r)
    :plock-color-g (param-plock-color-g)
    :plock-color-b (param-plock-color-b)
    :width 6.8 :height 3.2 :knob-size 2.0
	    :track-color '(rgba 0.4, 0.4, 0.4, 1)
    :on-change (lambda (v) (fx-set-effect-value fx p v))))

(def builtin-fx-dynamics-option (fx label-text p width)
  (h-stack :gap 0.22 :align :center
    (label label-text :font-size 8.5 :width 4.7 :color :dim :bg :transparent)
    (dropdown :value (get p :text-value)
      :options (get p :options)
      :on-change (lambda (v) (builtin-fx-set-effect-option fx p v))
      :plock-active (if (param-plock-active? fx p) 1 0)
      :plock-color-r (param-plock-color-r)
      :plock-color-g (param-plock-color-g)
      :plock-color-b (param-plock-color-b)
      :width width :height 1.05 :font-size 9.5)))

(def builtin-fx-dynamics-ui (fx)
  (let ((params (get fx :params)))
    (let ((amount-p (builtin-fx-param params "amount"))
        (attack-p (builtin-fx-param params "attack"))
        (release-p (builtin-fx-param params "release"))
        (low-cut-p (builtin-fx-param params "low cut"))
        (drive-p (builtin-fx-param params "drive"))
        (output-p (builtin-fx-param params "output"))
        (mix-p (builtin-fx-param params "mix"))
        (knee-p (builtin-fx-param params "knee"))
        (input-p (builtin-fx-param params "input")))
      (if (and amount-p attack-p release-p low-cut-p drive-p output-p mix-p)
        (v-stack :gap 0.34 :padding 0.1
          (h-stack :padding 0.5 :gap 0.45 :align :center
            (builtin-fx-dynamics-option fx "atk" attack-p 5.5)
            (builtin-fx-dynamics-option fx "rel" release-p 5.9))
          (box 
            :padding 1
            :corner-radius 16 :background-color :black
            (v-stack :gap 0.34
              (h-stack :gap 0.5 :align :center
                (if input-p (builtin-fx-dynamics-number-knob fx "in" input-p 1) (box :width 0 :height 0))
                (builtin-fx-dynamics-percent-knob fx "amt" amount-p)
                (builtin-fx-dynamics-number-knob fx "low" low-cut-p 0)
                (if knee-p (builtin-fx-dynamics-number-knob fx "knee" knee-p 1) (box :width 0 :height 0))
                )
              (h-stack :gap 0.5 :align :center
                (builtin-fx-dynamics-percent-knob fx "drive" drive-p)
                (builtin-fx-dynamics-number-knob fx "out" output-p 1)
                (builtin-fx-dynamics-percent-knob fx "mix" mix-p)
                )
              )
            )
          )
        (fx-param-grid params fx)))))

;; The Compressor panel lives in ui/effects/builtin/compressor.lisp.

(def builtin-fx-limiter-ui (fx)
  (let ((params (get fx :params)))
    (let ((input-p (builtin-fx-param params "input"))
          (ceiling-p (builtin-fx-param params "ceiling"))
          (release-p (builtin-fx-param params "release"))
          (lookahead-p (builtin-fx-param params "lookahead")))
      (if (and input-p ceiling-p release-p lookahead-p)
        (v-stack :gap 0.34
          (h-stack :gap 0.65 :align :center
            (builtin-fx-dynamics-number-knob fx "input" input-p 1)
            (builtin-fx-dynamics-number-knob fx "ceil" ceiling-p 1)
            (builtin-fx-dynamics-number-knob fx "rel" release-p 0)
            (builtin-fx-dynamics-number-knob fx "look" lookahead-p 1)))
        (fx-param-grid params fx)))))
