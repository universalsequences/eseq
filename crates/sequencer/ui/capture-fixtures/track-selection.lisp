;; Non-contiguous group members exercise selection in rendered row order.
(capture-project
  (track :sampler :name "Kick")
  (track :sampler :name "Bass")
  (track :sampler :name "Snare")
  (track :sampler :name "Hat")
  (group 0 2))

(def capture-after-sync ()
  (do
    (eseq.sequencer/track-click (dict) 0)
    (eseq.sequencer/track-click (dict :shift true) 1)))
