(def stft-id-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-medium-wide-s "STFT ID" (eseq.effects.custom-ui-lego/ui-accent-cyan) 0
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "mix" "mix" 4.8 (eseq.effects.custom-ui-lego/ui-accent-orange) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "output" "out" 4.8 (eseq.effects.custom-ui-lego/ui-accent-green) 2))))

(defeffect-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (eseq.effects.custom-ui-lego/ui-lego-column-wide-full
      (stft-id-block))))
