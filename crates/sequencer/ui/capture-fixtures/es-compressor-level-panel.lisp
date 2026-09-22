;; Level's nominal gain is part of its operating point, not the Output trim.
;; Capture with the same viewport as es-compressor-panel.lisp.
(capture-project
  (track :sampler :name "ES Compressor" :audio-fx ("ES Compressor")))

(seq-set-effect-param 0 0 1)
