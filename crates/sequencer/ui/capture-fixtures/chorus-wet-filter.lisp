(capture-project
  (track :sampler :name "Chorus" :audio-fx ("Chorus")))

(def capture-after-sync ()
  (eseq.effects.builtin.chorus/select-filter (nth SEQ.effects 0) 1))
