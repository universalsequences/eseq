;; Local-library fixture: requires .local/effects/granular-looper/.
(capture-project
  (track :sampler :name "Granular Looper" :audio-fx ("granular-looper"))
  (track :modulator :name "Motion")
  (mod-route 1 0 0))
