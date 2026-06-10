(def soothe-shape-block ()
  (ui-control-block-medium-wide-s "CUMSUM SOOTHE" (ui-accent-cyan) 0
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob-s 0 "amount" "amt" 4.8 (ui-accent-cyan) 2)
      (ui-lego-knob-s 0 "threshold" "thr" 4.8 (ui-accent-blue) 2)
      (ui-lego-knob-s 0 "gate" "gat" 4.8 (ui-accent-orange) 2)
      (ui-lego-knob-s 0 "attack" "atk" 4.8 (ui-accent-green) 2)
      (ui-lego-knob-s 0 "release" "rel" 4.8 (ui-accent-violet) 2))))

(def soothe-band-output-block ()
  (ui-control-block-small-wide-s "FILTER/OUT" (ui-accent-blue) 1
    (h-stack :gap 0.30 :align :start
      (ui-lego-num-s 1 "low" "low" 5.2 2 false (ui-accent-blue))
      (ui-lego-num-s 1 "high" "hi" 5.2 2 false (ui-accent-cyan))
      (ui-lego-num-s 1 "mix" "mix" 5.2 2 false (ui-accent-orange))
      (ui-lego-num-s 1 "delta" "del" 5.2 2 false (ui-accent-violet))
      (ui-lego-num-s 1 "output" "out" 5.2 2 false (ui-accent-green)))))

(def soothe-style-block ()
  (ui-readout-block-small-wide-s "STYLE" (ui-accent-violet) 1
    (h-stack :gap 0.30 :align :start
      (ui-lego-num-s 1 "freeze" "frz" 5.2 2 false (ui-accent-violet))
      (ui-lego-num-s 1 "hold" "hold" 5.2 2 false (ui-accent-cyan))
      (ui-lego-num-s 1 "alien" "aln" 5.2 2 false (ui-accent-orange)))))

(defeffect-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (ui-lego-column-wide
      (soothe-shape-block)
      (soothe-band-output-block)
      (soothe-style-block))))
