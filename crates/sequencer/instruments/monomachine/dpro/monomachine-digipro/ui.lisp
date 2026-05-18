(def mdigi-wave-block ()
  (ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (ui-lego-badge-s 0 "DIGI" 3.6 (ui-accent-cyan))
          (ui-lego-micro-num-s 0 "morph" "morph" 4.4 2 false (ui-accent-cyan)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 0 "table_jump" "jump" 3.3 2 false (ui-accent-blue))
          (ui-lego-micro-num-s 0 "detune_cents" "det" 3.3 0 "ct" (ui-accent-orange))
          (ui-lego-micro-num-s 0 "unison" "uni" 3.3 2 false (ui-accent-violet))))
      (h-stack :gap 0.08 :align :start
        (ui-lego-knob-s 0 "morph" "morph" 3.7 (ui-accent-cyan) 2)
        (ui-lego-knob-s 0 "shape" "shape" 3.7 (ui-accent-blue) 2)
        (ui-lego-knob-s 0 "formant" "form" 3.7 (ui-accent-violet) 2)))))

(def mdigi-texture-block ()
  (ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (ui-lego-badge-s 0 "TEX" 3.6 (ui-accent-blue))
          (ui-lego-micro-num-s 0 "alias" "alias" 4.4 2 false (ui-accent-blue)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 0 "sync_amt" "sync" 3.3 2 false (ui-accent-cyan))
          (ui-lego-micro-num-s 0 "noise_level" "noise" 3.3 2 false (ui-accent-blue))
          (ui-lego-micro-num-s 0 "vowel_amt" "vowel" 3.3 2 false (ui-accent-violet))))
      (h-stack :gap 0.08 :align :start
        (ui-lego-knob-s 0 "phase_distort" "phase" 3.7 (ui-accent-blue) 2)
        (ui-lego-knob-s 0 "comb_amt" "comb" 3.7 (ui-accent-cyan) 2)
        (ui-lego-knob-s 0 "comb_time" "time" 3.7 (ui-accent-cyan) 2)))))

(def mdigi-filter-block ()
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

(def mdigi-global-block ()
  (ui-control-panel-small-s 0
    (h-stack :gap 0.18 :align :start
      (ui-lego-badge-s 0 "GLB" 3.6 (ui-accent-orange))
      (ui-lego-micro-base-note-s 0 3.0 (ui-accent-orange))
      (ui-lego-micro-num-s 0 "gain" "gain" 3.0 2 false (ui-accent-orange))
      (ui-lego-micro-num-s 0 "drive" "drive" 3.0 2 false (ui-accent-orange))
      (ui-lego-micro-num-s 0 "unison" "uni" 3.0 2 false (ui-accent-violet)))))

(def mdigi-detail-column ()
  (v-stack :width (ui-lego-col-w) :gap (ui-lego-gap)
    (ui-control-panel-small-s 0 (box :width :fill :height :fill))
    (ui-detail-adsr-s 0 "AMP" "amp_attack_ms" "amp_decay_ms" "amp_sustain" "amp_release_ms")
    (ui-control-panel-small-s 0
      (h-stack :gap 0.18 :align :start
        (ui-lego-badge-s 0 "OUT" 3.6 (ui-accent-orange))
        (ui-lego-micro-base-note-s 0 3.0 (ui-accent-orange))
        (ui-lego-micro-num-s 0 "gain" "gain" 3.8 2 false (ui-accent-orange))))))

(def mdigi-source-strip ()
  (ui-lego-strip-panel-s 0
    (v-stack :width :fill :gap 0.08 :align :center
      (ui-lego-badge-s 0 "DPRO" 5.8 (ui-accent-cyan))
      (ui-lego-micro-num-s 0 "morph" "morph" 5.8 2 false (ui-accent-cyan))
      (ui-lego-micro-num-s 0 "shape" "shape" 5.8 2 false (ui-accent-blue))
      (ui-lego-micro-num-s 0 "formant" "form" 5.8 2 false (ui-accent-violet))
      (ui-lego-micro-num-s 0 "table_jump" "jump" 5.8 2 false (ui-accent-blue))
      (ui-lego-micro-num-s 0 "vowel_amt" "vowel" 5.8 2 false (ui-accent-violet)))))

(def mdigi-filter-strip ()
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
      (mdigi-wave-block)
      (mdigi-texture-block)
      (mdigi-global-block))
    (mdigi-detail-column)
    (ui-lego-column
      (mdigi-filter-block)
      (ui-control-panel-small-s 0
        (h-stack :gap 0.18 :align :start
          (ui-lego-badge-s 0 "FX" 3.6 (ui-accent-blue))
          (ui-lego-micro-num-s 0 "alias" "alias" 3.2 2 false (ui-accent-blue))
          (ui-lego-micro-num-s 0 "sync_amt" "sync" 3.2 2 false (ui-accent-cyan))
          (ui-lego-micro-num-s 0 "noise_level" "noise" 3.2 2 false (ui-accent-blue))))
      (ui-control-panel-small-s 0
        (h-stack :gap 0.18 :align :start
          (ui-lego-badge-s 0 "VOICE" 4.2 (ui-accent-violet))
          (ui-lego-micro-num-s 0 "detune_cents" "det" 3.0 0 "ct" (ui-accent-orange))
          (ui-lego-micro-num-s 0 "unison" "uni" 3.0 2 false (ui-accent-violet)))))
    (h-stack :width 14.7 :gap 0.30 :align :stretch
      (mdigi-source-strip)
      (mdigi-filter-strip))))
