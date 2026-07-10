;; Deterministic keys-tab capture with a C major triad marked active.
(capture-project
  (track :instrument "core/drift"))

;; The headless capture has no running note source. Override only the activity
;; predicate so the production keys widget can be inspected in its lit state.
(def instrument-key-note-active? (note)
  (fx-list-contains? '(60 64 67) note))

(def capture-after-sync ()
  (set! instrument-panel-tab 1))
