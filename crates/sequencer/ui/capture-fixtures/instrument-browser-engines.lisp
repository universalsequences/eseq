;; Instruments browser with a real project engine and saved-library folders.
(capture-project
  (track :instrument "core/drift"))

(def capture-after-sync ()
  (let ((v eseq.browser/browser-view)) (set! v.tab "instruments")))
