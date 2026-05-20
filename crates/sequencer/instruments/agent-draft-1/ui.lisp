(def wind-panel ()
  (ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (ui-lego-badge-s 0 "WIND" 3.6 (ui-accent-cyan))
          (ui-lego-micro-num-s 0 "noise_color" "colr" 6.0 0 "Hz" (ui-accent-cyan)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 0 "chiff_decay" "chfD" 4.8 0 "ms" (ui-accent-blue))
          (ui-lego-micro-num-s 0 "air_bleed" "bld" 4.8 2 false (ui-accent-orange))))
      (h-stack :gap 0.08 :align :start
        (ui-lego-knob-s 0 "pressure" "pres" 3.7 (ui-accent-cyan) 2)
        (ui-lego-knob-s 0 "noise_amt" "noise" 3.7 (ui-accent-cyan) 2)
        (ui-lego-knob-s 0 "chiff_amt" "chiff" 3.7 (ui-accent-blue) 2)))))

(def bore-panel ()
  (ui-control-panel-dense-s 1
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (ui-lego-badge-s 1 "BORE" 3.6 (ui-accent-green))
          (ui-lego-micro-num-s 1 "vibRate" "rate" 6.0 1 "Hz" (ui-accent-green)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 1 "vibDepth" "vDep" 4.8 2 "st" (ui-accent-green))
          (ui-lego-micro-num-s 1 "drive" "lip" 4.8 2 false (ui-accent-orange))))
      (h-stack :gap 0.08 :align :start
        (ui-lego-knob-s 1 "resonance" "bore" 3.7 (ui-accent-green) 1)
        (ui-lego-knob-s 1 "overblow" "over" 3.7 (ui-accent-orange) 2)
        (ui-lego-knob-s 1 "brightness" "tone" 3.7 (ui-accent-green) 0)))))

(def mix-panel ()
  (ui-control-panel-dense-s 2
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (ui-lego-badge-s 2 "HARM" 3.6 (ui-accent-violet))
          (ui-lego-micro-num-s 2 "mode3_gain" "12th" 6.0 2 false (ui-accent-violet)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 2 "mode4_gain" "15th" 9.8 2 false (ui-accent-violet))))
      (h-stack :gap 0.08 :align :start
        (ui-lego-knob-s 2 "mode1_gain" "fund" 3.7 (ui-accent-violet) 2)
        (ui-lego-knob-s 2 "mode2_gain" "oct" 3.7 (ui-accent-violet) 2)
        (ui-lego-knob-s 2 "gain" "vol" 3.7 (ui-accent-orange) 2)))))

(def envelope-column ()
  (ui-lego-column-full
    (box :width (ui-lego-col-w) :height (ui-lego-full-h)
      (ui-lego-adsr-s 0 "BREATH ENV" "amp_attack" "amp_decay" "amp_sustain" "amp_release"))))

(def global-block ()
  (ui-readout-block-small-s "SYSTEM" (ui-accent-orange) 0
    (h-stack :gap 0.30 :align :center
      (ui-lego-base-note 4.5 (ui-accent-orange))
      (ui-lego-num-s 0 "gain" "master" 4.5 2 false (ui-accent-orange)))))

(def pitch-block ()
  (ui-readout-block-small-s "VIBRATO" (ui-accent-green) 1
    (h-stack :gap 0.30 :align :center
      (ui-lego-num-s 1 "vibRate" "rate" 4.5 1 "Hz" (ui-accent-green))
      (ui-lego-num-s 1 "vibDepth" "depth" 4.5 2 "st" (ui-accent-green)))))

(def model-block ()
  (ui-readout-block-small-s "MODEL" (ui-accent-cyan) 0
    (ui-lego-text-row-3
      (label "MODAL RESONATOR" :font-size 9.0 :color (ui-accent-cyan) :bg :transparent)
      (label "NON-LINEAR JET" :font-size 9.0 :color (ui-accent-blue) :bg :transparent)
      (label "4-POLE HARMONICS" :font-size 9.0 :color (ui-accent-violet) :bg :transparent))))

(defsynth-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (ui-lego-column
      (wind-panel)
      (bore-panel)
      (mix-panel))
    (envelope-column)
    (ui-lego-column
      (global-block)
      (pitch-block)
      (model-block))))
