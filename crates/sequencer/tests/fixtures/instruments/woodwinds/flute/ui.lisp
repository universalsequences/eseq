;; Waveguide flute — breath / bore / free-jazz panels.

(def flute-breath-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-medium-s "BREATH" (eseq.effects.custom-ui-lego/ui-accent-cyan) 0
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "pressure" "press" 4.8 (eseq.effects.custom-ui-lego/ui-accent-cyan) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "breath" "breath" 4.8 (eseq.effects.custom-ui-lego/ui-accent-cyan) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "chiff" "chiff" 4.8 (eseq.effects.custom-ui-lego/ui-accent-blue) 2))))

(def flute-env-block ()
  (eseq.effects.custom-ui-lego/ui-readout-block-small-s "ENV" (eseq.effects.custom-ui-lego/ui-accent-blue) 0
    (h-stack :gap 0.30 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "attack" "atk" 4.2 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-blue))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "release" "rel" 4.2 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-blue))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "vel_to_press" "vel" 4.2 2 false (eseq.effects.custom-ui-lego/ui-accent-blue)))))

(def flute-out-block ()
  (eseq.effects.custom-ui-lego/ui-readout-block-small-s "OUT" (eseq.effects.custom-ui-lego/ui-accent-orange) 0
    (h-stack :gap 0.30 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-base-note 4.2 (eseq.effects.custom-ui-lego/ui-accent-orange))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "gain" "gain" 4.2 2 false (eseq.effects.custom-ui-lego/ui-accent-orange))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "tune" "tune" 4.2 1 "ct" (eseq.effects.custom-ui-lego/ui-accent-orange)))))

(def flute-bore-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-medium-s "BORE" (eseq.effects.custom-ui-lego/ui-accent-green) 1
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "brightness" "bright" 4.8 (eseq.effects.custom-ui-lego/ui-accent-green) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "lock" "lock" 4.8 (eseq.effects.custom-ui-lego/ui-accent-green) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "refl" "refl" 4.8 (eseq.effects.custom-ui-lego/ui-accent-green) 2))))

(def flute-pipe-block ()
  (eseq.effects.custom-ui-lego/ui-readout-block-small-s "EMBOUCHURE" (eseq.effects.custom-ui-lego/ui-accent-green) 1
    (h-stack :gap 0.30 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-num-s 1 "jet_ratio" "jet" 4.7 2 false (eseq.effects.custom-ui-lego/ui-accent-green)))))

(def flute-vib-block ()
  (eseq.effects.custom-ui-lego/ui-readout-block-small-s "VIBRATO" (eseq.effects.custom-ui-lego/ui-accent-blue) 1
    (h-stack :gap 0.30 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-num-s 1 "vib_rate" "rate" 4.2 1 "Hz" (eseq.effects.custom-ui-lego/ui-accent-blue))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 1 "vib_depth" "depth" 4.2 0 "ct" (eseq.effects.custom-ui-lego/ui-accent-blue)))))

(def flute-free-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-medium-s "FREE JAZZ" (eseq.effects.custom-ui-lego/ui-accent-violet) 2
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 2 "chaos" "chaos" 4.8 (eseq.effects.custom-ui-lego/ui-accent-violet) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 2 "overblow" "ovrblw" 4.8 (eseq.effects.custom-ui-lego/ui-accent-violet) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 2 "growl" "growl" 4.8 (eseq.effects.custom-ui-lego/ui-accent-violet) 2))))

(def flute-free-ctl-block ()
  (eseq.effects.custom-ui-lego/ui-readout-block-small-s "FREE CTL" (eseq.effects.custom-ui-lego/ui-accent-violet) 2
    (h-stack :gap 0.30 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-num-s 2 "chaos_rate" "rate" 4.2 1 "Hz" (eseq.effects.custom-ui-lego/ui-accent-violet))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 2 "growl_ratio" "ratio" 4.2 2 false (eseq.effects.custom-ui-lego/ui-accent-violet))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 2 "flutter" "fluttr" 4.2 2 false (eseq.effects.custom-ui-lego/ui-accent-violet)))))

(defsynth-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (eseq.effects.custom-ui-lego/ui-lego-column
      (flute-breath-block)
      (flute-env-block)
      (flute-out-block))
    (eseq.effects.custom-ui-lego/ui-lego-column
      (flute-bore-block)
      (flute-pipe-block)
      (flute-vib-block))
    (eseq.effects.custom-ui-lego/ui-lego-column-2
      (flute-free-block)
      (flute-free-ctl-block))))
