(defsynth-ui
  (h-stack :width :fill :gap 0.45 :align :start
    (v-stack :width 32.0 :gap 0.10
      (ui-panel "GLOB" 0
        (h-stack :gap 0.35
          (base-note)
          (ui-param-knob "gain" "gain")
          (ui-param-knob "drive" "drive")
          (ui-param-knob "tuning" "tune")
          (ui-param-knob "chaos" "chaos")))
      (ui-panel "STRIKE" 0
        (h-stack :gap 0.35
          (ui-param-knob "stick_level" "stick")
          (ui-param-knob "rim_level" "rim")
          (ui-param-knob "snap_level" "snap")
          (ui-param-knob "skin_level" "skin")
          (ui-param-knob "brightness" "bright")))
      (ui-panel "MEMBRANE" 0
        (h-stack :gap 0.35
          (ui-param-knob "membrane_level" "mem")
          (ui-param-knob "membrane_tension" "tens")
          (ui-param-knob "membrane_damping" "damp")
          (ui-param-knob "shell_level" "shell"))))
    (ui-adsr 0 "amp_attack" "amp_decay" "amp_sustain" "amp_release")
    (v-stack :width 31.0 :gap 0.10
      (ui-panel "FLAM" 0
        (h-stack :gap 0.35
          (ui-param-knob "flam_amount" "amt")
          (ui-param-knob "flam_rate" "rate")
          (ui-param-knob "flam_density" "dens")
          (ui-param-knob "flam_decay" "decay")))
      (ui-panel "WIRES" 0
        (h-stack :gap 0.35
          (ui-param-knob "wire_level" "level")
          (ui-param-knob "wire_tension" "tens")
          (ui-param-knob "wire_decay" "decay")
          (ui-param-knob "wire_rattle" "rattle")))
      (ui-panel "DECAY" 0
        (h-stack :gap 0.35
          (ui-param-knob "snap_decay" "snap")
          (ui-param-knob "body_decay" "body")
          (ui-param-knob "tail_decay" "tail"))))
    (v-stack :width 27.0 :gap 0.10
      (ui-panel "LOW/AIR" 0
        (h-stack :gap 0.35
          (ui-param-knob "boom_level" "boom")
          (ui-param-knob "air_level" "air")))
      (ui-panel "ROOM" 0
        (h-stack :gap 0.35
          (ui-param-knob "room_level" "level")
          (ui-param-knob "room_size" "size")
          (ui-param-knob "room_tone" "tone"))))))