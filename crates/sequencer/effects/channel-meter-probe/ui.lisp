(def channel-probe-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-medium-s "CHANNEL PROBE" (eseq.effects.custom-ui-lego/ui-accent-orange) 0
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "swap" "swap" 4.8 (eseq.effects.custom-ui-lego/ui-accent-orange) 2))))

(defeffect-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (eseq.effects.custom-ui-lego/ui-lego-column-full
      (channel-probe-block))))
