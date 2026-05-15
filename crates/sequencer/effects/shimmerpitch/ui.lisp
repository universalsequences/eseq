(defeffect-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (ui-lego-column-2
      (ui-control-block-medium-s "PITCH" (ui-accent-cyan) 0
        (h-stack :gap 0.32 :align :start
          (ui-lego-knob-s 0 "shift" "semi" 4.8 (ui-accent-cyan) 1)
          (ui-lego-knob-s 0 "fine" "fine" 4.8 (ui-accent-violet) 0)
          (ui-lego-knob-s 0 "window_ms" "win" 4.8 (ui-accent-blue) 0)))
      (ui-control-block-medium-s "FEEDBACK" (ui-accent-orange) 1
        (h-stack :gap 0.32 :align :start
          (ui-lego-knob-s 1 "delay_ms" "time" 4.8 (ui-accent-orange) 0)
          (ui-lego-knob-s 1 "feedback" "fbk" 4.8 (ui-accent-orange) 2)
          (ui-lego-knob-s 1 "shimmer" "shmr" 4.8 (ui-accent-cyan) 2))))
    (ui-lego-column-full
      (ui-control-block-medium-s "OUTPUT" (ui-accent-green) 2
        (h-stack :gap 0.32 :align :start
          (ui-lego-knob-s 2 "damping" "damp" 4.8 (ui-accent-green) 0)
          (ui-lego-knob-s 2 "width" "width" 4.8 (ui-accent-violet) 2)
          (ui-lego-knob-s 2 "mix" "mix" 4.8 (ui-accent-blue) 2)
          (ui-lego-knob-s 2 "output" "out" 4.8 (ui-accent-blue) 2))))))