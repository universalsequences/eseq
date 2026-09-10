(capture-project
  (track :instrument "factory:Physical Models/PM Saxophone"))
(def capture-after-sync ()
  ((eseq.effects.custom-ui-sections/ui-section-select-callback 5) false))
