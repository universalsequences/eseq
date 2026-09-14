(defsynth-ui
  (eseq.effects.physical-model-surface/panel "BREAK KICK 53"
    (list
      '("BODY" ("tune" "Tune" 2 :log) ("decay" "Decay" 2 :log))
      '("IMPACT" ("attack" "Attack" 2 :log) ("bend" "Pitch motion" 2 :linear))
      '("AIR" ("air" "Air" 2 :linear) ("air_ms" "Air ms" 1 :log))
      '("RECORDING" ("drive" "Drive" 2 :log) ("level" "Output" 2 :linear)))
    (list
      (dict :title "Body"
        :view (lambda () (v-stack :gap 0.4 :padding 0.3
          (label "Overlapping low resonances and a long ringing tail." :height 0.7 :font-size 10 :bg :transparent :color (rgba 0.16 0.06 0.11 1))
          (label "Body balances the lower modes; knock adds upper weight." :height 0.7 :font-size 10 :bg :transparent :color (rgba 0.16 0.06 0.11 1))))
        :controls (lambda () '(("body" "Body level" 2) ("knock" "Knock level" 2) ("length_ms" "Total length ms" 1)))
        :hint "A3 is the fitted register. All edits apply on the next hit.")
      (dict :title "Impact"
        :view (lambda () (v-stack :gap 0.4 :padding 0.3
          (label "Attack scales the rise time of each resonance." :height 0.7 :font-size 10 :bg :transparent :color (rgba 0.16 0.06 0.11 1))
          (label "Pitch motion scales their initial bends and settling." :height 0.7 :font-size 10 :bg :transparent :color (rgba 0.16 0.06 0.11 1))))
        :controls (lambda () '(("attack" "Attack scale" 2) ("bend" "Motion scale" 2)))
        :hint "Start near 1 to keep the fitted character.")
      (dict :title "Air"
        :view (lambda () (v-stack :gap 0.4 :padding 0.3
          (label "A fresh noise burst adds roughness to the attack." :height 0.7 :font-size 10 :bg :transparent :color (rgba 0.16 0.06 0.11 1))
          (label "This texture is synthesized independently on each hit." :height 0.7 :font-size 10 :bg :transparent :color (rgba 0.16 0.06 0.11 1))))
        :controls (lambda () '(("air" "Air amount" 2) ("air_ms" "Air decay ms" 1) ("air_hz" "Air cutoff Hz" 0)))
        :hint "Raise Air to hear cutoff and decay changes clearly.")
      (dict :title "Recording"
        :view (lambda () (v-stack :gap 0.4 :padding 0.3
          (label "Drive colours the upper attack; the low body stays separate." :height 0.7 :font-size 10 :bg :transparent :color (rgba 0.16 0.06 0.11 1))
          (label "Air stays clear of the recording clipping stage." :height 0.7 :font-size 10 :bg :transparent :color (rgba 0.16 0.06 0.11 1))))
        :controls (lambda () '(("clip_mix" "Recording clip mix" 2) ("ceiling" "Clip ceiling" 2) ("length_ms" "Length ms" 1) ("level" "Output" 2)))
        :hint "Drive 1 is clean. Clip mix 0 removes recording clipping."))))
