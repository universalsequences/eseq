(defsynth-ui
  (h-stack :width :fill :gap 0.45 :align :start
    (v-stack :width 27.0 :gap 0.10
      (ui-panel "GLOB" 0
        (h-stack :gap 0.35
          (base-note)
          (ui-param-knob "gain" "gain")))
      (ui-panel "OSC" 0
        (h-stack :gap 0.35
          (ui-param-knob "detune" "det")
          (ui-param-knob "osc_blend" "blend")
          (ui-param-knob "sub_level" "sub")))
      (ui-panel "MIX" 0
        (h-stack :gap 0.35
          (ui-param-knob "noise_level" "noise")
          (ui-param-knob "snap" "snap")
          (ui-param-knob "drive" "drive"))))
    (ui-adsr-switch
      0 "AMP ENV" "amp_attack" "amp_decay" "amp_sustain" "amp_release"
      1 "FILTER ENV" "filt_attack" "filt_decay" "filt_sustain" "filt_release")
    (v-stack :width 29.0 :gap 0.10
      (ui-panel "FILT" 1
        (h-stack :gap 0.35
          (ui-param-knob "cutoff" "cut"
          )
          (ui-param-knob "resonance" "res")
          (ui-param-knob "filter_env_amount" "env")))
      (ui-panel "TRACK" 1
        (h-stack :gap 0.35
          (ui-param-knob "keytrack" "key"))))))