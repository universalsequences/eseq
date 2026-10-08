(capture-project (track :instrument "factory:Synths/Melt"))
(def capture-after-sync ()
  (let ((iv eseq.effects.state/instrument-view))
    (do
      ((eseq.effects.custom-ui-sections/ui-section-select-callback 4) false)
      (set! iv.mods-open true))))
