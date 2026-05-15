(defeffect-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (ui-lego-column-2
      (ui-control-block-medium-s "CHORUS" (ui-accent-cyan) 0
        (h-stack :gap 0.32 :align :start
          (ui-lego-knob-s 0 "rate" "rate" 4.8 (ui-accent-blue) 2)
          (ui-lego-knob-s 0 "depth" "depth" 4.8 (ui-accent-cyan) 0)
          (ui-lego-knob-s 0 "base" "base" 4.8 (ui-accent-violet) 0)
          (ui-lego-knob-s 0 "spread" "sprd" 4.8 (ui-accent-green) 0)))
      (ui-control-block-medium-s "BLEND" (ui-accent-orange) 0
        (h-stack :gap 0.32 :align :start
          (ui-lego-knob-s 0 "feedback" "fbk" 4.8 (ui-accent-orange) 2)
          (ui-lego-knob-s 0 "mix" "mix" 4.8 (ui-accent-blue) 2)
          (ui-lego-knob-s 0 "width" "width" 4.8 (ui-accent-cyan) 2)
          (ui-lego-knob-s 0 "tone" "tone" 4.8 (ui-accent-green) 0))))))