(def pv-freeze-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-medium-wide-s "PV FREEZE" (eseq.effects.custom-ui-lego/ui-accent-cyan) 0
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "freeze" "freeze" 4.8 (eseq.effects.custom-ui-lego/ui-accent-cyan) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "pitch" "pitch" 4.8 (eseq.effects.custom-ui-lego/ui-accent-violet) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "feed" "feed" 4.8 (eseq.effects.custom-ui-lego/ui-accent-green) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "fade" "fade" 4.8 (eseq.effects.custom-ui-lego/ui-accent-blue) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "haze" "haze" 4.8 (eseq.effects.custom-ui-lego/ui-accent-orange) 2))))

(def pv-freeze-output-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-small-wide-s "TONE/OUT" (eseq.effects.custom-ui-lego/ui-accent-orange) 1
    (h-stack :gap 0.24 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-num-s 1 "tilt" "tilt" 4.2 2 false (eseq.effects.custom-ui-lego/ui-accent-violet))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 1 "mix" "mix" 4.2 2 false (eseq.effects.custom-ui-lego/ui-accent-orange))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 1 "output" "out" 4.2 2 false (eseq.effects.custom-ui-lego/ui-accent-green)))))

(defeffect-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (eseq.effects.custom-ui-lego/ui-lego-column-wide-2
      (pv-freeze-block)
      (pv-freeze-output-block))))
