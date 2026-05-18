(def p6-osc1-block ()
  (ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (ui-lego-badge-s 0 "OSC1" 3.6 (ui-accent-cyan))
          (ui-lego-micro-num-s 0 "osc1_shape" "shape" 4.4 2 false (ui-accent-cyan)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 0 "osc1_pw" "pw" 3.3 2 false (ui-accent-blue))
          (ui-lego-micro-num-s 0 "osc1_mix" "mix" 3.3 2 false (ui-accent-violet))))
      (h-stack :gap 0.08 :align :start
        (ui-lego-knob-s 0 "osc1_shape" "shape" 3.7 (ui-accent-cyan) 2)
        (ui-lego-knob-s 0 "osc1_pw" "pw" 3.7 (ui-accent-blue) 2)
        (ui-lego-knob-s 0 "osc1_mix" "mix" 3.7 (ui-accent-violet) 2)))))

(def p6-osc2-block ()
  (ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (ui-lego-badge-s 0 "OSC2" 3.6 (ui-accent-blue))
          (ui-lego-micro-num-s 0 "osc2_shape" "shape" 4.4 2 false (ui-accent-blue)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 0 "osc2_detune" "det" 3.3 0 "st" (ui-accent-orange))
          (ui-lego-micro-num-s 0 "osc2_fine" "fine" 3.3 0 "ct" (ui-accent-orange))))
      (h-stack :gap 0.08 :align :start
        (ui-lego-knob-s 0 "osc2_shape" "shape" 3.7 (ui-accent-blue) 2)
        (ui-lego-knob-s 0 "osc2_pw" "pw" 3.7 (ui-accent-cyan) 2)
        (ui-lego-knob-s 0 "osc2_mix" "mix" 3.7 (ui-accent-violet) 2)))))

(def p6-mix-block ()
  (ui-control-panel-small-s 0
    (h-stack :gap 0.18 :align :start
      (ui-lego-badge-s 0 "MIX" 3.6 (ui-accent-violet))
      (ui-lego-micro-num-s 0 "sub_mix" "sub" 3.0 2 false (ui-accent-violet))
      (ui-lego-micro-num-s 0 "noise_mix" "nz" 3.0 2 false (ui-accent-blue))
      (ui-lego-micro-num-s 0 "drift" "drift" 3.0 2 false (ui-accent-orange))
      (ui-lego-micro-num-s 0 "drive" "drv" 3.0 2 false (ui-accent-orange)))))

(def p6-filter-block ()
  (ui-control-panel-dense-s 1
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (ui-lego-badge-s 1 "FILT" 3.8 (ui-accent-green))
          (ui-lego-micro-num-s 1 "keytrack" "key" 4.4 2 false (ui-accent-green)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 1 "filter_env_amt" "env" 3.8 0 false (ui-accent-blue))))
      (h-stack :gap 0.08 :align :start
        (ui-lego-knob-s 1 "cutoff" "cut" 3.7 (ui-accent-green) 0)
        (ui-lego-knob-s 1 "resonance" "res" 3.7 (ui-accent-green) 2)
        (ui-lego-knob-s 1 "filter_env_amt" "env" 3.7 (ui-accent-blue) 0)))))

(def p6-mod-block ()
  (ui-control-panel-dense-s 2
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (ui-lego-badge-s 2 "MOD" 3.8 (ui-accent-blue))
          (ui-lego-micro-num-s 2 "lfo_rate" "rate" 4.4 2 "Hz" (ui-accent-blue)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 2 "vibrato_amt" "vib" 3.5 2 false (ui-accent-blue))
          (ui-lego-micro-num-s 2 "lfo_shape_amt" "shape" 3.5 2 false (ui-accent-cyan))))
      (h-stack :gap 0.08 :align :start
        (ui-lego-knob-s 2 "lfo_rate" "rate" 3.7 (ui-accent-blue) 2)
        (ui-lego-knob-s 2 "lfo_pitch_amt" "pitch" 3.7 (ui-accent-orange) 2)
        (ui-lego-knob-s 2 "vibrato_amt" "vib" 3.7 (ui-accent-blue) 2)))))

(def p6-global-block ()
  (ui-control-panel-small-s 0
    (h-stack :gap 0.18 :align :start
      (ui-lego-badge-s 0 "GLB" 3.6 (ui-accent-orange))
      (ui-lego-micro-base-note-s 0 3.0 (ui-accent-orange))
      (ui-lego-micro-num-s 0 "gain" "gain" 3.0 2 false (ui-accent-orange))
      (ui-lego-micro-num-s 0 "drive" "drive" 3.0 2 false (ui-accent-orange))
      (ui-lego-micro-num-s 0 "drift" "drift" 3.0 2 false (ui-accent-orange)))))

(def p6-env-detail ()
  (ui-detail-adsr-switch-s
    0 "AMP" "amp_attack_ms" "amp_decay_ms" "amp_sustain" "amp_release_ms"
    1 "FILTER" "filt_attack_ms" "filt_decay_ms" "filt_sustain" "filt_release_ms"))

(def p6-detail-column ()
  (v-stack :width (ui-lego-col-w) :gap (ui-lego-gap)
    (ui-control-panel-small-s 2 (box :width :fill :height :fill))
    (p6-env-detail)
    (ui-control-panel-small-s 0
      (h-stack :gap 0.18 :align :start
        (ui-lego-badge-s 0 "OUT" 3.6 (ui-accent-orange))
        (ui-lego-micro-base-note-s 0 3.0 (ui-accent-orange))
        (ui-lego-micro-num-s 0 "gain" "gain" 3.8 2 false (ui-accent-orange))))))

(def p6-lfo-strip ()
  (ui-lego-strip-panel-s 2
    (v-stack :width :fill :gap 0.08 :align :center
      (ui-lego-badge-s 2 "LFO" 5.8 (ui-accent-blue))
      (ui-lego-micro-num-s 2 "lfo_rate" "rate" 5.8 2 "Hz" (ui-accent-blue))
      (ui-lego-micro-num-s 2 "lfo_pitch_amt" "pitch" 5.8 2 false (ui-accent-orange))
      (ui-lego-micro-num-s 2 "lfo_shape_amt" "shape" 5.8 2 false (ui-accent-cyan))
      (ui-lego-micro-num-s 2 "vibrato_amt" "vib" 5.8 2 false (ui-accent-blue)))))

(def p6-performance-strip ()
  (ui-lego-strip-panel-s 0
    (v-stack :width :fill :gap 0.08 :align :center
      (ui-lego-badge-s 0 "PERF" 5.8 (ui-accent-violet))
      (ui-lego-micro-base-note-s 0 5.8 (ui-accent-orange))
      (ui-lego-micro-num-s 0 "drift" "drift" 5.8 2 false (ui-accent-orange))
      (ui-lego-micro-num-s 0 "drive" "drive" 5.8 2 false (ui-accent-orange))
      (ui-lego-micro-num-s 0 "gain" "gain" 5.8 2 false (ui-accent-orange)))))

(defsynth-ui
  (h-stack :width :fill :gap 0.30 :align :stretch
    (ui-lego-column
      (p6-osc1-block)
      (p6-osc2-block)
      (p6-mix-block))
    (p6-detail-column)
    (ui-lego-column
      (p6-filter-block)
      (p6-mod-block)
      (p6-global-block))
    (h-stack :width 14.7 :gap 0.30 :align :stretch
      (p6-lfo-strip)
      (p6-performance-strip))))
