(def short-ir-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-medium-wide-s "SHORT IR" (eseq.effects.custom-ui-lego/ui-accent-green) 0
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "wet" "wet" 5.2 (eseq.effects.custom-ui-lego/ui-accent-orange) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "gain" "gain" 5.2 (eseq.effects.custom-ui-lego/ui-accent-green) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "tone" "tone" 5.2 (eseq.effects.custom-ui-lego/ui-accent-cyan) 0))))

(defeffect-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (eseq.effects.custom-ui-lego/ui-lego-column-wide-full
      (short-ir-block))))
