;; Production-path capture for Multiverb's host-modulation source editor.
(capture-project
  (track :sampler
    :name "Multiverb Mods"
    :audio-fx ("Multiverb")))

(def capture-after-sync ()
  (let ((em eseq.effects.state/effect-mods))
    (do
      (set! em.open true)
      (set! em.chain "audio")
      (set! em.track 0)
      (set! em.slot 0)
      (set! em.rack-slot -1)
      (set! em.bus -1)
      (set! em.mod-slot 1))))
