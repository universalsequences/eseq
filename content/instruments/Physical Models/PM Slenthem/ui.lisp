(defsynth-ui
  (eseq.effects.physical-model-surface/panel "PM SLENTHEM"
    (list
      '("MALLET" ("mallet.hardness" "Hardness" 2 :linear) ("mallet.contact" "Contact" 2 :log))
      '("BAR & TUBE" ("body.decay" "Decay" 2 :log) ("body.bloom" "Bloom" 2 :linear))
      '("MODES" ("body.inharmonicity" "Inharmonic" 2 :linear) ("voicing.register" "Register" 1 :linear))
      '("DAMPING & LEVEL" ("damper.touch" "Hand damp" 2 :linear) ("output.gain" "Output" 2 :linear)))
    (list
      (dict :title "Mallet"
        :view (lambda () (eseq.effects.physical-model-surface/gamelan-mallet-view))
        :controls (lambda () '(("mallet.spread" "Mallet spread" 2) ("mallet.dynamics" "Vel > timbre" 2))))
      (dict :title "Resonance"
        :view (lambda () (eseq.effects.physical-model-surface/gamelan-body-view
          1.14178 0.0667088 2.4875e-08))
        :controls (lambda () '(("body.loss" "Upper-mode loss" 2) ("body.color" "Spectral color" 2))))
      (dict :title "Tuning"
        :view (lambda () (eseq.effects.physical-model-surface/gamelan-tuning-view -36.971))
        :controls (lambda () '(("tuning.amount" "Recorded tuning" 2) ("tuning.tune" "Tune" 1))))
      (dict :title "Damping"
        :view (lambda () (eseq.effects.physical-model-surface/gamelan-damper-view 1.14178))
        :controls (lambda () '(("damper.release_s" "Release" 2) ("damper.lift" "Hand lift" 2))))
      (dict :title "Output"
        :view (lambda () (eseq.effects.physical-model-surface/saron-output-view))
        :controls (lambda () '(("output.width" "Stereo spread" 2) ("output.drive" "Drive" 2) ("output.tone_hz" "Tone" 0)))))))
