;; Complete lego UI for Acoustic Grand Piano Synth

(def strings-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-medium-s "STRINGS" (eseq.effects.custom-ui-lego/ui-accent-cyan) 0
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "decay" "decay" 4.8 (eseq.effects.custom-ui-lego/ui-accent-cyan) 0)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "release" "release" 4.8 (eseq.effects.custom-ui-lego/ui-accent-cyan) 0)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "detune" "detune" 4.8 (eseq.effects.custom-ui-lego/ui-accent-blue) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "inharmonic" "stretch" 4.8 (eseq.effects.custom-ui-lego/ui-accent-blue) 2))))

(def global-block ()
  (eseq.effects.custom-ui-lego/ui-readout-block-small-s "GLOBAL" (eseq.effects.custom-ui-lego/ui-accent-orange) 0
    (h-stack :gap 0.30 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-base-note 4.2 (eseq.effects.custom-ui-lego/ui-accent-orange))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "dynamics" "dyn" 4.2 2 false (eseq.effects.custom-ui-lego/ui-accent-orange))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "gain" "gain" 4.2 2 false (eseq.effects.custom-ui-lego/ui-accent-orange)))))

(def hammer-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-medium-s "HAMMER STRIKE" (eseq.effects.custom-ui-lego/ui-accent-violet) 1
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "hammer_felt" "felt" 4.8 (eseq.effects.custom-ui-lego/ui-accent-violet) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "hammer_hard" "hard" 4.8 (eseq.effects.custom-ui-lego/ui-accent-violet) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "brightness" "lid" 4.8 (eseq.effects.custom-ui-lego/ui-accent-violet) 2))))

(def cabinet-block ()
  (eseq.effects.custom-ui-lego/ui-readout-block-small-s "CABINET" (eseq.effects.custom-ui-lego/ui-accent-green) 1
    (h-stack :gap 0.30 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "soundboard" "cab" 4.8 (eseq.effects.custom-ui-lego/ui-accent-green) 2)
      (eseq.effects.custom-ui-lego/ui-lego-text-row-3
        (label "soundboard" :font-size 9.0 :color (eseq.effects.custom-ui-lego/ui-accent-green) :bg :transparent)
        (label "cabinet res" :font-size 9.0 :color (eseq.effects.custom-ui-lego/ui-accent-green) :bg :transparent)
        (label "woody halo" :font-size 9.0 :color (eseq.effects.custom-ui-lego/ui-accent-green) :bg :transparent)))))

(defsynth-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (eseq.effects.custom-ui-lego/ui-lego-column-2
      (strings-block)
      (global-block))
    (eseq.effects.custom-ui-lego/ui-lego-column-2
      (hammer-block)
      (cabinet-block))))
