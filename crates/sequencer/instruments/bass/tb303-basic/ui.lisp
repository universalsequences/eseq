(def osc-block ()
  (ui-control-block-medium-s "OSC" (ui-accent-cyan) 0
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob-s 0 "wave_mix" "saw/sq" 4.8 (ui-accent-cyan) 2)
      (ui-lego-knob-s 0 "pulse_width" "width" 4.8 (ui-accent-violet) 2)
      (ui-lego-knob-s 0 "osc_level" "level" 4.8 (ui-accent-cyan) 2))))

(def tune-block ()
  (ui-readout-block-small-s "TUNE" (ui-accent-blue) 0
    (h-stack :gap 0.30 :align :start
      (ui-lego-base-note 4.2 (ui-accent-blue))
      (ui-lego-num-s 0 "tune_semitones" "semi" 4.2 0 "st" (ui-accent-blue))
      (ui-lego-num-s 0 "fine_cents" "fine" 4.2 0 "ct" (ui-accent-orange)))))

(def slide-block ()
  (ui-readout-block-small-s "SLIDE" (ui-accent-orange) 0
    (h-stack :gap 0.30 :align :start
      (ui-lego-num-s 0 "slide_time" "time" 4.7 0 "ms" (ui-accent-orange))
      (ui-lego-num-s 0 "post_tone" "tone" 4.7 2 false (ui-accent-blue))
      (ui-lego-num-s 0 "output_gain" "gain" 4.7 2 false (ui-accent-green)))))

(def filter-block ()
  (ui-control-block-medium-s "FILTER" (ui-accent-green) 1
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob-s 1 "cutoff" "cut" 4.8 (ui-accent-green) 0)
      (ui-lego-knob-s 1 "resonance" "res" 4.8 (ui-accent-green) 2)
      (ui-lego-knob-s 1 "env_mod" "env" 4.8 (ui-accent-blue) 0))))

(def acid-block ()
  (ui-control-block-medium-s "ACID" (ui-accent-orange) 1
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob-s 1 "drive" "drive" 4.8 (ui-accent-orange) 2)
      (ui-lego-knob-s 1 "accent_to_cutoff" "acc cut" 4.8 (ui-accent-green) 0)
      (ui-lego-knob-s 1 "accent_to_drive" "acc drv" 4.8 (ui-accent-orange) 2))))

(def mod-block ()
  (ui-readout-block-small-s "TRACK" (ui-accent-violet) 1
    (h-stack :gap 0.30 :align :start
      (ui-lego-num-s 1 "keytrack" "key" 4.7 2 false (ui-accent-green))
      (ui-lego-num-s 1 "accent_to_env" "acc env" 4.7 2 false (ui-accent-blue))
      (ui-lego-num-s 1 "filt_decay" "decay" 4.7 0 "ms" (ui-accent-violet)))))

(def envelope-column ()
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
      (slide-block))
    (envelope-column)
    (ui-lego-column
      (filter-block)
      (acid-block)
      (mod-block))))