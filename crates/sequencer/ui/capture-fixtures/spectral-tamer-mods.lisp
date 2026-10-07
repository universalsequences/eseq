;; Local-library fixture: requires .local/effects/spectral-tamer/{dsp,ui}.lisp.
(capture-project
  (track :sampler :name "Spectral" :audio-fx ("spectral-tamer")))

(def capture-after-sync ()
  (let ((em eseq.effects.state/effect-mods))
    (let ((fx (nth (filter |fx| (= (get fx :name) "spectral-tamer") SEQ.effects) 0)))
      (set! em.chain "audio")
      (set! em.track 0)
      (set! em.slot (get fx :slot-idx))
      (set! em.rack-slot -1)
      (set! em.bus -1)
      (set! em.mod-slot 1)
      (set! em.open true))))
