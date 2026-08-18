;; Production-path capture for Filter Table's modulation-depth controls.
(capture-project
  (track :sampler
    :name "Filter Table Mods"
    :audio-fx ("Filter Table")))

(def capture-after-sync ()
  (do
    (set! eseq.effects.state/effect-mods-open true)
    (set! eseq.effects.state/effect-mods-chain "audio")
    (set! eseq.effects.state/effect-mods-track 0)
    (set! eseq.effects.state/effect-mods-slot 0)
    (set! eseq.effects.state/effect-mods-rack-slot -1)
    (set! eseq.effects.state/effect-mods-bus -1)
    (set! eseq.effects.state/effect-selected-mod-slot 1)))
