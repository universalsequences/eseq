(def mdens-dens-block ()
  (ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (ui-lego-badge-s 0 "DENS" 3.8 (ui-accent-cyan))
          (ui-lego-micro-num-s 0 "tune_cents" "tune" 4.4 0 "ct" (ui-accent-orange)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 0 "pch2" "p2" 3.2 0 false (ui-accent-orange))
          (ui-lego-micro-num-s 0 "pch3" "p3" 3.2 0 false (ui-accent-orange))
          (ui-lego-micro-num-s 0 "pch4" "p4" 3.2 0 false (ui-accent-orange))))
      (h-stack :gap 0.08 :align :start
        (ui-lego-knob-s 0 "wave" "wave" 3.7 (ui-accent-cyan) 0)
        (ui-lego-knob-s 0 "pch2" "p2" 3.7 (ui-accent-orange) 0)
        (ui-lego-knob-s 0 "pch3" "p3" 3.7 (ui-accent-orange) 0)))))

(def mdens-chorus-block ()
  (ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (ui-lego-badge-s 0 "CHOR" 3.8 (ui-accent-blue))
          (ui-lego-micro-num-s 0 "chrw" "width" 4.4 2 false (ui-accent-blue)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 0 "drive" "drive" 3.5 2 false (ui-accent-orange))
          (ui-lego-micro-num-s 0 "gain" "gain" 3.5 2 false (ui-accent-orange))))
      (h-stack :gap 0.08 :align :start
        (ui-lego-knob-s 0 "chrl" "level" 3.7 (ui-accent-blue) 2)
        (ui-lego-knob-s 0 "chrw" "width" 3.7 (ui-accent-blue) 2)
        (ui-lego-knob-s 0 "drive" "drive" 3.7 (ui-accent-orange) 2)))))

(def mdens-filter-block ()
  (ui-control-panel-dense-s 1
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (ui-lego-badge-s 1 "FILT" 3.8 (ui-accent-green))
          (ui-lego-micro-num-s 1 "keytrack" "key" 4.4 2 false (ui-accent-green)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 1 "filter_env_amt" "env" 3.5 0 false (ui-accent-blue))
          (ui-lego-micro-num-s 1 "drive" "drive" 3.5 2 false (ui-accent-orange))))
      (h-stack :gap 0.08 :align :start
        (ui-lego-knob-s 1 "cutoff" "cut" 3.7 (ui-accent-green) 0)
        (ui-lego-knob-s 1 "resonance" "res" 3.7 (ui-accent-green) 2)
        (ui-lego-knob-s 1 "filter_env_amt" "env" 3.7 (ui-accent-blue) 0)))))

(def mdens-global-block ()
  (ui-control-panel-small-s 0
    (h-stack :gap 0.18 :align :start
      (ui-lego-badge-s 0 "GLB" 3.6 (ui-accent-orange))
      (ui-lego-micro-base-note-s 0 3.0 (ui-accent-orange))
      (ui-lego-micro-num-s 0 "tune_cents" "tune" 3.2 0 "ct" (ui-accent-orange))
      (ui-lego-micro-num-s 0 "gain" "gain" 3.0 2 false (ui-accent-orange)))))

(def mdens-detail-column ()
  (v-stack :width (ui-lego-col-w) :gap (ui-lego-gap)
    (ui-control-panel-small-s 0 (box :width :fill :height :fill))
    (ui-detail-adsr-switch-s
      0 "AMP" "amp_attack_ms" "amp_decay_ms" "amp_sustain" "amp_release_ms"
      1 "FILTER" "filter_attack_ms" "filter_decay_ms" "filter_sustain" "filter_release_ms")
    (mdens-global-block)))

(def mdens-dens-strip ()
  (ui-lego-strip-panel-s 0
    (v-stack :width :fill :gap 0.08 :align :center
      (ui-lego-badge-s 0 "DENS" 5.8 (ui-accent-cyan))
      (ui-lego-micro-num-s 0 "wave" "wave" 5.8 0 false (ui-accent-cyan))
      (ui-lego-micro-num-s 0 "pch2" "p2" 5.8 0 false (ui-accent-orange))
      (ui-lego-micro-num-s 0 "pch3" "p3" 5.8 0 false (ui-accent-orange))
      (ui-lego-micro-num-s 0 "pch4" "p4" 5.8 0 false (ui-accent-orange)))))

(def mdens-chorus-strip ()
  (ui-lego-strip-panel-s 0
    (v-stack :width :fill :gap 0.08 :align :center
      (ui-lego-badge-s 0 "CHOR" 5.8 (ui-accent-blue))
      (ui-lego-micro-num-s 0 "chrl" "level" 5.8 2 false (ui-accent-blue))
      (ui-lego-micro-num-s 0 "chrw" "width" 5.8 2 false (ui-accent-blue))
      (ui-lego-micro-num-s 0 "drive" "drive" 5.8 2 false (ui-accent-orange))
      (ui-lego-micro-num-s 0 "gain" "gain" 5.8 2 false (ui-accent-orange)))))

(defsynth-ui
  (h-stack :width :fill :gap 0.30 :align :stretch
    (ui-lego-column
      (mdens-dens-block)
      (mdens-chorus-block)
      (mdens-global-block))
    (mdens-detail-column)
    (ui-lego-column
      (mdens-filter-block)
      (ui-control-panel-small-s 1
        (h-stack :gap 0.18 :align :start
          (ui-lego-badge-s 1 "OUT" 3.6 (ui-accent-orange))
          (ui-lego-micro-num-s 1 "drive" "drive" 3.2 2 false (ui-accent-orange))
          (ui-lego-micro-num-s 0 "gain" "gain" 3.2 2 false (ui-accent-orange))))
      (ui-readout-panel-small-s 0
        (h-stack :gap 0.34 :align :start
          (label "density" :font-size 9.0 :color (ui-accent-cyan) :bg :transparent)
          (label "chorus" :font-size 9.0 :color (ui-accent-blue) :bg :transparent)
          (label "filter" :font-size 9.0 :color (ui-accent-green) :bg :transparent))))
    (h-stack :width 14.7 :gap 0.30 :align :stretch
      (mdens-dens-strip)
      (mdens-chorus-strip))))
