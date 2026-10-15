(defsynth-ui
  (eseq.effects.physical-model-surface/panel "BREAK KICK"
    (list
      '("BODY" ("tune" "Tune" 2 :log) ("decay" "Decay" 2 :log))
      '("IMPACT" ("attack" "Attack" 2 :log) ("bend" "Pitch motion" 2 :linear))
      '("AIR" ("air" "Air" 2 :linear) ("air_ms" "Air decay" 1 :log))
      '("RECORDING" ("drive" "Drive" 2 :log) ("level" "Output" 2 :linear)))
    (list
      (dict :title "Body"
        :view (lambda () (box))
        :controls (lambda () '(("body" "Body level" 2) ("knock" "Knock level" 2) ("length_ms" "Total length" 1))))
      (dict :title "Impact"
        :view (lambda () (box))
        :controls (lambda () '(("attack" "Attack scale" 2) ("bend" "Motion scale" 2))))
      (dict :title "Air"
        :view (lambda () (box))
        :controls (lambda () '(("air" "Air amount" 2) ("air_ms" "Air decay" 1) ("air_hz" "Air cutoff" 0))))
      (dict :title "Recording"
        :view (lambda () (box))
        :controls (lambda () '(("clip_mix" "Clip mix" 2) ("ceiling" "Clip ceiling" 2) ("length_ms" "Length" 1) ("level" "Output" 2)))))))
