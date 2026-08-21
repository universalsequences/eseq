(def lushlex-space-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-medium-s "SPACE" (eseq.effects.custom-ui-lego/ui-accent-cyan) 0
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "pre" "pre" 4.8 (eseq.effects.custom-ui-lego/ui-accent-cyan) 0)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "size" "size" 4.8 (eseq.effects.custom-ui-lego/ui-accent-violet) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "decay" "decay" 4.8 (eseq.effects.custom-ui-lego/ui-accent-orange) 2))))

(def lushlex-tone-block ()
  (eseq.effects.custom-ui-lego/ui-readout-block-small-s "TONE" (eseq.effects.custom-ui-lego/ui-accent-green) 1
    (h-stack :gap 0.30 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-num-s 1 "damping" "damp" 5.2 0 "Hz" (eseq.effects.custom-ui-lego/ui-accent-green))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 1 "mix" "mix" 5.2 2 false (eseq.effects.custom-ui-lego/ui-accent-orange)))))

(defeffect-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (eseq.effects.custom-ui-lego/ui-lego-column-2
      (lushlex-space-block)
      (lushlex-tone-block))))
