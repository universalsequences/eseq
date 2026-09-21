(capture-project
  (track :sampler :name "Sampler"))

(def capture-after-sync ()
  (set! sbrowser-tab "packages")
  (eseq.browser/refresh-buffer))
