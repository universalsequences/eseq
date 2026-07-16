;; Production-path capture for Multiverb's host-modulation source editor.
(capture-project
  (track :sampler
    :name "Multiverb Mods"
    :audio-fx ("Multiverb")))

(def capture-after-sync ()
  (do
    (set! effect-mods-open true)
    (set! effect-mods-chain "audio")
    (set! effect-mods-slot 0)
    (set! effect-mods-bus -1)
    (set! effect-selected-mod-slot 1)))
