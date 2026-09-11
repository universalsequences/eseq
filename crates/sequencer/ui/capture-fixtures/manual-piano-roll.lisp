;; Manual illustration; real project state through the production capture path.
(capture-project
  (track :instrument "factory:Synths/Digi Drift" :name "Digi Drift"
    :steps (0 (2 3) (4 7) (6 10) 8 (10 3) (12 7) (14 12))
    :step-params ((2 :velocity 0.45) (6 :velocity 0.65)
                  (10 :velocity 0.55) (14 :velocity 0.8))))
(def capture-after-sync () (eseq.seq-panels/seq-open-piano-roll-bottom-for-track 0))
