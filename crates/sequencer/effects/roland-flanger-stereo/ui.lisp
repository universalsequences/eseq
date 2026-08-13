(def roland-flanger-sweep-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-medium-s "SWEEP" (eseq.effects.custom-ui-lego/ui-accent-blue) 0
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "rate" "rate" 4.8 (eseq.effects.custom-ui-lego/ui-accent-blue) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "depth" "depth" 4.8 (eseq.effects.custom-ui-lego/ui-accent-cyan) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "manual" "manual" 4.8 (eseq.effects.custom-ui-lego/ui-accent-orange) 0))))

(def roland-flanger-tone-block ()
  (eseq.effects.custom-ui-lego/ui-readout-block-small-s "TONE" (eseq.effects.custom-ui-lego/ui-accent-green) 1
    (h-stack :gap 0.30 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-num-s 1 "feedback" "fbk" 5.2 2 false (eseq.effects.custom-ui-lego/ui-accent-orange))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 1 "color" "color" 5.2 0 "Hz" (eseq.effects.custom-ui-lego/ui-accent-green))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 1 "mix" "mix" 5.2 2 false (eseq.effects.custom-ui-lego/ui-accent-cyan)))))

(defeffect-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (eseq.effects.custom-ui-lego/ui-lego-column-2
      (roland-flanger-sweep-block)
      (roland-flanger-tone-block))))
