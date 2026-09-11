;; Manual illustration; real project state through the production capture path.
(capture-project
  (track :instrument "factory:Synths/Digi Drift" :name "Digi Drift"))
(seq-arm-track-exclusive 0)
(def capture-after-sync () (eseq.sequencer/collapse-all-tracks))
