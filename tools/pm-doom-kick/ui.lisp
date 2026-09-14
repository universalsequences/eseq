;; Shared instrument controls retain host preset and parameter-lock behavior.
(defsynth-ui
  (eseq.effects.physical-model-surface/panel "DOOM KICK"
    (list
      '("SWEEP" ("frequency" "Root Hz" 2 :linear) ("bend_fast" "Fast bend" 2 :linear))
      '("BODY" ("attack_ms" "Attack ms" 2 :linear) ("mode1_decay" "Decay ms" 2 :linear))
      '("MODE 2" ("mode2_gain" "Level" 2 :linear) ("mode2_ratio" "Ratio" 2 :linear))
      '("MODE 3" ("mode3_gain" "Level" 2 :linear) ("mode3_ratio" "Ratio" 2 :linear))
    )
    (list
      (dict :title "Sweep"
        :view (lambda () (v-stack :gap 0.4 :padding 0.3
          (label "Two falling pitch sweeps shape the punch." :height 0.7 :font-size 10 :bg :transparent :v-align :center :color (rgba 0.16 0.06 0.11 1))
          (label "Depth multiplies the root frequency." :height 0.7 :font-size 10 :bg :transparent :v-align :center :color (rgba 0.16 0.06 0.11 1))
        ))
        :controls (lambda () '(("frequency" "Root Hz" 2) ("bend_fast" "Fast depth" 2) ("fast_ms" "Fast ms" 2) ("bend_slow" "Slow depth" 2) ("slow_ms" "Slow ms" 2)))
        :hint "Play A3 for the fitted pitch; edits apply on the next hit.")
      (dict :title "Body"
        :view (lambda () (v-stack :gap 0.4 :padding 0.3
          (label "The main resonance supplies the low body." :height 0.7 :font-size 10 :bg :transparent :v-align :center :color (rgba 0.16 0.06 0.11 1))
          (label "Phase changes how the attack meets the other modes." :height 0.7 :font-size 10 :bg :transparent :v-align :center :color (rgba 0.16 0.06 0.11 1))
        ))
        :controls (lambda () '(("mode1_gain" "Body level" 2) ("attack_ms" "Attack ms" 2) ("mode1_decay" "Decay ms" 2) ("mode1_phase" "Phase radians" 2)))
        :hint "Decay is an exponential time constant, not total duration.")
      (dict :title "Mode 2"
        :view (lambda () (v-stack :gap 0.4 :padding 0.3
          (label "A second resonance adds weight and beating." :height 0.7 :font-size 10 :bg :transparent :v-align :center :color (rgba 0.16 0.06 0.11 1))
          (label "Ratio 1 follows the body pitch; sweep scale sets its bend." :height 0.7 :font-size 10 :bg :transparent :v-align :center :color (rgba 0.16 0.06 0.11 1))
        ))
        :controls (lambda () '(("mode2_ratio" "Pitch ratio" 2) ("mode2_gain" "Level" 2) ("mode2_decay" "Decay ms" 2) ("mode2_phase" "Phase radians" 2) ("mode2_bend" "Sweep scale" 2) ("mode2_attack_ms" "Attack ms" 2)))
        :hint "All controls apply on the next hit.")
      (dict :title "Mode 3"
        :view (lambda () (v-stack :gap 0.4 :padding 0.3
          (label "The upper resonance shapes the transient." :height 0.7 :font-size 10 :bg :transparent :v-align :center :color (rgba 0.16 0.06 0.11 1))
          (label "Raise its level to hear ratio and phase changes clearly." :height 0.7 :font-size 10 :bg :transparent :v-align :center :color (rgba 0.16 0.06 0.11 1))
        ))
        :controls (lambda () '(("mode3_ratio" "Pitch ratio" 2) ("mode3_gain" "Level" 2) ("mode3_decay" "Decay ms" 2) ("mode3_phase" "Phase radians" 2) ("mode3_bend" "Sweep scale" 2) ("mode3_attack_ms" "Attack ms" 2)))
        :hint "All controls apply on the next hit.")
      (dict :title "Output"
        :view (lambda () (v-stack :gap 0.4 :padding 0.3
          (label "Saturation rounds or flattens the combined resonances." :height 0.7 :font-size 10 :bg :transparent :v-align :center :color (rgba 0.16 0.06 0.11 1))
          (label "Higher knee shape makes clipping harder." :height 0.7 :font-size 10 :bg :transparent :v-align :center :color (rgba 0.16 0.06 0.11 1))
        ))
        :controls (lambda () '(("level" "Output level" 2) ("saturation" "Saturation" 2) ("shape" "Knee shape" 2) ("hold_ms" "Hold ms" 2) ("fade_ms" "Fade ms" 2)))
        :hint "Hold then fade ends the hit; note-off does not cut it.")
    )))
