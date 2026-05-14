(def analog-wave-options ()
  '("sine" "saw" "pulse" "tri"))

(def analog-filter-options ()
  '("LP12" "Lad24" "BP" "HP"))

(def analog-sub-options ()
  '("-2oct" "-1oct" "root"))

(def analog-osc1-block ()
  (ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (ui-lego-badge-s 0 "OSC1" 3.6 (ui-accent-cyan))
          (ui-lego-micro-option-s 0 "osc1_wave" "wave" 4.4 (analog-wave-options) (ui-accent-cyan)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 0 "osc1_octave" "oct" 2.5 0 false (ui-accent-cyan))
          (ui-lego-micro-num-s 0 "osc1_semitones" "semi" 3.3 0 "st" (ui-accent-cyan))
          (ui-lego-micro-num-s 0 "osc1_detune_cents" "det" 3.3 0 "ct" (ui-accent-orange))))
      (h-stack :gap 0.08 :align :start
        (ui-lego-knob-s 0 "osc1_level" "lvl" 3.7 (ui-accent-cyan) 2)
        (ui-lego-knob-s 0 "osc1_to_f2" "F2" 3.7 (ui-accent-green) 2)
        (ui-lego-knob-s 0 "osc1_pan" "pan" 3.7 (ui-accent-violet) 2)))))

(def analog-osc2-block ()
  (ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (ui-lego-badge-s 0 "OSC2" 3.6 (ui-accent-blue))
          (ui-lego-micro-option-s 0 "osc2_wave" "wave" 4.4 (analog-wave-options) (ui-accent-blue)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 0 "osc2_octave" "oct" 2.5 0 false (ui-accent-blue))
          (ui-lego-micro-num-s 0 "osc2_semitones" "semi" 3.3 0 "st" (ui-accent-blue))
          (ui-lego-micro-num-s 0 "osc2_detune_cents" "det" 3.3 0 "ct" (ui-accent-orange))))
      (h-stack :gap 0.08 :align :start
        (ui-lego-knob-s 0 "osc2_level" "lvl" 3.7 (ui-accent-blue) 2)
        (ui-lego-knob-s 0 "osc2_to_f2" "F2" 3.7 (ui-accent-green) 2)
        (ui-lego-knob-s 0 "pulse_width" "pw" 3.7 (ui-accent-cyan) 2)))))

(def analog-source-amp-block ()
  (ui-control-panel-small-s 0
    (h-stack :gap 0.18 :align :start
      (ui-lego-badge-s 0 "SRC" 3.6 (ui-accent-violet))
      (ui-lego-micro-option-s 0 "sub_mode" "sub" 4.2 (analog-sub-options) (ui-accent-violet))
      (ui-lego-micro-num-s 0 "sub_level" "lvl" 2.8 2 false (ui-accent-violet))
      (ui-lego-micro-num-s 0 "noise_level" "nz" 2.8 2 false (ui-accent-blue))
      (ui-lego-micro-num-s 0 "noise_to_f2" "F2" 2.8 2 false (ui-accent-green)))))

(def analog-filter1-block ()
  (ui-control-panel-dense-s 1
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (ui-lego-badge-s 1 "FIL1" 3.8 (ui-accent-green))
          (ui-lego-micro-option-s 1 "filt1_mode" "mode" 4.4 (analog-filter-options) (ui-accent-green)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 1 "filt1_keytrack" "key" 3.1 2 false (ui-accent-green))
          (ui-lego-micro-num-s 1 "filt1_drive" "drv" 3.1 2 false (ui-accent-orange))
          (ui-lego-micro-num-s 1 "f1_to_f2" "toF2" 3.4 2 false (ui-accent-green))))
      (h-stack :gap 0.08 :align :start
        (ui-lego-knob-s 1 "filt1_cutoff" "cut" 3.7 (ui-accent-green) 0)
        (ui-lego-knob-s 1 "filt1_resonance" "res" 3.7 (ui-accent-green) 2)
        (ui-lego-knob-s 1 "filt1_env_amt" "env" 3.7 (ui-accent-blue) 0)))))

(def analog-filter2-block ()
  (ui-control-panel-dense-s 1
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (ui-lego-badge-s 1 "FIL2" 3.8 (ui-accent-green))
          (ui-lego-micro-option-s 1 "filt2_mode" "mode" 4.4 (analog-filter-options) (ui-accent-green)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 1 "filt2_keytrack" "key" 3.1 2 false (ui-accent-green))
          (ui-lego-micro-num-s 1 "filt2_drive" "drv" 3.1 2 false (ui-accent-orange))
          (ui-lego-micro-num-s 1 "noise_color" "ncol" 3.4 2 false (ui-accent-blue))))
      (h-stack :gap 0.08 :align :start
        (ui-lego-knob-s 1 "filt2_cutoff" "cut" 3.7 (ui-accent-green) 0)
        (ui-lego-knob-s 1 "filt2_resonance" "res" 3.7 (ui-accent-green) 2)
        (ui-lego-knob-s 1 "filt2_env_amt" "env" 3.7 (ui-accent-blue) 0)))))

(def analog-global-block ()
  (ui-control-panel-small-s 0
    (h-stack :gap 0.18 :align :start
      (ui-lego-badge-s 0 "GLB" 3.6 (ui-accent-orange))
      (ui-lego-micro-base-note-s 0 3.0 (ui-accent-orange))
      (ui-lego-micro-num-s 0 "glide_ms" "gli" 3.0 0 "ms" (ui-accent-orange))
      (ui-lego-micro-num-s 0 "vibrato" "vib" 3.0 2 "st" (ui-accent-blue))
      (ui-lego-micro-num-s 0 "output_gain" "out" 3.0 2 false (ui-accent-orange)))))

(def analog-detail-tabs ()
  (ui-control-panel-small-s 2
    (box :width :fill :height :fill)))

(def analog-lfo-detail ()
  (ui-readout-panel-medium-s 2
    (h-stack :width :fill :height :fill :gap 0.24 :align :stretch
      (box :width 11.8 :height :fill
           :background-color :instrument-control-bg
           :border-width 1 :corner-radius 7 :padding 0.16
           :h-align :center :v-align :center
        (v-stack :gap 0.20 :align :center
          (label "LFO" :font-size 13.0 :color :dim :bg :transparent)
          (label "shape" :font-size 9.0 :color :dim :bg :transparent)))
      (v-stack :width 10.0 :height :fill :gap 0.12 :align :start
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-option-s 2 "lfo1_wave" "w1" 4.6 (analog-wave-options) (ui-accent-blue))
          (ui-lego-micro-option-s 2 "lfo2_wave" "w2" 4.6 (analog-wave-options) (ui-accent-violet)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 2 "lfo1_attack_ms" "a1" 4.6 0 "ms" (ui-accent-blue))
          (ui-lego-micro-num-s 2 "lfo2_attack_ms" "a2" 4.6 0 "ms" (ui-accent-violet)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 2 "lfo2_to_pan" "pan" 4.6 2 false (ui-accent-violet))
          (ui-lego-micro-num-s 2 "amp_vel_amt" "avel" 4.6 2 false (ui-accent-orange)))))))

(def analog-env-detail ()
  (ui-detail-adsr-switch-s
    0 "AMP" "amp_attack_ms" "amp_decay_ms" "amp_sustain" "amp_release_ms"
    1 "FILTER" "filt_attack_ms" "filt_decay_ms" "filt_sustain" "filt_release_ms"))

(def analog-detail-main ()
  (if (= custom-ui-selected-section 2)
    (analog-lfo-detail)
    (analog-env-detail)))

(def analog-amp-output-block ()
  (ui-control-panel-small-s 0
    (h-stack :gap 0.18 :align :start
      (ui-lego-badge-s 0 "AMP" 3.6 (ui-accent-orange))
      (ui-lego-micro-num-s 0 "amp1_level" "A1" 3.0 2 false (ui-accent-orange))
      (ui-lego-micro-num-s 0 "amp2_level" "A2" 3.0 2 false (ui-accent-orange))
      (ui-lego-micro-num-s 0 "amp1_pan" "P1" 3.0 2 false (ui-accent-violet))
      (ui-lego-micro-num-s 0 "amp2_pan" "P2" 3.0 2 false (ui-accent-violet)))))

(def analog-detail-column ()
  (v-stack :width (ui-lego-col-w) :gap (ui-lego-gap)
    (analog-detail-tabs)
    (analog-detail-main)
    (analog-amp-output-block)))

(def analog-lfo1-strip ()
  (ui-lego-strip-panel-s 2
    (v-stack :width :fill :gap 0.08 :align :center
      (ui-lego-badge-s 2 "LFO1" 5.8 (ui-accent-blue))
      (ui-lego-micro-option-s 2 "lfo1_wave" "wave" 5.8 (analog-wave-options) (ui-accent-blue))
      (ui-lego-micro-num-s 2 "lfo1_rate_hz" "rate" 5.8 2 "Hz" (ui-accent-blue))
      (ui-lego-micro-num-s 2 "lfo1_to_pitch" "pitch" 5.8 2 "st" (ui-accent-orange))
      (ui-lego-micro-num-s 2 "lfo1_to_pw" "pw" 5.8 2 false (ui-accent-cyan))
      (ui-lego-micro-num-s 2 "lfo1_width" "width" 5.8 2 false (ui-accent-blue)))))

(def analog-lfo2-strip ()
  (ui-lego-strip-panel-s 2
    (v-stack :width :fill :gap 0.08 :align :center
      (ui-lego-badge-s 2 "LFO2" 5.8 (ui-accent-violet))
      (ui-lego-micro-option-s 2 "lfo2_wave" "wave" 5.8 (analog-wave-options) (ui-accent-violet))
      (ui-lego-micro-num-s 2 "lfo2_rate_hz" "rate" 5.8 2 "Hz" (ui-accent-violet))
      (ui-lego-micro-num-s 2 "lfo2_to_f1" "F1" 5.8 0 "Hz" (ui-accent-green))
      (ui-lego-micro-num-s 2 "lfo2_to_f2" "F2" 5.8 0 "Hz" (ui-accent-green))
      (ui-lego-micro-num-s 2 "lfo2_width" "width" 5.8 2 false (ui-accent-violet)))))

(defsynth-ui
  (h-stack :width :fill :gap 0.30 :align :stretch
    (ui-lego-column
      (analog-osc1-block)
      (analog-osc2-block)
      (analog-source-amp-block))
    (analog-detail-column)
    (ui-lego-column
      (analog-filter1-block)
      (analog-filter2-block)
      (analog-global-block))
    (h-stack :width 14.7 :gap 0.30 :align :stretch
      (analog-lfo1-strip)
      (analog-lfo2-strip))))
