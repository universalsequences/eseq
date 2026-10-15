;; Shared instrument controls retain host preset and parameter-lock behavior.
(defsynth-ui
  (eseq.effects.physical-model-surface/panel "BOOM BAP KICK"
    (list
      '("SWEEP" ("frequency" "Root" 2 :linear) ("bend_fast" "Fast bend" 2 :linear))
      '("BODY" ("attack_ms" "Attack" 2 :linear) ("mode1_decay" "Decay" 2 :linear))
      '("MODE 2" ("mode2_gain" "Level" 2 :linear) ("mode2_ratio" "Ratio" 2 :linear))
      '("MODE 3" ("mode3_gain" "Level" 2 :linear) ("mode3_ratio" "Ratio" 2 :linear))
    )
    (list
      (dict :title "Sweep"
        :view (lambda () (box))
        :controls (lambda () '(("frequency" "Root" 2) ("bend_fast" "Fast depth" 2) ("fast_ms" "Fast time" 2) ("bend_slow" "Slow depth" 2) ("slow_ms" "Slow time" 2))))
      (dict :title "Body"
        :view (lambda () (box))
        :controls (lambda () '(("mode1_gain" "Body level" 2) ("attack_ms" "Attack" 2) ("mode1_decay" "Decay" 2) ("mode1_phase" "Phase" 2))))
      (dict :title "Mode 2"
        :view (lambda () (box))
        :controls (lambda () '(("mode2_ratio" "Pitch ratio" 2) ("mode2_gain" "Level" 2) ("mode2_decay" "Decay" 2) ("mode2_phase" "Phase" 2) ("mode2_bend" "Sweep scale" 2) ("mode2_attack_ms" "Attack" 2))))
      (dict :title "Mode 3"
        :view (lambda () (box))
        :controls (lambda () '(("mode3_ratio" "Pitch ratio" 2) ("mode3_gain" "Level" 2) ("mode3_decay" "Decay" 2) ("mode3_phase" "Phase" 2) ("mode3_bend" "Sweep scale" 2) ("mode3_attack_ms" "Attack" 2))))
      (dict :title "Output"
        :view (lambda () (box))
        :controls (lambda () '(("level" "Output level" 2) ("saturation" "Saturation" 2) ("shape" "Knee shape" 2) ("hold_ms" "Hold" 2) ("fade_ms" "Fade" 2))))
    )))
