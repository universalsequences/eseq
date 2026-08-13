(def osc-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-medium-s "OSC" (eseq.effects.custom-ui-lego/ui-accent-cyan) 0
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "wave_mix" "saw/sq" 4.8 (eseq.effects.custom-ui-lego/ui-accent-cyan) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "pulse_width" "width" 4.8 (eseq.effects.custom-ui-lego/ui-accent-violet) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "osc_level" "level" 4.8 (eseq.effects.custom-ui-lego/ui-accent-cyan) 2))))

(def tune-block ()
  (eseq.effects.custom-ui-lego/ui-readout-block-small-s "TUNE" (eseq.effects.custom-ui-lego/ui-accent-blue) 0
    (h-stack :gap 0.30 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-base-note 4.2 (eseq.effects.custom-ui-lego/ui-accent-blue))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "tune_semitones" "semi" 4.2 0 "st" (eseq.effects.custom-ui-lego/ui-accent-blue))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "fine_cents" "fine" 4.2 0 "ct" (eseq.effects.custom-ui-lego/ui-accent-orange)))))

(def slide-block ()
  (eseq.effects.custom-ui-lego/ui-readout-block-small-s "SLIDE" (eseq.effects.custom-ui-lego/ui-accent-orange) 0
    (h-stack :gap 0.30 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "slide_time" "time" 4.7 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-orange))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "post_tone" "tone" 4.7 2 false (eseq.effects.custom-ui-lego/ui-accent-blue))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "output_gain" "gain" 4.7 2 false (eseq.effects.custom-ui-lego/ui-accent-green)))))

(def filter-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-medium-s "FILTER" (eseq.effects.custom-ui-lego/ui-accent-green) 1
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "cutoff" "cut" 4.8 (eseq.effects.custom-ui-lego/ui-accent-green) 0)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "resonance" "res" 4.8 (eseq.effects.custom-ui-lego/ui-accent-green) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "env_mod" "env" 4.8 (eseq.effects.custom-ui-lego/ui-accent-blue) 0))))

(def acid-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-medium-s "ACID" (eseq.effects.custom-ui-lego/ui-accent-orange) 1
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "drive" "drive" 4.8 (eseq.effects.custom-ui-lego/ui-accent-orange) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "accent_to_cutoff" "acc cut" 4.8 (eseq.effects.custom-ui-lego/ui-accent-green) 0)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "accent_to_drive" "acc drv" 4.8 (eseq.effects.custom-ui-lego/ui-accent-orange) 2))))

(def mod-block ()
  (eseq.effects.custom-ui-lego/ui-readout-block-small-s "TRACK" (eseq.effects.custom-ui-lego/ui-accent-violet) 1
    (h-stack :gap 0.30 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-num-s 1 "keytrack" "key" 4.7 2 false (eseq.effects.custom-ui-lego/ui-accent-green))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 1 "accent_to_env" "acc env" 4.7 2 false (eseq.effects.custom-ui-lego/ui-accent-blue))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 1 "filt_decay" "decay" 4.7 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-violet)))))

(def envelope-column ()
  (eseq.effects.custom-ui-lego/ui-lego-column-full
    (box :width (eseq.effects.custom-ui-lego/ui-lego-col-w) :height (eseq.effects.custom-ui-lego/ui-lego-full-h)
      (eseq.effects.custom-ui-lego/ui-adsr-switch
        0 "AMP ENV" "amp_attack" "amp_decay" "amp_sustain" "amp_release"
        1 "FILTER ENV" "filt_attack" "filt_decay" "filt_sustain" "filt_release"))))

(defsynth-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (eseq.effects.custom-ui-lego/ui-lego-column
      (osc-block)
      (tune-block)
      (slide-block))
    (envelope-column)
    (eseq.effects.custom-ui-lego/ui-lego-column
      (filter-block)
      (acid-block)
      (mod-block))))