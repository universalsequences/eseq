(def mdp2-sync-options ()
  '("off" "soft" "hard"))

(def mdp2-wave-block ()
  (ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (ui-lego-badge-s 0 "WAVE" 3.8 (ui-accent-cyan))
          (ui-lego-micro-num-s 0 "wave" "wave" 4.4 0 false (ui-accent-cyan)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-option-s 0 "sync_mode" "sync" 3.5 (mdp2-sync-options) (ui-accent-blue))
          (ui-lego-micro-num-s 0 "tune_cents" "tune" 3.5 0 "ct" (ui-accent-orange))))
      (h-stack :gap 0.08 :align :start
        (ui-lego-knob-s 0 "wave" "wave" 3.7 (ui-accent-cyan) 0)
        (ui-lego-knob-s 0 "wp" "wp" 3.7 (ui-accent-blue) 2)
        (ui-lego-knob-s 0 "sfrq" "sfrq" 3.7 (ui-accent-violet) 0)))))

(def mdp2-filter-block ()
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

(def mdp2-global-block ()
  (ui-control-panel-small-s 0
    (h-stack :gap 0.18 :align :start
      (ui-lego-badge-s 0 "GLB" 3.6 (ui-accent-orange))
      (ui-lego-micro-base-note-s 0 3.0 (ui-accent-orange))
      (ui-lego-micro-num-s 0 "tune_cents" "tune" 3.2 0 "ct" (ui-accent-orange))
      (ui-lego-micro-num-s 0 "gain" "gain" 3.0 2 false (ui-accent-orange)))))

(def mdp2-detail-column ()
  (v-stack :width (ui-lego-col-w) :gap (ui-lego-gap)
    (ui-control-panel-small-s 0 (box :width :fill :height :fill))
    (ui-detail-adsr-switch-s
      0 "AMP" "amp_attack_ms" "amp_decay_ms" "amp_sustain" "amp_release_ms"
      1 "FILTER" "filter_attack_ms" "filter_decay_ms" "filter_sustain" "filter_release_ms")
    (mdp2-global-block)))

(def mdp2-wave-strip ()
  (ui-lego-strip-panel-s 0
    (v-stack :width :fill :gap 0.08 :align :center
      (ui-lego-badge-s 0 "WAVE" 5.8 (ui-accent-cyan))
      (ui-lego-micro-num-s 0 "wave" "wave" 5.8 0 false (ui-accent-cyan))
      (ui-lego-micro-num-s 0 "wp" "wp" 5.8 2 false (ui-accent-blue))
      (ui-lego-micro-option-s 0 "sync_mode" "sync" 5.8 (mdp2-sync-options) (ui-accent-blue))
      (ui-lego-micro-num-s 0 "sfrq" "sfrq" 5.8 0 false (ui-accent-violet)))))

(def mdp2-filter-strip ()
  (ui-lego-strip-panel-s 1
    (v-stack :width :fill :gap 0.08 :align :center
      (ui-lego-badge-s 1 "FILT" 5.8 (ui-accent-green))
      (ui-lego-micro-num-s 1 "cutoff" "cut" 5.8 0 false (ui-accent-green))
      (ui-lego-micro-num-s 1 "resonance" "res" 5.8 2 false (ui-accent-green))
      (ui-lego-micro-num-s 1 "filter_env_amt" "env" 5.8 0 false (ui-accent-blue))
      (ui-lego-micro-num-s 1 "drive" "drive" 5.8 2 false (ui-accent-orange)))))

(defsynth-ui
  (h-stack :width :fill :gap 0.30 :align :stretch
    (ui-lego-column
      (mdp2-wave-block)
      (mdp2-filter-block)
      (mdp2-global-block))
    (mdp2-detail-column)
    (ui-lego-column
      (ui-control-panel-small-s 0
        (h-stack :gap 0.18 :align :start
          (ui-lego-badge-s 0 "SYNC" 3.8 (ui-accent-blue))
          (ui-lego-micro-option-s 0 "sync_mode" "mode" 4.0 (mdp2-sync-options) (ui-accent-blue))
          (ui-lego-micro-num-s 0 "sfrq" "frq" 3.2 0 false (ui-accent-violet))))
      (ui-control-panel-small-s 1
        (h-stack :gap 0.18 :align :start
          (ui-lego-badge-s 1 "OUT" 3.6 (ui-accent-orange))
          (ui-lego-micro-num-s 1 "drive" "drive" 3.2 2 false (ui-accent-orange))
          (ui-lego-micro-num-s 0 "gain" "gain" 3.2 2 false (ui-accent-orange))))
      (ui-readout-panel-small-s 0
        (h-stack :gap 0.34 :align :start
          (label "dpro" :font-size 9.0 :color (ui-accent-cyan) :bg :transparent)
          (label "wave" :font-size 9.0 :color (ui-accent-blue) :bg :transparent)
          (label "filter" :font-size 9.0 :color (ui-accent-green) :bg :transparent))))
    (h-stack :width 14.7 :gap 0.30 :align :stretch
      (mdp2-wave-strip)
      (mdp2-filter-strip))))
