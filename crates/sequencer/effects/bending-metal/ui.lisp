(defsynth-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (v-stack :width 25.0 :gap 0.08
      (eseq.effects.custom-ui-controls/ui-panel-c "PLAY" 0
        (h-stack :gap 0.2
          (eseq.effects.custom-ui-controls/base-note-c)
          (eseq.effects.custom-ui-controls/ui-param-knob-c "gain" "gain")
          (eseq.effects.custom-ui-controls/ui-param-knob-c "strike" "strike")
          (eseq.effects.custom-ui-controls/ui-param-knob-c "tune" "tune")))
      (eseq.effects.custom-ui-controls/ui-panel-c "PLATE" 0
        (h-stack :gap 0.2
          (eseq.effects.custom-ui-controls/ui-param-knob-c "base_tension" "tension")
          (eseq.effects.custom-ui-controls/ui-param-knob-c "damping" "damp")
          (eseq.effects.custom-ui-controls/ui-param-knob-c "drive" "drive"))))
    (v-stack :width 22.0 :gap 0.08
      (eseq.effects.custom-ui-controls/ui-panel-c "MOTION" 0
        (h-stack :gap 0.2
          (eseq.effects.custom-ui-controls/ui-param-knob-c "bend_depth" "bend")
          (eseq.effects.custom-ui-controls/ui-param-knob-c "tension_coupling" "couple"))))))
