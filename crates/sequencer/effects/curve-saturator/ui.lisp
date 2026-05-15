(defeffect-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (ui-lego-column-full
      (ui-control-block-medium-s "SATURATOR" (ui-accent-orange) 0
        (h-stack :gap 0.32 :align :start
          (ui-lego-num-s 0 "curve_type" "curve" 5.2 0 false (ui-accent-violet))
          (ui-lego-knob-s 0 "shape" "shape" 4.8 (ui-accent-orange) 2)
          (ui-lego-knob-s 0 "mix" "mix" 4.8 (ui-accent-cyan) 2))))))
