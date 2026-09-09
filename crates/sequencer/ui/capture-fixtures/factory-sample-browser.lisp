;; Production Samples browser with the bundled piano collection.
(capture-project (track :sampler :name "Sampler"))
(def capture-after-sync ()
  (eseq.browser/select-tab "samples")
  (set! eseq.browser/selected-tags (list "salamander")))
