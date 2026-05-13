(defsynth-ui
  (ui-rack :breathe
    (list
      (ui-panel "GLOB" 0
        (h-stack :gap 0.35
          (base-note)
          (ui-param-knob "tune" "tune")
          (ui-param-knob "gain" "gain")))
      (ui-panel "STRIKE" 0
        (h-stack :gap 0.35
          (ui-param-knob "pitch_sweep" "sweep")
          (ui-param-knob "sweep_decay" "time")
          (ui-param-knob "bend" "bend")
          (ui-param-knob "mallet" "hard"))))
    (ui-adsr "AMP ENV" "amp_attack" "amp_decay" "amp_sustain" "amp_release")
    (list
      (ui-panel "MODEL" 0
        (h-stack :gap 0.35
          (ui-param-knob "material" "mat")
          (ui-param-knob "tension" "tens")
          (ui-param-knob "damping" "damp")
          (ui-param-knob "strike_pos" "pos")))
      (ui-panel "BODY" 0
        (h-stack :gap 0.35
          (ui-param-knob "skin_level" "skin")
          (ui-param-knob "shell_level" "shell")
          (ui-param-knob "cavity_level" "cav")
          (ui-param-knob "coupling" "cpl")))
      (ui-panel "IMPACT" 0
        (h-stack :gap 0.35
          (ui-param-knob "click_level" "click")
          (ui-param-knob "air_level" "air")))
      (ui-panel "TRACK FX" 0
        (h-stack :gap 0.35
          (ui-param-knob "hp" "hp")
          (ui-param-knob "tone" "tone")
          (ui-param-knob "dirt" "dirt")
          (ui-param-knob "drive" "drv"))))))
