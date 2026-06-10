(def bloom-main-block ()
  (ui-control-block-medium-wide-s "SPECTRAL BLOOM" (ui-accent-violet) 0
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob-s 0 "decay" "dcy" 4.8 (ui-accent-violet) 2)
      (ui-lego-knob-s 0 "drift" "drft" 4.8 (ui-accent-cyan) 2)
      (ui-lego-knob-s 0 "bloom" "blm" 4.8 (ui-accent-blue) 2)
      (ui-lego-knob-s 0 "haze" "haze" 4.8 (ui-accent-green) 2)
      (ui-lego-knob-s 0 "damp" "dmp" 4.8 (ui-accent-orange) 2))))

(def bloom-out-block ()
  (ui-control-block-small-wide-s "CLOUD/OUT" (ui-accent-blue) 1
    (h-stack :gap 0.30 :align :start
      (ui-lego-num-s 1 "freeze" "frz" 5.2 2 false (ui-accent-cyan))
      (ui-lego-num-s 1 "width" "wid" 5.2 2 false (ui-accent-violet))
      (ui-lego-num-s 1 "mix" "mix" 5.2 2 false (ui-accent-orange))
      (ui-lego-num-s 1 "output" "out" 5.2 2 false (ui-accent-green)))))

(defeffect-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (ui-lego-column-wide-2
      (bloom-main-block)
      (bloom-out-block))))
