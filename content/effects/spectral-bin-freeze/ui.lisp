(def spectral-freeze-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-medium-wide-s "FREEZE" (eseq.effects.custom-ui-lego/ui-accent-cyan) 0
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "freeze" "freeze" 4.8 (eseq.effects.custom-ui-lego/ui-accent-cyan) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "smear" "smear" 4.8 (eseq.effects.custom-ui-lego/ui-accent-blue) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "tone" "tone" 4.8 (eseq.effects.custom-ui-lego/ui-accent-green) 0))))

(def spectral-freeze-output-block ()
  (eseq.effects.custom-ui-lego/ui-readout-block-small-wide-s "OUTPUT" (eseq.effects.custom-ui-lego/ui-accent-orange) 1
    (h-stack :gap 0.30 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-num-s 1 "mix" "mix" 5.2 2 false (eseq.effects.custom-ui-lego/ui-accent-orange))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 1 "width" "width" 5.2 2 false (eseq.effects.custom-ui-lego/ui-accent-violet)))))

(defeffect-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (eseq.effects.custom-ui-lego/ui-lego-column-wide-2
      (spectral-freeze-block)
      (spectral-freeze-output-block))))
