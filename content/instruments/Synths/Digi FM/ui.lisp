; Compact four-operator surface; the host owns the title/header.
(def df-accent () :control-on-bg)
(def df-ink () :control-on-fg)
(def df-bound (name fallback)
  (eseq.effects.custom-ui-controls/ui-param-bound-value name fallback))
(def df-knob (section name title)
  (eseq.effects.custom-ui-lego/ui-lego-knob-styled-s section name title 4.7 2.15 2.58 (df-accent) 2 "linear" :widget-knob-track 10.5 9.5 :right))
(def df-short-knob (section name title)
  (eseq.effects.custom-ui-lego/ui-lego-knob-styled-s section name title 4.7 2.1 2.8 (df-accent) 2 "linear" :widget-knob-track 10.5 9.5 :right))
(def df-log (section name title)
  (df-log-sized section name title 4.7))
(def df-log-sized (section name title width)
  (eseq.effects.custom-ui-lego/ui-lego-knob-styled-s section name title width 2.15 2.58 (df-accent) 1 "log" :widget-knob-track 10.5 9.5 :right))
(def df-num (section name title)
  (df-num-labeled section name title false))
(def df-num-labeled (section name title labels)
  (df-readout section name title labels 5.1 1.05 0.43 2 0.01 (df-ink)))
(def df-compact (section name title)
  (df-readout section name title false 4.6 1.02 0.46 2 0.01 :dim))
(def df-readout (section name title labels width height label-height decimals step ink)
  (let ((p (eseq.effects.custom-ui-runtime/custom-ui-current-param name))
        (ink (if (eseq.effects.custom-ui-runtime/custom-ui-param-mod-highlighted? p) :white ink)))
    (eseq.effects.custom-ui-runtime/custom-ui-param-mod-wrapper p (str "df-num-mod-" (eseq.effects.custom-ui-runtime/custom-ui-scope-name) "-" name)
      (subtree :key (str "df-num-" (eseq.effects.custom-ui-runtime/custom-ui-scope-name)
          (eseq.effects.custom-ui-runtime/custom-ui-param-control-key-mode p) "-" name)
        (v-stack :width width :height height :gap 0.06
          (label title :v-align :center :height label-height :font-size 8.6 :color ink :bg :transparent)
          (number-picker :width width :height 0.50 :noui true :decimals decimals :step step :font-size 8.0 :value-labels labels
            :value (eseq.effects.custom-ui-runtime/custom-ui-param-binding p)
            :min (eseq.effects.custom-ui-runtime/custom-ui-param-control-min p)
            :process-value (eseq.effects.custom-ui-runtime/custom-ui-param-process-value p) :process-clamped (eseq.effects.custom-ui-runtime/custom-ui-param-process-clamped p)
            :max (eseq.effects.custom-ui-runtime/custom-ui-param-control-max p)
            :text-align :left
            :text-color ink :edit-color ink :cursor-color ink
            :plock-style :underline
            :plock-active (if (eseq.effects.custom-ui-runtime/custom-ui-param-plock-active? p) 1 0)
            :on-change (if (number? section)
              (eseq.effects.custom-ui-runtime/custom-ui-param-change-callback-s section p)
              (eseq.effects.custom-ui-runtime/custom-ui-param-change-callback p))))))))
(def df-option (section name title options)
  (df-option-sized section name title options 7 1.05 (df-ink) (df-accent)))
(def df-source-option (section name title options width)
  (df-option-sized section name title options width 1.05 :dim :instrument-control-bg))
(def df-option-sized (section name title options width height ink surface)
  (let ((p (eseq.effects.custom-ui-runtime/custom-ui-current-param name))
        (scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope)))
    (eseq.effects.custom-ui-runtime/custom-ui-param-mod-wrapper p (str "df-option-mod-" (eseq.effects.custom-ui-runtime/custom-ui-scope-name) "-" name)
      (subtree :key (str "df-option-" (eseq.effects.custom-ui-runtime/custom-ui-scope-name) "-" name)
        (v-stack :width width :height height :gap 0.04
          (label title :v-align :center :height (- height 0.79) :font-size 8.6 :color ink :bg :transparent)
          (dropdown :width width :height 0.75 :font-size 7.6
            :value-index (eseq.effects.custom-ui-runtime/custom-ui-param-binding p)
            :value-index-offset (get p :min) :options options
            :text-color ink :chevron-color ink :badge-color :transparent
            :bg-color surface :border-color :transparent :border-width 0
            :plock-active (if (eseq.effects.custom-ui-runtime/custom-ui-param-plock-active? p) 1 0)
            :plock-color-r (eseq.effects.param-controls/param-plock-color-r)
            :plock-color-g (eseq.effects.param-controls/param-plock-color-g)
            :plock-color-b (eseq.effects.param-controls/param-plock-color-b)
            :on-change (lambda (v)
              (do
                (eseq.effects.custom-ui-sections/custom-ui-select-section-in-scope scope section)
                (eseq.effects.custom-ui-runtime/custom-ui-set-param-in-scope scope p
                  (+ (get p :min) (eseq.effects.param-controls/custom-ui-option-index options v)))))))))))
(def df-switch (section name title)
  (let ((p (eseq.effects.custom-ui-runtime/custom-ui-current-param name))
      (scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope)))
    (let ((on (> (reactive-value (eseq.effects.custom-ui-runtime/custom-ui-param-binding p)) 0.5)))
      (button title :width 4.3 :height 0.75 :font-size 8 :padding 0 :corner-radius 1
        :color (if on (df-ink) :dim)
        :background-color (if on (df-accent) :instrument-control-bg)
        :border-color :transparent
        :on-click (lambda (x y r)
          (do
            (if (number? section)
              (eseq.effects.custom-ui-sections/custom-ui-select-section-in-scope scope section) false)
            (eseq.effects.custom-ui-runtime/custom-ui-set-param-in-scope scope p (if on 0 1))))))))
(def df-panel (section width height body)
  (box :width width :height height :padding 0.12
    :background-color (if (= eseq.vanilla/custom-ui-selected-section section) :instrument-panel-bg :instrument-group-bg) :corner-radius 3
    :on-click (eseq.effects.custom-ui-sections/ui-section-select-callback section)
    body))


(defwidget df-routing
  :width 7.8 :height 1.65
  :state (mode selected) :bindable (selected)
  :shader
  (let ((ink (if (> selected .5) :control-on-bg :control-on-fg)))
    (sdf/layer
      (sdf/fill (sdf/rect width height) (if (> selected .5) :control-on-fg :control-on-bg))
      (if (= mode 1) (sdf/layer
        (sdf/stroke (sdf/line (* width 0.4) (* height -0.6) (* width 0.4) (* height 0.25)) .04 ink)
        (sdf/stroke (sdf/line (* width 0.4) (* height 0.25) (* width -0.55) (* height 0.55)) .04 ink)
        (sdf/stroke (sdf/line (* width -0.55) (* height -0.2) (* width -0.55) (* height 0.55)) .04 ink)
        (sdf/stroke (sdf/translate (* width -0.6200000000000001) (* height -0.30000000000000004) (sdf/rect (* width .1) (* height .16))) .04 ink)
        (sdf/stroke (sdf/line (* width -0.55) (* height 0.55) (* width -0.35) (* height .88)) .018 ink)
        (sdf/stroke (sdf/line (* width 0.4) (* height 0.25) (* width 0.55) (* height .88)) .018 ink)
      ) (rgba 0 0 0 0))
      (if (= mode 2) (sdf/layer
        (sdf/stroke (sdf/line (* width -0.55) (* height -0.2) (* width -0.55) (* height 0.55)) .04 ink)
        (sdf/stroke (sdf/line (* width 0.4) (* height -0.6) (* width 0.4) (* height 0.25)) .04 ink)
        (sdf/stroke (sdf/translate (* width 0.33) (* height -0.7) (sdf/rect (* width .1) (* height .16))) .04 ink)
        (sdf/stroke (sdf/line (* width -0.55) (* height 0.55) (* width -0.35) (* height .88)) .018 ink)
        (sdf/stroke (sdf/line (* width 0.4) (* height 0.25) (* width 0.55) (* height .88)) .018 ink)
      ) (rgba 0 0 0 0))
      (if (= mode 3) (sdf/layer
        (sdf/stroke (sdf/line (* width -0.55) (* height -0.2) (* width -0.55) (* height 0.55)) .04 ink)
        (sdf/stroke (sdf/line (* width -0.55) (* height -0.2) (* width 0.4) (* height 0.25)) .04 ink)
        (sdf/stroke (sdf/line (* width -0.55) (* height -0.2) (* width 0.4) (* height -0.6)) .04 ink)
        (sdf/stroke (sdf/translate (* width -0.6200000000000001) (* height -0.30000000000000004) (sdf/rect (* width .1) (* height .16))) .04 ink)
        (sdf/stroke (sdf/line (* width -0.55) (* height 0.55) (* width -0.35) (* height .88)) .018 ink)
        (sdf/stroke (sdf/line (* width 0.4) (* height -0.6) (* width -0.35) (* height .88)) .018 ink)
        (sdf/stroke (sdf/line (* width 0.4) (* height 0.25) (* width 0.55) (* height .88)) .018 ink)
      ) (rgba 0 0 0 0))
      (if (= mode 4) (sdf/layer
        (sdf/stroke (sdf/line (* width 0.4) (* height -0.6) (* width 0.4) (* height 0.25)) .04 ink)
        (sdf/stroke (sdf/line (* width 0.4) (* height 0.25) (* width -0.55) (* height -0.2)) .04 ink)
        (sdf/stroke (sdf/line (* width -0.55) (* height -0.2) (* width -0.55) (* height 0.55)) .04 ink)
        (sdf/stroke (sdf/translate (* width 0.33) (* height -0.7) (sdf/rect (* width .1) (* height .16))) .04 ink)
        (sdf/stroke (sdf/line (* width -0.55) (* height 0.55) (* width -0.35) (* height .88)) .018 ink)
        (sdf/stroke (sdf/line (* width 0.4) (* height 0.25) (* width 0.55) (* height .88)) .018 ink)
      ) (rgba 0 0 0 0))
      (if (= mode 5) (sdf/layer
        (sdf/stroke (sdf/line (* width 0.4) (* height -0.6) (* width 0.4) (* height 0.25)) .04 ink)
        (sdf/stroke (sdf/line (* width 0.4) (* height -0.6) (* width -0.55) (* height -0.2)) .04 ink)
        (sdf/stroke (sdf/line (* width 0.4) (* height 0.25) (* width -0.55) (* height -0.2)) .04 ink)
        (sdf/stroke (sdf/line (* width -0.55) (* height -0.2) (* width -0.55) (* height 0.55)) .04 ink)
        (sdf/stroke (sdf/translate (* width 0.33) (* height 0.15) (sdf/rect (* width .1) (* height .16))) .04 ink)
        (sdf/stroke (sdf/line (* width -0.55) (* height 0.55) (* width -0.35) (* height .88)) .018 ink)
        (sdf/stroke (sdf/line (* width -0.55) (* height -0.2) (* width 0.55) (* height .88)) .018 ink)
      ) (rgba 0 0 0 0))
      (if (= mode 6) (sdf/layer
        (sdf/stroke (sdf/line (* width -0.55) (* height -0.2) (* width -0.55) (* height 0.55)) .04 ink)
        (sdf/stroke (sdf/line (* width -0.55) (* height -0.2) (* width 0.4) (* height 0.25)) .04 ink)
        (sdf/stroke (sdf/line (* width 0.4) (* height -0.6) (* width -0.55) (* height 0.55)) .04 ink)
        (sdf/stroke (sdf/line (* width 0.4) (* height -0.6) (* width 0.4) (* height 0.25)) .04 ink)
        (sdf/stroke (sdf/translate (* width -0.6200000000000001) (* height -0.30000000000000004) (sdf/rect (* width .1) (* height .16))) .04 ink)
        (sdf/stroke (sdf/line (* width -0.55) (* height 0.55) (* width -0.35) (* height .88)) .018 ink)
        (sdf/stroke (sdf/line (* width 0.4) (* height 0.25) (* width 0.55) (* height .88)) .018 ink)
      ) (rgba 0 0 0 0))
      (if (= mode 7) (sdf/layer
        (sdf/stroke (sdf/line (* width -0.55) (* height -0.2) (* width -0.55) (* height 0.55)) .04 ink)
        (sdf/stroke (sdf/line (* width 0.4) (* height -0.6) (* width 0.4) (* height 0.25)) .04 ink)
        (sdf/stroke (sdf/translate (* width -0.6200000000000001) (* height -0.30000000000000004) (sdf/rect (* width .1) (* height .16))) .04 ink)
        (sdf/stroke (sdf/line (* width -0.55) (* height 0.55) (* width -0.35) (* height .88)) .018 ink)
        (sdf/stroke (sdf/line (* width -0.55) (* height -0.2) (* width -0.35) (* height .88)) .04 ink)
        (sdf/stroke (sdf/line (* width 0.4) (* height 0.25) (* width 0.55) (* height .88)) .04 ink)
        (sdf/stroke (sdf/line (* width 0.4) (* height -0.6) (* width 0.55) (* height .88)) .04 ink)
      ) (rgba 0 0 0 0))
      (if (= mode 8) (sdf/layer
        (sdf/stroke (sdf/line (* width -0.55) (* height -0.2) (* width -0.55) (* height 0.55)) .04 ink)
        (sdf/stroke (sdf/translate (* width 0.33) (* height 0.15) (sdf/rect (* width .1) (* height .16))) .04 ink)
        (sdf/stroke (sdf/line (* width -0.55) (* height 0.55) (* width -0.35) (* height .88)) .018 ink)
        (sdf/stroke (sdf/line (* width 0.4) (* height -0.6) (* width -0.35) (* height .88)) .04 ink)
        (sdf/stroke (sdf/line (* width 0.4) (* height 0.25) (* width 0.55) (* height .88)) .04 ink)
      ) (rgba 0 0 0 0))
      (sdf/fill (sdf/translate (* width -0.55) (* height 0.55) (sdf/rect (* width .08) (* height .12))) ink)
      (sdf/fill (sdf/translate (* width -0.55) (* height -0.2) (sdf/rect (* width .08) (* height .12))) ink)
      (sdf/fill (sdf/translate (* width 0.4) (* height 0.25) (sdf/rect (* width .08) (* height .12))) ink)
      (sdf/fill (sdf/translate (* width 0.4) (* height -0.6) (sdf/rect (* width .08) (* height .12))) ink)
)))
(defwidget df-spectrum
  :width 32 :height 4.7
  :state (harm) :bindable (harm)
  :shader
  (let ((position (abs harm)))
    (sdf/layer
      (let ((amp (+ (* 1.0000000000 (max 0 (- 1 (abs (- position 0))))) (* 1.0000000000 (max 0 (- 1 (abs (- position 1))))) (* 1.0000000000 (max 0 (- 1 (abs (- position 2))))) (* 1.0000000000 (max 0 (- 1 (abs (- position 3))))) (* 1.0000000000 (max 0 (- 1 (abs (- position 4))))) (* 1.0000000000 (max 0 (- 1 (abs (- position 5))))) (* 1.0000000000 (max 0 (- 1 (abs (- position 6)))))))) (sdf/fill (sdf/translate (* width -0.88125000) (* height (- .8 (* .8 amp))) (sdf/rect (* width .04) (* height .8 amp))) :control-on-fg))
      (let ((amp (+ (* 0.5000000000 (max 0 (- 1 (abs (- position 1))))) (* 0.3778918707 (max 0 (- 1 (abs (- position 2))))) (* 0.1500000000 (max 0 (- 1 (abs (- position 3))))) (* 0.1200000000 (max 0 (- 1 (abs (- position 6)))))))) (sdf/fill (sdf/translate (* width -0.76375000) (* height (- .8 (* .8 amp))) (sdf/rect (* width .04) (* height .8 amp))) :control-on-fg))
      (let ((amp (+ (* 0.3333333333 (max 0 (- 1 (abs (- position 1))))) (* 0.1904030213 (max 0 (- 1 (abs (- position 2))))) (* 0.3333333333 (max 0 (- 1 (abs (- position 3))))) (* 0.3333333333 (max 0 (- 1 (abs (- position 4))))) (* 0.1757641413 (max 0 (- 1 (abs (- position 5))))) (* 0.5000000000 (max 0 (- 1 (abs (- position 6)))))))) (sdf/fill (sdf/translate (* width -0.64625000) (* height (- .8 (* .8 amp))) (sdf/rect (* width .04) (* height .8 amp))) :control-on-fg))
      (let ((amp (+ (* 0.2500000000 (max 0 (- 1 (abs (- position 1))))) (* 0.1079276309 (max 0 (- 1 (abs (- position 2))))) (* 0.0750000000 (max 0 (- 1 (abs (- position 3)))))))) (sdf/fill (sdf/translate (* width -0.52875000) (* height (- .8 (* .8 amp))) (sdf/rect (* width .04) (* height .8 amp))) :control-on-fg))
      (let ((amp (+ (* 0.2000000000 (max 0 (- 1 (abs (- position 1))))) (* 0.0652559589 (max 0 (- 1 (abs (- position 2))))) (* 0.2000000000 (max 0 (- 1 (abs (- position 3))))) (* 0.2000000000 (max 0 (- 1 (abs (- position 4))))) (* 0.0556074601 (max 0 (- 1 (abs (- position 5))))) (* 0.3000000000 (max 0 (- 1 (abs (- position 6)))))))) (sdf/fill (sdf/translate (* width -0.41125000) (* height (- .8 (* .8 amp))) (sdf/rect (* width .04) (* height .8 amp))) :control-on-fg))
      (let ((amp (+ (* 0.1666666667 (max 0 (- 1 (abs (- position 1))))) (* 0.0410994940 (max 0 (- 1 (abs (- position 2))))) (* 0.0500000000 (max 0 (- 1 (abs (- position 3)))))))) (sdf/fill (sdf/translate (* width -0.29375000) (* height (- .8 (* .8 amp))) (sdf/rect (* width .04) (* height .8 amp))) :control-on-fg))
      (let ((amp (+ (* 0.1428571429 (max 0 (- 1 (abs (- position 1))))) (* 0.0266248537 (max 0 (- 1 (abs (- position 2))))) (* 0.1428571429 (max 0 (- 1 (abs (- position 3))))) (* 0.1428571429 (max 0 (- 1 (abs (- position 4))))) (* 0.0209438517 (max 0 (- 1 (abs (- position 5)))))))) (sdf/fill (sdf/translate (* width -0.17625000) (* height (- .8 (* .8 amp))) (sdf/rect (* width .04) (* height .8 amp))) :control-on-fg))
      (let ((amp (+ (* 0.1250000000 (max 0 (- 1 (abs (- position 1))))) (* 0.0176073026 (max 0 (- 1 (abs (- position 2))))) (* 0.0375000000 (max 0 (- 1 (abs (- position 3))))) (* 0.1800000000 (max 0 (- 1 (abs (- position 6)))))))) (sdf/fill (sdf/translate (* width -0.05875000) (* height (- .8 (* .8 amp))) (sdf/rect (* width .04) (* height .8 amp))) :control-on-fg))
      (let ((amp (+ (* 0.1111111111 (max 0 (- 1 (abs (- position 1))))) (* 0.0118287227 (max 0 (- 1 (abs (- position 2))))) (* 0.1111111111 (max 0 (- 1 (abs (- position 3))))) (* 0.1111111111 (max 0 (- 1 (abs (- position 4))))) (* 0.0085894156 (max 0 (- 1 (abs (- position 5)))))))) (sdf/fill (sdf/translate (* width 0.05875000) (* height (- .8 (* .8 amp))) (sdf/rect (* width .04) (* height .8 amp))) :control-on-fg))
      (let ((amp (+ (* 0.1000000000 (max 0 (- 1 (abs (- position 1))))) (* 0.0080459607 (max 0 (- 1 (abs (- position 2))))) (* 0.0300000000 (max 0 (- 1 (abs (- position 3)))))))) (sdf/fill (sdf/translate (* width 0.17625000) (* height (- .8 (* .8 amp))) (sdf/rect (* width .04) (* height .8 amp))) :control-on-fg))
      (let ((amp (+ (* 0.0909090909 (max 0 (- 1 (abs (- position 1))))) (* 0.0055281875 (max 0 (- 1 (abs (- position 2))))) (* 0.0909090909 (max 0 (- 1 (abs (- position 3))))) (* 0.0909090909 (max 0 (- 1 (abs (- position 4))))) (* 0.0037056549 (max 0 (- 1 (abs (- position 5)))))))) (sdf/fill (sdf/translate (* width 0.29375000) (* height (- .8 (* .8 amp))) (sdf/rect (* width .04) (* height .8 amp))) :control-on-fg))
      (let ((amp (+ (* 0.0833333333 (max 0 (- 1 (abs (- position 1))))) (* 0.0038299381 (max 0 (- 1 (abs (- position 2))))) (* 0.0250000000 (max 0 (- 1 (abs (- position 3)))))))) (sdf/fill (sdf/translate (* width 0.41125000) (* height (- .8 (* .8 amp))) (sdf/rect (* width .04) (* height .8 amp))) :control-on-fg))
      (let ((amp (+ (* 0.0769230769 (max 0 (- 1 (abs (- position 1))))) (* 0.0026719430 (max 0 (- 1 (abs (- position 2))))) (* 0.0769230769 (max 0 (- 1 (abs (- position 3))))) (* 0.0769230769 (max 0 (- 1 (abs (- position 4))))) (* 0.0016533539 (max 0 (- 1 (abs (- position 5))))) (* 0.0800000000 (max 0 (- 1 (abs (- position 6)))))))) (sdf/fill (sdf/translate (* width 0.52875000) (* height (- .8 (* .8 amp))) (sdf/rect (* width .04) (* height .8 amp))) :control-on-fg))
      (let ((amp (+ (* 0.0714285714 (max 0 (- 1 (abs (- position 1))))) (* 0.0018751674 (max 0 (- 1 (abs (- position 2))))) (* 0.0214285714 (max 0 (- 1 (abs (- position 3)))))))) (sdf/fill (sdf/translate (* width 0.64625000) (* height (- .8 (* .8 amp))) (sdf/rect (* width .04) (* height .8 amp))) :control-on-fg))
      (let ((amp (+ (* 0.0666666667 (max 0 (- 1 (abs (- position 1))))) (* 0.0013227396 (max 0 (- 1 (abs (- position 2))))) (* 0.0666666667 (max 0 (- 1 (abs (- position 3))))) (* 0.0666666667 (max 0 (- 1 (abs (- position 4))))) (* 0.0007555609 (max 0 (- 1 (abs (- position 5)))))))) (sdf/fill (sdf/translate (* width 0.76375000) (* height (- .8 (* .8 amp))) (sdf/rect (* width .04) (* height .8 amp))) :control-on-fg))
      (let ((amp (+ (* 0.0625000000 (max 0 (- 1 (abs (- position 1))))) (* 0.0009372236 (max 0 (- 1 (abs (- position 2))))) (* 0.0187500000 (max 0 (- 1 (abs (- position 3)))))))) (sdf/fill (sdf/translate (* width 0.88125000) (* height (- .8 (* .8 amp))) (sdf/rect (* width .04) (* height .8 amp))) :control-on-fg))
)))
; Appended to the authored scoped-control helpers by build_ui.py.
(def df-row (section prefix title)
  (df-panel section 19.6 2.35
    (h-stack :gap 0.2 :align :center
      (button title :width 1.7 :height 1.7 :font-size 8 :padding 0
        :color (df-accent) :background-color :transparent
        :on-click (eseq.effects.custom-ui-sections/ui-section-select-callback section))
      (df-knob section (str prefix "_ratio") "Ratio")
      (df-knob section (str prefix "_fine") "Fine")
      (df-knob section (str prefix "_level_db") "Level dB"))))

(def df-tab (section title)
  (button title :width 4.55 :height 0.7 :font-size 7.5 :padding 0 :corner-radius 1
    :color (if (= eseq.vanilla/custom-ui-selected-section section) (df-accent) (df-ink))
    :background-color (if (= eseq.vanilla/custom-ui-selected-section section) (df-ink) (df-accent))
    :on-click (eseq.effects.custom-ui-sections/ui-section-select-callback section)))

(def df-timbre (section prefix)
  (v-stack :gap 0.3
    (adsr-editor :mode :ade :debug-name "df-timbre-contour" :width 32 :height 2.5
      :curve-color (df-ink) :point-color (df-ink) :grid-color (df-ink) :background-color (df-accent)
      :delay-max 5000 :attack-max 5000 :decay-max 12000
      :delay (df-bound (str prefix "_delay_ms") 0)
      :attack (df-bound (str prefix "_attack_ms") 4)
      :decay (df-bound (str prefix "_decay_ms") 400)
      :end (df-bound (str prefix "_end") 0.35)
      :gated (df-bound (str prefix "_gated") 0)
      :hold-on-release (df-bound (str prefix "_hold") 1)
      :on-change (let ((scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope)))
        (lambda (env)
          (eseq.effects.custom-ui-runtime/custom-ui-set-envelope-in-scope scope
            (list (list :delay (str prefix "_delay_ms"))
                  (list :attack (str prefix "_attack_ms"))
                  (list :decay (str prefix "_decay_ms"))
                  (list :end (str prefix "_end"))) env))))
    (h-stack :gap 0.4
      (df-num section (str prefix "_delay_ms") "Delay ms")
      (df-num section (str prefix "_attack_ms") "Attack ms")
      (df-num section (str prefix "_decay_ms") "Decay ms")
      (df-num section (str prefix "_end") "End")
      (df-num section (str prefix "_depth") "FM depth"))
    (h-stack :gap 0.4
      (df-option section (str prefix "_gated") "Contour" '("Triggered" "Gated"))
      (df-option section (str prefix "_reset") "Retrigger" '("Continue" "Reset"))
      (df-option-sized section (str prefix "_hold") "Release" '("Continue" "Hold") 7 1.05 (df-ink) (df-accent)))
    (if (= prefix "a")
      (df-num section "a_keyscale" "Keyscale")
      (h-stack :gap 0.4 (df-readout section "b1_keyscale" "B1 keyscale" false 7 1.05 0.43 2 0.01 (df-ink)) (df-readout section "b2_keyscale" "B2 keyscale" false 7 1.05 0.43 2 0.01 (df-ink))))))

(def df-adsr (section prefix)
  (v-stack :gap 0.4
    (adsr-editor :width 32 :height 2.5 :debug-name "df-adsr"
      :curve-color (df-ink) :point-color (df-ink) :grid-color (df-ink) :background-color (df-accent)
      :attack (df-bound (str prefix "_attack_ms") 4)
      :decay (df-bound (str prefix "_decay_ms") 400)
      :sustain (df-bound (str prefix "_sustain") 0.7)
      :release (df-bound (str prefix "_release_ms") 400)
      :attack-max 5000 :decay-max 12000 :release-max 12000
      :on-change (let ((scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope)))
        (lambda (env)
          (eseq.effects.custom-ui-runtime/custom-ui-set-adsr-in-scope scope
            (str prefix "_attack_ms") (str prefix "_decay_ms")
            (str prefix "_sustain") (str prefix "_release_ms") env))))
    (h-stack :gap 0.4
      (df-num section (str prefix "_attack_ms") "Attack ms")
      (df-num section (str prefix "_decay_ms") "Decay ms")
      (df-num section (str prefix "_sustain") "Sustain")
      (df-num section (str prefix "_release_ms") "Release ms"))
    (if (= prefix "filter")
      (h-stack :gap 0.4
        (df-num section "filter_depth" "Env oct")
        (df-num section "keytrack" "Keytrack")
        (df-num section "highpass" "Highpass Hz"))
      (h-stack :gap 0.4
        (df-num section "velocity_amount" "Velocity")
        (df-num section "pan" "Pan")))))

(def df-algorithm-button (index)
  (let ((p (eseq.effects.custom-ui-runtime/custom-ui-current-param "algorithm"))
        (scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope)))
    (eseq.effects.custom-ui-runtime/custom-ui-param-mod-wrapper p (str "df-algorithm-" index)
      (v-stack :gap 0.05
        (label (str index) :width 7.8 :height 0.55 :font-size 7.5 :v-align :center :color (df-ink) :bg :transparent)
        (df-routing :mode index :selected (= (reactive-value (df-bound "algorithm" 2)) index)
        :width 7.8 :height 1.65 :debug-name (str "df-algorithm-" index)
        :on-click (lambda (x y r)
          (eseq.effects.custom-ui-runtime/custom-ui-set-param-in-scope scope p index)))))))

(def df-routing-page ()
  (v-stack :gap 0.4
    (h-stack :gap 0.3 (df-algorithm-button 1) (df-algorithm-button 2) (df-algorithm-button 3) (df-algorithm-button 4))
    (h-stack :gap 0.3 (df-algorithm-button 5) (df-algorithm-button 6) (df-algorithm-button 7) (df-algorithm-button 8))
    (h-stack :gap 0.6
      (df-readout 0 "algorithm" "Algorithm" false 5.1 1.05 0.43 0 1 (df-ink))
      (df-num 0 "feedback" "Feedback")
      (df-num 0 "mix_xy" "X / Y")
      (df-num 0 "harmonics" "Harmonics"))))

(def df-spectrum-page ()
  (v-stack :gap 0.4
    (df-spectrum :width 32 :height 4.7 :harm (eseq.effects.param-controls/param-effective-value
        (eseq.effects.custom-ui-runtime/custom-ui-current-param "harmonics")) :debug-name "df-harmonic-spectrum")
    (h-stack :gap 0.5
      (df-num 3 "harmonics" "Harmonics")
      (df-option-sized 4 "phase_reset" "Phase reset" '("Free" "All" "C" "A+B" "A+B2") 8 1.05 (df-ink) (df-accent)))))

(def df-detail ()
  (let ((s eseq.vanilla/custom-ui-selected-section))
    (box :width 34 :height 9.7 :padding 0.35 :corner-radius 2 :background-color (df-accent) :debug-name "df-detail"
      (v-stack :gap 0.35
        (h-stack :gap 0.1 (df-tab 0 "Route") (df-tab 1 "A env") (df-tab 2 "B env") (df-tab 3 "Harm") (df-tab 4 "Phase") (df-tab 5 "Filter") (df-tab 6 "Amp"))
        (if (= s 1) (df-timbre 1 "a")
          (if (= s 2) (df-timbre 2 "b")
            (if (= s 3) (df-spectrum-page)
              (if (= s 4)
                (h-stack :gap 0.4
                  (df-option-sized 4 "phase_reset" "Phase reset" '("Free" "All" "C" "A+B" "A+B2") 8 1.05 (df-ink) (df-accent))
                  (df-num 4 "pan" "Pan") (df-num 4 "velocity_amount" "Velocity"))
                (if (= s 5) (df-adsr 5 "filter")
                  (if (= s 6) (df-adsr 6 "amp") (df-routing-page)))))))))))

(defsynth-ui
  (h-stack :gap 0.2 :align :start
    (v-stack :gap 0.1 :debug-name "df-operators"
      (df-row 2 "b2" "B2") (df-row 2 "b1" "B1") (df-row 1 "a" "A") (df-row 4 "c" "C"))
    (df-detail)
    (v-stack :gap 0.15 :debug-name "df-output"
      (df-panel 5 17 3.6
        (v-stack :gap 0.1
          (df-source-option 5 "filter_type" "Filter" '("I 12dB" "II 24dB") 6)
          (h-stack :gap 0.3 (df-log 5 "cutoff" "Cutoff Hz") (df-knob 5 "resonance" "Resonance") (df-log 5 "highpass" "HP Hz"))))
      (df-panel 6 17 2.9
        (h-stack :gap 0.4 :align :center
          (df-knob 6 "amp_attack_ms" "Attack ms") (df-log 6 "amp_release_ms" "Release ms")))
      (df-panel 4 17 2.9
        (h-stack :gap 0.4 :align :center
          (df-knob 4 "source_db" "Source dB") (df-knob 4 "volume_db" "Volume dB"))))))
