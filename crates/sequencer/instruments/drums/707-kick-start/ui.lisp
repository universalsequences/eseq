(def voice-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-medium-s "707 VOICE" (eseq.effects.custom-ui-lego/ui-accent-cyan) 0
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-option-s 0 "voice" "drum" 5.2 '("kick" "snare" "lo tom" "hi tom" "rim" "clap" "tamb" "closed" "open" "cym") (eseq.effects.custom-ui-lego/ui-accent-cyan))
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "tune" "tune" 4.8 (eseq.effects.custom-ui-lego/ui-accent-blue) 0)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "decay" "decay" 4.8 (eseq.effects.custom-ui-lego/ui-accent-orange) 0)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "tone" "tone" 4.8 (eseq.effects.custom-ui-lego/ui-accent-green) 2))))

(def global-block ()
  (eseq.effects.custom-ui-lego/ui-readout-block-small-s "GLOBAL" (eseq.effects.custom-ui-lego/ui-accent-orange) 0
    (h-stack :gap 0.30 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-base-note 4.2 (eseq.effects.custom-ui-lego/ui-accent-orange))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "gain" "gain" 4.2 2 false (eseq.effects.custom-ui-lego/ui-accent-orange))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "drive" "drive" 4.2 2 false (eseq.effects.custom-ui-lego/ui-accent-orange))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "grit" "grit" 4.2 2 false (eseq.effects.custom-ui-lego/ui-accent-violet)))))

(def source-readout ()
  (eseq.effects.custom-ui-lego/ui-readout-block-small-s "MODEL" (eseq.effects.custom-ui-lego/ui-accent-blue) 0
    (eseq.effects.custom-ui-lego/ui-lego-text-row-4
      (label "PCM snap" :font-size 9.0 :color (eseq.effects.custom-ui-lego/ui-accent-cyan) :bg :transparent)
      (label "tone bank" :font-size 9.0 :color (eseq.effects.custom-ui-lego/ui-accent-green) :bg :transparent)
      (label "metal/noise" :font-size 9.0 :color (eseq.effects.custom-ui-lego/ui-accent-violet) :bg :transparent)
      (label "mono out" :font-size 9.0 :color (eseq.effects.custom-ui-lego/ui-accent-orange) :bg :transparent))))

(def mix-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-medium-s "BALANCE" (eseq.effects.custom-ui-lego/ui-accent-green) 0
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "body_level" "body" 4.8 (eseq.effects.custom-ui-lego/ui-accent-green) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "noise_level" "noise" 4.8 (eseq.effects.custom-ui-lego/ui-accent-cyan) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "metal_level" "metal" 4.8 (eseq.effects.custom-ui-lego/ui-accent-violet) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "snap" "snap" 4.8 (eseq.effects.custom-ui-lego/ui-accent-orange) 2))))

(def pitch-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-small-s "PITCH" (eseq.effects.custom-ui-lego/ui-accent-blue) 0
    (h-stack :gap 0.30 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "pitch_sweep" "sweep" 4.7 0 false (eseq.effects.custom-ui-lego/ui-accent-blue))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "sweep_decay" "time" 4.7 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-blue))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "keytrack" "key" 4.7 2 false (eseq.effects.custom-ui-lego/ui-accent-cyan)))))

(def env-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-small-s "AMP" (eseq.effects.custom-ui-lego/ui-accent-violet) 0
    (h-stack :gap 0.30 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "amp_attack" "atk" 4.7 1 "ms" (eseq.effects.custom-ui-lego/ui-accent-violet))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "decay" "dec" 4.7 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-orange))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "amp_release" "rel" 4.7 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-violet)))))

(def recipe-block ()
  (eseq.effects.custom-ui-lego/ui-readout-block-small-s "707 SET" (eseq.effects.custom-ui-lego/ui-accent-cyan) 0
    (eseq.effects.custom-ui-lego/ui-lego-text-row-4
      (label "kick/snare" :font-size 9.0 :color (eseq.effects.custom-ui-lego/ui-accent-cyan) :bg :transparent)
      (label "toms/rim" :font-size 9.0 :color (eseq.effects.custom-ui-lego/ui-accent-blue) :bg :transparent)
      (label "clap/tamb" :font-size 9.0 :color (eseq.effects.custom-ui-lego/ui-accent-orange) :bg :transparent)
      (label "hats/cym" :font-size 9.0 :color (eseq.effects.custom-ui-lego/ui-accent-violet) :bg :transparent))))

(defsynth-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (eseq.effects.custom-ui-lego/ui-lego-column
      (voice-block)
      (global-block)
      (source-readout))
    (eseq.effects.custom-ui-lego/ui-lego-column
      (mix-block)
      (pitch-block)
      (env-block))
    (eseq.effects.custom-ui-lego/ui-lego-column-full
      (recipe-block))))