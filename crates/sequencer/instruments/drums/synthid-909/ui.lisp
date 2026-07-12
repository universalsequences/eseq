(def synthid909-pitch-block ()
  (ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.6 :gap 0.18 :align :start
        (h-stack :gap 0.18 :align :start
          (ui-lego-badge-s 0 "S909" 4.4 (ui-accent-orange))
          (ui-lego-micro-num-s 0 "pitch_decay" "P DEC" 3.8 3 "1/s" (ui-accent-violet))))
      (h-stack :gap 0.10 :align :start
        (ui-lego-knob-s 0 "start_ratio" "RATIO" 3.9 (ui-accent-blue) 3)
        (ui-lego-micro-num-s 0 "smoothing" "SMTH" 3.4 1 "ms" (ui-accent-cyan))
        (ui-lego-micro-num-s 0 "retrigger_fade" "XFD" 3.4 2 "ms" (ui-accent-violet))))))

(def synthid909-body-block ()
  (ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 12.5 :gap 0.18 :align :start
        (h-stack :gap 0.24 :align :start
          (ui-lego-badge-s 0 "BODY" 4.4 (ui-accent-blue))
          (ui-lego-micro-num-s 0 "release" "REL ms" 4.2 1 false (ui-accent-orange))
          (ui-lego-micro-num-s 0 "amp_curve" "CURVE" 3.4 2 false (ui-accent-violet))))
      (h-stack :gap 0.10 :align :start
        (ui-lego-knob-s 0 "body_amp" "AMP" 3.6 (ui-accent-orange) 3)
        (ui-lego-knob-s 0 "body_asymmetry" "ASYM" 3.6 (ui-accent-violet) 3)
        (ui-lego-micro-num-s 0 "body_harmonic" "HARM" 3.3 3 false (ui-accent-cyan))))))

(def synthid909-click-block ()
  (ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.6 :gap 0.18 :align :start
        (h-stack :gap 0.18 :align :start
          (ui-lego-badge-s 0 "CLICK" 4.4 (ui-accent-cyan))
          (ui-lego-micro-num-s 0 "click_decay" "DEC" 4.2 2 "1/s" (ui-accent-violet))))
      (h-stack :gap 0.10 :align :start
        (ui-lego-knob-s 0 "click_freq" "FREQ" 3.9 (ui-accent-cyan) 0)
        (ui-lego-knob-s 0 "click_amp" "AMP" 3.9 (ui-accent-orange) 3)))))

(def synthid909-noise-block ()
  (ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.6 :gap 0.18 :align :start
        (h-stack :gap 0.18 :align :start
          (ui-lego-badge-s 0 "NOISE" 4.4 (ui-accent-green))
          (ui-lego-micro-num-s 0 "noise_decay" "DEC" 4.2 3 "1/s" (ui-accent-violet))))
      (h-stack :gap 0.10 :align :start
        (ui-lego-knob-s 0 "noise_cutoff" "CUT" 3.9 (ui-accent-green) 0)
        (ui-lego-knob-s 0 "noise_amp" "AMP" 3.9 (ui-accent-cyan) 6)))))

(def synthid909-output-block ()
  (ui-readout-block-small-s "IDENTIFIED 909 OUTPUT" (ui-accent-orange) 0
    (h-stack :gap 0.24 :align :end
      (ui-lego-micro-num-s 0 "drive" "DRIVE" 4.0 4 false (ui-accent-orange))
      (ui-lego-micro-num-s 0 "out_gain" "GAIN" 4.0 4 false (ui-accent-orange))
      (ui-lego-micro-num-s 0 "fade_in" "ATT" 3.6 2 "ms" (ui-accent-cyan)))))

(defsynth-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (ui-lego-column
      (synthid909-pitch-block)
      (synthid909-body-block)
      (synthid909-output-block))
    (ui-lego-column-2
      (synthid909-click-block)
      (synthid909-noise-block))))
