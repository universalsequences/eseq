;; The first-session walkthrough's browser search before loading a sound.
(capture-project)
(def capture-after-sync ()
  (eseq.browser/select-tab "instruments")
  (let ((v eseq.browser/browser-view)) (set! v.search "Digi Syn")))
