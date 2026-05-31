(def sidechain-dynamics-block ()
  (ui-control-block-medium-s "SIDECHAIN" (ui-accent-orange) 0
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob-s 0 "threshold" "thr" 5.2 (ui-accent-orange) 1)
      (ui-lego-knob-s 0 "ratio" "ratio" 5.2 (ui-accent-violet) 1))))

(def sidechain-route-param ()
  (or
    (audio-fx-ui-param audio-fx-ui-current-fx "sidechain")
    (nth
      (filter |p| (string-starts-with? (get p :name) "sidechain")
        (get audio-fx-ui-current-fx :params))
      0)))

(def sidechain-route-selector ()
  (let ((fx audio-fx-ui-current-fx)
        (p (sidechain-route-param)))
    (box :debug-name "sidechain-route-selector" :width 10.4 :height 1.65 :padding 0
      (v-stack :width 10.4 :height 1.65 :gap 0.10 :align :start
        (label "source" :font-size 8.2 :width 10.4 :height 0.52 :color :dim :bg :transparent)
        (if p
          (dropdown :value (get p :text-value)
            :options (get p :options)
            :on-change (lambda (v) (param-set-option fx p v))
            :width 10.4 :height 0.92 :font-size 8.8)
          (label "missing route" :font-size 8.8 :width 10.4 :height 0.92
            :color :dim :bg :transparent))))))

(def sidechain-route-block ()
  (ui-readout-block-small-s "DETECTOR" (ui-accent-cyan) 0
    (h-stack :gap 0.32 :align :center
      (sidechain-route-selector)
      (label "drives ducking" :font-size 9.0 :color (ui-accent-orange) :bg :transparent))))

(defeffect-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (ui-lego-column-2
      (sidechain-dynamics-block)
      (sidechain-route-block))))
