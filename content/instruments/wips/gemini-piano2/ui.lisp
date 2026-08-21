(def hammer-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-medium-s "HAMMER STRIKE" (eseq.effects.custom-ui-lego/ui-accent-cyan) 0
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "hammer_hardness" "hard" 4.8 (eseq.effects.custom-ui-lego/ui-accent-cyan) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "hammer_noise" "noise" 4.8 (eseq.effects.custom-ui-lego/ui-accent-cyan) 2))))

(def cabinet-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-small-s "WOOD CABINET" (eseq.effects.custom-ui-lego/ui-accent-violet) 0
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "soundboard_mix" "s-mix" 4.8 (eseq.effects.custom-ui-lego/ui-accent-violet) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "wooden_damping" "damp" 4.8 (eseq.effects.custom-ui-lego/ui-accent-violet) 2))))

(def info-block ()
  (eseq.effects.custom-ui-lego/ui-readout-block-small-s "PHYSICS" (eseq.effects.custom-ui-lego/ui-accent-blue) 0
    (eseq.effects.custom-ui-lego/ui-lego-text-row-3
      (label "MODAL RESONANCE" :font-size 9.0 :color (eseq.effects.custom-ui-lego/ui-accent-blue) :bg :transparent)
      (label "12 STRINGS UNISON" :font-size 9.0 :color (eseq.effects.custom-ui-lego/ui-accent-cyan) :bg :transparent)
      (label "WOODEN SOUNDBOARD" :font-size 9.0 :color (eseq.effects.custom-ui-lego/ui-accent-violet) :bg :transparent))))

(def string-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-medium-s "STRING RESONANCE" (eseq.effects.custom-ui-lego/ui-accent-green) 1
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "inharmonicity" "inharm" 4.8 (eseq.effects.custom-ui-lego/ui-accent-green) 5)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "unison_detune" "detune" 4.8 (eseq.effects.custom-ui-lego/ui-accent-green) 1))))

(def global-block ()
  (eseq.effects.custom-ui-lego/ui-readout-block-small-s "GLOBAL" (eseq.effects.custom-ui-lego/ui-accent-orange) 1
    (h-stack :gap 0.30 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-base-note 4.2 (eseq.effects.custom-ui-lego/ui-accent-orange))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 1 "gain" "gain" 4.2 2 false (eseq.effects.custom-ui-lego/ui-accent-orange))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 1 "vel_sens" "v-sens" 4.2 2 false (eseq.effects.custom-ui-lego/ui-accent-orange)))))

(def sustain-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-medium-s "SUSTAIN" (eseq.effects.custom-ui-lego/ui-accent-orange) 2
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 2 "sustain_s" "decay" 4.8 (eseq.effects.custom-ui-lego/ui-accent-orange) 1)
      (eseq.effects.custom-ui-lego/ui-lego-num-s 2 "key_track" "k-track" 4.2 2 false (eseq.effects.custom-ui-lego/ui-accent-orange)))))

(def tone-block ()
  (eseq.effects.custom-ui-lego/ui-readout-block-small-s "DAMPER" (eseq.effects.custom-ui-lego/ui-accent-violet) 2
    (h-stack :gap 0.30 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-num-s 2 "damper_release" "damp" 4.2 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-violet))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 2 "decay_slope" "slope" 4.2 2 false (eseq.effects.custom-ui-lego/ui-accent-violet)))))

(defsynth-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (eseq.effects.custom-ui-lego/ui-lego-column
      (hammer-block)
      (cabinet-block)
      (info-block))
    (eseq.effects.custom-ui-lego/ui-lego-column-2
      (string-block)
      (global-block))
    (eseq.effects.custom-ui-lego/ui-lego-column-2
      (sustain-block)
      (tone-block))))
