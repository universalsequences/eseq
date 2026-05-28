(def osc-block ()
  (ui-control-block-medium-s "OSC" (ui-accent-cyan) 0
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob-s 0 "sub_level" "sub" 4.7 (ui-accent-cyan) 2)
      (ui-lego-knob-s 0 "mid_level" "mid" 4.7 (ui-accent-cyan) 2)
      (ui-lego-knob-s 0 "wave_blend" "wave" 4.7 (ui-accent-violet) 2)
      (ui-lego-knob-s 0 "pulse_width" "width" 4.7 (ui-accent-orange) 2))))

(def tune-block ()
  (ui-readout-block-small-s "TUNE" (ui-accent-blue) 0
    (h-stack :gap 0.30 :align :start
      (ui-lego-base-note 4.2 (ui-accent-blue))
      (ui-lego-num-s 0 "octave" "oct" 4.2 0 "st" (ui-accent-blue))
      (ui-lego-num-s 0 "detune_cents" "det" 4.2 0 "ct" (ui-accent-orange)))))

(def source-readout ()
  (ui-readout-block-small-s "DUB PLATE" (ui-accent-violet) 0
    (ui-lego-text-row-3
      (label "sine sub" :font-size 9.0 :color (ui-accent-cyan) :bg :transparent)
      (label "blep mids" :font-size 9.0 :color (ui-accent-violet) :bg :transparent)
      (label "vowel dirt" :font-size 9.0 :color (ui-accent-orange) :bg :transparent))))

(def filter-block ()
  (ui-control-block-medium-s "LOWPASS" (ui-accent-green) 1
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob-s 1 "cutoff" "cut" 4.7 (ui-accent-green) 0)
      (ui-lego-knob-s 1 "resonance" "res" 4.7 (ui-accent-green) 2)
      (ui-lego-knob-s 1 "filter_env_amt" "env" 4.7 (ui-accent-blue) 0)
      (ui-lego-knob-s 1 "drive" "drive" 4.7 (ui-accent-orange) 2))))

(def motion-block ()
  (ui-control-block-small-s "WOBBLE" (ui-accent-orange) 1
    (h-stack :gap 0.30 :align :start
      (ui-lego-knob-s 1 "lfo_rate" "rate" 4.7 (ui-accent-orange) 2)
      (ui-lego-knob-s 1 "wobble_to_cutoff" "cut" 4.7 (ui-accent-green) 0)
      (ui-lego-knob-s 1 "lfo_shape" "shape" 4.7 (ui-accent-violet) 2))))

(def filter-readout ()
  (ui-readout-block-small-s "TRACK" (ui-accent-green) 1
    (h-stack :gap 0.30 :align :start
      (ui-lego-num-s 1 "keytrack" "key" 4.2 2 false (ui-accent-green))
      (ui-lego-num-s 1 "lfo_skank" "skank" 4.2 2 false (ui-accent-orange))
      (ui-lego-num-s 1 "pitch_wobble" "bend" 4.2 0 "ct" (ui-accent-violet)))))

(def growl-block ()
  (ui-control-block-medium-s "GROWL" (ui-accent-violet) 2
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob-s 2 "growl_level" "mix" 4.7 (ui-accent-violet) 2)
      (ui-lego-knob-s 2 "growl_amount" "bite" 4.7 (ui-accent-orange) 2)
      (ui-lego-knob-s 2 "formant_base" "vowel" 4.7 (ui-accent-cyan) 0)
      (ui-lego-knob-s 2 "formant_q" "Q" 4.7 (ui-accent-green) 2))))

(def formant-block ()
  (ui-control-block-small-s "FORMANT" (ui-accent-cyan) 2
    (h-stack :gap 0.30 :align :start
      (ui-lego-knob-s 2 "formant_spread" "spread" 4.7 (ui-accent-cyan) 2)
      (ui-lego-knob-s 2 "wobble_to_growl" "talk" 4.7 (ui-accent-orange) 2)
      (ui-lego-knob-s 2 "dirt" "dirt" 4.7 (ui-accent-violet) 2))))

(def out-block ()
  (ui-readout-block-small-s "OUTPUT" (ui-accent-blue) 2
    (h-stack :gap 0.30 :align :start
      (ui-lego-num-s 2 "sub_clean" "clean sub" 4.7 2 false (ui-accent-cyan))
      (ui-lego-num-s 2 "output_gain" "gain" 4.7 2 false (ui-accent-blue)))))

(def env-column ()
  (ui-lego-column-full
    (box :width (ui-lego-col-w) :height (ui-lego-full-h)
      (ui-adsr-switch
        0 "AMP ENV" "amp_attack" "amp_decay" "amp_sustain" "amp_release"
        1 "FILTER ENV" "filt_attack" "filt_decay" "filt_sustain" "filt_release"))))

(defsynth-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (ui-lego-column
      (osc-block)
      (tune-block)
      (source-readout))
    (ui-lego-column
      (filter-block)
      (motion-block)
      (filter-readout))
    (env-column)
    (ui-lego-column
      (growl-block)
      (formant-block)
      (out-block))))