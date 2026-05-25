;; Modulator instrument panel controls.
(def modulator-param (inst name)
  (nth (filter |p| (= (get p :name) name) (get inst :synth)) 0))

(def modulator-knob (p label-text key)
  (subtree :key key
    (knob-number :label label-text
      :value (fx-param-value p)
      :min (instrument-param-control-min p) :max (instrument-param-control-max p) :decimals 0
      :font-size 12 :label-font-size 11
      :text-color :dim :label-color :dim
      :width 7.0 :height 4.15 :knob-size 2.55
      :value-align :center
      :on-change (lambda (v) (instrument-set-param-control-value p v)))))

(def modulator-panel (inst)
  (let ((rise-p (modulator-param inst "rise"))
        (fall-p (modulator-param inst "fall")))
    (box :background "fx-panel-bg" :color :instrument-panel-bg :header :fx-panel-header-bg :selected-header :fx-panel-header-selected-bg :selected 0 :padding 0
      :height fx-fixed-panel-height
      :debug-name "modulator-panel"
      (v-stack :gap 0
        (box :height 0.75 :padding 0 :v-align :center :h-align :start
          (h-stack :gap 0.5 :align :center
            (fx-panel-header-leading-spacer)
            (fx-enabled-toggle (enabled-param (get inst :synth)) false "modulator-enabled")
            (label "Modulator" :font-size 11 :color :white :bg :transparent)))
        (fx-panel-body "modulator-panel-content"
          (box :width :fill :height 7.85 :padding 0.35
            :debug-name "modulator-panel-body"
            (h-stack :width :fill :height :fill :gap 1.05 :align :center
              (if rise-p
                (modulator-knob rise-p "rise ms" "modulator-rise-knob")
                (label "missing: rise" :font-size 10 :color :red :bg :transparent))
              (if fall-p
                (modulator-knob fall-p "fall ms" "modulator-fall-knob")
                (label "missing: fall" :font-size 10 :color :red :bg :transparent))
              (box :width 0.45 :height 1)
              (box :width 12.8 :height 5.9 :padding 0.22
                :background-color :black
                :corner-radius 8
                :debug-name "modulator-curve-wrapper"
                (modulator-curve
                  :width 12.25 :height 5.45
                  :rise (if rise-p (fx-param-value rise-p) 0)
                  :fall (if fall-p (fx-param-value fall-p) 0)
                  :phase (bind-seq (get inst :phase-field))
                  :level (bind-seq (get inst :level-field))
                  :max-ms (if rise-p (get rise-p :max) 5000)
                  :background-color :instrument-control-bg
                  :grid-color :dim
                  :curve-color (ui-accent-orange)
                  :fill-color (rgba 1.0 0.48 0.18 0.16))))))))))
