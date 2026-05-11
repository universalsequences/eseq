(defsynth-ui
  (h-stack :width :fill :gap 0.45 :align :start
    (v-stack :width 28.0 :gap 0.10
      (ui-panel "GLOB" 0
        (h-stack :gap 0.35
          (base-note)
          (ui-param-knob "tune" "tune")
          (ui-param-knob "gain" "gain")))
      (ui-panel "IMPACT" 0
        (h-stack :gap 0.35
          (ui-param-knob "pitch_sweep" "sweep")
          (ui-param-knob "sweep_decay" "decay")
          (ui-param-knob "impact_decay" "hit")))
      (ui-panel "MIX" 0
        (h-stack :gap 0.35
          (ui-param-knob "sub_level" "sub")
          (ui-param-knob "body_level" "body")
          (ui-param-knob "shell_level" "shell")
          (ui-param-knob "cavity_level" "cavity"))))
    (ui-adsr "AMP ENV" "amp_attack" "amp_decay" "amp_sustain" "amp_release")
    (v-stack :width 29.0 :gap 0.10
      (ui-panel "3D MODES" 0
        (h-stack :gap 0.35
          (ui-param-knob "x_spread" "x")
          (ui-param-knob "y_spread" "y")
          (ui-param-knob "z_depth" "z")
          (ui-param-knob "membrane_damp" "damp")))
      (ui-panel "MATERIAL" 0
        (h-stack :gap 0.35
          (ui-param-knob "warp" "warp")
          (ui-param-knob "drive" "drive")
          (ui-param-knob "tone" "tone")
          (ui-param-knob "cavity_size" "size")))
      (ui-panel "TRANSIENT" 0
        (h-stack :gap 0.35
          (ui-param-knob "click_level" "click")
          (ui-param-knob "noise_level" "noise"))))))