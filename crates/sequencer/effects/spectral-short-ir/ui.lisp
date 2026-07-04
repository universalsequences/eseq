(def short-ir-block ()
  (ui-control-block-medium-wide-s "SHORT IR" (ui-accent-green) 0
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob-s 0 "wet" "wet" 5.2 (ui-accent-orange) 2)
      (ui-lego-knob-s 0 "gain" "gain" 5.2 (ui-accent-green) 2)
      (ui-lego-knob-s 0 "tone" "tone" 5.2 (ui-accent-cyan) 0))))

(defeffect-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (ui-lego-column-wide-full
      (short-ir-block))))
