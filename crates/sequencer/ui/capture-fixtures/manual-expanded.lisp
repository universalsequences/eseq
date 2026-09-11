;; Manual illustration; real project state through the production capture path.
(capture-project
  (track :instrument "factory:Synths/Digi Drift" :name "Digi Drift" :steps (0 4 8 12)
    :step-params ((4 :velocity 0.5) (12 :velocity 0.7))))
(def capture-after-sync ()
  (eseq.sequencer/set-track-expanded (nth SEQ.track-ids 0) true)
  (eseq.sequencer/set-track-param-mode (nth SEQ.track-ids 0) 0))
