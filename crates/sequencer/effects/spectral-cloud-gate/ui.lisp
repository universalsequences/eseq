(def cloud-gate-shape-block ()
  (ui-control-block-medium-s "CLOUD GATE" (ui-accent-cyan) 0
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob-s 0 "density" "dens" 4.8 (ui-accent-cyan) 2)
      (ui-lego-knob-s 0 "smear" "smear" 4.8 (ui-accent-blue) 2)
      (ui-lego-knob-s 0 "motion" "motion" 4.8 (ui-accent-violet) 2))))

(def cloud-gate-output-block ()
  (ui-readout-block-small-s "OUTPUT" (ui-accent-orange) 1
    (h-stack :gap 0.30 :align :start
      (ui-lego-num-s 1 "phase_amt" "phase" 5.2 2 false (ui-accent-blue))
      (ui-lego-num-s 1 "width" "width" 5.2 2 false (ui-accent-violet))
      (ui-lego-num-s 1 "mix" "mix" 5.2 2 false (ui-accent-orange)))))

(defeffect-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (ui-lego-column-2
      (cloud-gate-shape-block)
      (cloud-gate-output-block))))
