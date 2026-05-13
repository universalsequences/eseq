(def sidechain-dynamics-block ()
  (ui-control-block-medium-s "SIDECHAIN" (ui-accent-orange) 0
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob-s 0 "threshold" "thr" 5.2 (ui-accent-orange) 1)
      (ui-lego-knob-s 0 "ratio" "ratio" 5.2 (ui-accent-violet) 1))))

(def sidechain-route-block ()
  (ui-readout-block-small-s "DETECTOR" (ui-accent-cyan) 0
    (ui-lego-text-row-3
      (label "input 3" :font-size 9.0 :color (ui-accent-cyan) :bg :transparent)
      (label "drives" :font-size 9.0 :color :dim :bg :transparent)
      (label "ducking" :font-size 9.0 :color (ui-accent-orange) :bg :transparent))))

(defeffect-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (ui-lego-column-2
      (sidechain-dynamics-block)
      (sidechain-route-block))))
