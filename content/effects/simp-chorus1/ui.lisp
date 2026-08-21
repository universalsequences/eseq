(defeffect-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (eseq.effects.custom-ui-lego/ui-lego-column-2
      (eseq.effects.custom-ui-lego/ui-control-block-medium-s "CHORUS" (eseq.effects.custom-ui-lego/ui-accent-cyan) 0
        (h-stack :gap 0.32 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "rate" "rate" 4.8 (eseq.effects.custom-ui-lego/ui-accent-blue) 2)
          (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "depth" "depth" 4.8 (eseq.effects.custom-ui-lego/ui-accent-cyan) 0)
          (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "base" "base" 4.8 (eseq.effects.custom-ui-lego/ui-accent-violet) 0)
          (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "spread" "sprd" 4.8 (eseq.effects.custom-ui-lego/ui-accent-green) 0)))
      (eseq.effects.custom-ui-lego/ui-control-block-medium-s "BLEND" (eseq.effects.custom-ui-lego/ui-accent-orange) 0
        (h-stack :gap 0.32 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "feedback" "fbk" 4.8 (eseq.effects.custom-ui-lego/ui-accent-orange) 2)
          (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "mix" "mix" 4.8 (eseq.effects.custom-ui-lego/ui-accent-blue) 2)
          (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "width" "width" 4.8 (eseq.effects.custom-ui-lego/ui-accent-cyan) 2)
          (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "tone" "tone" 4.8 (eseq.effects.custom-ui-lego/ui-accent-green) 0))))))