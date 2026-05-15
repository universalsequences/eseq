(def operator-block ()
  (ui-control-block-medium-s "FM CORE" (ui-accent-cyan) 0
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob-s 0 "carrier_ratio" "car" 4.8 (ui-accent-cyan) 2)
      (ui-lego-knob-s 0 "mod_ratio" "mod" 4.8 (ui-accent-violet) 2)
      (ui-lego-knob-s 0 "mod_index" "index" 4.8 (ui-accent-orange) 2)
      (ui-lego-knob-s 0 "mod_env_amt" "env" 4.8 (ui-accent-blue) 2))))

(def mix-block ()
  (ui-control-block-small-s "MIX" (ui-accent-blue) 0
    (h-stack :gap 0.30 :align :start
      (ui-lego-knob-s 0 "carrier_level" "car" 4.7 (ui-accent-cyan) 2)
      (ui-lego-knob-s 0 "sub_level" "sub" 4.7 (ui-accent-violet) 2)
      (ui-lego-knob-s 0 "second_mod_level" "op3" 4.7 (ui-accent-orange) 2))))

(def transient-block ()
  (ui-readout-block-small-s "PUNCH" (ui-accent-orange) 0
    (h-stack :gap 0.30 :align :start
      (ui-lego-num-s 0 "pitch_env_amt" "bend" 4.2 0 "Hz" (ui-accent-orange))
      (ui-lego-num-s 0 "pitch_decay" "time" 4.2 0 "ms" (ui-accent-orange))
      (ui-lego-num-s 0 "click_level" "click" 4.2 2 false (ui-accent-blue)))))

(def filter-block ()
  (ui-control-block-medium-s "LADDER" (ui-accent-green) 1
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob-s 1 "cutoff" "cut" 4.8 (ui-accent-green) 0)
      (ui-lego-knob-s 1 "resonance" "res" 4.8 (ui-accent-green) 2)
      (ui-lego-knob-s 1 "filter_env_amt" "env" 4.8 (ui-accent-blue) 0)
      (ui-lego-knob-s 1 "drive" "drive" 4.8 (ui-accent-orange) 2))))

(def tone-block ()
  (ui-readout-block-small-s "OUTPUT" (ui-accent-violet) 1
    (h-stack :gap 0.30 :align :start
      (ui-lego-base-note 4.2 (ui-accent-violet))
      (ui-lego-num-s 1 "tone" "tone" 4.2 2 false (ui-accent-green))
      (ui-lego-num-s 1 "gain" "gain" 4.2 2 false (ui-accent-violet)))))

(def op3-block ()
  (ui-readout-block-small-s "OP3" (ui-accent-cyan) 0
    (h-stack :gap 0.30 :align :start
      (ui-lego-num-s 0 "second_mod_ratio" "ratio" 4.2 2 false (ui-accent-cyan))
      (ui-lego-num-s 0 "click_decay" "clk ms" 4.2 0 "ms" (ui-accent-orange))
      (ui-lego-num-s 0 "mod_sustain" "hold" 4.2 2 false (ui-accent-blue)))))

(def envelope-column ()
  (ui-lego-column-full
    (box :width (ui-lego-col-w) :height (ui-lego-full-h)
      (ui-adsr-switch
        0 "AMP ENV" "amp_attack" "amp_decay" "amp_sustain" "amp_release"
        1 "FILTER ENV" "filt_attack" "filt_decay" "filt_sustain" "filt_release"))))

(defsynth-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (ui-lego-column
      (operator-block)
      (mix-block)
      (transient-block))
    (envelope-column)
    (ui-lego-column
      (filter-block)
      (tone-block)
      (op3-block))))
