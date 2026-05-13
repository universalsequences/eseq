(defsynth-ui
  (ui-rack :breathe
    (list
      (ui-panel "GLOBAL" 0
        (h-stack :gap 0.2
          (base-note)
          (ui-param-knob "gain" "gain")
          (ui-param-knob "analog_drift" "drift")))
      (ui-panel "VCO 1" 0
        (h-stack :gap 0.2
          (ui-param-knob "vco1_saw" "saw")
          (ui-param-knob "vco1_pulse" "pulse")
          (ui-param-knob "pulse_width" "width")
          (ui-param-knob "pwm_amount" "pwm")))
      (ui-panel "VCO 2 / MIX" 0
        (h-stack :gap 0.2
          (ui-param-knob "vco2_level" "vco2")
          (ui-param-knob "vco2_interval" "semi")
          (ui-param-knob "vco2_fine" "fine")
          (ui-param-knob "sub_level" "sub")))
      (ui-panel "DIRT" 0
        (h-stack :gap 0.2
          (ui-param-knob "input_drive" "input")
          (ui-param-knob "output_bite" "bite"))))
    (ui-adsr-switch
      0 "AMP ENV" "amp_attack" "amp_decay" "amp_sustain" "amp_release"
      1 "FILTER ENV" "filt_attack" "filt_decay" "filt_sustain" "filt_release")
    (list
      (ui-panel "MS FILTER" 1
        (h-stack :gap 0.2
          (ui-param-knob "cutoff" "cut")
          (ui-param-knob "resonance" "peak")
          (ui-param-knob "filter_env_amount" "env")
          (ui-param-knob "keytrack" "track")))
      (ui-panel "HP / SCREAM" 1
        (h-stack :gap 0.2
          (ui-param-knob "hp_cutoff" "hp")
          (ui-param-knob "hp_resonance" "hp peak")
          (ui-param-knob "scream" "scream")
          (ui-param-knob "filter_drive" "drive")))
      (ui-panel "MOD" 0
        (h-stack :gap 0.2
          (ui-param-knob "lfo_rate" "rate")
          (ui-param-knob "lfo_filter_amount" "filt")
          (ui-param-knob "lfo_pitch" "pitch")
          (ui-param-knob "pitch_env_amount" "snap")))
      (ui-panel "NOISE / RING" 0
        (h-stack :gap 0.2
          (ui-param-knob "noise_level" "noise")
          (ui-param-knob "ring_level" "ring"))))))
