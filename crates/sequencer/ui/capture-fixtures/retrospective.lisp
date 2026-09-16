(capture-project
  (track :sampler :name "Kick")
  (track :sampler :name "Snare")
  (track :sampler :name "Hi-hat"))

(load "retrospective-preview.lisp")
(def capture-after-sync ()
  (eseq.retrospective/open 20 24))
