(def vocoder-shift-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-medium-wide-s "VOCODER SHIFT" (eseq.effects.custom-ui-lego/ui-accent-violet) 0
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "ratio" "ratio" 4.8 (eseq.effects.custom-ui-lego/ui-accent-violet) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "color" "color" 4.8 (eseq.effects.custom-ui-lego/ui-accent-green) 0)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "drive" "drive" 4.8 (eseq.effects.custom-ui-lego/ui-accent-orange) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "mix" "mix" 4.8 (eseq.effects.custom-ui-lego/ui-accent-cyan) 2))))

(defeffect-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (eseq.effects.custom-ui-lego/ui-lego-column-wide-full
      (vocoder-shift-block))))
