(def minimoog-lad-mixer-block ()
  (ui-control-block-medium "MIXER" (ui-accent-cyan)
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob "osc1_level" "vco1" 4.8 (ui-accent-cyan) 2)
      (ui-lego-knob "osc2_level" "vco2" 4.8 (ui-accent-cyan) 2)
      (ui-lego-knob "osc3_level" "vco3" 4.8 (ui-accent-cyan) 2)
      (ui-lego-knob "noise_level" "noise" 4.8 (ui-accent-violet) 2))))

(def minimoog-lad-model-block ()
  (ui-readout-block-small "MODEL" (ui-accent-orange)
    (h-stack :gap 0.30 :align :start
      (ui-lego-base-note 4.2 (ui-accent-orange))
      (ui-lego-num "gain" "gain" 4.2 2 false (ui-accent-orange))
      (ui-lego-num "drive" "drive" 4.2 2 false (ui-accent-orange))
      (ui-lego-num "key_track" "track" 4.2 2 false (ui-accent-green)))))

(def minimoog-lad-mix-readout-block ()
  (ui-readout-block-small "BALANCE" (ui-accent-cyan)
    (ui-lego-text-row-4
      (label "3 VCO" :font-size 9.0 :color (ui-accent-cyan) :bg :transparent)
      (label "+" :font-size 9.0 :color :dim :bg :transparent)
      (label "noise" :font-size 9.0 :color (ui-accent-violet) :bg :transparent)
      (label "into ladder" :font-size 9.0 :color (ui-accent-green) :bg :transparent))))

(def minimoog-lad-shape-block ()
  (ui-control-block-medium "OSC SHAPE" (ui-accent-cyan)
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob "osc1_wave" "vco1" 4.8 (ui-accent-cyan) 2)
      (ui-lego-knob "osc2_wave" "vco2" 4.8 (ui-accent-cyan) 2)
      (ui-lego-knob "osc3_wave" "vco3" 4.8 (ui-accent-cyan) 2)
      (ui-lego-knob "pulse_width" "pw" 4.8 (ui-accent-cyan) 2))))

(def minimoog-lad-tune-block ()
  (ui-readout-block-small "TUNE" (ui-accent-cyan)
    (h-stack :gap 0.22 :align :start
      (ui-lego-num "osc1_oct" "o1" 3.7 2 false (ui-accent-cyan))
      (ui-lego-num "osc2_oct" "o2" 3.7 2 false (ui-accent-cyan))
      (ui-lego-num "osc3_oct" "o3" 3.7 0 false (ui-accent-cyan))
      (ui-lego-num "osc2_detune" "d2" 3.7 0 false (ui-accent-orange))
      (ui-lego-num "osc3_detune" "d3" 3.7 0 false (ui-accent-orange)))))

(def minimoog-lad-osc-readout-block ()
  (ui-readout-block-small "SOURCE" (ui-accent-cyan)
    (ui-lego-text-row-4
      (label "saw" :font-size 9.0 :color (ui-accent-cyan) :bg :transparent)
      (label "<->" :font-size 9.0 :color :dim :bg :transparent)
      (label "pulse" :font-size 9.0 :color (ui-accent-cyan) :bg :transparent)
      (label "pw mod" :font-size 9.0 :color (ui-accent-blue) :bg :transparent))))

(def minimoog-lad-filter-block ()
  (ui-control-block-medium "LADDER FILTER" (ui-accent-green)
    (h-stack :gap 0.46 :align :start
      (ui-lego-knob "cutoff" "cut" 5.4 (ui-accent-green) 0)
      (ui-lego-knob "resonance" "emph" 5.4 (ui-accent-green) 2)
      (ui-lego-knob "filter_env_amount" "env" 5.4 (ui-accent-blue) 0))))

(def minimoog-lad-filter-model-block ()
  (ui-readout-block-small "FILTER MOD" (ui-accent-green)
    (h-stack :gap 0.32 :align :start
      (ui-lego-num "drive" "drive" 4.7 2 false (ui-accent-orange))
      (ui-lego-num "key_track" "key" 4.7 2 false (ui-accent-green))
      (ui-lego-num "filter_env_amount" "env" 6.4 0 "Hz" (ui-accent-blue)))))

(def minimoog-lad-filter-readout-block ()
  (ui-readout-block-small "TOPOLOGY" (ui-accent-green)
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
