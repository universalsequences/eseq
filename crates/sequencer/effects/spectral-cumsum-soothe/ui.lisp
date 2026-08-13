(def soothe-shape-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-medium-wide-s "CUMSUM SOOTHE" (eseq.effects.custom-ui-lego/ui-accent-cyan) 0
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "amount" "amt" 4.8 (eseq.effects.custom-ui-lego/ui-accent-cyan) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "threshold" "thr" 4.8 (eseq.effects.custom-ui-lego/ui-accent-blue) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "gate" "gat" 4.8 (eseq.effects.custom-ui-lego/ui-accent-orange) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "attack" "atk" 4.8 (eseq.effects.custom-ui-lego/ui-accent-green) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "release" "rel" 4.8 (eseq.effects.custom-ui-lego/ui-accent-violet) 2))))

(def soothe-band-output-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-small-wide-s "FILTER/OUT" (eseq.effects.custom-ui-lego/ui-accent-blue) 1
    (h-stack :gap 0.30 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-num-s 1 "low" "low" 5.2 2 false (eseq.effects.custom-ui-lego/ui-accent-blue))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 1 "high" "hi" 5.2 2 false (eseq.effects.custom-ui-lego/ui-accent-cyan))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 1 "mix" "mix" 5.2 2 false (eseq.effects.custom-ui-lego/ui-accent-orange))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 1 "delta" "del" 5.2 2 false (eseq.effects.custom-ui-lego/ui-accent-violet))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 1 "output" "out" 5.2 2 false (eseq.effects.custom-ui-lego/ui-accent-green)))))

(def soothe-style-block ()
  (eseq.effects.custom-ui-lego/ui-readout-block-small-wide-s "STYLE" (eseq.effects.custom-ui-lego/ui-accent-violet) 1
    (h-stack :gap 0.30 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-num-s 1 "freeze" "frz" 5.2 2 false (eseq.effects.custom-ui-lego/ui-accent-violet))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 1 "hold" "hold" 5.2 2 false (eseq.effects.custom-ui-lego/ui-accent-cyan))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 1 "alien" "aln" 5.2 2 false (eseq.effects.custom-ui-lego/ui-accent-orange)))))

(defeffect-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (eseq.effects.custom-ui-lego/ui-lego-column-wide
      (soothe-shape-block)
      (soothe-band-output-block)
      (soothe-style-block))))
