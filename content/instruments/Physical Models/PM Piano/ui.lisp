(defsynth-ui
  (eseq.effects.physical-model-surface/panel "PM PIANO"
    (list
      '("FELT & HAMMER" ("hammer.hardness" "Hardness" 2 :linear) ("hammer.contact" "Contact" 2 :log))
      '("STRINGS" ("string.decay" "Decay" 2 :log) ("damper.release_s" "Release" 2 :log))
      '("PIANO VOICING" ("voicing.register" "Register" 1 :linear) ("voicing.lid" "Lid" 2 :linear))
      '("UNISON & TUNING" ("tuning.unison" "Unison" 1 :linear) ("tuning.width" "Width" 2 :linear)))
    (list
      (dict :title "Hammer"
        :view (lambda () (eseq.effects.physical-model-surface/piano-hammer-view))
        :controls (lambda () '(("hammer.position" "Strike position" 3) ("hammer.knock" "Knock" 2)
          ("hammer.velocity_tone" "Vel > tone" 2) ("hammer.velocity_curve" "Vel curve" 2))))
      (dict :title "Strings"
        :view (lambda () (eseq.effects.physical-model-surface/piano-string-view))
        :controls (lambda () '(("string.damping" "Damping" 2) ("string.stiffness" "Stiffness" 2)
          ("string.aftersound" "Aftersound" 2))))
      (dict :title "Body"
        :view (lambda () (eseq.effects.physical-model-surface/piano-body-view))
        :controls (lambda () '(("body.color" "Wood color" 2) ("body.size" "Body size" 2)
          ("body.resonance" "Resonance" 2) ("body.low_hz" "Low mode" 0) ("body.high_hz" "High mode" 0)
          ("voicing.balance" "Register balance" 2))))
      (dict :title "Tuning"
        :view (lambda () (eseq.effects.physical-model-surface/piano-tuning-view))
        :controls (lambda () '(("tuning.tune" "Tune" 1) ("tuning.stretch" "Tuning stretch" 2))))
      (dict :title "Dampers"
        :view (lambda () (eseq.effects.physical-model-surface/piano-damper-view))
        :controls (lambda () '(("damper.release_s" "Release" 2) ("damper.pedal" "Pedal lift" 2)
          ("damper.upper_free" "Free treble" 2) ("damper.key_noise" "Key-up noise" 2))))
      (dict :title "Motion"
        :view (lambda () (eseq.effects.physical-model-surface/piano-motion-view))
        :controls (lambda () '(("motion.tremolo" "Tremolo" 2) ("motion.tremolo_hz" "Tremolo rate" 2)
          ("motion.pan" "Auto pan" 2) ("motion.pan_hz" "Pan rate" 2))))
      (dict :title "Swell"
        :view (lambda () (eseq.effects.physical-model-surface/piano-swell-view))
        :controls (lambda () '(("swell.amount" "Reverse blend" 2) ("swell.length_s" "Reverse length" 2)
          ("swell.curve" "Rise curve" 2) ("swell.tail" "Ringing tail" 2))))
      (dict :title "Output"
        :view (lambda () (eseq.effects.physical-model-surface/piano-output-view))
        :controls (lambda () '(("output.drive" "Drive" 2) ("output.tone_hz" "Tone" 0) ("output.gain" "Output" 2)))))))
