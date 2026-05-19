(def breath-block ()
  (ui-control-block-medium-s "BREATH & ARTICULATION" (ui-accent-cyan) 0
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob-s 0 "pressure_ctrl" "blow" 4.8 (ui-accent-cyan) 2)
      (ui-lego-knob-s 0 "breath_noise" "breath" 4.8 (ui-accent-cyan) 2)
      (ui-lego-knob-s 0 "key_click" "click" 4.8 (ui-accent-blue) 2))))

(def growl-block ()
  (ui-control-block-medium-s "GROWL & VIBRATO" (ui-accent-orange) 0
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob-s 0 "growl_amt" "growl" 4.8 (ui-accent-orange) 2)
      (ui-lego-knob-s 0 "vib_depth" "vibrato" 4.8 (ui-accent-orange) 2)
      (ui-lego-num-s 0 "glide" "glide" 4.8 0 "ms" (ui-accent-blue)))))

(def global-block ()
  (ui-readout-block-small-s "GLOBAL" (ui-accent-orange) 0
    (h-stack :gap 0.30 :align :start
      (ui-lego-base-note 4.2 (ui-accent-orange))
      (ui-lego-num-s 0 "gain" "gain" 4.2 2 false (ui-accent-orange)))))

(def reed-block ()
  (ui-control-block-medium-s "REED & BORE" (ui-accent-green) 1
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob-s 1 "reed_stiffness" "stiff" 4.8 (ui-accent-green) 2)
      (ui-lego-knob-s 1 "impedance" "imped" 4.8 (ui-accent-green) 2)
      (ui-lego-knob-s 1 "bore_type" "bore" 4.8 (ui-accent-violet) 2))))

(def body-block ()
  (ui-control-block-medium-s "BODY RESONATOR" (ui-accent-violet) 1
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob-s 1 "body_cutoff" "cutoff" 4.8 (ui-accent-violet) 0)
      (ui-lego-knob-s 1 "body_q" "resonance" 4.8 (ui-accent-violet) 2)
      (ui-lego-knob-s 1 "saturation" "drive" 4.8 (ui-accent-orange) 2))))

(def tube-block ()
  (ui-readout-block-small-s "TUBE DETAILS" (ui-accent-green) 1
    (h-stack :gap 0.30 :align :start
      (ui-lego-num-s 1 "reflection_cutoff" "refl-lp" 4.7 0 "Hz" (ui-accent-green))
      (ui-lego-num-s 1 "growl_rate" "growl-hz" 4.7 0 "Hz" (ui-accent-orange))
      (ui-lego-num-s 1 "vib_rate" "vib-hz" 4.7 1 "Hz" (ui-accent-orange)))))

(def envelope-column ()
  (ui-lego-column-full
    (box :width (ui-lego-col-w) :height (ui-lego-full-h)
      (ui-lego-adsr-s 0 "BREATH ENV" "amp_attack" "amp_decay" "amp_sustain" "amp_release"))))

(defsynth-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (ui-lego-column
      (breath-block)
      (growl-block)
      (global-block))
    (envelope-column)
    (ui-lego-column
      (reed-block)
      (body-block)
      (tube-block))))