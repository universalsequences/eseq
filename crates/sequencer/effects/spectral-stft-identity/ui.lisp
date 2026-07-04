(def stft-id-block ()
  (ui-control-block-medium-wide-s "STFT ID" (ui-accent-cyan) 0
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob-s 0 "mix" "mix" 4.8 (ui-accent-orange) 2)
      (ui-lego-knob-s 0 "output" "out" 4.8 (ui-accent-green) 2))))

(defeffect-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (ui-lego-column-wide-full
      (stft-id-block))))
