(def lowdcmp-dynamics-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-medium-s "LOW DCOMP" (eseq.effects.custom-ui-lego/ui-accent-orange) 0
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "threshold" "thr" 4.8 (eseq.effects.custom-ui-lego/ui-accent-orange) 1)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "ratio" "ratio" 4.8 (eseq.effects.custom-ui-lego/ui-accent-violet) 1)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "knee" "knee" 4.8 (eseq.effects.custom-ui-lego/ui-accent-cyan) 1)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "gain" "gain" 4.8 (eseq.effects.custom-ui-lego/ui-accent-green) 2))))

(defeffect-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (eseq.effects.custom-ui-lego/ui-lego-column-full
      (lowdcmp-dynamics-block))))
