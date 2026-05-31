(def hammer-block ()
  (ui-control-block-medium-s "HAMMER STRIKE" (ui-accent-cyan) 0
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob-s 0 "hammer_hardness" "hard" 4.8 (ui-accent-cyan) 2)
      (ui-lego-knob-s 0 "hammer_noise" "noise" 4.8 (ui-accent-cyan) 2))))

(def cabinet-block ()
  (ui-control-block-small-s "WOOD CABINET" (ui-accent-violet) 0
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob-s 0 "soundboard_mix" "s-mix" 4.8 (ui-accent-violet) 2)
      (ui-lego-knob-s 0 "wooden_damping" "damp" 4.8 (ui-accent-violet) 2))))

(def info-block ()
  (ui-readout-block-small-s "PHYSICS" (ui-accent-blue) 0
    (ui-lego-text-row-3
      (label "MODAL RESONANCE" :font-size 9.0 :color (ui-accent-blue) :bg :transparent)
      (label "12 STRINGS UNISON" :font-size 9.0 :color (ui-accent-cyan) :bg :transparent)
      (label "WOODEN SOUNDBOARD" :font-size 9.0 :color (ui-accent-violet) :bg :transparent))))

(def string-block ()
  (ui-control-block-medium-s "STRING RESONANCE" (ui-accent-green) 1
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob-s 1 "inharmonicity" "inharm" 4.8 (ui-accent-green) 5)
      (ui-lego-knob-s 1 "unison_detune" "detune" 4.8 (ui-accent-green) 1))))

(def global-block ()
  (ui-readout-block-small-s "GLOBAL" (ui-accent-orange) 1
    (h-stack :gap 0.30 :align :start
      (ui-lego-base-note 4.2 (ui-accent-orange))
      (ui-lego-num-s 1 "gain" "gain" 4.2 2 false (ui-accent-orange))
      (ui-lego-num-s 1 "vel_sens" "v-sens" 4.2 2 false (ui-accent-orange)))))

(def sustain-block ()
  (ui-control-block-medium-s "SUSTAIN" (ui-accent-orange) 2
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob-s 2 "sustain_s" "decay" 4.8 (ui-accent-orange) 1)
      (ui-lego-num-s 2 "key_track" "k-track" 4.2 2 false (ui-accent-orange)))))

(def tone-block ()
  (ui-readout-block-small-s "DAMPER" (ui-accent-violet) 2
    (h-stack :gap 0.30 :align :start
      (ui-lego-num-s 2 "damper_release" "damp" 4.2 0 "ms" (ui-accent-violet))
      (ui-lego-num-s 2 "decay_slope" "slope" 4.2 2 false (ui-accent-violet)))))

(defsynth-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (ui-lego-column
      (hammer-block)
      (cabinet-block)
      (info-block))
    (ui-lego-column-2
      (string-block)
      (global-block))
    (ui-lego-column-2
      (sustain-block)
      (tone-block))))
