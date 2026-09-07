;; Local-library fixture: requires .local/effects/spectral-tamer/{dsp,ui}.lisp.
(capture-project
  (track :sampler :name "Spectral" :audio-fx ("spectral-tamer")))
