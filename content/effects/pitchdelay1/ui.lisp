(defeffect-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (eseq.effects.custom-ui-lego/ui-lego-column-2
      (eseq.effects.custom-ui-lego/ui-control-block-medium-s "PITCH" (eseq.effects.custom-ui-lego/ui-accent-cyan) 0
        (h-stack :gap 0.32 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "shift" "semi" 4.8 (eseq.effects.custom-ui-lego/ui-accent-cyan) 1)
          (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "fine" "fine" 4.8 (eseq.effects.custom-ui-lego/ui-accent-violet) 0)
          (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "window_ms" "win" 4.8 (eseq.effects.custom-ui-lego/ui-accent-blue) 0)))
      (eseq.effects.custom-ui-lego/ui-control-block-medium-s "DELAY" (eseq.effects.custom-ui-lego/ui-accent-orange) 1
        (h-stack :gap 0.32 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "delay_ms" "time" 4.8 (eseq.effects.custom-ui-lego/ui-accent-orange) 0)
          (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "feedback" "fbk" 4.8 (eseq.effects.custom-ui-lego/ui-accent-orange) 2)
          (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "mix" "mix" 4.8 (eseq.effects.custom-ui-lego/ui-accent-blue) 2))))
    (eseq.effects.custom-ui-lego/ui-lego-column-full
      (eseq.effects.custom-ui-lego/ui-control-block-medium-s "OUTPUT" (eseq.effects.custom-ui-lego/ui-accent-green) 2
        (h-stack :gap 0.32 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-knob-s 2 "tone" "tone" 4.8 (eseq.effects.custom-ui-lego/ui-accent-green) 0)
          (eseq.effects.custom-ui-lego/ui-lego-knob-s 2 "width" "width" 4.8 (eseq.effects.custom-ui-lego/ui-accent-violet) 2)
          (eseq.effects.custom-ui-lego/ui-lego-knob-s 2 "output" "out" 4.8 (eseq.effects.custom-ui-lego/ui-accent-blue) 2))))))
