(def notch-main-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-medium-wide-s "SPECTRAL NOTCH" (eseq.effects.custom-ui-lego/ui-accent-cyan) 0
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "width" "wid" 4.8 (eseq.effects.custom-ui-lego/ui-accent-cyan) 1)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "offset" "ofs" 4.8 (eseq.effects.custom-ui-lego/ui-accent-blue) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "depth" "dep" 4.8 (eseq.effects.custom-ui-lego/ui-accent-orange) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "speed" "spd" 4.8 (eseq.effects.custom-ui-lego/ui-accent-green) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "blur" "blur" 4.8 (eseq.effects.custom-ui-lego/ui-accent-violet) 2))))

(def notch-motion-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-small-wide-s "TONE/OUT" (eseq.effects.custom-ui-lego/ui-accent-blue) 1
    (h-stack :gap 0.30 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-num-s 1 "mix" "mix" 5.2 2 false (eseq.effects.custom-ui-lego/ui-accent-orange))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 1 "curve" "hz/oct" 5.2 2 false (eseq.effects.custom-ui-lego/ui-accent-cyan))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 1 "fast" "x5" 5.2 2 false (eseq.effects.custom-ui-lego/ui-accent-green))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 1 "lowkeep" "low" 5.2 2 false (eseq.effects.custom-ui-lego/ui-accent-blue))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 1 "output" "out" 5.2 2 false (eseq.effects.custom-ui-lego/ui-accent-green)))))

(defeffect-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (eseq.effects.custom-ui-lego/ui-lego-column-wide-2
      (notch-main-block)
      (notch-motion-block))))
