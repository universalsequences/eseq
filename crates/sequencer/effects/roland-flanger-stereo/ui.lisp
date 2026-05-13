(def roland-flanger-sweep-block ()
  (ui-control-block-medium-s "SWEEP" (ui-accent-blue) 0
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob-s 0 "rate" "rate" 4.8 (ui-accent-blue) 2)
      (ui-lego-knob-s 0 "depth" "depth" 4.8 (ui-accent-cyan) 2)
      (ui-lego-knob-s 0 "manual" "manual" 4.8 (ui-accent-orange) 0))))

(def roland-flanger-tone-block ()
  (ui-readout-block-small-s "TONE" (ui-accent-green) 1
    (h-stack :gap 0.30 :align :start
      (ui-lego-num-s 1 "feedback" "fbk" 5.2 2 false (ui-accent-orange))
      (ui-lego-num-s 1 "color" "color" 5.2 0 "Hz" (ui-accent-green))
      (ui-lego-num-s 1 "mix" "mix" 5.2 2 false (ui-accent-cyan)))))

(defeffect-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (ui-lego-column-2
      (roland-flanger-sweep-block)
      (roland-flanger-tone-block))))
