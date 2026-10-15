(defsynth-ui
  (eseq.effects.physical-model-surface/panel "PM CELLO"
    (list
      '("BOW CONTACT" ("bow.pressure" "Pressure" 2 :linear) ("bow.speed" "Bow speed" 3 :linear))
      '("STRING" ("string.position" "Contact pos" 3 :linear) ("string.decay_s" "Decay" 2 :log))
      '("WOOD & BODY" ("body.wood" "Wood" 2 :linear) ("body.tone_hz" "Tone" 0 :log))
      '("EXPRESSION" ("expression.vib_cent" "Vibrato depth" 1 :linear) ("section.blend" "Section" 2 :linear)))
    (list
      (dict :title "Bow"
        :view (lambda () (eseq.effects.physical-model-surface/bow-view))
        :controls (lambda () '(("bow.amount" "Bow contact" 2) ("bow.rosin" "Rosin curve" 2)
          ("bow.noise" "Bow texture" 3) ("bow.stroke_ms" "Stroke limit" 0))))
      (dict :title "String"
        :view (lambda () (eseq.effects.physical-model-surface/column-view
          (eseq.effects.physical-model-surface/bind "string.position") "Contact point"))
        :controls (lambda () '(("string.damping_hz" "Damping" 0) ("string.stiffness" "Stiffness" 3)
          ("string.tune" "Tune" 1))))
      (dict :title "Body"
        :view (lambda () (eseq.effects.physical-model-surface/cello-body-view))
        :controls (lambda () '(("body.size" "Body size" 2) ("body.resonance" "Resonance Q" 2)
          ("body.low_hz" "Low mode" 0) ("body.low" "Low mode level" 2))))
      (dict :title "Motion"
        :view (lambda () (eseq.effects.physical-model-surface/vibrato-view))
        :controls (lambda () '(("expression.vib_hz" "Vibrato rate" 2) ("expression.vib_wait" "Vibrato delay" 0)
          ("expression.vel_bow" "Velocity > bow" 2))))
      (dict :title "Pluck"
        :view (lambda () (eseq.effects.physical-model-surface/pluck-view))
        :controls (lambda () '(("pluck.strength" "Pluck strength" 2) ("pluck.width_ms" "Contact" 2)
          ("pluck.texture" "Pluck texture" 2))))
      (dict :title "Modes"
        :view (lambda () (eseq.effects.physical-model-surface/cello-body-view))
        :controls (lambda () '(("body.mid_hz" "Mid mode" 0) ("body.high_hz" "High mode" 0)
          ("body.air_hz" "Air mode" 0) ("body.mid" "Mid mode level" 2)
          ("body.high" "High mode level" 2) ("body.air" "Air mode level" 2))))
      (dict :title "Section"
        :view (lambda () (eseq.effects.physical-model-surface/section-view))
        :controls (lambda () '(("section.spread" "Spread" 1) ("section.lag_ms" "Entry spacing" 1)
          ("section.width" "Stereo width" 2))))
      (dict :title "Amp"
        :view (lambda () (eseq.effects.physical-model-surface/envelope-titled 7 "Bow speed"))
        :controls (lambda () '(("amp.attack" "Attack" 1) ("amp.decay" "Decay" 1) ("amp.sustain" "Sustain" 2)
          ("amp.release" "Release" 1) ("gain" "Output" 2)))))))
