;; Dynamics, compressor, and limiter built-in FX panels.
(module eseq.effects.builtin.dynamics)

(import eseq.effects.builtin.filter-core :refer
  (builtin-fx-param
   builtin-fx-set-effect-option))
(import eseq.effects.param-controls :refer
  (fx-param-value
   fx-set-effect-value
   param-plock-active?
   param-plock-color-b
   param-plock-color-g
   param-plock-color-r
   param-plock-default
   param-plock-text-color))
(import eseq.effects.param-grid :refer (fx-param-grid))

(export percent-knob
        number-knob
        option-row
        dynamics-ui
        limiter-ui)

(def percent-knob (fx label-text p)
  (knob-number :label label-text
    :value (eseq.effects.param-controls/fx-param-value p)
    :min (get p :min) :max (get p :max) :value-scale 100 :decimals 0
    :font-size 9.5 :label-font-size 9.5
    :text-color (eseq.effects.param-controls/param-plock-text-color fx p) :label-color :dim
    :plock-active (if (eseq.effects.param-controls/param-plock-active? fx p) 1 0)
    :plock-default (eseq.effects.param-controls/param-plock-default fx p)
    :plock-color-r (eseq.effects.param-controls/param-plock-color-r)
    :plock-color-g (eseq.effects.param-controls/param-plock-color-g)
    :plock-color-b (eseq.effects.param-controls/param-plock-color-b)
    :width 6.4 :height 3.2 :knob-size 2.0
	    :track-color '(rgba 0.4, 0.4, 0.4, 1)
    :on-change (lambda (v) (eseq.effects.param-controls/fx-set-effect-value fx p v))))

(def number-knob (fx label-text p decimals)
  (knob-number :label label-text
    :value (eseq.effects.param-controls/fx-param-value p)
    :min (get p :min) :max (get p :max) :decimals decimals
    :font-size 9.5 :label-font-size 9.5
    :text-color (eseq.effects.param-controls/param-plock-text-color fx p) :label-color :dim
    :plock-active (if (eseq.effects.param-controls/param-plock-active? fx p) 1 0)
    :plock-default (eseq.effects.param-controls/param-plock-default fx p)
    :plock-color-r (eseq.effects.param-controls/param-plock-color-r)
    :plock-color-g (eseq.effects.param-controls/param-plock-color-g)
    :plock-color-b (eseq.effects.param-controls/param-plock-color-b)
    :width 6.8 :height 3.2 :knob-size 2.0
	    :track-color '(rgba 0.4, 0.4, 0.4, 1)
    :on-change (lambda (v) (eseq.effects.param-controls/fx-set-effect-value fx p v))))

(def option-row (fx label-text p width)
  (h-stack :gap 0.22 :align :center
    (label label-text :font-size 8.5 :width 4.7 :color :dim :bg :transparent)
    (dropdown :value (get p :text-value)
      :options (get p :options)
      :on-change (lambda (v) (eseq.effects.builtin.filter-core/builtin-fx-set-effect-option fx p v))
      :plock-active (if (eseq.effects.param-controls/param-plock-active? fx p) 1 0)
      :plock-color-r (eseq.effects.param-controls/param-plock-color-r)
      :plock-color-g (eseq.effects.param-controls/param-plock-color-g)
      :plock-color-b (eseq.effects.param-controls/param-plock-color-b)
      :width width :height 1.05 :font-size 9.5)))

(def dynamics-ui (fx)
  (let ((params (get fx :params)))
    (let ((amount-p (eseq.effects.builtin.filter-core/builtin-fx-param params "amount"))
        (attack-p (eseq.effects.builtin.filter-core/builtin-fx-param params "attack"))
        (release-p (eseq.effects.builtin.filter-core/builtin-fx-param params "release"))
        (low-cut-p (eseq.effects.builtin.filter-core/builtin-fx-param params "low cut"))
        (drive-p (eseq.effects.builtin.filter-core/builtin-fx-param params "drive"))
        (output-p (eseq.effects.builtin.filter-core/builtin-fx-param params "output"))
        (mix-p (eseq.effects.builtin.filter-core/builtin-fx-param params "mix"))
        (knee-p (eseq.effects.builtin.filter-core/builtin-fx-param params "knee"))
        (input-p (eseq.effects.builtin.filter-core/builtin-fx-param params "input")))
      (if (and amount-p attack-p release-p low-cut-p drive-p output-p mix-p)
        (v-stack :gap 0.34 :padding 0.1
          (h-stack :padding 0.5 :gap 0.45 :align :center
            (option-row fx "atk" attack-p 5.5)
            (option-row fx "rel" release-p 5.9))
          (box
            :padding 1
            :corner-radius 16 :background-color :black
            (v-stack :gap 0.34
              (h-stack :gap 0.5 :align :center
                (if input-p (number-knob fx "in" input-p 1) (box :width 0 :height 0))
                (percent-knob fx "amt" amount-p)
                (number-knob fx "low" low-cut-p 0)
                (if knee-p (number-knob fx "knee" knee-p 1) (box :width 0 :height 0))
                )
              (h-stack :gap 0.5 :align :center
                (percent-knob fx "drive" drive-p)
                (number-knob fx "out" output-p 1)
                (percent-knob fx "mix" mix-p)
                )
              )
            )
          )
        (eseq.effects.param-grid/fx-param-grid params fx)))))

;; The Compressor panel lives in ui/effects/builtin/compressor.lisp.

(def limiter-ui (fx)
  (let ((params (get fx :params)))
    (let ((input-p (eseq.effects.builtin.filter-core/builtin-fx-param params "input"))
          (ceiling-p (eseq.effects.builtin.filter-core/builtin-fx-param params "ceiling"))
          (release-p (eseq.effects.builtin.filter-core/builtin-fx-param params "release"))
          (lookahead-p (eseq.effects.builtin.filter-core/builtin-fx-param params "lookahead")))
      (if (and input-p ceiling-p release-p lookahead-p)
        (v-stack :gap 0.34
          (h-stack :gap 0.65 :align :center
            (number-knob fx "input" input-p 1)
            (number-knob fx "ceil" ceiling-p 1)
            (number-knob fx "rel" release-p 0)
            (number-knob fx "look" lookahead-p 1)))
        (eseq.effects.param-grid/fx-param-grid params fx)))))
