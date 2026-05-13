(defsynth-ui
  (ui-rack :breathe
    (list
      (ui-panel "GLOB" 0
        (h-stack :gap 0.35
          (base-note)
          (ui-param-knob "gain" "gain")
          (ui-param-knob "drive" "drive")
          (ui-param-knob "key_track" "track")))
      (ui-panel "MIXER" 0
        (h-stack :gap 0.35
          (ui-param-knob "osc1_level" "vco1")
          (ui-param-knob "osc2_level" "vco2")
          (ui-param-knob "osc3_level" "vco3")
          (ui-param-knob "noise_level" "noise")))
      (ui-panel "WAVES" 0
        (h-stack :gap 0.35
          (ui-param-knob "osc1_wave" "vco1")
          (ui-param-knob "osc2_wave" "vco2")
          (ui-param-knob "osc3_wave" "vco3")
          (ui-param-knob "pulse_width" "pw"))))
    (ui-adsr-switch
      0 "AMP ENV" "amp_attack" "amp_decay" "amp_sustain" "amp_release"
      1 "FILTER ENV" "filt_attack" "filt_decay" "filt_sustain" "filt_release")
    (list
      (ui-panel "TUNE" 0
        (h-stack :gap 0.35
          (ui-param-knob "osc1_oct" "oct1")
          (ui-param-knob "osc2_oct" "oct2")
          (ui-param-knob "osc3_oct" "oct3")
          (ui-param-knob "osc2_detune" "dt2")
          (ui-param-knob "osc3_detune" "dt3")))
      (ui-panel "FILTER" 1
        (h-stack :gap 0.35
          (ui-param-knob "cutoff" "cut")
          (ui-param-knob "resonance" "emph")
          (ui-param-knob "filter_env_amount" "contour"))))))
