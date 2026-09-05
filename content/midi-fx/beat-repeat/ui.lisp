;; Beat repeat panel: one block — clock division plus the gate and velocity
;; scale applied to every retriggered note.
(def beat-repeat-rate-labels
  (list "1" "1/2" "1/4" "1/8" "1/16" "1/32" "1/64"
        "1/2T" "1/4T" "1/8T" "1/16T" "1/32T" "1/64T"))

(def beat-repeat-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-medium "REPEAT" (eseq.effects.custom-ui-lego/ui-accent-blue)
    (h-stack :gap 0.6 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-option-s 0 "rate" "rate" 6.0 beat-repeat-rate-labels (eseq.effects.custom-ui-lego/ui-accent-blue))
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "gate" "gate" 4.7 (eseq.effects.custom-ui-lego/ui-accent-blue) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "velocity" "vel x" 4.7 (eseq.effects.custom-ui-lego/ui-accent-blue) 2))))

(def-midi-fx-ui
  (h-stack :gap 0.35 :align :stretch
    (eseq.effects.custom-ui-lego/ui-lego-column-full
      (beat-repeat-block))))
