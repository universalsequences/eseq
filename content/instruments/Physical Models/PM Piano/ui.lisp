(defsynth-ui
  (eseq.effects.physical-model-surface/panel "PM PIANO"
    (list
      '("FELT & HAMMER" ("hammer.hardness" "Hardness" 2 :linear) ("hammer.contact" "Contact" 2 :log))
      '("STRINGS" ("string.decay" "Decay" 2 :log) ("damper.release_s" "Release s" 2 :log))
      '("PIANO VOICING" ("voicing.register" "Register st" 1 :linear) ("voicing.lid" "Lid" 2 :linear))
      '("UNISON & TUNING" ("tuning.unison" "Unison ct" 1 :linear) ("tuning.width" "Width" 2 :linear)))
    (list
      (dict :title "Hammer"
        :view (lambda () (eseq.effects.physical-model-surface/piano-hammer-view))
        :controls (lambda () '(("hammer.position" "Strike position" 3) ("hammer.knock" "Knock" 2)
          ("hammer.velocity_tone" "Vel > tone" 2) ("hammer.velocity_curve" "Vel curve" 2)))
        :hint "Hard felt and short contact brighten the strike.")
      (dict :title "Strings"
        :view (lambda () (eseq.effects.physical-model-surface/piano-string-view))
        :controls (lambda () '(("string.damping" "Damping" 2) ("string.stiffness" "Stiffness" 2)
          ("string.aftersound" "Aftersound" 2)))
        :hint "Decay shapes held notes; Release shapes key-up tails.")
      (dict :title "Body"
        :view (lambda () (eseq.effects.physical-model-surface/piano-body-view))
        :controls (lambda () '(("body.color" "Wood color" 2) ("body.size" "Body size" 2)
          ("body.resonance" "Resonance" 2) ("body.low_hz" "Low mode Hz" 0) ("body.high_hz" "High mode Hz" 0)
          ("voicing.balance" "Register balance" 2)))
        :hint "Register changes the piano voice at the played pitch.")
      (dict :title "Tuning"
        :view (lambda () (eseq.effects.physical-model-surface/piano-tuning-view))
        :controls (lambda () '(("tuning.tune" "Tune cents" 1) ("tuning.stretch" "Tuning stretch" 2)))
        :hint "Stretch: 0 = equal temperament, 1 = recorded tuning.")
      (dict :title "Dampers"
        :view (lambda () (eseq.effects.physical-model-surface/piano-damper-view))
        :controls (lambda () '(("damper.release_s" "Release seconds" 2) ("damper.pedal" "Pedal lift" 2)
          ("damper.upper_free" "Free treble" 2) ("damper.key_noise" "Key-up noise" 2)))
        :hint "Pedal lift lets released strings ring freely.")
      (dict :title "Motion"
        :view (lambda () (eseq.effects.physical-model-surface/piano-motion-view))
        :controls (lambda () '(("motion.tremolo" "Tremolo" 2) ("motion.tremolo_hz" "Tremolo Hz" 2)
          ("motion.pan" "Auto pan" 2) ("motion.pan_hz" "Pan Hz" 2)))
        :hint "Tremolo and pan add independent rhythmic movement.")
      (dict :title "Swell"
        :view (lambda () (eseq.effects.physical-model-surface/piano-swell-view))
        :controls (lambda () '(("swell.amount" "Reverse blend" 2) ("swell.length_s" "Reverse seconds" 2)
          ("swell.curve" "Rise curve" 2) ("swell.tail" "Ringing tail" 2)))
        :hint "Hold or lift pedal; shape changes start next note.")
      (dict :title "Output"
        :view (lambda () (eseq.effects.physical-model-surface/piano-output-view))
        :controls (lambda () '(("output.drive" "Drive" 2) ("output.tone_hz" "Tone Hz" 0) ("output.gain" "Output" 2)))
        :hint "Tone and soft drive shape the entire instrument."))))
