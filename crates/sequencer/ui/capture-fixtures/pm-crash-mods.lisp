(capture-project (track :instrument "factory:Physical Models/PM Crash"))
(def capture-after-sync ()
  (let ((iv eseq.effects.state/instrument-view))
    (do
      ((eseq.effects.custom-ui-sections/ui-section-select-callback 1) false)
      (set! iv.mods-open true))))
