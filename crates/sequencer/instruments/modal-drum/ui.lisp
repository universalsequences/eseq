(defsynth-ui
  (ui-rack :breathe
    (list
      (ui-panel "GLOB" 0
        (h-stack :gap 0.35
          (base-note)
          (ui-param-knob "tune" "tune")
          (ui-param-knob "gain" "gain")))
      (ui-panel "BODY" 0
        (h-stack :gap 0.35
          (ui-param-knob "body_level" "body")
          (ui-param-knob "amp_decay" "decay")
          (ui-param-knob "punch" "punch")
          (ui-param-knob "punch_decay" "p.decay")))
      (ui-panel "CLICK" 0
        (h-stack :gap 0.35
          (ui-param-knob "click_level" "click")
          (ui-param-knob "click_decay" "c.decay")
          (ui-param-knob "click_tone" "tone"))))
    (ui-adsr "AMP ENV" "amp_attack" "amp_decay" "amp_sustain" "amp_release")
    (list
      (ui-panel "PITCH" 0
        (h-stack :gap 0.35
          (ui-param-knob "sweep_amount" "drop")
          (ui-param-knob "sweep_decay" "time")
          (ui-param-knob "sweep_curve" "curve")))
      (ui-panel "MODES" 0
        (h-stack :gap 0.35
          (ui-param-knob "mode2_level" "m2")
          (ui-param-knob "mode2_ratio" "m2.r")
          (ui-param-knob "mode2_decay" "m2.d")
          (ui-param-knob "mode3_level" "m3")
          (ui-param-knob "mode3_ratio" "m3.r")
          (ui-param-knob "mode3_decay" "m3.d")))
      (ui-panel "COLOR" 0
        (h-stack :gap 0.35
          (ui-param-knob "damping" "damp")
          (ui-param-knob "drive" "drive"))))))
