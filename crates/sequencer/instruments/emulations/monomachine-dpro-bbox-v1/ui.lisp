(def mbbox-main-block ()
  (ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (ui-lego-badge-s 0 "BBOX" 3.8 (ui-accent-cyan))
          (ui-lego-micro-num-s 0 "start" "start" 4.4 2 false (ui-accent-cyan)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 0 "rtrg" "rtrg" 3.3 2 false (ui-accent-blue))
          (ui-lego-micro-num-s 0 "rtim" "rtim" 3.3 0 "ms" (ui-accent-blue))))
      (h-stack :gap 0.08 :align :start
        (ui-lego-knob-s 0 "ptch" "pitch" 3.7 (ui-accent-orange) 0)
        (ui-lego-knob-s 0 "start" "start" 3.7 (ui-accent-cyan) 2)
        (ui-lego-knob-s 0 "rtrg" "rtrg" 3.7 (ui-accent-blue) 2)))))

(def mbbox-filter-block ()
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

(def mbbox-global-block ()
  (ui-control-panel-small-s 0
    (h-stack :gap 0.18 :align :start
      (ui-lego-badge-s 0 "GLB" 3.6 (ui-accent-orange))
      (ui-lego-micro-base-note-s 0 3.0 (ui-accent-orange))
      (ui-lego-micro-num-s 0 "drive" "drive" 3.0 2 false (ui-accent-orange))
      (ui-lego-micro-num-s 0 "gain" "gain" 3.0 2 false (ui-accent-orange)))))

(def mbbox-detail-column ()
  (v-stack :width (ui-lego-col-w) :gap (ui-lego-gap)
    (ui-control-panel-small-s 0 (box :width :fill :height :fill))
    (ui-detail-adsr-switch-s
      0 "AMP" "amp_attack_ms" "amp_decay_ms" "amp_sustain" "amp_release_ms"
      1 "FILTER" "filter_attack_ms" "filter_decay_ms" "filter_sustain" "filter_release_ms")
    (mbbox-global-block)))

(def mbbox-source-strip ()
  (ui-lego-strip-panel-s 0
    (v-stack :width :fill :gap 0.08 :align :center
      (ui-lego-badge-s 0 "SAMPLE" 5.8 (ui-accent-cyan))
      (ui-lego-micro-num-s 0 "ptch" "pitch" 5.8 0 false (ui-accent-orange))
      (ui-lego-micro-num-s 0 "start" "start" 5.8 2 false (ui-accent-cyan))
      (ui-lego-micro-num-s 0 "rtrg" "rtrg" 5.8 2 false (ui-accent-blue))
      (ui-lego-micro-num-s 0 "rtim" "rtim" 5.8 0 "ms" (ui-accent-blue)))))

(def mbbox-filter-strip ()
  (ui-lego-strip-panel-s 1
    (v-stack :width :fill :gap 0.08 :align :center
      (ui-lego-badge-s 1 "FILT" 5.8 (ui-accent-green))
      (ui-lego-micro-num-s 1 "cutoff" "cut" 5.8 0 false (ui-accent-green))
      (ui-lego-micro-num-s 1 "resonance" "res" 5.8 2 false (ui-accent-green))
      (ui-lego-micro-num-s 1 "filter_env_amt" "env" 5.8 0 false (ui-accent-blue))
      (ui-lego-micro-num-s 1 "keytrack" "key" 5.8 2 false (ui-accent-green)))))

(defsynth-ui
  (h-stack :width :fill :gap 0.30 :align :stretch
    (ui-lego-column
      (mbbox-main-block)
      (mbbox-filter-block)
      (mbbox-global-block))
    (mbbox-detail-column)
    (ui-lego-column
      (ui-control-panel-small-s 0
        (h-stack :gap 0.18 :align :start
          (ui-lego-badge-s 0 "RTRG" 3.8 (ui-accent-blue))
          (ui-lego-micro-num-s 0 "rtrg" "amt" 3.2 2 false (ui-accent-blue))
          (ui-lego-micro-num-s 0 "rtim" "time" 3.2 0 "ms" (ui-accent-blue))))
      (ui-control-panel-small-s 1
        (h-stack :gap 0.18 :align :start
          (ui-lego-badge-s 1 "OUT" 3.6 (ui-accent-orange))
          (ui-lego-micro-num-s 1 "drive" "drive" 3.2 2 false (ui-accent-orange))
          (ui-lego-micro-num-s 1 "gain" "gain" 3.2 2 false (ui-accent-orange))))
      (ui-readout-panel-small-s 0
        (h-stack :gap 0.34 :align :start
          (label "bbox" :font-size 9.0 :color (ui-accent-cyan) :bg :transparent)
          (label "through" :font-size 9.0 :color :dim :bg :transparent)
          (label "filter" :font-size 9.0 :color (ui-accent-green) :bg :transparent))))
    (h-stack :width 14.7 :gap 0.30 :align :stretch
      (mbbox-source-strip)
      (mbbox-filter-strip))))
