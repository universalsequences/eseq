; Compact four-operator surface; the host owns the title/header.
(def df-accent () :control-on-bg)
(def df-ink () :control-on-fg)
(def df-bound (name fallback)
  (eseq.effects.custom-ui-controls/ui-param-bound-value name fallback))
(def df-knob (section name title)
  (eseq.effects.custom-ui-lego/ui-lego-knob-styled-s section name title 4.7 2.15 2.58 (df-accent) 2 "linear" :widget-knob-track 9.0 8.0 :right))
(def df-short-knob (section name title)
  (eseq.effects.custom-ui-lego/ui-lego-knob-styled-s section name title 4.7 2.1 2.8 (df-accent) 2 "linear" :widget-knob-track 9.0 8.0 :right))
(def df-log (section name title)
  (df-log-sized section name title 4.7))
(def df-log-sized (section name title width)
  (eseq.effects.custom-ui-lego/ui-lego-knob-styled-s section name title width 2.15 2.58 (df-accent) 1 "log" :widget-knob-track 9.0 8.0 :right))
(def df-num (section name title)
  (df-num-labeled section name title false))
(def df-num-labeled (section name title labels)
  (df-readout section name title labels 5.1 1.05 0.43 2 0.01 (df-ink)))
(def df-compact (section name title)
  (df-readout section name title false 4.6 1.02 0.46 2 0.01 :dim))
(def df-readout (section name title labels width height label-height decimals step ink)
  (let ((p (eseq.effects.custom-ui-runtime/custom-ui-current-param name)))
    (eseq.effects.custom-ui-runtime/custom-ui-param-mod-wrapper p (str "df-num-mod-" (eseq.effects.custom-ui-runtime/custom-ui-scope-name) "-" name)
      (subtree :key (str "df-num-" (eseq.effects.custom-ui-runtime/custom-ui-scope-name)
          (eseq.effects.custom-ui-runtime/custom-ui-param-control-key-mode p) "-" name)
        (v-stack :width width :height height :gap 0.06
          (label title :v-align :center :height label-height :font-size 7.6 :color ink :bg :transparent)
          (number-picker :width width :height 0.50 :noui true :decimals decimals :step step :font-size 8.0 :value-labels labels
            :value (eseq.effects.custom-ui-runtime/custom-ui-param-binding p)
            :min (eseq.effects.custom-ui-runtime/custom-ui-param-control-min p)
            :max (eseq.effects.custom-ui-runtime/custom-ui-param-control-max p)
            :text-align :left
            :text-color (if (eseq.effects.custom-ui-runtime/custom-ui-param-plock-active? p)
              (eseq.effects.custom-ui-runtime/custom-ui-param-plock-text-color p) ink)
            :plock-active (if (eseq.effects.custom-ui-runtime/custom-ui-param-plock-active? p) 1 0)
            :plock-color-r (eseq.effects.param-controls/param-plock-color-r)
            :plock-color-g (eseq.effects.param-controls/param-plock-color-g)
            :plock-color-b (eseq.effects.param-controls/param-plock-color-b)
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
          (label title :v-align :center :height (- height 0.79) :font-size 7.6 :color ink :bg :transparent)
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
