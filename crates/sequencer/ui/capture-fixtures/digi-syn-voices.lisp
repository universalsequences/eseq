;; Digi Syn's display on the VOICES screen (section 2).
(capture-project
  (track :instrument "factory:Synths/Digi Syn@2"))
(def capture-after-sync ()
  (do
    (custom-instrument-synth-ui (eseq.effects.panel-data/current-instrument-panel))
    ((eseq.effects.custom-ui-sections/ui-section-select-callback 2) false)))
