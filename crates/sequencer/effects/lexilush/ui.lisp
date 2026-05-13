(def lexilush-space-block ()
  (ui-control-block-medium-s "SPACE" (ui-accent-cyan) 0
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob-s 0 "pre_dly" "pre" 4.8 (ui-accent-cyan) 0)
      (ui-lego-knob-s 0 "size" "size" 4.8 (ui-accent-violet) 2)
      (ui-lego-knob-s 0 "decay" "decay" 4.8 (ui-accent-orange) 2)
      (ui-lego-knob-s 0 "diffusion" "diff" 4.8 (ui-accent-blue) 2))))

(def lexilush-tone-block ()
  (ui-readout-block-small-s "TONE" (ui-accent-green) 1
    (h-stack :gap 0.30 :align :start
      (ui-lego-num-s 1 "damping" "damp" 5.2 0 "Hz" (ui-accent-green))
      (ui-lego-num-s 1 "mod_freq" "rate" 5.2 2 "Hz" (ui-accent-blue))
      (ui-lego-num-s 1 "mod_amt" "mod" 5.2 0 false (ui-accent-blue)))))

(def lexilush-output-block ()
  (ui-readout-block-small-s "OUTPUT" (ui-accent-orange) 1
    (h-stack :gap 0.30 :align :start
      (ui-lego-num-s 1 "mix" "mix" 5.2 2 false (ui-accent-orange)))))

(defeffect-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (ui-lego-column
      (lexilush-space-block)
      (lexilush-tone-block)
      (lexilush-output-block))))
