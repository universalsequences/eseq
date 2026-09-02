(def synthid-pitch-block ()
  (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.6 :gap 0.18 :align :start
        (h-stack :gap 0.18 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "S808T" 4.4 (eseq.effects.custom-ui-lego/ui-accent-orange))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "pitch_decay" "P DEC" 3.8 3 "1/s" (eseq.effects.custom-ui-lego/ui-accent-violet))))
      (h-stack :gap 0.10 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "start_ratio" "RATIO" 3.9 (eseq.effects.custom-ui-lego/ui-accent-blue) 3)
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "smoothing" "SMTH" 3.4 1 "ms" (eseq.effects.custom-ui-lego/ui-accent-cyan))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "retrigger_fade" "XFD" 3.4 2 "ms" (eseq.effects.custom-ui-lego/ui-accent-violet))))))

(def synthid-body-block ()
  (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.6 :gap 0.18 :align :start
        (h-stack :gap 0.18 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "BODY" 4.4 (eseq.effects.custom-ui-lego/ui-accent-blue))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "release" "REL" 4.2 1 "ms" (eseq.effects.custom-ui-lego/ui-accent-orange))))
      (h-stack :gap 0.10 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "body_amp" "AMP" 3.9 (eseq.effects.custom-ui-lego/ui-accent-orange) 3)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "body_asymmetry" "ASYM" 3.9 (eseq.effects.custom-ui-lego/ui-accent-violet) 3)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "body_harmonic" "ODD" 3.9 (eseq.effects.custom-ui-lego/ui-accent-violet) 3)))))

(def synthid-click-block ()
  (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.6 :gap 0.18 :align :start
        (h-stack :gap 0.18 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "CLICK" 4.4 (eseq.effects.custom-ui-lego/ui-accent-cyan))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "click_decay" "DEC" 4.2 2 "1/s" (eseq.effects.custom-ui-lego/ui-accent-violet))))
      (h-stack :gap 0.10 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "click_freq" "FREQ" 3.9 (eseq.effects.custom-ui-lego/ui-accent-cyan) 0)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "click_amp" "AMP" 3.9 (eseq.effects.custom-ui-lego/ui-accent-orange) 3)))))

(def synthid-noise-block ()
  (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.6 :gap 0.18 :align :start
        (h-stack :gap 0.18 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "NOISE" 4.4 (eseq.effects.custom-ui-lego/ui-accent-green))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "noise_decay" "DEC" 4.2 3 "1/s" (eseq.effects.custom-ui-lego/ui-accent-violet))))
      (h-stack :gap 0.10 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "noise_cutoff" "CUT" 3.9 (eseq.effects.custom-ui-lego/ui-accent-green) 0)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "noise_amp" "AMP" 3.9 (eseq.effects.custom-ui-lego/ui-accent-cyan) 6)))))

(def synthid-output-block ()
  (eseq.effects.custom-ui-lego/ui-readout-block-small-s "IDENTIFIED OUTPUT" (eseq.effects.custom-ui-lego/ui-accent-orange) 0
    (h-stack :gap 0.24 :align :end
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "drive" "DRIVE" 4.0 4 false (eseq.effects.custom-ui-lego/ui-accent-orange))
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "out_gain" "GAIN" 4.0 4 false (eseq.effects.custom-ui-lego/ui-accent-orange))
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "fade_in" "ATT" 3.6 2 "ms" (eseq.effects.custom-ui-lego/ui-accent-cyan)))))

(defsynth-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (eseq.effects.custom-ui-lego/ui-lego-column
      (synthid-pitch-block)
      (synthid-body-block)
      (synthid-output-block))
    (eseq.effects.custom-ui-lego/ui-lego-column-2
      (synthid-click-block)
      (synthid-noise-block))))
