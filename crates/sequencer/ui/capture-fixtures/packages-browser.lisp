(capture-project
  (track :sampler :name "Sampler"))

(def capture-after-sync ()
  (let ((v eseq.browser/browser-view)) (set! v.tab "packages"))
  (eseq.browser/refresh-buffer))
