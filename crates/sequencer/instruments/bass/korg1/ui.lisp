(def korg1-mix-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-medium-s "OSC MIX" (eseq.effects.custom-ui-lego/ui-accent-cyan) 0
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "vco1_saw" "saw" 4.8 (eseq.effects.custom-ui-lego/ui-accent-cyan) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "vco1_pulse" "pulse" 4.8 (eseq.effects.custom-ui-lego/ui-accent-cyan) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "vco2_level" "vco2" 4.8 (eseq.effects.custom-ui-lego/ui-accent-cyan) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "sub_level" "sub" 4.8 (eseq.effects.custom-ui-lego/ui-accent-violet) 2))))

(def korg1-global-block ()
  (eseq.effects.custom-ui-lego/ui-readout-block-small-s "GLOBAL" (eseq.effects.custom-ui-lego/ui-accent-orange) 0
    (h-stack :gap 0.30 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-base-note 4.2 (eseq.effects.custom-ui-lego/ui-accent-orange))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "gain" "gain" 4.2 2 false (eseq.effects.custom-ui-lego/ui-accent-orange))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "analog_drift" "drift" 4.2 1 false (eseq.effects.custom-ui-lego/ui-accent-green))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "noise_level" "noise" 4.2 2 false (eseq.effects.custom-ui-lego/ui-accent-violet)))))

(def korg1-source-readout-block ()
  (eseq.effects.custom-ui-lego/ui-readout-block-small-s "SOURCE" (eseq.effects.custom-ui-lego/ui-accent-cyan) 0
    (eseq.effects.custom-ui-lego/ui-lego-text-row-4
      (label "saw" :font-size 9.0 :color (eseq.effects.custom-ui-lego/ui-accent-cyan) :bg :transparent)
      (label "+ pulse" :font-size 9.0 :color (eseq.effects.custom-ui-lego/ui-accent-cyan) :bg :transparent)
      (label "vco2" :font-size 9.0 :color (eseq.effects.custom-ui-lego/ui-accent-blue) :bg :transparent)
      (label "sub/noise" :font-size 9.0 :color (eseq.effects.custom-ui-lego/ui-accent-violet) :bg :transparent))))

(def korg1-shape-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-medium-s "OSC SHAPE" (eseq.effects.custom-ui-lego/ui-accent-cyan) 0
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "vco2_interval" "semi" 4.8 (eseq.effects.custom-ui-lego/ui-accent-orange) 0)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "vco2_fine" "fine" 4.8 (eseq.effects.custom-ui-lego/ui-accent-orange) 0)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "pulse_width" "width" 4.8 (eseq.effects.custom-ui-lego/ui-accent-cyan) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "pwm_amount" "pwm" 4.8 (eseq.effects.custom-ui-lego/ui-accent-blue) 2))))

(def korg1-saturation-block ()
  (eseq.effects.custom-ui-lego/ui-readout-block-small-s "SAT" (eseq.effects.custom-ui-lego/ui-accent-orange) 0
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "input_drive" "input" 4.7 2 false (eseq.effects.custom-ui-lego/ui-accent-orange))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "output_bite" "bite" 4.7 2 false (eseq.effects.custom-ui-lego/ui-accent-orange))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "ring_level" "ring" 4.7 2 false (eseq.effects.custom-ui-lego/ui-accent-violet)))))

(def korg1-osc-readout-block ()
  (eseq.effects.custom-ui-lego/ui-readout-block-small-s "ROUTING" (eseq.effects.custom-ui-lego/ui-accent-cyan) 0
    (eseq.effects.custom-ui-lego/ui-lego-text-row-4
      (label "dual osc" :font-size 9.0 :color (eseq.effects.custom-ui-lego/ui-accent-cyan) :bg :transparent)
      (label "ring" :font-size 9.0 :color (eseq.effects.custom-ui-lego/ui-accent-violet) :bg :transparent)
      (label "into" :font-size 9.0 :color :dim :bg :transparent)
      (label "MS filter" :font-size 9.0 :color (eseq.effects.custom-ui-lego/ui-accent-green) :bg :transparent))))

(def korg1-filter-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-medium-s "MS FILTER" (eseq.effects.custom-ui-lego/ui-accent-green) 1
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "cutoff" "cut" 4.8 (eseq.effects.custom-ui-lego/ui-accent-green) 0)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "resonance" "peak" 4.8 (eseq.effects.custom-ui-lego/ui-accent-green) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "filter_env_amount" "env" 4.8 (eseq.effects.custom-ui-lego/ui-accent-blue) 0)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "keytrack" "track" 4.8 (eseq.effects.custom-ui-lego/ui-accent-green) 2))))

(def korg1-hp-block ()
  (eseq.effects.custom-ui-lego/ui-readout-block-small-s "HP" (eseq.effects.custom-ui-lego/ui-accent-green) 1
    (h-stack :gap 0.30 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-num-s 1 "hp_cutoff" "hp" 4.2 0 false (eseq.effects.custom-ui-lego/ui-accent-green))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 1 "hp_resonance" "hp q" 4.2 2 false (eseq.effects.custom-ui-lego/ui-accent-green))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 1 "scream" "scream" 4.2 2 false (eseq.effects.custom-ui-lego/ui-accent-blue))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 1 "filter_drive" "drive" 4.2 2 false (eseq.effects.custom-ui-lego/ui-accent-orange)))))

(def korg1-mod-block ()
  (eseq.effects.custom-ui-lego/ui-readout-block-small-s "MOD" (eseq.effects.custom-ui-lego/ui-accent-blue) 0
    (h-stack :gap 0.30 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "lfo_rate" "rate" 4.2 1 false (eseq.effects.custom-ui-lego/ui-accent-blue))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "lfo_filter_amount" "filt" 4.2 0 false (eseq.effects.custom-ui-lego/ui-accent-green))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "lfo_pitch" "pitch" 4.2 0 false (eseq.effects.custom-ui-lego/ui-accent-blue))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "pitch_env_amount" "snap" 4.2 0 false (eseq.effects.custom-ui-lego/ui-accent-orange)))))

(def korg1-envelope-column ()
  (eseq.effects.custom-ui-lego/ui-lego-column-full
    (box :width (eseq.effects.custom-ui-lego/ui-lego-col-w) :height (eseq.effects.custom-ui-lego/ui-lego-full-h)
      (eseq.effects.custom-ui-lego/ui-adsr-switch
        0 "AMP ENV" "amp_attack" "amp_decay" "amp_sustain" "amp_release"
        1 "FILTER ENV" "filt_attack" "filt_decay" "filt_sustain" "filt_release"))))

(defsynth-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (eseq.effects.custom-ui-lego/ui-lego-column
      (korg1-mix-block)
      (korg1-global-block)
      (korg1-source-readout-block))
    (eseq.effects.custom-ui-lego/ui-lego-column
      (korg1-shape-block)
      (korg1-saturation-block)
      (korg1-osc-readout-block))
    (korg1-envelope-column)
    (eseq.effects.custom-ui-lego/ui-lego-column
      (korg1-filter-block)
      (korg1-hp-block)
      (korg1-mod-block))))
