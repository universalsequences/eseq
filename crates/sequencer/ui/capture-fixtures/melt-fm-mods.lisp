(capture-project (track :instrument "factory:Synths/Melt"))
(def capture-after-sync ()
  (do
    ((eseq.effects.custom-ui-sections/ui-section-select-callback 4) false)
    (set! eseq.effects.state/instrument-mods-open true)))
