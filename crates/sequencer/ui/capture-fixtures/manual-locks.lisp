;; Manual illustration; real project state through the production capture path.
(capture-project
  (track :instrument "factory:Synths/Digi Drift" :name "Digi Drift" :steps (0 4 8 12)
    :instrument-locks ((8 "lp_freq" 650))))
(seq-select-step 8)
(def capture-after-sync () (eseq.sequencer/collapse-all-tracks))
