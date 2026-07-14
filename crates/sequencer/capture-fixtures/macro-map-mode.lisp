;; Macro-map mode capture using the production project sync and *fx* buffer.
(capture-project
  (track :sampler :name "Sampler"))

(def capture-after-sync ()
  (do
    (set! macro-mapping-open true)
    (set! macro-mapping-selected 1)))
