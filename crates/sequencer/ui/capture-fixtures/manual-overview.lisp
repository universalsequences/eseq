;; Manual illustration; real project state through the production capture path.
(capture-project
  (scenes 3)
  (track :instrument "factory:Drums/808 Kick" :name "Kick" :steps (0 4 8 12))
  (track :instrument "factory:Drums/Digi Snare" :name "Snare" :steps (4 12))
  (track :instrument "factory:Drums/Digi Hat" :name "Hat" :steps (0 2 4 6 8 10 12 14))
  (track :instrument "factory:Synths/Digi Drift" :name "Bass" :steps (0 (3 7) 6 (8 -5) (11 7) 14)))
(def capture-after-sync () (eseq.sequencer/collapse-all-tracks))
