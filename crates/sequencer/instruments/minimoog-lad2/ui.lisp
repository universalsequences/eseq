(def minimoog-lad-mixer-block ()
  (ui-control-block-medium-s "MIXER" (ui-accent-cyan) 0
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob-s 0 "osc1_level" "vco1" 4.8 (ui-accent-cyan) 2)
      (ui-lego-knob-s 0 "osc2_level" "vco2" 4.8 (ui-accent-cyan) 2)
      (ui-lego-knob-s 0 "osc3_level" "vco3" 4.8 (ui-accent-cyan) 2)
      (ui-lego-knob-s 0 "noise_level" "noise" 4.8 (ui-accent-violet) 2))))

(def minimoog-lad-model-block ()
  (ui-readout-block-small-s "MODEL" (ui-accent-orange) 0
    (h-stack :gap 0.30 :align :start
      (ui-lego-base-note 4.2 (ui-accent-orange))
      (ui-lego-num-s 0 "gain" "gain" 4.2 2 false (ui-accent-orange))
      (ui-lego-num-s 0 "drive" "drive" 4.2 2 false (ui-accent-orange))
      (ui-lego-num-s 0 "key_track" "track" 4.2 2 false (ui-accent-green)))))

(def minimoog-lad-mix-readout-block ()
  (ui-readout-block-small-s "BALANCE" (ui-accent-cyan) 0
    (ui-lego-text-row-4
      (label "3 VCO" :font-size 9.0 :color (ui-accent-cyan) :bg :transparent)
      (label "+" :font-size 9.0 :color :dim :bg :transparent)
      (label "noise" :font-size 9.0 :color (ui-accent-violet) :bg :transparent)
      (label "into ladder" :font-size 9.0 :color (ui-accent-green) :bg :transparent))))

(def minimoog-lad-shape-block ()
  (ui-control-block-medium-s "OSC SHAPE" (ui-accent-cyan) 0
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob-s 0 "osc1_wave" "vco1" 4.8 (ui-accent-cyan) 2)
      (ui-lego-knob-s 0 "osc2_wave" "vco2" 4.8 (ui-accent-cyan) 2)
      (ui-lego-knob-s 0 "osc3_wave" "vco3" 4.8 (ui-accent-cyan) 2)
      (ui-lego-knob-s 0 "pulse_width" "pw" 4.8 (ui-accent-cyan) 2))))

(def minimoog-lad-tune-block ()
  (ui-readout-block-small-s "TUNE" (ui-accent-cyan) 0
    (h-stack :gap 0.22 :align :start
      (ui-lego-num-s 0 "osc1_oct" "o1" 3.7 2 false (ui-accent-cyan))
      (ui-lego-num-s 0 "osc2_oct" "o2" 3.7 2 false (ui-accent-cyan))
      (ui-lego-num-s 0 "osc3_oct" "o3" 3.7 0 false (ui-accent-cyan))
      (ui-lego-num-s 0 "osc2_detune" "d2" 3.7 0 false (ui-accent-orange))
      (ui-lego-num-s 0 "osc3_detune" "d3" 3.7 0 false (ui-accent-orange)))))

(def minimoog-lad-osc-readout-block ()
  (ui-readout-block-small-s "SOURCE" (ui-accent-cyan) 0
    (ui-lego-text-row-4
      (label "saw" :font-size 9.0 :color (ui-accent-cyan) :bg :transparent)
      (label "<->" :font-size 9.0 :color :dim :bg :transparent)
      (label "pulse" :font-size 9.0 :color (ui-accent-cyan) :bg :transparent)
      (label "pw mod" :font-size 9.0 :color (ui-accent-blue) :bg :transparent))))

(def minimoog-lad-filter-block ()
  (ui-control-block-medium-s "LADDER FILTER" (ui-accent-green) 1
    (h-stack :gap 0.46 :align :start
      (ui-lego-knob-s 1 "cutoff" "cut" 5.4 (ui-accent-green) 0)
      (ui-lego-knob-s 1 "resonance" "emph" 5.4 (ui-accent-green) 2)
      (ui-lego-knob-s 1 "filter_env_amount" "env" 5.4 (ui-accent-blue) 0))))

(def minimoog-lad-filter-model-block ()
  (ui-readout-block-small-s "FILTER MOD" (ui-accent-green) 1
    (h-stack :gap 0.32 :align :start
      (ui-lego-num-s 1 "drive" "drive" 4.7 2 false (ui-accent-orange))
      (ui-lego-num-s 1 "key_track" "key" 4.7 2 false (ui-accent-green))
      (ui-lego-num-s 1 "filter_env_amount" "env" 6.4 0 "Hz" (ui-accent-blue)))))

(def minimoog-lad-filter-readout-block ()
  (ui-readout-block-small-s "TOPOLOGY" (ui-accent-green) 1
    (ui-lego-text-row-3
      (label "24 dB" :font-size 9.0 :color (ui-accent-green) :bg :transparent)
      (label "4-pole ladder" :font-size 9.0 :color :dim :bg :transparent)
      (label "warm sat" :font-size 9.0 :color (ui-accent-orange) :bg :transparent))))

(def minimoog-lad-envelope-column ()
  (ui-lego-column-full
    (box :width (ui-lego-col-w) :height (ui-lego-full-h)
      (ui-adsr-switch
        0 "AMP ENV" "amp_attack" "amp_decay" "amp_sustain" "amp_release"
        1 "FILTER ENV" "filt_attack" "filt_decay" "filt_sustain" "filt_release"))))

(defsynth-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (ui-lego-column
      (minimoog-lad-mixer-block)
      (minimoog-lad-model-block)
      (minimoog-lad-mix-readout-block))
    (ui-lego-column
      (minimoog-lad-shape-block)
      (minimoog-lad-tune-block)
      (minimoog-lad-osc-readout-block))
    (minimoog-lad-envelope-column)
    (ui-lego-column
      (minimoog-lad-filter-block)
      (minimoog-lad-filter-model-block)
      (minimoog-lad-filter-readout-block))))
