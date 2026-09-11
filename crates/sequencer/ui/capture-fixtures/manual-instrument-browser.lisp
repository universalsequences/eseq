;; Manual illustration; real project state through the production capture path.
(capture-project (track :instrument "factory:Synths/Digi Drift"))
(def capture-after-sync ()
  (eseq.browser/select-tab "instruments")
  (set! eseq.browser/instrument-origin-filter "factory"))
