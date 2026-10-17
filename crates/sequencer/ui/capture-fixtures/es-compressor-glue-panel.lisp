;; ES Compressor in Glue mode, with Auto release selected.
;; Capture with the same viewport as es-compressor-panel.lisp.
(capture-project
  (track :sampler :name "ES Compressor" :audio-fx ("ES Compressor")))

(seq-set-effect-param 0 0 3)
(seq-set-effect-param 0 12 4)
