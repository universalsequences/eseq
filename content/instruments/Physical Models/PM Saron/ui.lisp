(defsynth-ui
  (eseq.effects.physical-model-surface/panel "PM SARON"
    (list
      '("MALLET" ("mallet.hardness" "Hardness" 2 :linear) ("mallet.contact" "Contact" 2 :log))
      '("BRONZE BAR" ("bar.decay" "Decay" 2 :log) ("bar.bloom" "Bloom" 2 :linear))
      '("MODES & REGISTER" ("bar.inharmonicity" "Inharmonic" 2 :linear) ("voicing.register" "Register" 1 :linear))
      '("DAMPING & LEVEL" ("damper.touch" "Hand damp" 2 :linear) ("output.gain" "Output" 2 :linear)))
    (list
      (dict :title "Mallet"
        :view (lambda () (eseq.effects.physical-model-surface/saron-mallet-view))
        :controls (lambda () '(("mallet.spread" "Mallet spread" 2) ("mallet.dynamics" "Vel > timbre" 2))))
      (dict :title "Bar"
        :view (lambda () (eseq.effects.physical-model-surface/saron-bar-view))
        :controls (lambda () '(("bar.loss" "Upper-mode loss" 2) ("bar.color" "Spectral color" 2))))
      (dict :title "Tuning"
        :view (lambda () (eseq.effects.physical-model-surface/saron-tuning-view))
        :controls (lambda () '(("tuning.amount" "Recorded tuning" 2) ("tuning.tune" "Tune" 1))))
      (dict :title "Damping"
        :view (lambda () (eseq.effects.physical-model-surface/saron-damper-view))
        :controls (lambda () '(("damper.release_s" "Release" 2) ("damper.lift" "Hand lift" 2))))
      (dict :title "Output"
        :view (lambda () (eseq.effects.physical-model-surface/saron-output-view))
        :controls (lambda () '(("output.width" "Stereo spread" 2) ("output.drive" "Drive" 2)
          ("output.tone_hz" "Tone" 0)))))))
