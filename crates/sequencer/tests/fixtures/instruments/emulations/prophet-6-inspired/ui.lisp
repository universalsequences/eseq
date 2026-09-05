(def p6i-osc1-block ()
  (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "OSC1" 3.6 (eseq.effects.custom-ui-lego/ui-accent-cyan))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "osc1_shape" "shape" 4.4 2 false (eseq.effects.custom-ui-lego/ui-accent-cyan)))
        (h-stack :gap 0.18 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "osc1_semitones" "semi" 3.3 0 "st" (eseq.effects.custom-ui-lego/ui-accent-cyan))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "pulse_width" "pw" 3.3 2 false (eseq.effects.custom-ui-lego/ui-accent-blue))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "brass" "brass" 3.3 2 false (eseq.effects.custom-ui-lego/ui-accent-orange))))
      (h-stack :gap 0.08 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "osc1_shape" "shape" 3.7 (eseq.effects.custom-ui-lego/ui-accent-cyan) 2)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "pulse_width" "pw" 3.7 (eseq.effects.custom-ui-lego/ui-accent-blue) 2)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "brass" "brass" 3.7 (eseq.effects.custom-ui-lego/ui-accent-orange) 2)))))

(def p6i-osc2-block ()
  (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "OSC2" 3.6 (eseq.effects.custom-ui-lego/ui-accent-blue))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "osc2_shape" "shape" 4.4 2 false (eseq.effects.custom-ui-lego/ui-accent-blue)))
        (h-stack :gap 0.18 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "osc2_semitones" "semi" 3.3 0 "st" (eseq.effects.custom-ui-lego/ui-accent-blue))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "osc_detune_cents" "det" 3.3 0 "ct" (eseq.effects.custom-ui-lego/ui-accent-orange))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "pulse_width" "pw" 3.3 2 false (eseq.effects.custom-ui-lego/ui-accent-cyan))))
      (h-stack :gap 0.08 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "osc2_shape" "shape" 3.7 (eseq.effects.custom-ui-lego/ui-accent-blue) 2)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "osc_detune_cents" "det" 3.7 (eseq.effects.custom-ui-lego/ui-accent-orange) 0)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "osc_mix" "mix" 3.7 (eseq.effects.custom-ui-lego/ui-accent-violet) 2)))))

(def p6i-mix-block ()
  (eseq.effects.custom-ui-lego/ui-control-panel-small-s 0
    (h-stack :gap 0.18 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "MIX" 3.6 (eseq.effects.custom-ui-lego/ui-accent-violet))
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "osc_mix" "mix" 3.0 2 false (eseq.effects.custom-ui-lego/ui-accent-violet))
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "sub_level" "sub" 3.0 2 false (eseq.effects.custom-ui-lego/ui-accent-violet))
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "noise_level" "nz" 3.0 2 false (eseq.effects.custom-ui-lego/ui-accent-blue))
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "osc_slop" "slop" 3.0 2 false (eseq.effects.custom-ui-lego/ui-accent-orange))
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "shape_drift" "drift" 3.0 2 false (eseq.effects.custom-ui-lego/ui-accent-orange)))))

(def p6i-filter-block ()
  (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 1
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-badge-s 1 "FILT" 3.8 (eseq.effects.custom-ui-lego/ui-accent-green))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 1 "keytrack" "key" 4.4 2 false (eseq.effects.custom-ui-lego/ui-accent-green)))
        (h-stack :gap 0.18 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 1 "vel_to_filter" "vel" 3.1 2 false (eseq.effects.custom-ui-lego/ui-accent-green))))
      (h-stack :gap 0.08 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "cutoff" "cut" 3.7 (eseq.effects.custom-ui-lego/ui-accent-green) 0)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "resonance" "res" 3.7 (eseq.effects.custom-ui-lego/ui-accent-green) 2)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "filter_env_amt" "env" 3.7 (eseq.effects.custom-ui-lego/ui-accent-blue) 0)))))

(def p6i-color-block ()
  (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 1
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-badge-s 1 "COL" 3.8 (eseq.effects.custom-ui-lego/ui-accent-orange))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 1 "filter_tone" "tone" 4.4 2 false (eseq.effects.custom-ui-lego/ui-accent-orange)))
        (h-stack :gap 0.18 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 1 "filter_drive" "drive" 3.5 2 false (eseq.effects.custom-ui-lego/ui-accent-orange))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 1 "cutoff_skew" "skew" 3.4 2 false (eseq.effects.custom-ui-lego/ui-accent-blue))))
      (h-stack :gap 0.08 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "filter_drive" "drive" 3.7 (eseq.effects.custom-ui-lego/ui-accent-orange) 2)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "filter_tone" "tone" 3.7 (eseq.effects.custom-ui-lego/ui-accent-orange) 2)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "cutoff_skew" "skew" 3.7 (eseq.effects.custom-ui-lego/ui-accent-blue) 2)))))

(def p6i-global-block ()
  (eseq.effects.custom-ui-lego/ui-control-panel-small-s 0
    (h-stack :gap 0.18 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "GLB" 3.6 (eseq.effects.custom-ui-lego/ui-accent-orange))
      (eseq.effects.custom-ui-lego/ui-lego-micro-base-note-s 0 3.0 (eseq.effects.custom-ui-lego/ui-accent-orange))
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "stereo_spread" "spr" 3.1 2 false (eseq.effects.custom-ui-lego/ui-accent-violet))
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "gain" "gain" 3.0 2 false (eseq.effects.custom-ui-lego/ui-accent-orange))
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 1 "vibrato" "vib" 3.0 2 false (eseq.effects.custom-ui-lego/ui-accent-blue)))))

(def p6i-detail-tabs ()
  (eseq.effects.custom-ui-lego/ui-control-panel-small-s 2
    (box :width :fill :height :fill)))

(def p6i-mod-detail ()
  (eseq.effects.custom-ui-lego/ui-readout-panel-medium-s 2
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
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 2 "lfo_rate_hz" "rate" 4.6 2 "Hz" (eseq.effects.custom-ui-lego/ui-accent-blue))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 2 "vibrato" "vib" 4.6 2 false (eseq.effects.custom-ui-lego/ui-accent-blue)))
        (h-stack :gap 0.18 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 2 "lfo_to_pw" "pw" 4.6 2 false (eseq.effects.custom-ui-lego/ui-accent-cyan))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 2 "lfo_to_cutoff" "cut" 4.6 0 "Hz" (eseq.effects.custom-ui-lego/ui-accent-green)))
        (h-stack :gap 0.18 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 2 "env_to_pitch" "envp" 4.6 2 "st" (eseq.effects.custom-ui-lego/ui-accent-orange))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "stereo_spread" "spr" 4.6 2 false (eseq.effects.custom-ui-lego/ui-accent-violet)))))))

(def p6i-env-detail ()
  (eseq.effects.custom-ui-lego/ui-detail-adsr-switch-s
    0 "AMP" "amp_attack_ms" "amp_decay_ms" "amp_sustain" "amp_release_ms"
    1 "FILTER" "filt_attack_ms" "filt_decay_ms" "filt_sustain" "filt_release_ms"))

(def p6i-detail-main ()
  (if (= custom-ui-selected-section 2)
    (p6i-mod-detail)
    (p6i-env-detail)))

(def p6i-out-block ()
  (eseq.effects.custom-ui-lego/ui-control-panel-small-s 0
    (h-stack :gap 0.18 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "OUT" 3.6 (eseq.effects.custom-ui-lego/ui-accent-orange))
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "stereo_spread" "spread" 4.1 2 false (eseq.effects.custom-ui-lego/ui-accent-violet))
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "gain" "gain" 3.8 2 false (eseq.effects.custom-ui-lego/ui-accent-orange)))))

(def p6i-detail-column ()
  (v-stack :width (eseq.effects.custom-ui-lego/ui-lego-col-w) :gap (eseq.effects.custom-ui-lego/ui-lego-gap)
    (p6i-detail-tabs)
    (p6i-detail-main)
    (p6i-out-block)))

(def p6i-lfo-strip ()
  (eseq.effects.custom-ui-lego/ui-lego-strip-panel-s 2
    (v-stack :width :fill :gap 0.08 :align :center
      (eseq.effects.custom-ui-lego/ui-lego-badge-s 2 "LFO" 5.8 (eseq.effects.custom-ui-lego/ui-accent-blue))
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 2 "lfo_rate_hz" "rate" 5.8 2 "Hz" (eseq.effects.custom-ui-lego/ui-accent-blue))
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 2 "lfo_to_pw" "pw" 5.8 2 false (eseq.effects.custom-ui-lego/ui-accent-cyan))
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 2 "lfo_to_cutoff" "cut" 5.8 0 "Hz" (eseq.effects.custom-ui-lego/ui-accent-green))
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 2 "env_to_pitch" "env p" 5.8 2 "st" (eseq.effects.custom-ui-lego/ui-accent-orange))
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 2 "vibrato" "vib" 5.8 2 false (eseq.effects.custom-ui-lego/ui-accent-blue)))))

(def p6i-performance-strip ()
  (eseq.effects.custom-ui-lego/ui-lego-strip-panel-s 0
    (v-stack :width :fill :gap 0.08 :align :center
      (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "PERF" 5.8 (eseq.effects.custom-ui-lego/ui-accent-violet))
      (eseq.effects.custom-ui-lego/ui-lego-micro-base-note-s 0 5.8 (eseq.effects.custom-ui-lego/ui-accent-orange))
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "osc_slop" "slop" 5.8 2 false (eseq.effects.custom-ui-lego/ui-accent-orange))
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "shape_drift" "drift" 5.8 2 false (eseq.effects.custom-ui-lego/ui-accent-orange))
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "stereo_spread" "spread" 5.8 2 false (eseq.effects.custom-ui-lego/ui-accent-violet))
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "gain" "gain" 5.8 2 false (eseq.effects.custom-ui-lego/ui-accent-orange)))))

(defsynth-ui
  (h-stack :width :fill :gap 0.30 :align :stretch
    (eseq.effects.custom-ui-lego/ui-lego-column
      (p6i-osc1-block)
      (p6i-osc2-block)
      (p6i-mix-block))
    (p6i-detail-column)
    (eseq.effects.custom-ui-lego/ui-lego-column
      (p6i-filter-block)
      (p6i-color-block)
      (p6i-global-block))
    (h-stack :width 14.7 :gap 0.30 :align :stretch
      (p6i-lfo-strip)
      (p6i-performance-strip))))
