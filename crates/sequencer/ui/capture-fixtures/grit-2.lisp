(capture-project (track :instrument "factory:Synths/Grit"))
(def capture-after-sync ()
  (do (custom-instrument-synth-ui (nth SEQ.instrument-panel 0))
    ((eseq.effects.custom-ui-sections/ui-section-select-callback 2) false)))
