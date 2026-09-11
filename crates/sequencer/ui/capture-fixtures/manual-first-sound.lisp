;; The first-session walkthrough's browser search before loading a sound.
(capture-project)
(def capture-after-sync ()
  (eseq.browser/select-tab "instruments")
  (set! eseq.browser/search-filter "Digi Drift"))
