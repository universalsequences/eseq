(defsynth-ui
  (eseq.effects.custom-ui-lego/ui-rack :breathe
    (list
      (eseq.effects.custom-ui-sections/ui-panel "GLOB" 0
        (h-stack :gap 0.35
          (eseq.effects.custom-ui-runtime/base-note)
          (eseq.effects.custom-ui-controls/ui-param-knob "tune" "tune")
          (eseq.effects.custom-ui-controls/ui-param-knob "gain" "gain")))
      (eseq.effects.custom-ui-sections/ui-panel "STRIKE" 0
        (h-stack :gap 0.35
          (eseq.effects.custom-ui-controls/ui-param-knob "pitch_sweep" "sweep")
          (eseq.effects.custom-ui-controls/ui-param-knob "sweep_decay" "time")
          (eseq.effects.custom-ui-controls/ui-param-knob "bend" "bend")
          (eseq.effects.custom-ui-controls/ui-param-knob "mallet" "hard"))))
    (eseq.effects.custom-ui-lego/ui-adsr "AMP ENV" "amp_attack" "amp_decay" "amp_sustain" "amp_release")
    (list
      (eseq.effects.custom-ui-sections/ui-panel "MODEL" 0
        (h-stack :gap 0.35
          (eseq.effects.custom-ui-controls/ui-param-knob "material" "mat")
          (eseq.effects.custom-ui-controls/ui-param-knob "tension" "tens")
          (eseq.effects.custom-ui-controls/ui-param-knob "damping" "damp")
          (eseq.effects.custom-ui-controls/ui-param-knob "strike_pos" "pos")))
      (eseq.effects.custom-ui-sections/ui-panel "BODY" 0
        (h-stack :gap 0.35
          (eseq.effects.custom-ui-controls/ui-param-knob "skin_level" "skin")
          (eseq.effects.custom-ui-controls/ui-param-knob "shell_level" "shell")
          (eseq.effects.custom-ui-controls/ui-param-knob "cavity_level" "cav")
          (eseq.effects.custom-ui-controls/ui-param-knob "coupling" "cpl")))
      (eseq.effects.custom-ui-sections/ui-panel "IMPACT" 0
        (h-stack :gap 0.35
          (eseq.effects.custom-ui-controls/ui-param-knob "click_level" "click")
          (eseq.effects.custom-ui-controls/ui-param-knob "air_level" "air")))
      (eseq.effects.custom-ui-sections/ui-panel "TRACK FX" 0
        (h-stack :gap 0.35
          (eseq.effects.custom-ui-controls/ui-param-knob "hp" "hp")
          (eseq.effects.custom-ui-controls/ui-param-knob "tone" "tone")
          (eseq.effects.custom-ui-controls/ui-param-knob "dirt" "dirt")
          (eseq.effects.custom-ui-controls/ui-param-knob "drive" "drv"))))))
