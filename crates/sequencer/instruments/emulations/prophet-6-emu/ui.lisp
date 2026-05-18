(def p6e-osc1-block ()
  (ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (ui-lego-badge-s 0 "OSC1" 3.6 (ui-accent-cyan))
          (ui-lego-micro-num-s 0 "osc1_shape" "shape" 4.4 2 false (ui-accent-cyan)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 0 "osc1_semitones" "semi" 3.3 0 "st" (ui-accent-cyan))
          (ui-lego-micro-num-s 0 "pulse_width" "pw" 3.3 2 false (ui-accent-blue))
          (ui-lego-micro-num-s 0 "brass" "brass" 3.3 2 false (ui-accent-orange))))
      (h-stack :gap 0.08 :align :start
        (ui-lego-knob-s 0 "osc1_shape" "shape" 3.7 (ui-accent-cyan) 2)
        (ui-lego-knob-s 0 "pulse_width" "pw" 3.7 (ui-accent-blue) 2)
        (ui-lego-knob-s 0 "brass" "brass" 3.7 (ui-accent-orange) 2)))))

(def p6e-osc2-block ()
  (ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (ui-lego-badge-s 0 "OSC2" 3.6 (ui-accent-blue))
          (ui-lego-micro-num-s 0 "osc2_shape" "shape" 4.4 2 false (ui-accent-blue)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 0 "osc2_semitones" "semi" 3.3 0 "st" (ui-accent-blue))
          (ui-lego-micro-num-s 0 "osc_detune_cents" "det" 3.3 0 "ct" (ui-accent-orange))
          (ui-lego-micro-num-s 0 "osc_slop" "slop" 3.3 2 false (ui-accent-orange))))
      (h-stack :gap 0.08 :align :start
        (ui-lego-knob-s 0 "osc2_shape" "shape" 3.7 (ui-accent-blue) 2)
        (ui-lego-knob-s 0 "osc_detune_cents" "det" 3.7 (ui-accent-orange) 0)
        (ui-lego-knob-s 0 "osc_mix" "mix" 3.7 (ui-accent-violet) 2)))))

(def p6e-mix-block ()
  (ui-control-panel-small-s 0
    (h-stack :gap 0.18 :align :start
      (ui-lego-badge-s 0 "MIX" 3.6 (ui-accent-violet))
      (ui-lego-micro-num-s 0 "osc_mix" "mix" 3.0 2 false (ui-accent-violet))
      (ui-lego-micro-num-s 0 "sub_level" "sub" 3.0 2 false (ui-accent-violet))
      (ui-lego-micro-num-s 0 "noise_level" "nz" 3.0 2 false (ui-accent-blue))
      (ui-lego-micro-num-s 0 "shape_drift" "drift" 3.0 2 false (ui-accent-orange)))))

(def p6e-filter-block ()
  (ui-control-panel-dense-s 1
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (ui-lego-badge-s 1 "FILT" 3.8 (ui-accent-green))
          (ui-lego-micro-num-s 1 "keytrack" "key" 4.4 2 false (ui-accent-green)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 1 "vel_to_filter" "vel" 3.1 2 false (ui-accent-green))
          (ui-lego-micro-num-s 1 "filter_env_amt" "env" 3.1 0 false (ui-accent-blue))))
      (h-stack :gap 0.08 :align :start
        (ui-lego-knob-s 1 "cutoff" "cut" 3.7 (ui-accent-green) 0)
        (ui-lego-knob-s 1 "resonance" "res" 3.7 (ui-accent-green) 2)
        (ui-lego-knob-s 1 "filter_env_amt" "env" 3.7 (ui-accent-blue) 0)))))

(def p6e-color-block ()
  (ui-control-panel-dense-s 1
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (ui-lego-badge-s 1 "COL" 3.8 (ui-accent-orange))
          (ui-lego-micro-num-s 1 "filter_tone" "tone" 4.4 2 false (ui-accent-orange)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 1 "filter_drive" "drive" 3.5 2 false (ui-accent-orange))
          (ui-lego-micro-num-s 1 "cutoff_skew" "skew" 3.4 2 false (ui-accent-blue))))
      (h-stack :gap 0.08 :align :start
        (ui-lego-knob-s 1 "filter_drive" "drive" 3.7 (ui-accent-orange) 2)
        (ui-lego-knob-s 1 "filter_tone" "tone" 3.7 (ui-accent-orange) 2)
        (ui-lego-knob-s 1 "cutoff_skew" "skew" 3.7 (ui-accent-blue) 2)))))

(def p6e-global-block ()
  (ui-control-panel-small-s 0
    (h-stack :gap 0.18 :align :start
      (ui-lego-badge-s 0 "GLB" 3.6 (ui-accent-orange))
      (ui-lego-micro-base-note-s 0 3.0 (ui-accent-orange))
      (ui-lego-micro-num-s 0 "stereo_spread" "spr" 3.1 2 false (ui-accent-violet))
      (ui-lego-micro-num-s 0 "gain" "gain" 3.0 2 false (ui-accent-orange))
      (ui-lego-micro-num-s 2 "vibrato" "vib" 3.0 2 false (ui-accent-blue)))))

(def p6e-mod-detail ()
  (ui-readout-panel-medium-s 2
    (h-stack :width :fill :height :fill :gap 0.24 :align :stretch
      (box :width 11.8 :height :fill
           :background-color :instrument-control-bg
           :border-width 1 :corner-radius 7 :padding 0.16
           :h-align :center :v-align :center
        (v-stack :gap 0.20 :align :center
          (label "LFO" :font-size 13.0 :color :dim :bg :transparent)
          (label "pitch/filter/pw" :font-size 9.0 :color :dim :bg :transparent)))
      (v-stack :width 10.0 :height :fill :gap 0.12 :align :start
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 2 "lfo_rate_hz" "rate" 4.6 2 "Hz" (ui-accent-blue))
          (ui-lego-micro-num-s 2 "vibrato" "vib" 4.6 2 false (ui-accent-blue)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 2 "lfo_to_pw" "pw" 4.6 2 false (ui-accent-cyan))
          (ui-lego-micro-num-s 2 "lfo_to_cutoff" "cut" 4.6 0 "Hz" (ui-accent-green)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 2 "env_to_pitch" "envp" 4.6 2 "st" (ui-accent-orange))
          (ui-lego-micro-num-s 0 "stereo_spread" "spr" 4.6 2 false (ui-accent-violet)))))))

(def p6e-env-detail ()
  (ui-detail-adsr-switch-s
    0 "AMP" "amp_attack_ms" "amp_decay_ms" "amp_sustain" "amp_release_ms"
    1 "FILTER" "filt_attack_ms" "filt_decay_ms" "filt_sustain" "filt_release_ms"))

(def p6e-detail-main ()
  (if (= custom-ui-selected-section 2)
    (p6e-mod-detail)
    (p6e-env-detail)))

(def p6e-detail-column ()
  (v-stack :width (ui-lego-col-w) :gap (ui-lego-gap)
    (ui-control-panel-small-s 2 (box :width :fill :height :fill))
    (p6e-detail-main)
    (ui-control-panel-small-s 0
      (h-stack :gap 0.18 :align :start
        (ui-lego-badge-s 0 "OUT" 3.6 (ui-accent-orange))
        (ui-lego-micro-num-s 0 "stereo_spread" "spread" 4.1 2 false (ui-accent-violet))
        (ui-lego-micro-num-s 0 "gain" "gain" 3.8 2 false (ui-accent-orange))))))

(def p6e-lfo-strip ()
  (ui-lego-strip-panel-s 2
    (v-stack :width :fill :gap 0.08 :align :center
      (ui-lego-badge-s 2 "LFO" 5.8 (ui-accent-blue))
      (ui-lego-micro-num-s 2 "lfo_rate_hz" "rate" 5.8 2 "Hz" (ui-accent-blue))
      (ui-lego-micro-num-s 2 "lfo_to_pw" "pw" 5.8 2 false (ui-accent-cyan))
      (ui-lego-micro-num-s 2 "lfo_to_cutoff" "cut" 5.8 0 "Hz" (ui-accent-green))
      (ui-lego-micro-num-s 2 "env_to_pitch" "env p" 5.8 2 "st" (ui-accent-orange))
      (ui-lego-micro-num-s 2 "vibrato" "vib" 5.8 2 false (ui-accent-blue)))))

(def p6e-performance-strip ()
  (ui-lego-strip-panel-s 0
    (v-stack :width :fill :gap 0.08 :align :center
      (ui-lego-badge-s 0 "PERF" 5.8 (ui-accent-violet))
      (ui-lego-micro-base-note-s 0 5.8 (ui-accent-orange))
      (ui-lego-micro-num-s 0 "osc_slop" "slop" 5.8 2 false (ui-accent-orange))
      (ui-lego-micro-num-s 0 "shape_drift" "drift" 5.8 2 false (ui-accent-orange))
      (ui-lego-micro-num-s 0 "stereo_spread" "spread" 5.8 2 false (ui-accent-violet))
      (ui-lego-micro-num-s 0 "gain" "gain" 5.8 2 false (ui-accent-orange)))))

(defsynth-ui
  (h-stack :width :fill :gap 0.30 :align :stretch
    (ui-lego-column
      (p6e-osc1-block)
      (p6e-osc2-block)
      (p6e-mix-block))
    (p6e-detail-column)
    (ui-lego-column
      (p6e-filter-block)
      (p6e-color-block)
      (p6e-global-block))
    (h-stack :width 14.7 :gap 0.30 :align :stretch
      (p6e-lfo-strip)
      (p6e-performance-strip))))
