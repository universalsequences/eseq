;; The *manual* buffer (content/ui/manual.lisp) on the authored index page.
(capture-project (track :sampler :name "Sampler"))
(def capture-after-sync ()
  (eseq.manual/open-manual))
