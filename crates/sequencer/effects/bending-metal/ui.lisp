(defsynth-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (v-stack :width 25.0 :gap 0.08
      (ui-panel-c "PLAY" 0
        (h-stack :gap 0.2
          (base-note-c)
          (ui-param-knob-c "gain" "gain")
          (ui-param-knob-c "strike" "strike")
          (ui-param-knob-c "tune" "tune")))
      (ui-panel-c "PLATE" 0
        (h-stack :gap 0.2
          (ui-param-knob-c "base_tension" "tension")
          (ui-param-knob-c "damping" "damp")
          (ui-param-knob-c "drive" "drive"))))
    (v-stack :width 22.0 :gap 0.08
      (ui-panel-c "MOTION" 0
        (h-stack :gap 0.2
          (ui-param-knob-c "bend_depth" "bend")
          (ui-param-knob-c "tension_coupling" "couple"))))))
