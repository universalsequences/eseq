(def voice-block ()
  (ui-control-block-medium-s "707 VOICE" (ui-accent-cyan) 0
    (h-stack :gap 0.32 :align :start
      (ui-lego-option-s 0 "voice" "drum" 5.2 '("kick" "snare" "lo tom" "hi tom" "rim" "clap" "tamb" "closed" "open" "cym") (ui-accent-cyan))
      (ui-lego-knob-s 0 "tune" "tune" 4.8 (ui-accent-blue) 0)
      (ui-lego-knob-s 0 "decay" "decay" 4.8 (ui-accent-orange) 0)
      (ui-lego-knob-s 0 "tone" "tone" 4.8 (ui-accent-green) 2))))

(def global-block ()
  (ui-readout-block-small-s "GLOBAL" (ui-accent-orange) 0
    (h-stack :gap 0.30 :align :start
      (ui-lego-base-note 4.2 (ui-accent-orange))
      (ui-lego-num-s 0 "gain" "gain" 4.2 2 false (ui-accent-orange))
      (ui-lego-num-s 0 "drive" "drive" 4.2 2 false (ui-accent-orange))
      (ui-lego-num-s 0 "grit" "grit" 4.2 2 false (ui-accent-violet)))))

(def source-readout ()
  (ui-readout-block-small-s "MODEL" (ui-accent-blue) 0
    (ui-lego-text-row-4
      (label "PCM snap" :font-size 9.0 :color (ui-accent-cyan) :bg :transparent)
      (label "tone bank" :font-size 9.0 :color (ui-accent-green) :bg :transparent)
      (label "metal/noise" :font-size 9.0 :color (ui-accent-violet) :bg :transparent)
      (label "mono out" :font-size 9.0 :color (ui-accent-orange) :bg :transparent))))

(def mix-block ()
  (ui-control-block-medium-s "BALANCE" (ui-accent-green) 0
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob-s 0 "body_level" "body" 4.8 (ui-accent-green) 2)
      (ui-lego-knob-s 0 "noise_level" "noise" 4.8 (ui-accent-cyan) 2)
      (ui-lego-knob-s 0 "metal_level" "metal" 4.8 (ui-accent-violet) 2)
      (ui-lego-knob-s 0 "snap" "snap" 4.8 (ui-accent-orange) 2))))

(def pitch-block ()
  (ui-control-block-small-s "PITCH" (ui-accent-blue) 0
    (h-stack :gap 0.30 :align :start
      (ui-lego-num-s 0 "pitch_sweep" "sweep" 4.7 0 false (ui-accent-blue))
      (ui-lego-num-s 0 "sweep_decay" "time" 4.7 0 "ms" (ui-accent-blue))
      (ui-lego-num-s 0 "keytrack" "key" 4.7 2 false (ui-accent-cyan)))))

(def env-block ()
  (ui-control-block-small-s "AMP" (ui-accent-violet) 0
    (h-stack :gap 0.30 :align :start
      (ui-lego-num-s 0 "amp_attack" "atk" 4.7 1 "ms" (ui-accent-violet))
      (ui-lego-num-s 0 "decay" "dec" 4.7 0 "ms" (ui-accent-orange))
      (ui-lego-num-s 0 "amp_release" "rel" 4.7 0 "ms" (ui-accent-violet)))))

(def recipe-block ()
  (ui-readout-block-small-s "707 SET" (ui-accent-cyan) 0
    (ui-lego-text-row-4
      (label "kick/snare" :font-size 9.0 :color (ui-accent-cyan) :bg :transparent)
      (label "toms/rim" :font-size 9.0 :color (ui-accent-blue) :bg :transparent)
      (label "clap/tamb" :font-size 9.0 :color (ui-accent-orange) :bg :transparent)
      (label "hats/cym" :font-size 9.0 :color (ui-accent-violet) :bg :transparent))))

(defsynth-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (ui-lego-column
      (voice-block)
      (global-block)
      (source-readout))
    (ui-lego-column
      (mix-block)
      (pitch-block)
      (env-block))
    (ui-lego-column-full
      (recipe-block))))