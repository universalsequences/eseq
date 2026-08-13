;; Production-path capture for Multiverb's host-modulation source editor.
(capture-project
  (track :sampler
    :name "Multiverb Mods"
    :audio-fx ("Multiverb")))

(def capture-after-sync ()
  (do
    (set! eseq.effects.state/effect-mods-open true)
    (set! eseq.effects.state/effect-mods-chain "audio")
    (set! eseq.effects.state/effect-mods-track 0)
    (set! eseq.effects.state/effect-mods-slot 0)
    (set! eseq.effects.state/effect-mods-rack-slot -1)
    (set! eseq.effects.state/effect-mods-bus -1)
    (set! eseq.effects.state/effect-selected-mod-slot 1)))
