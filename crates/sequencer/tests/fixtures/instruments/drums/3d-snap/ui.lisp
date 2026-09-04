(defsynth-ui
  (eseq.effects.custom-ui-lego/ui-rack :breathe
    (list
      (eseq.effects.custom-ui-sections/ui-panel "GLOB" 0
        (h-stack :gap 0.35
          (eseq.effects.custom-ui-runtime/base-note)
          (eseq.effects.custom-ui-controls/ui-param-knob "gain" "gain")
          (eseq.effects.custom-ui-controls/ui-param-knob "drive" "drive")
          (eseq.effects.custom-ui-controls/ui-param-knob "tuning" "tune")
          (eseq.effects.custom-ui-controls/ui-param-knob "chaos" "chaos")))
      (eseq.effects.custom-ui-sections/ui-panel "STRIKE" 0
        (h-stack :gap 0.35
          (eseq.effects.custom-ui-controls/ui-param-knob "stick_level" "stick")
          (eseq.effects.custom-ui-controls/ui-param-knob "rim_level" "rim")
          (eseq.effects.custom-ui-controls/ui-param-knob "snap_level" "snap")
          (eseq.effects.custom-ui-controls/ui-param-knob "skin_level" "skin")
          (eseq.effects.custom-ui-controls/ui-param-knob "brightness" "bright")))
      (eseq.effects.custom-ui-sections/ui-panel "MEMBRANE" 0
        (h-stack :gap 0.35
          (eseq.effects.custom-ui-controls/ui-param-knob "membrane_level" "mem")
          (eseq.effects.custom-ui-controls/ui-param-knob "membrane_tension" "tens")
          (eseq.effects.custom-ui-controls/ui-param-knob "membrane_damping" "damp")
          (eseq.effects.custom-ui-controls/ui-param-knob "shell_level" "shell"))))
    (eseq.effects.custom-ui-lego/ui-adsr "AMP ENV" "amp_attack" "amp_decay" "amp_sustain" "amp_release")
    (list
      (eseq.effects.custom-ui-sections/ui-panel "FLAM" 0
        (h-stack :gap 0.35
          (eseq.effects.custom-ui-controls/ui-param-knob "flam_amount" "amt")
          (eseq.effects.custom-ui-controls/ui-param-knob "flam_rate" "rate")
          (eseq.effects.custom-ui-controls/ui-param-knob "flam_density" "dens")
          (eseq.effects.custom-ui-controls/ui-param-knob "flam_decay" "decay")))
      (eseq.effects.custom-ui-sections/ui-panel "WIRES" 0
        (h-stack :gap 0.35
          (eseq.effects.custom-ui-controls/ui-param-knob "wire_level" "level")
          (eseq.effects.custom-ui-controls/ui-param-knob "wire_tension" "tens")
          (eseq.effects.custom-ui-controls/ui-param-knob "wire_decay" "decay")
          (eseq.effects.custom-ui-controls/ui-param-knob "wire_rattle" "rattle")))
      (eseq.effects.custom-ui-sections/ui-panel "DECAY" 0
        (h-stack :gap 0.35
          (eseq.effects.custom-ui-controls/ui-param-knob "snap_decay" "snap")
          (eseq.effects.custom-ui-controls/ui-param-knob "body_decay" "body")
          (eseq.effects.custom-ui-controls/ui-param-knob "tail_decay" "tail")))
      (eseq.effects.custom-ui-sections/ui-panel "LOW/AIR" 0
        (h-stack :gap 0.35
          (eseq.effects.custom-ui-controls/ui-param-knob "boom_level" "boom")
          (eseq.effects.custom-ui-controls/ui-param-knob "air_level" "air")))
      (eseq.effects.custom-ui-sections/ui-panel "ROOM" 0
        (h-stack :gap 0.35
          (eseq.effects.custom-ui-controls/ui-param-knob "room_level" "level")
          (eseq.effects.custom-ui-controls/ui-param-knob "room_size" "size")
          (eseq.effects.custom-ui-controls/ui-param-knob "room_tone" "tone"))))))
