(defeffect-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (eseq.effects.custom-ui-lego/ui-lego-column-full
      (eseq.effects.custom-ui-lego/ui-control-block-medium-s "SATURATOR" (eseq.effects.custom-ui-lego/ui-accent-orange) 0
        (h-stack :gap 0.32 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "curve_type" "curve" 5.2 0 false (eseq.effects.custom-ui-lego/ui-accent-violet))
          (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "shape" "shape" 4.8 (eseq.effects.custom-ui-lego/ui-accent-orange) 2)
          (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "mix" "mix" 4.8 (eseq.effects.custom-ui-lego/ui-accent-cyan) 2))))))
