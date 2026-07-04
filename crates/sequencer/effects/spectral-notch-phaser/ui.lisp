(def notch-main-block ()
  (ui-control-block-medium-wide-s "SPECTRAL NOTCH" (ui-accent-cyan) 0
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob-s 0 "width" "wid" 4.8 (ui-accent-cyan) 1)
      (ui-lego-knob-s 0 "offset" "ofs" 4.8 (ui-accent-blue) 2)
      (ui-lego-knob-s 0 "depth" "dep" 4.8 (ui-accent-orange) 2)
      (ui-lego-knob-s 0 "speed" "spd" 4.8 (ui-accent-green) 2)
      (ui-lego-knob-s 0 "blur" "blur" 4.8 (ui-accent-violet) 2))))

(def notch-motion-block ()
  (ui-control-block-small-wide-s "TONE/OUT" (ui-accent-blue) 1
    (h-stack :gap 0.30 :align :start
      (ui-lego-num-s 1 "mix" "mix" 5.2 2 false (ui-accent-orange))
      (ui-lego-num-s 1 "curve" "hz/oct" 5.2 2 false (ui-accent-cyan))
      (ui-lego-num-s 1 "fast" "x5" 5.2 2 false (ui-accent-green))
      (ui-lego-num-s 1 "lowkeep" "low" 5.2 2 false (ui-accent-blue))
      (ui-lego-num-s 1 "output" "out" 5.2 2 false (ui-accent-green)))))

(defeffect-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (ui-lego-column-wide-2
      (notch-main-block)
      (notch-motion-block))))
