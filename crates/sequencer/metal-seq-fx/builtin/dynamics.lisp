;; Dynamics, compressor, and limiter built-in FX panels.
(def builtin-fx-dynamics-percent-knob (fx label-text p)
  (knob-number :label label-text
    :value (fx-param-value p)
    :min (get p :min) :max (get p :max) :value-scale 100 :decimals 0
    :font-size 9.5 :label-font-size 9.5
    :text-color :fg :label-color :dim
    :width 6.4 :height 3.2 :knob-size 2.0
    :on-change (lambda (v) (fx-set-effect-value fx p v))))

(def builtin-fx-dynamics-number-knob (fx label-text p decimals)
  (knob-number :label label-text
    :value (fx-param-value p)
    :min (get p :min) :max (get p :max) :decimals decimals
    :font-size 9.5 :label-font-size 9.5
    :text-color :fg :label-color :dim
    :width 6.8 :height 3.2 :knob-size 2.0
    :on-change (lambda (v) (fx-set-effect-value fx p v))))

(def builtin-fx-dynamics-option (fx label-text p width)
  (h-stack :gap 0.22 :align :center
    (label label-text :font-size 8.5 :width 4.7 :color :dim :bg :transparent)
    (dropdown :value (get p :text-value)
      :options (get p :options)
      :on-change (lambda (v) (builtin-fx-set-effect-option fx p v))
      :width width :height 1.05 :font-size 9.5)))

(def builtin-fx-dynamics-ui (fx)
  (let ((params (get fx :params)))
    (let ((amount-p (builtin-fx-param params "amount"))
          (attack-p (builtin-fx-param params "attack"))
          (release-p (builtin-fx-param params "release"))
          (low-cut-p (builtin-fx-param params "low cut"))
          (drive-p (builtin-fx-param params "drive"))
          (output-p (builtin-fx-param params "output"))
          (mix-p (builtin-fx-param params "mix")))
      (if (and amount-p attack-p release-p low-cut-p drive-p output-p mix-p)
        (v-stack :gap 0.34
          (h-stack :gap 0.45 :align :center
            (builtin-fx-dynamics-option fx "atk" attack-p 5.5)
            (builtin-fx-dynamics-option fx "rel" release-p 5.9))
          (h-stack :gap 0.5 :align :center
            (builtin-fx-dynamics-percent-knob fx "amt" amount-p)
            (builtin-fx-dynamics-number-knob fx "low" low-cut-p 0)
            (builtin-fx-dynamics-percent-knob fx "drive" drive-p)
            (builtin-fx-dynamics-number-knob fx "out" output-p 1)
            (builtin-fx-dynamics-percent-knob fx "mix" mix-p)))
        (fx-param-grid params fx)))))

(def builtin-fx-compressor-ui (fx)
  (let ((params (get fx :params)))
    (let ((threshold-p (builtin-fx-param params "threshold"))
          (ratio-p (builtin-fx-param params "ratio"))
          (attack-p (builtin-fx-param params "attack"))
          (release-p (builtin-fx-param params "release"))
          (makeup-p (builtin-fx-param params "makeup"))
          (mix-p (builtin-fx-param params "mix")))
      (if (and threshold-p ratio-p attack-p release-p makeup-p mix-p)
        (v-stack :gap 0.34
          (h-stack :gap 0.5 :align :center
            (builtin-fx-dynamics-number-knob fx "thr" threshold-p 1)
            (builtin-fx-dynamics-number-knob fx "ratio" ratio-p 1)
            (builtin-fx-dynamics-number-knob fx "atk" attack-p 1)
            (builtin-fx-dynamics-number-knob fx "rel" release-p 0)
            (builtin-fx-dynamics-number-knob fx "mkup" makeup-p 1)
            (builtin-fx-dynamics-percent-knob fx "mix" mix-p)))
        (fx-param-grid params fx)))))

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
