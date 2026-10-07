;; Macro-map mode capture using the production project sync and *fx* buffer.
(capture-project
  (track :sampler :name "Sampler"))

(def capture-after-sync ()
  (eseq.macro-state/arm-macro! 1))
