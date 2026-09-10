;; Both instruments use section 3 for their breath/pitch motion detail.
(capture-project
  (track :instrument "factory:Physical Models/PM Saxophone")
  (track :instrument "factory:Physical Models/PM Flute"))
(def capture-after-sync ()
  ((eseq.effects.custom-ui-sections/ui-section-select-callback 3) false))
