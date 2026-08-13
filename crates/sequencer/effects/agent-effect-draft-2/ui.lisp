(def tape-delay-time ()
  (eseq.effects.custom-ui-lego/ui-control-block-medium-s "DELAY" (eseq.effects.custom-ui-lego/ui-accent-cyan) 0
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "time" "time" 4.8 (eseq.effects.custom-ui-lego/ui-accent-cyan) 0)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "spread" "spread" 4.8 (eseq.effects.custom-ui-lego/ui-accent-blue) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "feedback" "fbk" 4.8 (eseq.effects.custom-ui-lego/ui-accent-orange) 2))))

(def tape-delay-character ()
  (eseq.effects.custom-ui-lego/ui-control-block-medium-s "TAPE" (eseq.effects.custom-ui-lego/ui-accent-violet) 0
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "drive" "drive" 4.8 (eseq.effects.custom-ui-lego/ui-accent-orange) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "tone" "tone" 4.8 (eseq.effects.custom-ui-lego/ui-accent-green) 0)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "wow_rate" "wow" 4.8 (eseq.effects.custom-ui-lego/ui-accent-violet) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "wow_depth" "depth" 4.8 (eseq.effects.custom-ui-lego/ui-accent-violet) 3))))

(def tape-delay-mix ()
  (eseq.effects.custom-ui-lego/ui-control-block-medium-s "MIX" (eseq.effects.custom-ui-lego/ui-accent-blue) 0
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "mix" "mix" 4.8 (eseq.effects.custom-ui-lego/ui-accent-blue) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "output" "out" 4.8 (eseq.effects.custom-ui-lego/ui-accent-cyan) 2))))

(defeffect-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (eseq.effects.custom-ui-lego/ui-lego-column-2
      (tape-delay-time)
      (tape-delay-character))
    (eseq.effects.custom-ui-lego/ui-lego-column-full
      (tape-delay-mix))))
