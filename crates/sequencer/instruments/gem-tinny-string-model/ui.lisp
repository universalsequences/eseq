; eseqlisp UI for Hammer-Stein Piano Synth
; Elegant three-column physical modeling grid layout

(def hammer-block ()
  (ui-control-block-medium-s "HAMMER & STRIKE" (ui-accent-cyan) 0
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob-s 0 "hardness" "hard" 4.8 (ui-accent-cyan) 2)
      (ui-lego-knob-s 0 "hammer_noise" "noise" 4.8 (ui-accent-blue) 2)
      (ui-lego-knob-s 0 "strike_pos" "pos" 4.8 (ui-accent-violet) 2))))

(def eq-block ()
  (ui-control-block-small-s "TONE EQ" (ui-accent-orange) 0
    (h-stack :gap 0.30 :align :start
      (ui-lego-num-s 0 "eq_bass" "bass" 4.2 1 "dB" (ui-accent-orange))
      (ui-lego-num-s 0 "eq_treble" "treble" 4.2 1 "dB" (ui-accent-orange))
      (ui-lego-num-s 0 "gain" "gain" 4.2 2 false (ui-accent-orange)))))

(def tone-block ()
  (ui-control-block-small-s "CHARACTER" (ui-accent-cyan) 0
    (h-stack :gap 0.30 :align :start
      (ui-lego-knob-s 0 "brightness" "bright" 4.8 (ui-accent-cyan) 2)
      (ui-lego-num-s 0 "brightness_vel" "vel>bright" 4.2 2 false (ui-accent-cyan))
      (ui-lego-num-s 0 "hardness_vel" "vel>stiff" 4.2 2 false (ui-accent-cyan)))))

(def damper-column ()
  (ui-lego-column-full
    (ui-control-block-full-s "DAMPER & SUSTAIN" (ui-accent-violet) 0
      (v-stack :gap 0.38 :align :center :width :fill
        (label "STRING DECAY" :font-size 11.0 :color (ui-accent-violet) :font-weight :bold)
        (h-stack :gap 0.32 :align :center
          (ui-lego-knob-s 0 "sustain_s" "sustain" 5.2 (ui-accent-violet) 1)
          (ui-lego-knob-s 0 "damper_decay" "damp" 5.2 (ui-accent-violet) 2))
        (label "SUSTAIN PEDAL" :font-size 10.0 :color (ui-accent-violet) :font-weight :bold)
        (ui-lego-option-s 0 "pedal_sustain" "damper pedal" 6.4 '("Pedal Off" "Pedal Down") (ui-accent-violet))))))

(def resonance-block ()
  (ui-control-block-medium-s "RESONANCE" (ui-accent-green) 1
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob-s 1 "soundboard" "board" 4.8 (ui-accent-green) 2)
      (ui-lego-knob-s 1 "sympathetic" "symp" 4.8 (ui-accent-green) 2)
      (ui-lego-knob-s 1 "duplex_ring" "duplex" 4.8 (ui-accent-green) 2))))

(def voicing-block ()
  (ui-control-block-small-s "VOICING & PAN" (ui-accent-blue) 1
    (h-stack :gap 0.30 :align :start
      (ui-lego-num-s 1 "unison_detune" "detune" 4.2 1 "ct" (ui-accent-blue))
      (ui-lego-num-s 1 "stereo_width" "width" 4.2 2 false (ui-accent-blue))
      (ui-lego-num-s 1 "key_pan" "keypan" 4.2 2 false (ui-accent-blue)))))

(def voicing-readout ()
  (ui-readout-block-small-s "PHYSICS PROFILE" (ui-accent-blue) 1
    (ui-lego-text-row-3
      (label "unison: dual string physical modeling" :font-size 9.0 :color (ui-accent-blue) :bg :transparent)
      (label "resonance: soundboard + sympathetic" :font-size 9.0 :color (ui-accent-green) :bg :transparent)
      (label "panning: dynamic key-to-stereo" :font-size 9.0 :color (ui-accent-violet) :bg :transparent))))

(defsynth-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (ui-lego-column
      (hammer-block)
      (eq-block)
      (tone-block))
    (damper-column)
    (ui-lego-column
      (resonance-block)
      (voicing-block)
      (voicing-readout))))
