(def drum3d-impact-block ()
  (ui-control-block-medium-s "IMPACT" (ui-accent-cyan) 0
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob-s 0 "pitch_sweep" "sweep" 4.8 (ui-accent-blue) 2)
      (ui-lego-knob-s 0 "sweep_decay" "decay" 4.8 (ui-accent-blue) 2)
      (ui-lego-knob-s 0 "impact_decay" "hit" 4.8 (ui-accent-orange) 2)
      (ui-lego-knob-s 0 "tune" "tune" 4.8 (ui-accent-cyan) 2))))

(def drum3d-global-block ()
  (ui-readout-block-small-s "GLOBAL" (ui-accent-orange) 0
    (h-stack :gap 0.30 :align :start
      (ui-lego-base-note 4.2 (ui-accent-orange))
      (ui-lego-num-s 0 "gain" "gain" 4.2 2 false (ui-accent-orange))
      (ui-lego-num-s 0 "click_level" "click" 4.2 2 false (ui-accent-cyan))
      (ui-lego-num-s 0 "noise_level" "noise" 4.2 2 false (ui-accent-violet)))))

(def drum3d-source-block ()
  (ui-readout-block-small-s "SOURCE" (ui-accent-cyan) 0
    (ui-lego-text-row-4
      (label "sub" :font-size 9.0 :color (ui-accent-cyan) :bg :transparent)
      (label "+ body" :font-size 9.0 :color (ui-accent-green) :bg :transparent)
      (label "+ shell" :font-size 9.0 :color (ui-accent-orange) :bg :transparent)
      (label "+ cavity" :font-size 9.0 :color (ui-accent-violet) :bg :transparent))))

(def drum3d-mix-block ()
  (ui-control-block-medium-s "BODY MIX" (ui-accent-green) 0
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob-s 0 "sub_level" "sub" 4.8 (ui-accent-cyan) 2)
      (ui-lego-knob-s 0 "body_level" "body" 4.8 (ui-accent-green) 2)
      (ui-lego-knob-s 0 "shell_level" "shell" 4.8 (ui-accent-orange) 2)
      (ui-lego-knob-s 0 "cavity_level" "cavity" 4.8 (ui-accent-violet) 2))))

(def drum3d-material-block ()
  (ui-readout-block-small-s "MATERIAL" (ui-accent-orange) 0
    (h-stack :gap 0.30 :align :start
      (ui-lego-num-s 0 "warp" "warp" 4.2 2 false (ui-accent-blue))
      (ui-lego-num-s 0 "drive" "drive" 4.2 2 false (ui-accent-orange))
      (ui-lego-num-s 0 "tone" "tone" 4.2 2 false (ui-accent-green))
      (ui-lego-num-s 0 "cavity_size" "size" 4.2 2 false (ui-accent-violet)))))

(def drum3d-space-block ()
  (ui-readout-block-small-s "SPACE" (ui-accent-blue) 0
    (h-stack :gap 0.30 :align :start
      (ui-lego-num-s 0 "x_spread" "x" 4.2 2 false (ui-accent-blue))
      (ui-lego-num-s 0 "y_spread" "y" 4.2 2 false (ui-accent-blue))
      (ui-lego-num-s 0 "z_depth" "z" 4.2 2 false (ui-accent-blue))
      (ui-lego-num-s 0 "membrane_damp" "damp" 4.2 2 false (ui-accent-green)))))

(def drum3d-envelope-column ()
  (ui-lego-column-full
    (ui-lego-adsr-s 0 "AMP ENV" "amp_attack" "amp_decay" "amp_sustain" "amp_release")))

(defsynth-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (ui-lego-column
      (drum3d-impact-block)
      (drum3d-global-block)
      (drum3d-source-block))
    (ui-lego-column
      (drum3d-mix-block)
      (drum3d-material-block)
      (drum3d-space-block))
    (drum3d-envelope-column)))
