(capture-project (track :instrument "factory:Synths/Vox"))
(def capture-after-sync ()
  (do (custom-instrument-synth-ui (eseq.effects.panel-data/current-instrument-panel))
    ((eseq.effects.custom-ui-sections/ui-section-select-callback 5) false)))
