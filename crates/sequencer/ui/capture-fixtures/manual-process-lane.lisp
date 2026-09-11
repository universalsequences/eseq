;; Manual illustration; real project state through the production capture path.
(capture-project
  (track :instrument "factory:Synths/Digi Drift" :name "Digi Drift" :steps (0 4 8 12)))
(def capture-after-sync ()
  (eseq.sequencer/set-track-expanded (nth SEQ.track-ids 0) true)
  (eseq.sequencer/set-track-param-mode (nth SEQ.track-ids 0)
    (+ eseq.seqv-track-params/seqv-process-lane-mode-offset 4)))
