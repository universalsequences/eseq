(def flavor-block ()
  (ui-control-block-medium-s "FLAVOR" (ui-accent-orange) 0
    (h-stack :gap 0.32 :align :start
      (ui-lego-option-s 0 "model" "model" 5.2 '("808" "909" "tom") (ui-accent-orange))
      (ui-lego-knob-s 0 "tune" "tune" 4.8 (ui-accent-cyan) 0)
      (ui-lego-knob-s 0 "fine" "fine" 4.8 (ui-accent-blue) 0)
      (ui-lego-knob-s 0 "gain" "gain" 4.8 (ui-accent-orange) 2))))

(def pitch-block ()
  (ui-control-block-medium-s "PITCH" (ui-accent-blue) 0
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob-s 0 "bend" "bend" 4.8 (ui-accent-blue) 0)
      (ui-lego-knob-s 0 "bend_decay" "decay" 4.8 (ui-accent-blue) 0)
      (ui-lego-knob-s 0 "punch" "punch" 4.8 (ui-accent-cyan) 0)
      (ui-lego-knob-s 0 "punch_decay" "hit" 4.8 (ui-accent-cyan) 0))))

(def body-block ()
  (ui-control-block-medium-s "BODY" (ui-accent-green) 0
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob-s 0 "body_level" "body" 4.8 (ui-accent-green) 2)
      (ui-lego-knob-s 0 "sub_level" "sub" 4.8 (ui-accent-cyan) 2)
      (ui-lego-knob-s 0 "sub_ratio" "ratio" 4.8 (ui-accent-cyan) 2)
      (ui-lego-knob-s 0 "body_shape" "shape" 4.8 (ui-accent-orange) 2))))

(def membrane-block ()
  (ui-control-block-medium-s "MEMBRANE" (ui-accent-violet) 0
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob-s 0 "overtone" "tone" 4.8 (ui-accent-violet) 2)
      (ui-lego-knob-s 0 "membrane_decay" "decay" 4.8 (ui-accent-violet) 2)
      (ui-lego-knob-s 0 "amp_decay" "boom" 4.8 (ui-accent-green) 0)
      (ui-lego-knob-s 0 "drive" "drive" 4.8 (ui-accent-orange) 2))))

(def click-block ()
  (ui-control-block-medium-s "CLICK" (ui-accent-cyan) 0
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob-s 0 "click_level" "level" 4.8 (ui-accent-cyan) 2)
      (ui-lego-knob-s 0 "click_tone" "tone" 4.8 (ui-accent-cyan) 0)
      (ui-lego-knob-s 0 "click_decay" "decay" 4.8 (ui-accent-blue) 0)
      (ui-lego-knob-s 0 "click_noise_mix" "noise" 4.8 (ui-accent-violet) 2))))

(def noise-block ()
  (ui-control-block-medium-s "SNAP" (ui-accent-violet) 0
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob-s 0 "noise_level" "level" 4.8 (ui-accent-violet) 2)
      (ui-lego-knob-s 0 "noise_tone" "tone" 4.8 (ui-accent-violet) 0)
      (ui-lego-knob-s 0 "noise_decay" "decay" 4.8 (ui-accent-blue) 0)
      (ui-lego-knob-s 0 "noise_color" "color" 4.8 (ui-accent-orange) 2))))

(def tone-block ()
  (ui-control-block-medium-s "OUTPUT" (ui-accent-green) 0
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob-s 0 "low_cut" "lowcut" 4.8 (ui-accent-green) 0)
      (ui-lego-knob-s 0 "body_tone" "body LP" 4.8 (ui-accent-green) 0)
      (ui-lego-knob-s 0 "tone" "tone" 4.8 (ui-accent-green) 0)
      (ui-lego-knob-s 0 "dirt" "dirt" 4.8 (ui-accent-orange) 2))))

(def util-block ()
  (ui-readout-block-small-s "GLOBAL" (ui-accent-orange) 0
    (h-stack :gap 0.30 :align :start
      (ui-lego-base-note 4.2 (ui-accent-orange))
      (ui-lego-num-s 0 "resonance" "res" 4.2 2 false (ui-accent-green))
      (ui-lego-num-s 0 "noise_q" "nQ" 4.2 2 false (ui-accent-violet))
      (ui-lego-num-s 0 "amp_release" "rel" 4.2 0 "ms" (ui-accent-blue)))))

(def source-block ()
  (ui-readout-block-small-s "GUIDE" (ui-accent-blue) 0
    (ui-lego-text-row-4
      (label "808: sub+long" :font-size 9.0 :color (ui-accent-cyan) :bg :transparent)
      (label "909: punch+snap" :font-size 9.0 :color (ui-accent-orange) :bg :transparent)
      (label "tom: overtones" :font-size 9.0 :color (ui-accent-violet) :bg :transparent)
      (label "pitch follows MIDI" :font-size 9.0 :color (ui-accent-green) :bg :transparent))))

(def envelope-column ()
  (ui-lego-column-full
    (ui-lego-adsr-s 0 "AMP ENV" "amp_attack" "amp_decay" "amp_sustain" "amp_release")))

(defsynth-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (ui-lego-column
      (flavor-block)
      (pitch-block)
      (source-block))
    (ui-lego-column
      (body-block)
      (membrane-block)
      (util-block))
    (ui-lego-column
      (click-block)
      (noise-block)
      (tone-block))
    (envelope-column)))