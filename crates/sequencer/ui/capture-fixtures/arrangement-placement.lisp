(capture-project
  (track :sampler :name "Kick" :steps (0 4 8 12))
  (track :sampler :name "Snare" :steps (4 12)))

(def capture-after-sync ()
  (do
    (eseq.seq-panels/seq-open-arrangement)
    (eseq.arrangement/set-cursor 8 0)
    (eseq.arrangement/begin-placement)
    (eseq.arrangement/track-action 1 (dict :type :place-item :time 8))
    (eseq.arrangement/begin-placement)))
