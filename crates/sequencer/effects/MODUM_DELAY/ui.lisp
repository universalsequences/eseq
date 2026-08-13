(def modum-delay-lines ()
  (eseq.effects.custom-ui-lego/ui-control-block-medium-s "DELAYS" (eseq.effects.custom-ui-lego/ui-accent-cyan) 0
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "max1" "left" 4.8 (eseq.effects.custom-ui-lego/ui-accent-cyan) 0)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "max2" "right" 4.8 (eseq.effects.custom-ui-lego/ui-accent-cyan) 0)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "fbk" "fbk" 4.8 (eseq.effects.custom-ui-lego/ui-accent-orange) 2))))

(def modum-delay-filter ()
  (eseq.effects.custom-ui-lego/ui-readout-block-small-s "FILTER" (eseq.effects.custom-ui-lego/ui-accent-green) 1
    (h-stack :gap 0.30 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-num-s 1 "cutoff" "cut" 5.2 0 "Hz" (eseq.effects.custom-ui-lego/ui-accent-green))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 1 "res" "res" 5.2 2 false (eseq.effects.custom-ui-lego/ui-accent-green))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 1 "rate" "rate" 5.2 2 "Hz" (eseq.effects.custom-ui-lego/ui-accent-blue)))))

(defeffect-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (eseq.effects.custom-ui-lego/ui-lego-column-2
      (modum-delay-lines)
      (modum-delay-filter))))
