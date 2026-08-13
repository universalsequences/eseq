(def mix-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-medium-s "OSC MIX" (eseq.effects.custom-ui-lego/ui-accent-cyan) 0
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "sub_level" "sub" 4.7 (eseq.effects.custom-ui-lego/ui-accent-violet) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "deep_sub_level" "deep" 4.7 (eseq.effects.custom-ui-lego/ui-accent-violet) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "saw_level" "saw" 4.7 (eseq.effects.custom-ui-lego/ui-accent-cyan) 2))))

(def pulse-block ()
  (eseq.effects.custom-ui-lego/ui-readout-block-small-s "PULSE" (eseq.effects.custom-ui-lego/ui-accent-blue) 0
    (h-stack :gap 0.30 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "pulse_level" "pulse" 4.2 2 false (eseq.effects.custom-ui-lego/ui-accent-blue))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "pulse_width" "width" 4.2 2 false (eseq.effects.custom-ui-lego/ui-accent-blue))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "pulse_transpose" "semi" 4.2 0 "st" (eseq.effects.custom-ui-lego/ui-accent-orange)))))

(def phat-block ()
  (eseq.effects.custom-ui-lego/ui-readout-block-small-s "PHAT" (eseq.effects.custom-ui-lego/ui-accent-orange) 0
    (h-stack :gap 0.30 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "fatness" "fat" 4.2 2 false (eseq.effects.custom-ui-lego/ui-accent-orange))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "fat_spread" "sprd" 4.2 0 "ct" (eseq.effects.custom-ui-lego/ui-accent-orange))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "bass_boost" "low" 4.2 2 false (eseq.effects.custom-ui-lego/ui-accent-violet)))))

(def filter-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-medium-s "FILTER" (eseq.effects.custom-ui-lego/ui-accent-green) 1
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "cutoff" "cut" 4.8 (eseq.effects.custom-ui-lego/ui-accent-green) 0)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "resonance" "res" 4.8 (eseq.effects.custom-ui-lego/ui-accent-green) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "filter_env_amount" "env" 4.8 (eseq.effects.custom-ui-lego/ui-accent-blue) 0))))

(def filter-detail-block ()
  (eseq.effects.custom-ui-lego/ui-readout-block-small-s "FILTER TYPE" (eseq.effects.custom-ui-lego/ui-accent-green) 1
    (h-stack :gap 0.30 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-num-s 1 "filter_model" "0 svf / 1 moog" 5.8 0 false (eseq.effects.custom-ui-lego/ui-accent-green))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "drive" "drive" 4.2 2 false (eseq.effects.custom-ui-lego/ui-accent-orange))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "gain" "gain" 4.2 2 false (eseq.effects.custom-ui-lego/ui-accent-orange)))))

(def tone-block ()
  (eseq.effects.custom-ui-lego/ui-readout-block-small-s "DUB CORE" (eseq.effects.custom-ui-lego/ui-accent-violet) 0
    (h-stack :gap 0.30 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-base-note 4.2 (eseq.effects.custom-ui-lego/ui-accent-orange))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "tone" "tone" 4.2 2 false (eseq.effects.custom-ui-lego/ui-accent-blue))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 1 "filt_decay" "f.dec" 4.2 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-green)))))

(def env-column ()
  (eseq.effects.custom-ui-lego/ui-lego-column-full
    (box :width (eseq.effects.custom-ui-lego/ui-lego-col-w) :height (eseq.effects.custom-ui-lego/ui-lego-full-h)
      (eseq.effects.custom-ui-lego/ui-adsr-switch
        0 "AMP ENV" "amp_attack" "amp_decay" "amp_sustain" "amp_release"
        1 "FILTER ENV" "filt_attack" "filt_decay" "filt_sustain" "filt_release"))))

(defsynth-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (eseq.effects.custom-ui-lego/ui-lego-column
      (mix-block)
      (pulse-block)
      (phat-block))
    (env-column)
    (eseq.effects.custom-ui-lego/ui-lego-column
      (filter-block)
      (filter-detail-block)
      (tone-block))))