;; Complete lego UI for Acoustic Grand Piano Synth

(def strings-block ()
  (ui-control-block-medium-s "STRINGS" (ui-accent-cyan) 0
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob-s 0 "decay" "decay" 4.8 (ui-accent-cyan) 0)
      (ui-lego-knob-s 0 "release" "release" 4.8 (ui-accent-cyan) 0)
      (ui-lego-knob-s 0 "detune" "detune" 4.8 (ui-accent-blue) 2)
      (ui-lego-knob-s 0 "inharmonic" "stretch" 4.8 (ui-accent-blue) 2))))

(def global-block ()
  (ui-readout-block-small-s "GLOBAL" (ui-accent-orange) 0
    (h-stack :gap 0.30 :align :start
      (ui-lego-base-note 4.2 (ui-accent-orange))
      (ui-lego-num-s 0 "dynamics" "dyn" 4.2 2 false (ui-accent-orange))
      (ui-lego-num-s 0 "gain" "gain" 4.2 2 false (ui-accent-orange)))))

(def hammer-block ()
  (ui-control-block-medium-s "HAMMER STRIKE" (ui-accent-violet) 1
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob-s 1 "hammer_felt" "felt" 4.8 (ui-accent-violet) 2)
      (ui-lego-knob-s 1 "hammer_hard" "hard" 4.8 (ui-accent-violet) 2)
      (ui-lego-knob-s 1 "brightness" "lid" 4.8 (ui-accent-violet) 2))))

(def cabinet-block ()
  (ui-readout-block-small-s "CABINET" (ui-accent-green) 1
    (h-stack :gap 0.30 :align :start
      (ui-lego-knob-s 1 "soundboard" "cab" 4.8 (ui-accent-green) 2)
      (ui-lego-text-row-3
        (label "soundboard" :font-size 9.0 :color (ui-accent-green) :bg :transparent)
        (label "cabinet res" :font-size 9.0 :color (ui-accent-green) :bg :transparent)
        (label "woody halo" :font-size 9.0 :color (ui-accent-green) :bg :transparent)))))

(defsynth-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (ui-lego-column-2
      (strings-block)
      (global-block))
    (ui-lego-column-2
      (hammer-block)
      (cabinet-block))))
