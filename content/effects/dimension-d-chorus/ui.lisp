(def dimension-motion-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-medium-s "MOTION" (eseq.effects.custom-ui-lego/ui-accent-blue) 0
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "rate" "rate" 4.7 (eseq.effects.custom-ui-lego/ui-accent-blue) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "depth" "depth" 4.7 (eseq.effects.custom-ui-lego/ui-accent-blue) 1)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "base" "base" 4.7 (eseq.effects.custom-ui-lego/ui-accent-cyan) 1)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "spread" "spread" 4.7 (eseq.effects.custom-ui-lego/ui-accent-violet) 1))))

(def dimension-output-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-medium-s "OUTPUT" (eseq.effects.custom-ui-lego/ui-accent-orange) 1
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "mix" "mix" 4.8 (eseq.effects.custom-ui-lego/ui-accent-orange) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "width" "width" 4.8 (eseq.effects.custom-ui-lego/ui-accent-violet) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "tone" "tone" 4.8 (eseq.effects.custom-ui-lego/ui-accent-green) 0)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "shimmer" "shim" 4.8 (eseq.effects.custom-ui-lego/ui-accent-cyan) 2))))

(defeffect-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (eseq.effects.custom-ui-lego/ui-lego-column-2
      (dimension-motion-block)
      (dimension-output-block))))
