(def mddrw-wave-block ()
  (ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (ui-lego-badge-s 0 "DDRW" 3.8 (ui-accent-cyan))
          (ui-lego-micro-num-s 0 "mix" "mix" 4.4 2 false (ui-accent-blue)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 0 "tune_cents" "tune" 3.5 0 "ct" (ui-accent-orange))))
      (h-stack :gap 0.08 :align :start
        (ui-lego-knob-s 0 "wav1" "wav1" 3.7 (ui-accent-cyan) 0)
        (ui-lego-knob-s 0 "mix" "mix" 3.7 (ui-accent-blue) 2)
        (ui-lego-knob-s 0 "wav2" "wav2" 3.7 (ui-accent-cyan) 0)))))

(def mddrw-draw-block ()
  (ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (ui-lego-badge-s 0 "DRAW" 3.8 (ui-accent-orange))
          (ui-lego-micro-num-s 0 "wid" "width" 4.4 0 false (ui-accent-cyan)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 0 "br1" "br1" 3.3 0 false (ui-accent-orange))
          (ui-lego-micro-num-s 0 "br2" "br2" 3.3 0 false (ui-accent-orange))))
      (h-stack :gap 0.08 :align :start
        (ui-lego-knob-s 0 "time" "time" 3.7 (ui-accent-blue) 0)
        (ui-lego-knob-s 0 "br1" "br1" 3.7 (ui-accent-orange) 0)
        (ui-lego-knob-s 0 "wid" "width" 3.7 (ui-accent-cyan) 0)))))

(def mddrw-filter-block ()
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

(def mddrw-global-block ()
  (ui-control-panel-small-s 0
    (h-stack :gap 0.18 :align :start
      (ui-lego-badge-s 0 "GLB" 3.6 (ui-accent-orange))
      (ui-lego-micro-base-note-s 0 3.0 (ui-accent-orange))
      (ui-lego-micro-num-s 0 "drive" "drive" 3.0 2 false (ui-accent-orange))
      (ui-lego-micro-num-s 0 "gain" "gain" 3.0 2 false (ui-accent-orange)))))

(def mddrw-detail-column ()
  (v-stack :width (ui-lego-col-w) :gap (ui-lego-gap)
    (ui-control-panel-small-s 0 (box :width :fill :height :fill))
    (ui-detail-adsr-switch-s
      0 "AMP" "amp_attack_ms" "amp_decay_ms" "amp_sustain" "amp_release_ms"
      1 "FILTER" "filter_attack_ms" "filter_decay_ms" "filter_sustain" "filter_release_ms")
    (mddrw-global-block)))

(def mddrw-wave-strip ()
  (ui-lego-strip-panel-s 0
    (v-stack :width :fill :gap 0.08 :align :center
      (ui-lego-badge-s 0 "WAVE" 5.8 (ui-accent-cyan))
      (ui-lego-micro-num-s 0 "wav1" "wav1" 5.8 0 false (ui-accent-cyan))
      (ui-lego-micro-num-s 0 "mix" "mix" 5.8 2 false (ui-accent-blue))
      (ui-lego-micro-num-s 0 "wav2" "wav2" 5.8 0 false (ui-accent-cyan))
      (ui-lego-micro-num-s 0 "tune_cents" "tune" 5.8 0 "ct" (ui-accent-orange)))))

(def mddrw-draw-strip ()
  (ui-lego-strip-panel-s 0
    (v-stack :width :fill :gap 0.08 :align :center
      (ui-lego-badge-s 0 "DRAW" 5.8 (ui-accent-orange))
      (ui-lego-micro-num-s 0 "time" "time" 5.8 0 false (ui-accent-blue))
      (ui-lego-micro-num-s 0 "br1" "br1" 5.8 0 false (ui-accent-orange))
      (ui-lego-micro-num-s 0 "wid" "width" 5.8 0 false (ui-accent-cyan))
      (ui-lego-micro-num-s 0 "br2" "br2" 5.8 0 false (ui-accent-orange)))))

(defsynth-ui
  (h-stack :width :fill :gap 0.30 :align :stretch
    (ui-lego-column
      (mddrw-wave-block)
      (mddrw-draw-block)
      (mddrw-global-block))
    (mddrw-detail-column)
    (ui-lego-column
      (mddrw-filter-block)
      (ui-control-panel-small-s 1
        (h-stack :gap 0.18 :align :start
          (ui-lego-badge-s 1 "OUT" 3.6 (ui-accent-orange))
          (ui-lego-micro-num-s 1 "drive" "drive" 3.2 2 false (ui-accent-orange))
          (ui-lego-micro-num-s 0 "gain" "gain" 3.2 2 false (ui-accent-orange))))
      (ui-readout-panel-small-s 0
        (h-stack :gap 0.34 :align :start
          (label "draw" :font-size 9.0 :color (ui-accent-orange) :bg :transparent)
          (label "into" :font-size 9.0 :color :dim :bg :transparent)
          (label "filter" :font-size 9.0 :color (ui-accent-green) :bg :transparent))))
    (h-stack :width 14.7 :gap 0.30 :align :stretch
      (mddrw-wave-strip)
      (mddrw-draw-strip))))
