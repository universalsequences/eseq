(capture-project (track :instrument "factory:Physical Models/PM Crash"))
(def capture-after-sync ()
  (do
    ((eseq.effects.custom-ui-sections/ui-section-select-callback 1) false)
    (set! eseq.effects.state/instrument-mods-open true)))
