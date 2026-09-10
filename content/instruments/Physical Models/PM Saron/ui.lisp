(defsynth-ui
  (eseq.effects.physical-model-surface/panel "PM SARON"
    (list
      '("MALLET" ("mallet.hardness" "Hardness" 2 :linear) ("mallet.contact" "Contact" 2 :log))
      '("BRONZE BAR" ("bar.decay" "Decay" 2 :log) ("bar.bloom" "Bloom" 2 :linear))
      '("MODES & REGISTER" ("bar.inharmonicity" "Inharmonic" 2 :linear) ("voicing.register" "Register st" 1 :linear))
      '("DAMPING & LEVEL" ("damper.touch" "Hand damp" 2 :linear) ("output.gain" "Output" 2 :linear)))
    (list
      (dict :title "Mallet"
        :view (lambda () (eseq.effects.physical-model-surface/saron-mallet-view))
        :controls (lambda () '(("mallet.spread" "Mallet spread" 2) ("mallet.dynamics" "Vel > timbre" 2)))
        :hint "Five measured strike strengths blend continuously.")
      (dict :title "Bar"
        :view (lambda () (eseq.effects.physical-model-surface/saron-bar-view))
        :controls (lambda () '(("bar.loss" "Upper-mode loss" 2) ("bar.color" "Spectral color" 2)))
        :hint "Bloom builds resonance; Decay controls the free ring.")
      (dict :title "Tuning"
        :view (lambda () (eseq.effects.physical-model-surface/saron-tuning-view))
        :controls (lambda () '(("tuning.amount" "Recorded tuning" 2) ("tuning.tune" "Tune cents" 1)))
        :hint "Bars 1-7: D5 Eb5 F5 Ab5 A5 Bb5 C6. Other keys blend.")
      (dict :title "Damping"
        :view (lambda () (eseq.effects.physical-model-surface/saron-damper-view))
        :controls (lambda () '(("damper.release_s" "Release seconds" 2) ("damper.lift" "Hand lift" 2)))
        :hint "Hand lift keeps key-up tails ringing; Hand damp mutes.")
      (dict :title "Output"
        :view (lambda () (eseq.effects.physical-model-surface/saron-output-view))
        :controls (lambda () '(("output.width" "Stereo spread" 2) ("output.drive" "Drive" 2)
          ("output.tone_hz" "Tone Hz" 0)))
        :hint "Stereo spread places the modes across the panorama."))))
