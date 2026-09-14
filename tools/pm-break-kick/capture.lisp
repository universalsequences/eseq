(capture-project
  (track :instrument "user:Studies/Break Kick 53"))

(def capture-after-sync ()
  ((eseq.effects.custom-ui-sections/ui-section-select-callback 3) false))
