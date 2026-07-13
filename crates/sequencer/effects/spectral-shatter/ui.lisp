(def shatter-main-block ()
  (ui-control-block-medium-wide-s "SPECTRAL SHATTER" (ui-accent-cyan) 0
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob-s 0 "time" "time" 4.8 (ui-accent-cyan) 2)
      (ui-lego-knob-s 0 "tilt" "tilt" 4.8 (ui-accent-violet) 2)
      (ui-lego-knob-s 0 "scatter" "scat" 4.8 (ui-accent-blue) 2)
      (ui-lego-knob-s 0 "fb" "fb" 4.8 (ui-accent-orange) 2)
      (ui-lego-knob-s 0 "damp" "dmp" 4.8 (ui-accent-green) 2))))

(def shatter-out-block ()
  (ui-control-block-small-wide-s "ICE/OUT" (ui-accent-violet) 1
    (h-stack :gap 0.24 :align :start
      (ui-lego-num-s 1 "freeze" "frz" 4.2 2 false (ui-accent-cyan))
      (ui-lego-num-s 1 "haze" "haze" 4.2 2 false (ui-accent-green))
      (ui-lego-num-s 1 "width" "wid" 4.2 2 false (ui-accent-violet))
      (ui-lego-num-s 1 "mix" "mix" 4.2 2 false (ui-accent-orange))
      (ui-lego-num-s 1 "output" "out" 4.2 2 false (ui-accent-blue)))))

(defeffect-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (ui-lego-column-wide-2
      (shatter-main-block)
      (shatter-out-block))))
