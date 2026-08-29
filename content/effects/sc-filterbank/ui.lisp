; Sherman-yellow accent — the hardware panel is famously school-bus yellow
(def scfb-accent-yellow () (rgba 0.98 0.80 0.10 1.0))

(def scfb-filter-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-medium-s "CLOCKED FILTERS" (scfb-accent-yellow) 0
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "freq" "freq" 4.7 (scfb-accent-yellow) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "res" "res" 4.7 (scfb-accent-yellow) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "mode" "mode 1" 4.7 (eseq.effects.custom-ui-lego/ui-accent-cyan) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "mode2" "mode 2" 4.7 (eseq.effects.custom-ui-lego/ui-accent-cyan) 2))))

(def scfb-clock-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-medium-s "CLOCK DIVIDER" (eseq.effects.custom-ui-lego/ui-accent-orange) 1
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "harmonics" "harm" 4.7 (eseq.effects.custom-ui-lego/ui-accent-orange) 0)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "crunch" "crunch" 4.7 (eseq.effects.custom-ui-lego/ui-accent-orange) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "serial" "ser" 4.7 (eseq.effects.custom-ui-lego/ui-accent-violet) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "blend" "blend" 4.7 (eseq.effects.custom-ui-lego/ui-accent-violet) 2))))

(def scfb-motion-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-medium-s "DRIVE / MOTION" (eseq.effects.custom-ui-lego/ui-accent-blue) 2
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 2 "drive" "drive" 4.7 (eseq.effects.custom-ui-lego/ui-accent-blue) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 2 "lfo-rate" "rate" 4.7 (eseq.effects.custom-ui-lego/ui-accent-blue) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 2 "lfo-depth" "depth" 4.7 (eseq.effects.custom-ui-lego/ui-accent-blue) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 2 "mix-wet" "mix" 4.7 (eseq.effects.custom-ui-lego/ui-accent-green) 2))))

(defeffect-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (eseq.effects.custom-ui-lego/ui-lego-column-2
      (scfb-filter-block)
      (scfb-clock-block))
    (scfb-motion-block)))
