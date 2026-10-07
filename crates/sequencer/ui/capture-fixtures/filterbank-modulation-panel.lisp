;; Production Filterbank panel with the first modulation slot selected.
(capture-project
  (track :sampler
    :name "Filterbank"
    :audio-fx ("Filterbank")))

(def capture-after-sync ()
  (let ((em eseq.effects.state/effect-mods))
    (do
      (set! em.chain "audio")
      (set! em.track 0)
      (set! em.slot 0)
      (set! em.rack-slot -1)
      (set! em.bus -1)
      (set! em.mod-slot 1)
      (set! em.open true))))
