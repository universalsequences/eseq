;; Production Samples browser with the bundled piano collection.
(capture-project (track :sampler :name "Sampler"))
(def capture-after-sync ()
  (eseq.browser/select-tab "samples")
  (let ((p eseq.browser/sample-pick)) (set! p.tags (list "salamander"))))
