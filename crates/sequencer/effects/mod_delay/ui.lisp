(def mod-delay-core-block ()
  (ui-control-block-medium-s "MOD DELAY" (ui-accent-blue) 0
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob-s 0 "freq" "rate" 5.2 (ui-accent-blue) 2)
      (ui-lego-knob-s 0 "delay_max" "range" 5.2 (ui-accent-cyan) 0)
      (ui-lego-knob-s 0 "fbk" "fbk" 5.2 (ui-accent-orange) 2))))

(defeffect-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (ui-lego-column-full
      (mod-delay-core-block))))
