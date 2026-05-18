(def mfm-ratio-block ()
  (ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (ui-lego-badge-s 0 "RATIO" 4.2 (ui-accent-cyan))
          (ui-lego-micro-num-s 0 "ratio_warp" "warp" 4.0 2 false (ui-accent-violet)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 0 "ratio_c" "C" 3.1 2 false (ui-accent-cyan))
          (ui-lego-micro-num-s 0 "ratio_d" "D" 3.1 2 false (ui-accent-cyan))))
      (h-stack :gap 0.08 :align :start
        (ui-lego-knob-s 0 "ratio_a" "A" 3.7 (ui-accent-cyan) 2)
        (ui-lego-knob-s 0 "ratio_b" "B" 3.7 (ui-accent-cyan) 2)
        (ui-lego-knob-s 0 "ratio_warp" "warp" 3.7 (ui-accent-violet) 2)))))

(def mfm-index-block ()
  (ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (ui-lego-badge-s 0 "INDEX" 4.2 (ui-accent-orange))
          (ui-lego-micro-num-s 0 "dyn_index_amt" "dyn" 4.0 2 false (ui-accent-blue)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 0 "crossmod" "cross" 3.3 2 false (ui-accent-violet))
          (ui-lego-micro-num-s 0 "self_fm" "self" 3.3 2 false (ui-accent-violet))))
      (h-stack :gap 0.08 :align :start
        (ui-lego-knob-s 0 "index_a" "A" 3.7 (ui-accent-orange) 2)
        (ui-lego-knob-s 0 "index_b" "B" 3.7 (ui-accent-orange) 2)
        (ui-lego-knob-s 0 "dyn_index_amt" "dyn" 3.7 (ui-accent-blue) 2)))))

(def mfm-feedback-block ()
  (ui-control-panel-small-s 0
    (h-stack :gap 0.18 :align :start
      (ui-lego-badge-s 0 "FB" 3.6 (ui-accent-violet))
      (ui-lego-micro-num-s 0 "feedback_a" "A" 3.0 2 false (ui-accent-violet))
      (ui-lego-micro-num-s 0 "feedback_b" "B" 3.0 2 false (ui-accent-violet))
      (ui-lego-micro-num-s 0 "crossmod" "cross" 3.4 2 false (ui-accent-violet))
      (ui-lego-micro-num-s 0 "self_fm" "self" 3.0 2 false (ui-accent-violet)))))

(def mfm-tone-block ()
  (ui-control-panel-dense-s 1
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (ui-lego-badge-s 1 "TONE" 3.8 (ui-accent-green))
          (ui-lego-micro-num-s 1 "keytrack" "key" 4.4 2 false (ui-accent-green)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 1 "filter_drive" "drive" 3.5 2 false (ui-accent-orange))
          (ui-lego-micro-num-s 1 "parallel_mix" "par" 3.5 2 false (ui-accent-blue))))
      (h-stack :gap 0.08 :align :start
        (ui-lego-knob-s 1 "tone" "tone" 3.7 (ui-accent-green) 2)
        (ui-lego-knob-s 1 "resonance" "res" 3.7 (ui-accent-green) 2)
        (ui-lego-knob-s 1 "filter_drive" "drv" 3.7 (ui-accent-orange) 2)))))

(def mfm-glitch-block ()
  (ui-control-panel-small-s 0
    (h-stack :gap 0.18 :align :start
      (ui-lego-badge-s 0 "GLT" 3.6 (ui-accent-blue))
      (ui-lego-micro-num-s 0 "glitch_rate" "rate" 3.4 2 false (ui-accent-blue))
      (ui-lego-micro-num-s 0 "glitch_amt" "amt" 3.4 2 false (ui-accent-blue))
      (ui-lego-micro-num-s 0 "parallel_mix" "par" 3.4 2 false (ui-accent-violet)))))

(def mfm-global-block ()
  (ui-control-panel-small-s 0
    (h-stack :gap 0.18 :align :start
      (ui-lego-badge-s 0 "GLB" 3.6 (ui-accent-orange))
      (ui-lego-micro-base-note-s 0 3.0 (ui-accent-orange))
      (ui-lego-micro-num-s 0 "gain" "gain" 3.0 2 false (ui-accent-orange)))))

(def mfm-detail-column ()
  (v-stack :width (ui-lego-col-w) :gap (ui-lego-gap)
    (ui-control-panel-small-s 0 (box :width :fill :height :fill))
    (ui-detail-adsr-s 0 "AMP" "amp_attack_ms" "amp_decay_ms" "amp_sustain" "amp_release_ms")
    (mfm-global-block)))

(def mfm-ratio-strip ()
  (ui-lego-strip-panel-s 0
    (v-stack :width :fill :gap 0.08 :align :center
      (ui-lego-badge-s 0 "OPS" 5.8 (ui-accent-cyan))
      (ui-lego-micro-num-s 0 "ratio_a" "ra" 5.8 2 false (ui-accent-cyan))
      (ui-lego-micro-num-s 0 "ratio_b" "rb" 5.8 2 false (ui-accent-cyan))
      (ui-lego-micro-num-s 0 "ratio_c" "rc" 5.8 2 false (ui-accent-cyan))
      (ui-lego-micro-num-s 0 "ratio_d" "rd" 5.8 2 false (ui-accent-cyan)))))

(def mfm-index-strip ()
  (ui-lego-strip-panel-s 0
    (v-stack :width :fill :gap 0.08 :align :center
      (ui-lego-badge-s 0 "FM" 5.8 (ui-accent-orange))
      (ui-lego-micro-num-s 0 "index_a" "ia" 5.8 2 false (ui-accent-orange))
      (ui-lego-micro-num-s 0 "index_b" "ib" 5.8 2 false (ui-accent-orange))
      (ui-lego-micro-num-s 0 "feedback_a" "fba" 5.8 2 false (ui-accent-violet))
      (ui-lego-micro-num-s 0 "feedback_b" "fbb" 5.8 2 false (ui-accent-violet)))))

(defsynth-ui
  (h-stack :width :fill :gap 0.30 :align :stretch
    (ui-lego-column
      (mfm-ratio-block)
      (mfm-index-block)
      (mfm-feedback-block))
    (mfm-detail-column)
    (ui-lego-column
      (mfm-tone-block)
      (mfm-glitch-block)
      (mfm-global-block))
    (h-stack :width 14.7 :gap 0.30 :align :stretch
      (mfm-ratio-strip)
      (mfm-index-strip))))
