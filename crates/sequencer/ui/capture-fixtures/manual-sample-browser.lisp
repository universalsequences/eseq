;; The same browser selection action as a click, with audio audition disabled.
(capture-project (track :sampler :name "Sampler"))
(def capture-after-sync ()
  (eseq.browser/select-tab "samples")
  (set! eseq.browser/selected-tags (list "salamander" "C4"))
  (eseq.browser/select-sample
    (nth (get (seq-sample-browser "" (list "salamander" "C4") (list)) :items) 0)))
