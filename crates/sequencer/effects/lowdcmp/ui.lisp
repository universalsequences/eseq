(def lowdcmp-dynamics-block ()
  (ui-control-block-medium-s "LOW DCOMP" (ui-accent-orange) 0
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob-s 0 "threshold" "thr" 4.8 (ui-accent-orange) 1)
      (ui-lego-knob-s 0 "ratio" "ratio" 4.8 (ui-accent-violet) 1)
      (ui-lego-knob-s 0 "knee" "knee" 4.8 (ui-accent-cyan) 1)
      (ui-lego-knob-s 0 "gain" "gain" 4.8 (ui-accent-green) 2))))

(defeffect-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (ui-lego-column-full
      (lowdcmp-dynamics-block))))
