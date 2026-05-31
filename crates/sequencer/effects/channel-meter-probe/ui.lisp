(def channel-probe-block ()
  (ui-control-block-medium-s "CHANNEL PROBE" (ui-accent-orange) 0
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob-s 0 "swap" "swap" 4.8 (ui-accent-orange) 2))))

(defeffect-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (ui-lego-column-full
      (channel-probe-block))))
