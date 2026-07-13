;; Instruments browser with a real project engine and saved-library folders.
(capture-project
  (track :instrument "core/drift"))

(def capture-after-sync ()
  (set! sbrowser-tab "instruments"))
