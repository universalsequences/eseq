(def drum3d-impact-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-medium-s "IMPACT" (eseq.effects.custom-ui-lego/ui-accent-cyan) 0
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "pitch_sweep" "sweep" 4.8 (eseq.effects.custom-ui-lego/ui-accent-blue) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "sweep_decay" "decay" 4.8 (eseq.effects.custom-ui-lego/ui-accent-blue) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "impact_decay" "hit" 4.8 (eseq.effects.custom-ui-lego/ui-accent-orange) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "tune" "tune" 4.8 (eseq.effects.custom-ui-lego/ui-accent-cyan) 2))))

(def drum3d-global-block ()
  (eseq.effects.custom-ui-lego/ui-readout-block-small-s "GLOBAL" (eseq.effects.custom-ui-lego/ui-accent-orange) 0
    (h-stack :gap 0.30 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-base-note 4.2 (eseq.effects.custom-ui-lego/ui-accent-orange))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "gain" "gain" 4.2 2 false (eseq.effects.custom-ui-lego/ui-accent-orange))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "click_level" "click" 4.2 2 false (eseq.effects.custom-ui-lego/ui-accent-cyan))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "noise_level" "noise" 4.2 2 false (eseq.effects.custom-ui-lego/ui-accent-violet)))))

(def drum3d-source-block ()
  (eseq.effects.custom-ui-lego/ui-readout-block-small-s "SOURCE" (eseq.effects.custom-ui-lego/ui-accent-cyan) 0
    (eseq.effects.custom-ui-lego/ui-lego-text-row-4
      (label "sub" :font-size 9.0 :color (eseq.effects.custom-ui-lego/ui-accent-cyan) :bg :transparent)
      (label "+ body" :font-size 9.0 :color (eseq.effects.custom-ui-lego/ui-accent-green) :bg :transparent)
      (label "+ shell" :font-size 9.0 :color (eseq.effects.custom-ui-lego/ui-accent-orange) :bg :transparent)
      (label "+ cavity" :font-size 9.0 :color (eseq.effects.custom-ui-lego/ui-accent-violet) :bg :transparent))))

(def drum3d-mix-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-medium-s "BODY MIX" (eseq.effects.custom-ui-lego/ui-accent-green) 0
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "sub_level" "sub" 4.8 (eseq.effects.custom-ui-lego/ui-accent-cyan) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "body_level" "body" 4.8 (eseq.effects.custom-ui-lego/ui-accent-green) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "shell_level" "shell" 4.8 (eseq.effects.custom-ui-lego/ui-accent-orange) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "cavity_level" "cavity" 4.8 (eseq.effects.custom-ui-lego/ui-accent-violet) 2))))

(def drum3d-material-block ()
  (eseq.effects.custom-ui-lego/ui-readout-block-small-s "MATERIAL" (eseq.effects.custom-ui-lego/ui-accent-orange) 0
    (h-stack :gap 0.30 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "warp" "warp" 4.2 2 false (eseq.effects.custom-ui-lego/ui-accent-blue))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "drive" "drive" 4.2 2 false (eseq.effects.custom-ui-lego/ui-accent-orange))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "tone" "tone" 4.2 2 false (eseq.effects.custom-ui-lego/ui-accent-green))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "cavity_size" "size" 4.2 2 false (eseq.effects.custom-ui-lego/ui-accent-violet)))))

(def drum3d-space-block ()
  (eseq.effects.custom-ui-lego/ui-readout-block-small-s "SPACE" (eseq.effects.custom-ui-lego/ui-accent-blue) 0
    (h-stack :gap 0.30 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "x_spread" "x" 4.2 2 false (eseq.effects.custom-ui-lego/ui-accent-blue))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "y_spread" "y" 4.2 2 false (eseq.effects.custom-ui-lego/ui-accent-blue))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "z_depth" "z" 4.2 2 false (eseq.effects.custom-ui-lego/ui-accent-blue))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "membrane_damp" "damp" 4.2 2 false (eseq.effects.custom-ui-lego/ui-accent-green)))))

(def drum3d-envelope-column ()
  (eseq.effects.custom-ui-lego/ui-lego-column-full
    (eseq.effects.custom-ui-lego/ui-lego-adsr-s 0 "AMP ENV" "amp_attack" "amp_decay" "amp_sustain" "amp_release")))

(defsynth-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (eseq.effects.custom-ui-lego/ui-lego-column
      (drum3d-impact-block)
      (drum3d-global-block)
      (drum3d-source-block))
    (eseq.effects.custom-ui-lego/ui-lego-column
      (drum3d-mix-block)
      (drum3d-material-block)
      (drum3d-space-block))
    (drum3d-envelope-column)))
