(def mix-block ()
  (ui-control-block-medium-s "OSC MIX" (ui-accent-cyan) 0
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob-s 0 "sub_level" "sub" 4.7 (ui-accent-violet) 2)
      (ui-lego-knob-s 0 "deep_sub_level" "deep" 4.7 (ui-accent-violet) 2)
      (ui-lego-knob-s 0 "saw_level" "saw" 4.7 (ui-accent-cyan) 2))))

(def pulse-block ()
  (ui-readout-block-small-s "PULSE" (ui-accent-blue) 0
    (h-stack :gap 0.30 :align :start
      (ui-lego-num-s 0 "pulse_level" "pulse" 4.2 2 false (ui-accent-blue))
      (ui-lego-num-s 0 "pulse_width" "width" 4.2 2 false (ui-accent-blue))
      (ui-lego-num-s 0 "pulse_transpose" "semi" 4.2 0 "st" (ui-accent-orange)))))

(def phat-block ()
  (ui-readout-block-small-s "PHAT" (ui-accent-orange) 0
    (h-stack :gap 0.30 :align :start
      (ui-lego-num-s 0 "fatness" "fat" 4.2 2 false (ui-accent-orange))
      (ui-lego-num-s 0 "fat_spread" "sprd" 4.2 0 "ct" (ui-accent-orange))
      (ui-lego-num-s 0 "bass_boost" "low" 4.2 2 false (ui-accent-violet)))))

(def filter-block ()
  (ui-control-block-medium-s "FILTER" (ui-accent-green) 1
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob-s 1 "cutoff" "cut" 4.8 (ui-accent-green) 0)
      (ui-lego-knob-s 1 "resonance" "res" 4.8 (ui-accent-green) 2)
      (ui-lego-knob-s 1 "filter_env_amount" "env" 4.8 (ui-accent-blue) 0))))

(def filter-detail-block ()
  (ui-readout-block-small-s "FILTER TYPE" (ui-accent-green) 1
    (h-stack :gap 0.30 :align :start
      (ui-lego-num-s 1 "filter_model" "0 svf / 1 moog" 5.8 0 false (ui-accent-green))
      (ui-lego-num-s 0 "drive" "drive" 4.2 2 false (ui-accent-orange))
      (ui-lego-num-s 0 "gain" "gain" 4.2 2 false (ui-accent-orange)))))

(def tone-block ()
  (ui-readout-block-small-s "DUB CORE" (ui-accent-violet) 0
    (h-stack :gap 0.30 :align :start
      (ui-lego-base-note 4.2 (ui-accent-orange))
      (ui-lego-num-s 0 "tone" "tone" 4.2 2 false (ui-accent-blue))
      (ui-lego-num-s 1 "filt_decay" "f.dec" 4.2 0 "ms" (ui-accent-green)))))

(def env-column ()
  (ui-lego-column-full
    (box :width (ui-lego-col-w) :height (ui-lego-full-h)
      (ui-adsr-switch
        0 "AMP ENV" "amp_attack" "amp_decay" "amp_sustain" "amp_release"
        1 "FILTER ENV" "filt_attack" "filt_decay" "filt_sustain" "filt_release"))))

(defsynth-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (ui-lego-column
      (mix-block)
      (pulse-block)
      (phat-block))
    (env-column)
    (ui-lego-column
      (filter-block)
      (filter-detail-block)
      (tone-block))))