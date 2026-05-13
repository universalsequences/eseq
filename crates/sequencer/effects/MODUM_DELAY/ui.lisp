(def modum-delay-lines ()
  (ui-control-block-medium-s "DELAYS" (ui-accent-cyan) 0
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob-s 0 "max1" "left" 4.8 (ui-accent-cyan) 0)
      (ui-lego-knob-s 0 "max2" "right" 4.8 (ui-accent-cyan) 0)
      (ui-lego-knob-s 0 "fbk" "fbk" 4.8 (ui-accent-orange) 2))))

(def modum-delay-filter ()
  (ui-readout-block-small-s "FILTER" (ui-accent-green) 1
    (h-stack :gap 0.30 :align :start
      (ui-lego-num-s 1 "cutoff" "cut" 5.2 0 "Hz" (ui-accent-green))
      (ui-lego-num-s 1 "res" "res" 5.2 2 false (ui-accent-green))
      (ui-lego-num-s 1 "rate" "rate" 5.2 2 "Hz" (ui-accent-blue)))))

(defeffect-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (ui-lego-column-2
      (modum-delay-lines)
      (modum-delay-filter))))
