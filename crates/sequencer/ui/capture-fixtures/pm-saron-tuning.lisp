(capture-project
  (track :instrument "factory:Physical Models/PM Saron"))

(def capture-after-sync ()
  ((eseq.effects.custom-ui-sections/ui-section-select-callback 2) false))
