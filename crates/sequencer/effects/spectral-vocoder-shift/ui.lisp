(def vocoder-shift-block ()
  (ui-control-block-medium-wide-s "VOCODER SHIFT" (ui-accent-violet) 0
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob-s 0 "ratio" "ratio" 4.8 (ui-accent-violet) 2)
      (ui-lego-knob-s 0 "color" "color" 4.8 (ui-accent-green) 0)
      (ui-lego-knob-s 0 "drive" "drive" 4.8 (ui-accent-orange) 2)
      (ui-lego-knob-s 0 "mix" "mix" 4.8 (ui-accent-cyan) 2))))

(defeffect-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (ui-lego-column-wide-full
      (vocoder-shift-block))))
