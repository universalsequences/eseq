(def mod-delay-core-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-medium-s "MOD DELAY" (eseq.effects.custom-ui-lego/ui-accent-blue) 0
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "freq" "rate" 5.2 (eseq.effects.custom-ui-lego/ui-accent-blue) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "delay_max" "range" 5.2 (eseq.effects.custom-ui-lego/ui-accent-cyan) 0)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "fbk" "fbk" 5.2 (eseq.effects.custom-ui-lego/ui-accent-orange) 2))))

(defeffect-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (eseq.effects.custom-ui-lego/ui-lego-column-full
      (mod-delay-core-block))))
