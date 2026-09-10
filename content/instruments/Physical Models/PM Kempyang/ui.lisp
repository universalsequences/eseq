(defsynth-ui
  (eseq.effects.physical-model-surface/panel "PM KEMPYANG"
    (list
      '("MALLET" ("mallet.hardness" "Hardness" 2 :linear) ("mallet.contact" "Contact" 2 :log))
      '("POT" ("body.decay" "Decay" 2 :log) ("body.bloom" "Bloom" 2 :linear))
      '("MODES" ("body.inharmonicity" "Inharmonic" 2 :linear) ("body.loss" "Upper loss" 2 :log))
      '("DAMPING & LEVEL" ("damper.touch" "Hand damp" 2 :linear) ("output.gain" "Output" 2 :linear)))
    (list
      (dict :title "Mallet"
        :view (lambda () (eseq.effects.physical-model-surface/gamelan-mallet-view 0.72))
        :controls (lambda () '(("mallet.spread" "Mallet spread" 2) ("mallet.dynamics" "Vel > timbre" 2)))
        :hint "3 measured strike strengths; continuous force and color.")
      (dict :title "Resonance"
        :view (lambda () (eseq.effects.physical-model-surface/gamelan-body-view
          2.72403 0.0918557 0.410344))
        :controls (lambda () '( ("body.color" "Spectral color" 2)))
        :hint "Representative measured mode: resonance build-up and natural loss.")
      (dict :title "Tuning"
        :view (lambda () (eseq.effects.physical-model-surface/gamelan-tuning-view -8.99276))
        :controls (lambda () '(("tuning.amount" "Recorded tuning" 2) ("tuning.tune" "Tune cents" 1)))
        :hint "Measured MIDI keys: 82. Other pitches transpose this pot.")
      (dict :title "Damping"
        :view (lambda () (eseq.effects.physical-model-surface/gamelan-damper-view 2.72403))
        :controls (lambda () '(("damper.release_s" "Release seconds" 2) ("damper.lift" "Hand lift" 2)))
        :hint "Hand lift lets key-up ring naturally; Hand damp mutes.")
      (dict :title "Output"
        :view (lambda () (eseq.effects.physical-model-surface/saron-output-view))
        :controls (lambda () '(("output.width" "Stereo spread" 2) ("output.drive" "Drive" 2) ("output.tone_hz" "Tone Hz" 0)))
        :hint "Stereo spread places the modes across the panorama."))))
