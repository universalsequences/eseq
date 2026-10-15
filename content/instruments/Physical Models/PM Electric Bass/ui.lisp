(defsynth-ui
  (eseq.effects.physical-model-surface/panel "PM ELECTRIC BASS"
    (list
      '("PLUCK" ("pluck.position" "Position" 2 :linear) ("pluck.softness" "Softness" 2 :linear))
      '("STRING" ("string.mute" "Mute" 2 :linear) ("string.damping" "Damping" 2 :linear))
      '("PICKUP" ("pickup.position" "Position" 2 :linear) ("pickup.aperture" "Width" 3 :linear))
      '("OUTPUT" ("output.tone_hz" "Tone" 0 :log) ("texture.slow" "Slow" 1 :linear)))
    (list
      (dict :title "Pluck"
        :view (lambda () (eseq.effects.physical-model-surface/column-view
          (eseq.effects.physical-model-surface/bind "pluck.position") "Pluck"))
        :controls (lambda () '(("pluck.velocity_tone" "Velocity > brightness" 2) ("pluck.attack_ms" "Fundamental delay" 0))))
      (dict :title "String"
        :view (lambda () (box))
        :controls (lambda () '(("string.decay_s" "Open decay" 2) ("string.stiffness" "Stiffness" 2) ("string.friction" "Friction" 1)
          ("string.release_s" "Note-off decay" 3) ("string.tune" "Tune" 1))))
      (dict :title "Pickup"
        :view (lambda () (eseq.effects.physical-model-surface/column-view
          (eseq.effects.physical-model-surface/bind "pickup.position") "Pickup"))
        :controls (lambda () '(("pickup.position" "Pickup position" 3) ("pickup.aperture" "Pickup width" 3)
          ("vinyl.hiss" "Vinyl hiss" 2) ("vinyl.rumble" "Vinyl rumble" 2))))
      (dict :title "Output"
        :view (lambda () (box))
        :controls (lambda () '(("output.resonance" "Tone resonance" 2) ("output.steep" "Tone steep" 2) ("output.gain" "Output" 2)
          ("texture.grain_ms" "Slice" 0) ("texture.xfade_ms" "Slice crossfade" 1)))))))
