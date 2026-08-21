; eseqlisp UI for Hammer-Stein Piano Synth
; Elegant three-column physical modeling grid layout

(def hammer-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-medium-s "HAMMER & STRIKE" (eseq.effects.custom-ui-lego/ui-accent-cyan) 0
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "hardness" "hard" 4.8 (eseq.effects.custom-ui-lego/ui-accent-cyan) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "hammer_noise" "noise" 4.8 (eseq.effects.custom-ui-lego/ui-accent-blue) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "strike_pos" "pos" 4.8 (eseq.effects.custom-ui-lego/ui-accent-violet) 2))))

(def eq-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-small-s "TONE EQ" (eseq.effects.custom-ui-lego/ui-accent-orange) 0
    (h-stack :gap 0.30 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "eq_bass" "bass" 4.2 1 "dB" (eseq.effects.custom-ui-lego/ui-accent-orange))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "eq_treble" "treble" 4.2 1 "dB" (eseq.effects.custom-ui-lego/ui-accent-orange))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "gain" "gain" 4.2 2 false (eseq.effects.custom-ui-lego/ui-accent-orange)))))

(def tone-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-small-s "CHARACTER" (eseq.effects.custom-ui-lego/ui-accent-cyan) 0
    (h-stack :gap 0.30 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "brightness" "bright" 4.8 (eseq.effects.custom-ui-lego/ui-accent-cyan) 2)
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "brightness_vel" "vel>bright" 4.2 2 false (eseq.effects.custom-ui-lego/ui-accent-cyan))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "hardness_vel" "vel>stiff" 4.2 2 false (eseq.effects.custom-ui-lego/ui-accent-cyan)))))

(def damper-column ()
  (eseq.effects.custom-ui-lego/ui-lego-column-full
    (eseq.effects.custom-ui-lego/ui-control-block-full-s "DAMPER & SUSTAIN" (eseq.effects.custom-ui-lego/ui-accent-violet) 0
      (v-stack :gap 0.38 :align :center :width :fill
        (label "STRING DECAY" :font-size 11.0 :color (eseq.effects.custom-ui-lego/ui-accent-violet) :font-weight :bold)
        (h-stack :gap 0.32 :align :center
          (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "sustain_s" "sustain" 5.2 (eseq.effects.custom-ui-lego/ui-accent-violet) 1)
          (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "damper_decay" "damp" 5.2 (eseq.effects.custom-ui-lego/ui-accent-violet) 2))
        (label "SUSTAIN PEDAL" :font-size 10.0 :color (eseq.effects.custom-ui-lego/ui-accent-violet) :font-weight :bold)
        (eseq.effects.custom-ui-lego/ui-lego-option-s 0 "pedal_sustain" "damper pedal" 6.4 '("Pedal Off" "Pedal Down") (eseq.effects.custom-ui-lego/ui-accent-violet))))))

(def resonance-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-medium-s "RESONANCE" (eseq.effects.custom-ui-lego/ui-accent-green) 1
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "soundboard" "board" 4.8 (eseq.effects.custom-ui-lego/ui-accent-green) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "sympathetic" "symp" 4.8 (eseq.effects.custom-ui-lego/ui-accent-green) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "duplex_ring" "duplex" 4.8 (eseq.effects.custom-ui-lego/ui-accent-green) 2))))

(def voicing-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-small-s "VOICING & PAN" (eseq.effects.custom-ui-lego/ui-accent-blue) 1
    (h-stack :gap 0.30 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-num-s 1 "unison_detune" "detune" 4.2 1 "ct" (eseq.effects.custom-ui-lego/ui-accent-blue))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 1 "stereo_width" "width" 4.2 2 false (eseq.effects.custom-ui-lego/ui-accent-blue))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 1 "key_pan" "keypan" 4.2 2 false (eseq.effects.custom-ui-lego/ui-accent-blue)))))

(def voicing-readout ()
  (eseq.effects.custom-ui-lego/ui-readout-block-small-s "PHYSICS PROFILE" (eseq.effects.custom-ui-lego/ui-accent-blue) 1
    (eseq.effects.custom-ui-lego/ui-lego-text-row-3
      (label "unison: dual string physical modeling" :font-size 9.0 :color (eseq.effects.custom-ui-lego/ui-accent-blue) :bg :transparent)
      (label "resonance: soundboard + sympathetic" :font-size 9.0 :color (eseq.effects.custom-ui-lego/ui-accent-green) :bg :transparent)
      (label "panning: dynamic key-to-stereo" :font-size 9.0 :color (eseq.effects.custom-ui-lego/ui-accent-violet) :bg :transparent))))

(defsynth-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (eseq.effects.custom-ui-lego/ui-lego-column
      (hammer-block)
      (eq-block)
      (tone-block))
    (damper-column)
    (eseq.effects.custom-ui-lego/ui-lego-column
      (resonance-block)
      (voicing-block)
      (voicing-readout))))
