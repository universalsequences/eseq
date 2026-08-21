(def shatter-main-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-medium-wide-s "SPECTRAL SHATTER" (eseq.effects.custom-ui-lego/ui-accent-cyan) 0
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "time" "time" 4.8 (eseq.effects.custom-ui-lego/ui-accent-cyan) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "tilt" "tilt" 4.8 (eseq.effects.custom-ui-lego/ui-accent-violet) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "scatter" "scat" 4.8 (eseq.effects.custom-ui-lego/ui-accent-blue) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "fb" "fb" 4.8 (eseq.effects.custom-ui-lego/ui-accent-orange) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "damp" "dmp" 4.8 (eseq.effects.custom-ui-lego/ui-accent-green) 2))))

(def shatter-out-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-small-wide-s "ICE/OUT" (eseq.effects.custom-ui-lego/ui-accent-violet) 1
    (h-stack :gap 0.24 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-num-s 1 "freeze" "frz" 4.2 2 false (eseq.effects.custom-ui-lego/ui-accent-cyan))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 1 "haze" "haze" 4.2 2 false (eseq.effects.custom-ui-lego/ui-accent-green))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 1 "width" "wid" 4.2 2 false (eseq.effects.custom-ui-lego/ui-accent-violet))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 1 "mix" "mix" 4.2 2 false (eseq.effects.custom-ui-lego/ui-accent-orange))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 1 "output" "out" 4.2 2 false (eseq.effects.custom-ui-lego/ui-accent-blue)))))

(defeffect-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (eseq.effects.custom-ui-lego/ui-lego-column-wide-2
      (shatter-main-block)
      (shatter-out-block))))
