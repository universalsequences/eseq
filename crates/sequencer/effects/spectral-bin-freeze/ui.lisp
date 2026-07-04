(def spectral-freeze-block ()
  (ui-control-block-medium-wide-s "FREEZE" (ui-accent-cyan) 0
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob-s 0 "freeze" "freeze" 4.8 (ui-accent-cyan) 2)
      (ui-lego-knob-s 0 "smear" "smear" 4.8 (ui-accent-blue) 2)
      (ui-lego-knob-s 0 "tone" "tone" 4.8 (ui-accent-green) 0))))

(def spectral-freeze-output-block ()
  (ui-readout-block-small-wide-s "OUTPUT" (ui-accent-orange) 1
    (h-stack :gap 0.30 :align :start
      (ui-lego-num-s 1 "mix" "mix" 5.2 2 false (ui-accent-orange))
      (ui-lego-num-s 1 "width" "width" 5.2 2 false (ui-accent-violet)))))

(defeffect-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (ui-lego-column-wide-2
      (spectral-freeze-block)
      (spectral-freeze-output-block))))
