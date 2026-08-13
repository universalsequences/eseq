(def snareo-strike-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-medium-s "STRIKE" (eseq.effects.custom-ui-lego/ui-accent-cyan) 0
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "stick_level" "stick" 4.8 (eseq.effects.custom-ui-lego/ui-accent-cyan) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "rim_level" "rim" 4.8 (eseq.effects.custom-ui-lego/ui-accent-orange) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "snap_level" "snap" 4.8 (eseq.effects.custom-ui-lego/ui-accent-blue) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "skin_level" "skin" 4.8 (eseq.effects.custom-ui-lego/ui-accent-green) 2))))

(def snareo-global-block ()
  (eseq.effects.custom-ui-lego/ui-readout-block-small-s "GLOBAL" (eseq.effects.custom-ui-lego/ui-accent-orange) 0
    (h-stack :gap 0.30 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-base-note 3.7 (eseq.effects.custom-ui-lego/ui-accent-orange))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "gain" "gain" 3.7 2 false (eseq.effects.custom-ui-lego/ui-accent-orange))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "drive" "drive" 3.7 2 false (eseq.effects.custom-ui-lego/ui-accent-orange))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "tuning" "tune" 3.7 2 false (eseq.effects.custom-ui-lego/ui-accent-cyan))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "chaos" "chaos" 3.7 2 false (eseq.effects.custom-ui-lego/ui-accent-violet)))))

(def snareo-source-block ()
  (eseq.effects.custom-ui-lego/ui-readout-block-small-s "SOURCE" (eseq.effects.custom-ui-lego/ui-accent-cyan) 0
    (eseq.effects.custom-ui-lego/ui-lego-text-row-4
      (label "stick" :font-size 9.0 :color (eseq.effects.custom-ui-lego/ui-accent-cyan) :bg :transparent)
      (label "+ rim" :font-size 9.0 :color (eseq.effects.custom-ui-lego/ui-accent-orange) :bg :transparent)
      (label "+ snap" :font-size 9.0 :color (eseq.effects.custom-ui-lego/ui-accent-blue) :bg :transparent)
      (label "+ skin" :font-size 9.0 :color (eseq.effects.custom-ui-lego/ui-accent-green) :bg :transparent))))

(def snareo-membrane-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-medium-s "MEMBRANE" (eseq.effects.custom-ui-lego/ui-accent-green) 0
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "membrane_level" "mem" 4.8 (eseq.effects.custom-ui-lego/ui-accent-green) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "membrane_tension" "tens" 4.8 (eseq.effects.custom-ui-lego/ui-accent-green) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "membrane_damping" "damp" 4.8 (eseq.effects.custom-ui-lego/ui-accent-green) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "shell_level" "shell" 4.8 (eseq.effects.custom-ui-lego/ui-accent-orange) 2))))

(def snareo-decay-block ()
  (eseq.effects.custom-ui-lego/ui-readout-block-small-s "DECAY" (eseq.effects.custom-ui-lego/ui-accent-blue) 0
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "snap_decay" "snap" 4.7 2 false (eseq.effects.custom-ui-lego/ui-accent-blue))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "body_decay" "body" 4.7 2 false (eseq.effects.custom-ui-lego/ui-accent-green))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "tail_decay" "tail" 4.7 2 false (eseq.effects.custom-ui-lego/ui-accent-violet)))))

(def snareo-lowair-block ()
  (eseq.effects.custom-ui-lego/ui-readout-block-small-s "LOW/AIR" (eseq.effects.custom-ui-lego/ui-accent-violet) 0
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "boom_level" "boom" 4.7 2 false (eseq.effects.custom-ui-lego/ui-accent-violet))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "air_level" "air" 4.7 2 false (eseq.effects.custom-ui-lego/ui-accent-cyan))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "brightness" "bright" 4.7 2 false (eseq.effects.custom-ui-lego/ui-accent-orange)))))

(def snareo-flam-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-medium-s "FLAM" (eseq.effects.custom-ui-lego/ui-accent-blue) 0
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "flam_amount" "amt" 4.8 (eseq.effects.custom-ui-lego/ui-accent-blue) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "flam_rate" "rate" 4.8 (eseq.effects.custom-ui-lego/ui-accent-blue) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "flam_density" "dens" 4.8 (eseq.effects.custom-ui-lego/ui-accent-blue) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "flam_decay" "decay" 4.8 (eseq.effects.custom-ui-lego/ui-accent-blue) 2))))

(def snareo-wires-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-medium-s "WIRES" (eseq.effects.custom-ui-lego/ui-accent-violet) 0
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "wire_level" "level" 4.8 (eseq.effects.custom-ui-lego/ui-accent-violet) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "wire_tension" "tens" 4.8 (eseq.effects.custom-ui-lego/ui-accent-violet) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "wire_decay" "decay" 4.8 (eseq.effects.custom-ui-lego/ui-accent-violet) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "wire_rattle" "rattle" 4.8 (eseq.effects.custom-ui-lego/ui-accent-violet) 2))))

(def snareo-room-block ()
  (eseq.effects.custom-ui-lego/ui-readout-block-small-s "ROOM" (eseq.effects.custom-ui-lego/ui-accent-green) 0
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "room_level" "level" 4.7 2 false (eseq.effects.custom-ui-lego/ui-accent-green))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "room_size" "size" 4.7 2 false (eseq.effects.custom-ui-lego/ui-accent-green))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "room_tone" "tone" 4.7 2 false (eseq.effects.custom-ui-lego/ui-accent-green)))))

(def snareo-envelope-column ()
  (eseq.effects.custom-ui-lego/ui-lego-column-full
    (eseq.effects.custom-ui-lego/ui-lego-adsr-s 0 "AMP ENV" "amp_attack" "amp_decay" "amp_sustain" "amp_release")))

(defsynth-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (eseq.effects.custom-ui-lego/ui-lego-column
      (snareo-strike-block)
      (snareo-global-block)
      (snareo-source-block))
    (eseq.effects.custom-ui-lego/ui-lego-column
      (snareo-membrane-block)
      (snareo-decay-block)
      (snareo-lowair-block))
    (snareo-envelope-column)
    (eseq.effects.custom-ui-lego/ui-lego-column
      (snareo-flam-block)
      (snareo-room-block)
      (snareo-wires-block))))
