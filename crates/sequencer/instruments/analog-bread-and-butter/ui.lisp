(def analog-wave-options ()
  '("sine" "saw" "pulse" "tri"))

(def analog-filter-options ()
  '("LP12" "Lad24" "BP" "HP"))

(def analog-sub-options ()
  '("-2oct" "-1oct" "root"))

(def analog-osc1-block ()
  (ui-control-block-medium-s "OSC1" (ui-accent-cyan) 0
    (h-stack :gap 0.24 :align :start
      (ui-lego-option-s 0 "osc1_wave" "wave" 5.0 (analog-wave-options) (ui-accent-cyan))
      (ui-lego-num-s 0 "osc1_octave" "oct" 3.4 0 false (ui-accent-cyan))
      (ui-lego-num-s 0 "osc1_semitones" "semi" 3.7 0 "st" (ui-accent-cyan))
      (ui-lego-num-s 0 "osc1_detune_cents" "det" 3.7 0 "ct" (ui-accent-orange)))))

(def analog-osc1-mix-block ()
  (ui-readout-block-small-s "OSC1 MIX" (ui-accent-cyan) 0
    (h-stack :gap 0.24 :align :start
      (ui-lego-num-s 0 "osc1_level" "lvl" 3.8 2 false (ui-accent-cyan))
      (ui-lego-num-s 0 "osc1_to_f2" "F2" 3.8 2 false (ui-accent-green))
      (ui-lego-num-s 0 "osc1_pan" "pan" 3.8 2 false (ui-accent-violet)))))

(def analog-osc2-block ()
  (ui-control-block-medium-s "OSC2" (ui-accent-blue) 0
    (h-stack :gap 0.24 :align :start
      (ui-lego-option-s 0 "osc2_wave" "wave" 5.0 (analog-wave-options) (ui-accent-blue))
      (ui-lego-num-s 0 "osc2_octave" "oct" 3.4 0 false (ui-accent-blue))
      (ui-lego-num-s 0 "osc2_semitones" "semi" 3.7 0 "st" (ui-accent-blue))
      (ui-lego-num-s 0 "osc2_detune_cents" "det" 3.7 0 "ct" (ui-accent-orange)))))

(def analog-osc2-mix-block ()
  (ui-readout-block-small-s "OSC2 MIX" (ui-accent-blue) 0
    (h-stack :gap 0.24 :align :start
      (ui-lego-num-s 0 "osc2_level" "lvl" 3.8 2 false (ui-accent-blue))
      (ui-lego-num-s 0 "osc2_to_f2" "F2" 3.8 2 false (ui-accent-green))
      (ui-lego-num-s 0 "osc2_pan" "pan" 3.8 2 false (ui-accent-violet)))))

(def analog-source-block ()
  (ui-readout-block-small-s "SUB/NOISE" (ui-accent-violet) 0
    (h-stack :gap 0.20 :align :start
      (ui-lego-option-s 0 "sub_mode" "sub" 4.6 (analog-sub-options) (ui-accent-violet))
      (ui-lego-num-s 0 "sub_level" "lvl" 3.4 2 false (ui-accent-violet))
      (ui-lego-num-s 0 "noise_level" "nz" 3.4 2 false (ui-accent-blue))
      (ui-lego-num-s 0 "noise_to_f2" "F2" 3.4 2 false (ui-accent-green)))))

(def analog-filter1-block ()
  (ui-control-block-medium-s "FILTER1" (ui-accent-green) 1
    (h-stack :gap 0.22 :align :start
      (ui-lego-option-s 1 "filt1_mode" "mode" 5.1 (analog-filter-options) (ui-accent-green))
      (ui-lego-knob-s 1 "filt1_cutoff" "cut" 4.5 (ui-accent-green) 0)
      (ui-lego-knob-s 1 "filt1_resonance" "res" 4.5 (ui-accent-green) 2)
      (ui-lego-knob-s 1 "filt1_env_amt" "env" 4.5 (ui-accent-blue) 0))))

(def analog-filter1-mod-block ()
  (ui-readout-block-small-s "F1 MOD" (ui-accent-green) 1
    (h-stack :gap 0.22 :align :start
      (ui-lego-num-s 1 "filt1_keytrack" "key" 3.7 2 false (ui-accent-green))
      (ui-lego-num-s 1 "filt1_drive" "drv" 3.7 2 false (ui-accent-orange))
      (ui-lego-num-s 1 "f1_to_f2" "toF2" 4.2 2 false (ui-accent-green)))))

(def analog-filter2-block ()
  (ui-control-block-medium-s "FILTER2" (ui-accent-green) 1
    (h-stack :gap 0.22 :align :start
      (ui-lego-option-s 1 "filt2_mode" "mode" 5.1 (analog-filter-options) (ui-accent-green))
      (ui-lego-knob-s 1 "filt2_cutoff" "cut" 4.5 (ui-accent-green) 0)
      (ui-lego-knob-s 1 "filt2_resonance" "res" 4.5 (ui-accent-green) 2)
      (ui-lego-knob-s 1 "filt2_env_amt" "env" 4.5 (ui-accent-blue) 0))))

(def analog-filter2-mod-block ()
  (ui-readout-block-small-s "F2 MOD" (ui-accent-green) 1
    (h-stack :gap 0.22 :align :start
      (ui-lego-num-s 1 "filt2_keytrack" "key" 3.7 2 false (ui-accent-green))
      (ui-lego-num-s 1 "filt2_drive" "drv" 3.7 2 false (ui-accent-orange))
      (ui-lego-num-s 1 "env_vel_amt" "evel" 4.2 2 false (ui-accent-blue)))))

(def analog-lfo1-block ()
  (ui-readout-block-small-s "LFO1" (ui-accent-blue) 2
    (h-stack :gap 0.22 :align :start
      (ui-lego-option-s 2 "lfo1_wave" "wave" 5.1 (analog-wave-options) (ui-accent-blue))
      (ui-lego-num-s 2 "lfo1_rate_hz" "rate" 3.8 2 "Hz" (ui-accent-blue))
      (ui-lego-num-s 2 "lfo1_to_pitch" "pit" 3.8 2 "st" (ui-accent-orange))
      (ui-lego-num-s 2 "lfo1_to_pw" "pw" 3.8 2 false (ui-accent-cyan)))))

(def analog-lfo2-block ()
  (ui-readout-block-small-s "LFO2" (ui-accent-violet) 2
    (h-stack :gap 0.22 :align :start
      (ui-lego-option-s 2 "lfo2_wave" "wave" 5.1 (analog-wave-options) (ui-accent-violet))
      (ui-lego-num-s 2 "lfo2_rate_hz" "rate" 3.8 2 "Hz" (ui-accent-violet))
      (ui-lego-num-s 2 "lfo2_to_f1" "F1" 3.8 0 "Hz" (ui-accent-green))
      (ui-lego-num-s 2 "lfo2_to_f2" "F2" 3.8 0 "Hz" (ui-accent-green)))))

(def analog-lfo-shape-block ()
  (ui-readout-block-small-s "LFO SHAPE" (ui-accent-blue) 2
    (h-stack :gap 0.20 :align :start
      (ui-lego-num-s 2 "lfo1_width" "w1" 3.4 2 false (ui-accent-blue))
      (ui-lego-num-s 2 "lfo2_width" "w2" 3.4 2 false (ui-accent-violet))
      (ui-lego-num-s 2 "lfo1_attack_ms" "a1" 3.6 0 "ms" (ui-accent-blue))
      (ui-lego-num-s 2 "lfo2_attack_ms" "a2" 3.6 0 "ms" (ui-accent-violet)))))

(def analog-amp-block ()
  (ui-readout-block-small-s "AMP" (ui-accent-orange) 0
    (h-stack :gap 0.20 :align :start
      (ui-lego-num-s 0 "amp1_level" "A1" 3.5 2 false (ui-accent-orange))
      (ui-lego-num-s 0 "amp2_level" "A2" 3.5 2 false (ui-accent-orange))
      (ui-lego-num-s 0 "amp1_pan" "P1" 3.5 2 false (ui-accent-violet))
      (ui-lego-num-s 0 "amp2_pan" "P2" 3.5 2 false (ui-accent-violet)))))

(def analog-global-block ()
  (ui-readout-block-small-s "GLOBAL" (ui-accent-orange) 0
    (h-stack :gap 0.20 :align :start
      (ui-lego-base-note 3.8 (ui-accent-orange))
      (ui-lego-num-s 0 "glide_ms" "gli" 3.7 0 "ms" (ui-accent-orange))
      (ui-lego-num-s 0 "vibrato" "vib" 3.7 2 "st" (ui-accent-blue))
      (ui-lego-num-s 0 "output_gain" "gain" 3.7 2 false (ui-accent-orange)))))

(def analog-envelope-column ()
  (ui-lego-column-full
    (box :width (ui-lego-col-w) :height (ui-lego-full-h)
      (ui-adsr-switch
        0 "AMP ENV" "amp_attack_ms" "amp_decay_ms" "amp_sustain" "amp_release_ms"
        1 "FILTER ENV" "filt_attack_ms" "filt_decay_ms" "filt_sustain" "filt_release_ms"))))

(defsynth-ui
  (h-stack :width :fill :gap 0.30 :align :stretch
    (ui-lego-column
      (analog-osc1-block)
      (analog-osc1-mix-block)
      (analog-source-block))
    (ui-lego-column
      (analog-osc2-block)
      (analog-osc2-mix-block)
      (analog-amp-block))
    (analog-envelope-column)
    (ui-lego-column
      (analog-filter1-block)
      (analog-filter1-mod-block)
      (analog-filter2-mod-block))
    (ui-lego-column
      (analog-filter2-block)
      (analog-lfo1-block)
      (analog-lfo-shape-block))
    (ui-lego-column
      (analog-lfo2-block)
      (analog-global-block)
      (ui-readout-block-small-s "ROUTING" (ui-accent-green) 0
        (ui-lego-text-row-4
          (label "src->F1/F2" :font-size 9.0 :color (ui-accent-green) :bg :transparent)
          (label "F1->F2" :font-size 9.0 :color (ui-accent-green) :bg :transparent)
          (label "A1/A2" :font-size 9.0 :color (ui-accent-orange) :bg :transparent)
          (label "wide" :font-size 9.0 :color (ui-accent-violet) :bg :transparent))))))
