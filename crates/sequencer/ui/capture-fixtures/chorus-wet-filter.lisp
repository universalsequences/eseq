(capture-project
  (track :sampler :name "Chorus" :audio-fx ("Chorus")))

(def capture-after-sync ()
  (eseq.effects.builtin.chorus/select-filter (first (eseq.effects.panel-data/current-effect-panels)) 1))
