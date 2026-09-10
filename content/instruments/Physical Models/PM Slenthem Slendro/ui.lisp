(defsynth-ui
  (eseq.effects.physical-model-surface/panel "PM SLENTHEM SLENDRO"
    (list
      '("MALLET" ("mallet.hardness" "Hardness" 2 :linear) ("mallet.contact" "Contact" 2 :log))
      '("BAR & TUBE" ("body.decay" "Decay" 2 :log) ("body.bloom" "Bloom" 2 :linear))
      '("MODES" ("body.inharmonicity" "Inharmonic" 2 :linear) ("voicing.register" "Register st" 1 :linear))
      '("DAMPING & LEVEL" ("damper.touch" "Hand damp" 2 :linear) ("output.gain" "Output" 2 :linear)))
    (list
      (dict :title "Mallet"
        :view (lambda () (eseq.effects.physical-model-surface/gamelan-mallet-view 0.72))
        :controls (lambda () '(("mallet.spread" "Mallet spread" 2) ("mallet.dynamics" "Vel > timbre" 2)))
        :hint "3 measured strike strengths; continuous force and color.")
      (dict :title "Resonance"
        :view (lambda () (eseq.effects.physical-model-surface/gamelan-body-view
          0.425263 0.0645991 0.374021))
        :controls (lambda () '(("body.loss" "Upper-mode loss" 2) ("body.color" "Spectral color" 2)))
        :hint "Representative measured mode: resonance build-up and natural loss.")
      (dict :title "Tuning"
        :view (lambda () (eseq.effects.physical-model-surface/gamelan-tuning-view 24.3574))
        :controls (lambda () '(("tuning.amount" "Recorded tuning" 2) ("tuning.tune" "Tune cents" 1)))
        :hint "Measured MIDI keys: 46, 48, 51, 53, 56, 58, 60.")
      (dict :title "Damping"
        :view (lambda () (eseq.effects.physical-model-surface/gamelan-damper-view 0.425263))
        :controls (lambda () '(("damper.release_s" "Release seconds" 2) ("damper.lift" "Hand lift" 2)))
        :hint "Hand lift lets key-up ring naturally; Hand damp mutes.")
      (dict :title "Output"
        :view (lambda () (eseq.effects.physical-model-surface/saron-output-view))
        :controls (lambda () '(("output.width" "Stereo spread" 2) ("output.drive" "Drive" 2) ("output.tone_hz" "Tone Hz" 0)))
        :hint "Stereo spread places the modes across the panorama."))))
