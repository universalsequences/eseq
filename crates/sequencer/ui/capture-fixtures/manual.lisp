;; The *manual* buffer (content/ui/manual.lisp) on the placeholder index page.
(capture-project (track :sampler :name "Sampler"))
(def capture-after-sync ()
  (eseq.manual/open-manual))
