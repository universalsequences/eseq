(def tremolo-motion-block ()
  (ui-control-block-medium-s "TREMOLO" (ui-accent-blue) 0
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob-s 0 "rate" "rate" 5.2 (ui-accent-blue) 2)
      (ui-lego-knob-s 0 "depth" "depth" 5.2 (ui-accent-cyan) 2)
      (ui-lego-knob-s 0 "spread" "spread" 5.2 (ui-accent-violet) 2))))

(defeffect-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (ui-lego-column-full
      (tremolo-motion-block))))
