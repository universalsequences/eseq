(def fm-vowel-options ()
  '("A" "E" "I" "O" "U"))

(def fm-op1-block ()
  (ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (ui-lego-badge-s 0 "OP1" 3.6 (ui-accent-cyan))
          (ui-lego-micro-num-s 0 "op1_ratio" "ratio" 3.0 2 false (ui-accent-cyan))
          (ui-lego-micro-num-s 0 "op1_detune" "det" 3.0 0 "ct" (ui-accent-orange)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 0 "op2_to_op1" "2>1" 3.1 2 false (ui-accent-violet))
          (ui-lego-micro-num-s 0 "op3_to_op1" "3>1" 3.1 2 false (ui-accent-violet))))
      (h-stack :gap 0.08 :align :start
        (ui-lego-knob-s 0 "op1_level" "lvl" 3.7 (ui-accent-cyan) 2)
        (ui-lego-knob-s 0 "op2_to_op1" "2>1" 3.7 (ui-accent-violet) 2)
        (ui-lego-knob-s 0 "op3_to_op1" "3>1" 3.7 (ui-accent-violet) 2)))))

(def fm-op2-block ()
  (ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (ui-lego-badge-s 0 "OP2" 3.6 (ui-accent-blue))
          (ui-lego-micro-num-s 0 "op2_ratio" "ratio" 3.0 2 false (ui-accent-blue))
          (ui-lego-micro-num-s 0 "op2_detune" "det" 3.0 0 "ct" (ui-accent-orange)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 0 "mod_vel_amt" "mvel" 3.1 2 false (ui-accent-orange))
          (ui-lego-micro-num-s 0 "amp_vel_amt" "avel" 3.1 2 false (ui-accent-orange))))
      (h-stack :gap 0.08 :align :start
        (ui-lego-knob-s 0 "op2_level" "lvl" 3.7 (ui-accent-blue) 2)
        (ui-lego-knob-s 0 "op3_to_op2" "3>2" 3.7 (ui-accent-violet) 2)
        (ui-lego-knob-s 0 "op2_to_op1" "2>1" 3.7 (ui-accent-violet) 2)))))

(def fm-op3-block ()
  (ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (ui-lego-badge-s 0 "OP3" 3.6 (ui-accent-violet))
          (ui-lego-micro-num-s 0 "op3_ratio" "ratio" 3.0 2 false (ui-accent-violet))
          (ui-lego-micro-num-s 0 "op3_detune" "det" 3.0 0 "ct" (ui-accent-orange)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 0 "op4_to_op3" "4>3" 3.1 2 false (ui-accent-violet))
          (ui-lego-micro-num-s 0 "op3_to_op2" "3>2" 3.1 2 false (ui-accent-violet))))
      (h-stack :gap 0.08 :align :start
        (ui-lego-knob-s 0 "op3_level" "lvl" 3.7 (ui-accent-violet) 2)
        (ui-lego-knob-s 0 "op3_to_op2" "3>2" 3.7 (ui-accent-violet) 2)
        (ui-lego-knob-s 0 "op3_to_op1" "3>1" 3.7 (ui-accent-violet) 2)))))

(def fm-op4-block ()
  (ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (ui-lego-badge-s 0 "OP4" 3.6 (ui-accent-green))
          (ui-lego-micro-num-s 0 "op4_ratio" "ratio" 3.0 2 false (ui-accent-green))
          (ui-lego-micro-num-s 0 "op4_detune" "det" 3.0 0 "ct" (ui-accent-orange)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 0 "op4_to_op3" "4>3" 3.1 2 false (ui-accent-violet))
          (ui-lego-micro-num-s 0 "op4_feedback" "fb" 3.1 2 false (ui-accent-orange))))
      (h-stack :gap 0.08 :align :start
        (ui-lego-knob-s 0 "op4_level" "lvl" 3.7 (ui-accent-green) 2)
        (ui-lego-knob-s 0 "op4_to_op3" "4>3" 3.7 (ui-accent-violet) 2)
        (ui-lego-knob-s 0 "op4_feedback" "fb" 3.7 (ui-accent-orange) 2)))))

(def fm-formant-block ()
  (ui-control-panel-dense-s 1
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (ui-lego-badge-s 1 "VOW" 3.6 (ui-accent-green))
          (ui-lego-micro-option-s 1 "vowel" "vowel" 4.4 (fm-vowel-options) (ui-accent-green)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 1 "formant_env_shift" "env" 3.1 1 "st" (ui-accent-blue))
          (ui-lego-micro-num-s 1 "body_tone" "tone" 3.4 0 "Hz" (ui-accent-orange))))
      (h-stack :gap 0.08 :align :start
        (ui-lego-knob-s 1 "formant_shift" "shift" 3.7 (ui-accent-green) 1)
        (ui-lego-knob-s 1 "formant_q" "Q" 3.7 (ui-accent-green) 1)
        (ui-lego-knob-s 1 "formant_mix" "mix" 3.7 (ui-accent-cyan) 2)))))

(def fm-lfo-block ()
  (ui-control-panel-small-s 2
    (h-stack :gap 0.18 :align :start
      (ui-lego-badge-s 2 "LFO" 3.6 (ui-accent-blue))
      (ui-lego-micro-num-s 2 "lfo_rate" "rate" 3.1 2 "Hz" (ui-accent-blue))
      (ui-lego-micro-num-s 2 "lfo_to_pitch" "pitch" 3.1 2 "st" (ui-accent-orange))
      (ui-lego-micro-num-s 2 "lfo_to_index" "index" 3.1 2 false (ui-accent-violet))
      (ui-lego-micro-num-s 2 "lfo_to_formant" "vow" 3.1 1 "st" (ui-accent-green)))))

(def fm-output-block ()
  (ui-control-panel-small-s 1
    (h-stack :gap 0.18 :align :start
      (ui-lego-badge-s 1 "OUT" 3.6 (ui-accent-orange))
      (ui-lego-micro-base-note-s 1 3.0 (ui-accent-orange))
      (ui-lego-micro-num-s 1 "formant_drive" "drv" 3.0 2 false (ui-accent-orange))
      (ui-lego-micro-num-s 1 "body_tone" "tone" 3.0 0 "Hz" (ui-accent-green))
      (ui-lego-micro-num-s 1 "output_gain" "gain" 3.0 2 false (ui-accent-orange)))))

(def fm-env-column ()
  (ui-lego-column-full
    (box :width (ui-lego-col-w) :height (ui-lego-full-h)
      (ui-adsr-switch
        0 "AMP ENV" "amp_attack" "amp_decay" "amp_sustain" "amp_release"
        1 "MOD ENV" "mod_attack" "mod_decay" "mod_sustain" "mod_release"))))

(defsynth-ui
  (h-stack :width :fill :gap 0.30 :align :stretch
    (ui-lego-column
      (fm-op1-block)
      (fm-op2-block)
      (fm-lfo-block))
    (ui-lego-column
      (fm-op3-block)
      (fm-op4-block)
      (fm-output-block))
    (ui-lego-column-2
      (fm-formant-block)
      (fm-output-block))
    (fm-env-column)))
