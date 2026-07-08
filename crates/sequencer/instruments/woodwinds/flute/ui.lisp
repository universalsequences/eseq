;; Waveguide flute — breath / bore / free-jazz panels.

(def flute-breath-block ()
  (ui-control-block-medium-s "BREATH" (ui-accent-cyan) 0
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob-s 0 "pressure" "press" 4.8 (ui-accent-cyan) 2)
      (ui-lego-knob-s 0 "breath" "breath" 4.8 (ui-accent-cyan) 2)
      (ui-lego-knob-s 0 "chiff" "chiff" 4.8 (ui-accent-blue) 2))))

(def flute-env-block ()
  (ui-readout-block-small-s "ENV" (ui-accent-blue) 0
    (h-stack :gap 0.30 :align :start
      (ui-lego-num-s 0 "attack" "atk" 4.2 0 "ms" (ui-accent-blue))
      (ui-lego-num-s 0 "release" "rel" 4.2 0 "ms" (ui-accent-blue))
      (ui-lego-num-s 0 "vel_to_press" "vel" 4.2 2 false (ui-accent-blue)))))

(def flute-out-block ()
  (ui-readout-block-small-s "OUT" (ui-accent-orange) 0
    (h-stack :gap 0.30 :align :start
      (ui-lego-base-note 4.2 (ui-accent-orange))
      (ui-lego-num-s 0 "gain" "gain" 4.2 2 false (ui-accent-orange))
      (ui-lego-num-s 0 "tune" "tune" 4.2 1 "ct" (ui-accent-orange)))))

(def flute-bore-block ()
  (ui-control-block-medium-s "BORE" (ui-accent-green) 1
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob-s 1 "brightness" "bright" 4.8 (ui-accent-green) 2)
      (ui-lego-knob-s 1 "lock" "lock" 4.8 (ui-accent-green) 2)
      (ui-lego-knob-s 1 "refl" "refl" 4.8 (ui-accent-green) 2))))

(def flute-pipe-block ()
  (ui-readout-block-small-s "EMBOUCHURE" (ui-accent-green) 1
    (h-stack :gap 0.30 :align :start
      (ui-lego-num-s 1 "jet_ratio" "jet" 4.7 2 false (ui-accent-green)))))

(def flute-vib-block ()
  (ui-readout-block-small-s "VIBRATO" (ui-accent-blue) 1
    (h-stack :gap 0.30 :align :start
      (ui-lego-num-s 1 "vib_rate" "rate" 4.2 1 "Hz" (ui-accent-blue))
      (ui-lego-num-s 1 "vib_depth" "depth" 4.2 0 "ct" (ui-accent-blue)))))

(def flute-free-block ()
  (ui-control-block-medium-s "FREE JAZZ" (ui-accent-violet) 2
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob-s 2 "chaos" "chaos" 4.8 (ui-accent-violet) 2)
      (ui-lego-knob-s 2 "overblow" "ovrblw" 4.8 (ui-accent-violet) 2)
      (ui-lego-knob-s 2 "growl" "growl" 4.8 (ui-accent-violet) 2))))

(def flute-free-ctl-block ()
  (ui-readout-block-small-s "FREE CTL" (ui-accent-violet) 2
    (h-stack :gap 0.30 :align :start
      (ui-lego-num-s 2 "chaos_rate" "rate" 4.2 1 "Hz" (ui-accent-violet))
      (ui-lego-num-s 2 "growl_ratio" "ratio" 4.2 2 false (ui-accent-violet))
      (ui-lego-num-s 2 "flutter" "fluttr" 4.2 2 false (ui-accent-violet)))))

(defsynth-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (ui-lego-column
      (flute-breath-block)
      (flute-env-block)
      (flute-out-block))
    (ui-lego-column
      (flute-bore-block)
      (flute-pipe-block)
      (flute-vib-block))
    (ui-lego-column-2
      (flute-free-block)
      (flute-free-ctl-block))))
