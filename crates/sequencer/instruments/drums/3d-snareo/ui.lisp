(def snareo-strike-block ()
  (ui-control-block-medium-s "STRIKE" (ui-accent-cyan) 0
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob-s 0 "stick_level" "stick" 4.8 (ui-accent-cyan) 2)
      (ui-lego-knob-s 0 "rim_level" "rim" 4.8 (ui-accent-orange) 2)
      (ui-lego-knob-s 0 "snap_level" "snap" 4.8 (ui-accent-blue) 2)
      (ui-lego-knob-s 0 "skin_level" "skin" 4.8 (ui-accent-green) 2))))

(def snareo-global-block ()
  (ui-readout-block-small-s "GLOBAL" (ui-accent-orange) 0
    (h-stack :gap 0.30 :align :start
      (ui-lego-base-note 3.7 (ui-accent-orange))
      (ui-lego-num-s 0 "gain" "gain" 3.7 2 false (ui-accent-orange))
      (ui-lego-num-s 0 "drive" "drive" 3.7 2 false (ui-accent-orange))
      (ui-lego-num-s 0 "tuning" "tune" 3.7 2 false (ui-accent-cyan))
      (ui-lego-num-s 0 "chaos" "chaos" 3.7 2 false (ui-accent-violet)))))

(def snareo-source-block ()
  (ui-readout-block-small-s "SOURCE" (ui-accent-cyan) 0
    (ui-lego-text-row-4
      (label "stick" :font-size 9.0 :color (ui-accent-cyan) :bg :transparent)
      (label "+ rim" :font-size 9.0 :color (ui-accent-orange) :bg :transparent)
      (label "+ snap" :font-size 9.0 :color (ui-accent-blue) :bg :transparent)
      (label "+ skin" :font-size 9.0 :color (ui-accent-green) :bg :transparent))))

(def snareo-membrane-block ()
  (ui-control-block-medium-s "MEMBRANE" (ui-accent-green) 0
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob-s 0 "membrane_level" "mem" 4.8 (ui-accent-green) 2)
      (ui-lego-knob-s 0 "membrane_tension" "tens" 4.8 (ui-accent-green) 2)
      (ui-lego-knob-s 0 "membrane_damping" "damp" 4.8 (ui-accent-green) 2)
      (ui-lego-knob-s 0 "shell_level" "shell" 4.8 (ui-accent-orange) 2))))

(def snareo-decay-block ()
  (ui-readout-block-small-s "DECAY" (ui-accent-blue) 0
    (h-stack :gap 0.32 :align :start
      (ui-lego-num-s 0 "snap_decay" "snap" 4.7 2 false (ui-accent-blue))
      (ui-lego-num-s 0 "body_decay" "body" 4.7 2 false (ui-accent-green))
      (ui-lego-num-s 0 "tail_decay" "tail" 4.7 2 false (ui-accent-violet)))))

(def snareo-lowair-block ()
  (ui-readout-block-small-s "LOW/AIR" (ui-accent-violet) 0
    (h-stack :gap 0.32 :align :start
      (ui-lego-num-s 0 "boom_level" "boom" 4.7 2 false (ui-accent-violet))
      (ui-lego-num-s 0 "air_level" "air" 4.7 2 false (ui-accent-cyan))
      (ui-lego-num-s 0 "brightness" "bright" 4.7 2 false (ui-accent-orange)))))

(def snareo-flam-block ()
  (ui-control-block-medium-s "FLAM" (ui-accent-blue) 0
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob-s 0 "flam_amount" "amt" 4.8 (ui-accent-blue) 2)
      (ui-lego-knob-s 0 "flam_rate" "rate" 4.8 (ui-accent-blue) 2)
      (ui-lego-knob-s 0 "flam_density" "dens" 4.8 (ui-accent-blue) 2)
      (ui-lego-knob-s 0 "flam_decay" "decay" 4.8 (ui-accent-blue) 2))))

(def snareo-wires-block ()
  (ui-control-block-medium-s "WIRES" (ui-accent-violet) 0
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob-s 0 "wire_level" "level" 4.8 (ui-accent-violet) 2)
      (ui-lego-knob-s 0 "wire_tension" "tens" 4.8 (ui-accent-violet) 2)
      (ui-lego-knob-s 0 "wire_decay" "decay" 4.8 (ui-accent-violet) 2)
      (ui-lego-knob-s 0 "wire_rattle" "rattle" 4.8 (ui-accent-violet) 2))))

(def snareo-room-block ()
  (ui-readout-block-small-s "ROOM" (ui-accent-green) 0
    (h-stack :gap 0.32 :align :start
      (ui-lego-num-s 0 "room_level" "level" 4.7 2 false (ui-accent-green))
      (ui-lego-num-s 0 "room_size" "size" 4.7 2 false (ui-accent-green))
      (ui-lego-num-s 0 "room_tone" "tone" 4.7 2 false (ui-accent-green)))))

(def snareo-envelope-column ()
  (ui-lego-column-full
    (ui-lego-adsr-s 0 "AMP ENV" "amp_attack" "amp_decay" "amp_sustain" "amp_release")))

(defsynth-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (ui-lego-column
      (snareo-strike-block)
      (snareo-global-block)
      (snareo-source-block))
    (ui-lego-column
      (snareo-membrane-block)
      (snareo-decay-block)
      (snareo-lowair-block))
    (snareo-envelope-column)
    (ui-lego-column
      (snareo-flam-block)
      (snareo-room-block)
      (snareo-wires-block))))
