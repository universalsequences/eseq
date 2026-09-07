;; Local-library fixture: requires .local/effects/spectral-tamer/{dsp,ui}.lisp.
(capture-project
  (track :sampler :name "Spectral" :audio-fx ("spectral-tamer")))

(def capture-after-sync ()
  (let ((fx (nth (filter |fx| (= (get fx :name) "spectral-tamer") SEQ.effects) 0)))
    (set! eseq.effects.state/effect-mods-chain "audio")
    (set! eseq.effects.state/effect-mods-track 0)
    (set! eseq.effects.state/effect-mods-slot (get fx :slot-idx))
    (set! eseq.effects.state/effect-mods-rack-slot -1)
    (set! eseq.effects.state/effect-mods-bus -1)
    (set! eseq.effects.state/effect-selected-mod-slot 1)
    (set! eseq.effects.state/effect-mods-open true)))
