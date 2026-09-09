(capture-project (track :instrument "factory:Synths/Digi FM"))
(def capture-after-sync ()
  (do
    (custom-instrument-synth-ui (nth SEQ.instrument-panel 0))
    ((eseq.effects.custom-ui-sections/ui-section-select-callback 5) false)))
