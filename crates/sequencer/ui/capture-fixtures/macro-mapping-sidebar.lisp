;; Macro mapping sidebar mounted through the production sequencer capture path.
;; Row content is covered by the reactive layout test; this fixture verifies
;; the mapping-mode shell and its empty-state composition.
(capture-project
  (track :sampler :name "Sampler"))

(def capture-after-sync ()
  (do
    (set! macro-mapping-open true)
    (set! macro-mapping-selected 1)))
