(defsynth-ui
  (eseq.effects.physical-model-surface/panel "PM CELLO"
    (list
      '("BOW CONTACT" ("bow.pressure" "Pressure" 2 :linear) ("bow.speed" "Bow speed" 3 :linear))
      '("STRING" ("string.position" "Contact pos" 3 :linear) ("string.decay_s" "Decay s" 2 :log))
      '("WOOD & BODY" ("body.wood" "Wood" 2 :linear) ("body.tone_hz" "Tone Hz" 0 :log))
      '("EXPRESSION" ("expression.vib_cent" "Vibrato ct" 1 :linear) ("section.blend" "Section" 2 :linear)))
    (list
      (dict :title "Bow"
        :view (lambda () (eseq.effects.physical-model-surface/bow-view))
        :controls (lambda () '(("bow.amount" "Bow contact" 2) ("bow.rosin" "Rosin curve" 2)
          ("bow.noise" "Bow texture" 3) ("bow.stroke_ms" "Stroke limit ms" 0)))
        :hint "Stroke limit 0 follows the held note; Bow contact 0 lifts the bow.")
      (dict :title "String"
        :view (lambda () (eseq.effects.physical-model-surface/column-view
          (eseq.effects.physical-model-surface/bind "string.position") "Contact point / bridge to finger"))
        :controls (lambda () '(("string.damping_hz" "Damping Hz" 0) ("string.stiffness" "Stiffness" 3)
          ("string.tune" "Tune cents" 1)))
        :hint "Bowing and plucking excite the same string at the contact point.")
      (dict :title "Body"
        :view (lambda () (eseq.effects.physical-model-surface/cello-body-view))
        :controls (lambda () '(("body.size" "Body size" 2) ("body.resonance" "Resonance Q" 2)
          ("body.low_hz" "Low mode Hz" 0) ("body.low" "Low mode level" 2)))
        :hint "Larger bodies lower all four modes; Modes edits the upper three.")
      (dict :title "Motion"
        :view (lambda () (eseq.effects.physical-model-surface/vibrato-view))
        :controls (lambda () '(("expression.vib_hz" "Vibrato Hz" 2) ("expression.vib_wait" "Vibrato delay ms" 0)
          ("expression.vel_bow" "Velocity > bow" 2)))
        :hint "Vibrato fades in after its delay; velocity also sets output level.")
      (dict :title "Pluck"
        :view (lambda () (eseq.effects.physical-model-surface/pluck-view))
        :controls (lambda () '(("pluck.strength" "Pluck strength" 2) ("pluck.width_ms" "Contact ms" 2)
          ("pluck.texture" "Pluck texture" 2)))
        :hint "A pluck fires on each trigger and can be combined with the bow.")
      (dict :title "Modes"
        :view (lambda () (eseq.effects.physical-model-surface/cello-body-view))
        :controls (lambda () '(("body.mid_hz" "Mid mode Hz" 0) ("body.high_hz" "High mode Hz" 0)
          ("body.air_hz" "Air mode Hz" 0) ("body.mid" "Mid mode level" 2)
          ("body.high" "High mode level" 2) ("body.air" "Air mode level" 2)))
        :hint "Independent wood resonances shape each articulation's color.")
      (dict :title "Section"
        :view (lambda () (eseq.effects.physical-model-surface/section-view))
        :controls (lambda () '(("section.spread" "Spread cents" 1) ("section.lag_ms" "Entry spacing ms" 1)
          ("section.width" "Stereo width" 2)))
        :hint "Section adds two players of the same model with spread and stagger.")
      (dict :title "Amp"
        :view (lambda () (eseq.effects.physical-model-surface/envelope-titled 7 "Bow speed envelope"))
        :controls (lambda () '(("amp.attack" "Attack ms" 1) ("amp.decay" "Decay ms" 1) ("amp.sustain" "Sustain" 2)
          ("amp.release" "Release ms" 1) ("gain" "Output" 2)))
        :hint "The contour drives the bow; String decay controls the free ring."))))
