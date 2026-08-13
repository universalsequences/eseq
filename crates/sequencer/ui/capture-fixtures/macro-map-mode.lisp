;; Macro-map mode capture using the production project sync and *fx* buffer.
(capture-project
  (track :sampler :name "Sampler"))

(def capture-after-sync ()
  (do
    (set! eseq.macro-state/mapping-open true)
    (set! eseq.macro-state/mapping-selected 1)))
