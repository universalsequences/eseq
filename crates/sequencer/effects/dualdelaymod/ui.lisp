(def dualdelay-time-block ()
  (ui-control-block-medium-s "TIME" (ui-accent-cyan) 0
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob-s 0 "m1" "time 1" 5.2 (ui-accent-cyan) 0)
      (ui-lego-knob-s 0 "m2" "time 2" 5.2 (ui-accent-violet) 0)
      (ui-lego-knob-s 0 "fbk" "fbk" 5.2 (ui-accent-orange) 2))))

(def dualdelay-filter-block ()
  (ui-readout-block-small-s "MOD FILTER" (ui-accent-green) 1
    (h-stack :gap 0.30 :align :start
      (ui-lego-num-s 1 "cutoff" "cut" 5.2 0 "Hz" (ui-accent-green))
      (ui-lego-num-s 1 "res" "res" 5.2 2 false (ui-accent-green))
      (ui-lego-num-s 1 "lforate" "rate" 5.2 2 "Hz" (ui-accent-blue)))))

(defeffect-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (ui-lego-column-2
      (dualdelay-time-block)
      (dualdelay-filter-block))))
