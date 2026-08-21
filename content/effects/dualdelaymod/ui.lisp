(def dualdelay-time-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-medium-s "TIME" (eseq.effects.custom-ui-lego/ui-accent-cyan) 0
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "m1" "time 1" 5.2 (eseq.effects.custom-ui-lego/ui-accent-cyan) 0)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "m2" "time 2" 5.2 (eseq.effects.custom-ui-lego/ui-accent-violet) 0)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "fbk" "fbk" 5.2 (eseq.effects.custom-ui-lego/ui-accent-orange) 2))))

(def dualdelay-filter-block ()
  (eseq.effects.custom-ui-lego/ui-readout-block-small-s "MOD FILTER" (eseq.effects.custom-ui-lego/ui-accent-green) 1
    (h-stack :gap 0.30 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-num-s 1 "cutoff" "cut" 5.2 0 "Hz" (eseq.effects.custom-ui-lego/ui-accent-green))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 1 "res" "res" 5.2 2 false (eseq.effects.custom-ui-lego/ui-accent-green))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 1 "lforate" "rate" 5.2 2 "Hz" (eseq.effects.custom-ui-lego/ui-accent-blue)))))

(defeffect-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (eseq.effects.custom-ui-lego/ui-lego-column-2
      (dualdelay-time-block)
      (dualdelay-filter-block))))
